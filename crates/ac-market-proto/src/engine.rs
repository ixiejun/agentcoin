//! The local protocol between an inference engine plugin and the provider agent (spec
//! `market/engine-plugin` "与提供者的本机协议").
//!
//! A Unix stream socket carries frames: a 4-byte big-endian length, then the message. Messages
//! have a fixed big-endian layout so that the plugin encodes them with Python's `struct`:
//!
//! | Message | Layout |
//! |---|---|
//! | `Hello` (plugin → provider) | `0x10`, `"ACTL"`, version `u8`, hidden size `u32` |
//! | `Welcome` (provider → plugin) | `0x11`, version `u8`, top-k `u16` |
//! | `Segment` (plugin → provider) | `0x01`, request ID (`u16` length + UTF-8), phase `u8` (0 prefill, 1 decode), values `u32`, count `u16`, count × (index `u32`, bf16 bits `u16`) |
//! | `Finish` (plugin → provider) | `0x02`, request ID (`u16` length + UTF-8) |

use ac_toploc::{Bf16, Candidate, Phase};

use crate::Error;

/// Version of this protocol.
pub const ENGINE_PROTOCOL_VERSION: u8 = 1;

/// Marks a `Hello`.
pub const MAGIC: [u8; 4] = *b"ACTL";

/// Largest accepted frame.
pub const MAX_ENGINE_FRAME: usize = 1 << 20;

const HELLO: u8 = 0x10;
const WELCOME: u8 = 0x11;
const SEGMENT: u8 = 0x01;
const FINISH: u8 = 0x02;

/// One message of the plugin protocol.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineMsg {
    /// The plugin connects.
    Hello {
        /// Protocol version the plugin speaks.
        version: u8,
        /// Hidden size of the model (values per token).
        hidden_size: u32,
    },
    /// The provider answers.
    Welcome {
        /// Protocol version the provider speaks.
        version: u8,
        /// Candidates to send per segment.
        topk: u16,
    },
    /// The candidates of one forward step of one request.
    Segment {
        /// The engine's request ID.
        request: String,
        /// Prefill or decode.
        phase: Phase,
        /// Values in the segment (tokens × hidden size).
        len: u32,
        /// The segment's top-k values and their indices.
        candidates: Vec<Candidate>,
    },
    /// A request finished or was aborted.
    Finish {
        /// The engine's request ID.
        request: String,
    },
}

fn put_id(out: &mut Vec<u8>, id: &str) -> Result<(), Error> {
    let len = u16::try_from(id.len()).map_err(|_| Error::FrameTooLarge)?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(id.as_bytes());
    Ok(())
}

impl EngineMsg {
    /// Encodes the message (without the frame length).
    ///
    /// # Errors
    ///
    /// [`Error::FrameTooLarge`] for a request ID over 65,535 bytes or more than 65,535
    /// candidates, [`Error::Decode`] for a phase this version cannot express.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        match self {
            Self::Hello {
                version,
                hidden_size,
            } => {
                out.push(HELLO);
                out.extend_from_slice(&MAGIC);
                out.push(*version);
                out.extend_from_slice(&hidden_size.to_be_bytes());
            }
            Self::Welcome { version, topk } => {
                out.push(WELCOME);
                out.push(*version);
                out.extend_from_slice(&topk.to_be_bytes());
            }
            Self::Segment {
                request,
                phase,
                len,
                candidates,
            } => {
                out.push(SEGMENT);
                put_id(&mut out, request)?;
                out.push(match phase {
                    Phase::Prefill => 0,
                    Phase::Decode => 1,
                    _ => return Err(Error::Decode),
                });
                out.extend_from_slice(&len.to_be_bytes());
                let count = u16::try_from(candidates.len()).map_err(|_| Error::FrameTooLarge)?;
                out.extend_from_slice(&count.to_be_bytes());
                for c in candidates {
                    out.extend_from_slice(&c.index.to_be_bytes());
                    out.extend_from_slice(&c.value.0.to_be_bytes());
                }
            }
            Self::Finish { request } => {
                out.push(FINISH);
                put_id(&mut out, request)?;
            }
        }
        Ok(out)
    }

    /// Encodes the message as a frame.
    ///
    /// # Errors
    ///
    /// Those of [`EngineMsg::encode`], and [`Error::FrameTooLarge`] above
    /// [`MAX_ENGINE_FRAME`].
    pub fn to_frame(&self) -> Result<Vec<u8>, Error> {
        let payload = self.encode()?;
        if payload.len() > MAX_ENGINE_FRAME {
            return Err(Error::FrameTooLarge);
        }
        let len = u32::try_from(payload.len()).map_err(|_| Error::FrameTooLarge)?;
        let mut out = Vec::with_capacity(payload.len().saturating_add(4));
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&payload);
        Ok(out)
    }

    /// Decodes a message (without the frame length).
    ///
    /// # Errors
    ///
    /// [`Error::Decode`] for an unknown type, a bad magic, a truncated or overlong message, a
    /// non-UTF-8 request ID or an unknown phase.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader(bytes);
        let msg = match r.u8()? {
            HELLO => {
                if r.take(4)? != MAGIC {
                    return Err(Error::Decode);
                }
                Self::Hello {
                    version: r.u8()?,
                    hidden_size: r.u32()?,
                }
            }
            WELCOME => Self::Welcome {
                version: r.u8()?,
                topk: r.u16()?,
            },
            SEGMENT => {
                let request = r.id()?;
                let phase = match r.u8()? {
                    0 => Phase::Prefill,
                    1 => Phase::Decode,
                    _ => return Err(Error::Decode),
                };
                let len = r.u32()?;
                let count = r.u16()?;
                let candidates = (0..count)
                    .map(|_| {
                        Ok(Candidate {
                            index: r.u32()?,
                            value: Bf16(r.u16()?),
                        })
                    })
                    .collect::<Result<Vec<_>, Error>>()?;
                Self::Segment {
                    request,
                    phase,
                    len,
                    candidates,
                }
            }
            FINISH => Self::Finish { request: r.id()? },
            _ => return Err(Error::Decode),
        };
        if !r.0.is_empty() {
            return Err(Error::Decode);
        }
        Ok(msg)
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.0.len() < n {
            return Err(Error::Decode);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(u8::from_be_bytes(self.array()?))
    }

    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn id(&mut self) -> Result<String, Error> {
        let len = usize::from(self.u16()?);
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| Error::Decode)
    }
}

/// Incremental frame reader for the plugin protocol.
#[derive(Debug, Default)]
pub struct EngineReader {
    buf: Vec<u8>,
}

impl EngineReader {
    /// An empty reader.
    #[must_use]
    pub const fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Appends received bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete message, if one has arrived.
    ///
    /// # Errors
    ///
    /// [`Error::FrameTooLarge`] if a frame announces more than [`MAX_ENGINE_FRAME`] bytes, and
    /// the errors of [`EngineMsg::decode`]; the reader is unusable afterwards (close the
    /// connection).
    pub fn next_msg(&mut self) -> Result<Option<EngineMsg>, Error> {
        let Some(head) = self.buf.get(..4) else {
            return Ok(None);
        };
        let mut len = [0u8; 4];
        len.copy_from_slice(head);
        let len = usize::try_from(u32::from_be_bytes(len)).map_err(|_| Error::FrameTooLarge)?;
        if len > MAX_ENGINE_FRAME {
            return Err(Error::FrameTooLarge);
        }
        let end = len.saturating_add(4);
        let Some(payload) = self.buf.get(4..end) else {
            return Ok(None);
        };
        let msg = EngineMsg::decode(payload)?;
        self.buf.drain(..end);
        Ok(Some(msg))
    }
}

/// The market request ID inside an engine request ID: the provider forwards requests with
/// `X-Request-Id` set to the 64 lowercase hex digits of the gateway's request ID, and the
/// engine derives its own ID from it (vLLM: `chatcmpl-<X-Request-Id>-<suffix>`). The ID is
/// the one `-`-separated part of exactly 64 lowercase hex digits.
#[must_use]
pub fn market_request_id(engine_id: &str) -> Option<[u8; 32]> {
    engine_id
        .split('-')
        .filter(|p| p.len() == 64 && p.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .find_map(|p| {
            let mut id = [0u8; 32];
            hex::decode_to_slice(p, &mut id).ok()?;
            Some(id)
        })
}

/// The `X-Request-Id` header value for a market request ID.
#[must_use]
pub fn request_id_header(id: &[u8; 32]) -> String {
    hex::encode(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn segment() -> EngineMsg {
        EngineMsg::Segment {
            request: "chatcmpl-ab-1".into(),
            phase: Phase::Decode,
            len: 896,
            candidates: vec![
                Candidate {
                    index: 7,
                    value: Bf16(0x3f80),
                },
                Candidate {
                    index: 300,
                    value: Bf16(0xc000),
                },
            ],
        }
    }

    #[test]
    fn layouts() {
        let hello = EngineMsg::Hello {
            version: 1,
            hidden_size: 896,
        };
        assert_eq!(
            hello.encode().unwrap(),
            [0x10, b'A', b'C', b'T', b'L', 1, 0, 0, 3, 0x80]
        );
        let welcome = EngineMsg::Welcome {
            version: 1,
            topk: 128,
        };
        assert_eq!(welcome.encode().unwrap(), [0x11, 1, 0, 128]);
        let finish = EngineMsg::Finish {
            request: "r".into(),
        };
        assert_eq!(finish.encode().unwrap(), [0x02, 0, 1, b'r']);
        assert_eq!(
            segment().to_frame().unwrap(),
            [
                0, 0, 0, 35, 0x01, 0, 13, b'c', b'h', b'a', b't', b'c', b'm', b'p', b'l', b'-',
                b'a', b'b', b'-', b'1', 1, 0, 0, 3, 0x80, 0, 2, 0, 0, 0, 7, 0x3f, 0x80, 0, 0, 1,
                0x2c, 0xc0, 0
            ]
        );
    }

    #[test]
    fn malformed_messages_are_errors() {
        let bytes = segment().encode().unwrap();
        for cut in 0..bytes.len() {
            assert_eq!(EngineMsg::decode(&bytes[..cut]), Err(Error::Decode));
        }
        let mut long = bytes.clone();
        long.push(0);
        assert_eq!(EngineMsg::decode(&long), Err(Error::Decode));
        assert_eq!(EngineMsg::decode(&[0x42]), Err(Error::Decode));
        assert_eq!(
            EngineMsg::decode(&[0x10, b'X', b'C', b'T', b'L', 1, 0, 0, 0, 1]),
            Err(Error::Decode)
        );
        // Phase 2 is unknown.
        let mut bad = bytes;
        bad[16] = 2;
        assert_eq!(EngineMsg::decode(&bad), Err(Error::Decode));
    }

    #[test]
    fn reader_splits_frames_and_refuses_oversized_ones() {
        let mut frames = segment().to_frame().unwrap();
        frames.extend(
            EngineMsg::Finish {
                request: "x".into(),
            }
            .to_frame()
            .unwrap(),
        );
        let mut r = EngineReader::new();
        for b in &frames {
            r.push(&[*b]);
        }
        assert_eq!(r.next_msg().unwrap(), Some(segment()));
        assert!(matches!(
            r.next_msg().unwrap(),
            Some(EngineMsg::Finish { .. })
        ));
        assert_eq!(r.next_msg().unwrap(), None);
        let mut r = EngineReader::new();
        r.push(&u32::try_from(MAX_ENGINE_FRAME + 1).unwrap().to_be_bytes());
        assert_eq!(r.next_msg(), Err(Error::FrameTooLarge));
    }

    #[test]
    fn market_request_ids() {
        let id = [0xab; 32];
        let header = request_id_header(&id);
        assert_eq!(header.len(), 64);
        assert_eq!(
            market_request_id(&format!("chatcmpl-{header}-1f2e3d4c")),
            Some(id)
        );
        assert_eq!(market_request_id(&header), Some(id));
        assert_eq!(market_request_id("chatcmpl-0123-abcd"), None);
        assert_eq!(
            market_request_id(&format!("chatcmpl-{}", header.to_uppercase())),
            None
        );
        assert_eq!(market_request_id(&format!("x{header}")), None);
    }

    proptest! {
        #[test]
        fn segments_round_trip(
            request in "[ -~]{0,80}",
            decode in any::<bool>(),
            len in any::<u32>(),
            cands in proptest::collection::vec((any::<u32>(), any::<u16>()), 0..300),
        ) {
            let msg = EngineMsg::Segment {
                request,
                phase: if decode { Phase::Decode } else { Phase::Prefill },
                len,
                candidates: cands.into_iter().map(|(index, v)| Candidate { index, value: Bf16(v) }).collect(),
            };
            let mut r = EngineReader::new();
            r.push(&msg.to_frame().unwrap());
            prop_assert_eq!(r.next_msg().unwrap(), Some(msg));
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
            let _ = EngineMsg::decode(&bytes);
        }
    }
}
