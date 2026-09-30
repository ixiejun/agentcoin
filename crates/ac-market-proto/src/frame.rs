//! Length-prefixed framing of a sealed channel over an HTTP body.
//!
//! A body is a sequence of frames, each a 4-byte big-endian length followed by that many bytes.
//! The first frame of a request is the signed handshake; every other frame is one sealed chunk.

use crate::Error;

/// Largest accepted frame: a signed handshake with the largest key and signature, or one sealed
/// chunk (64 KiB plaintext, tag and flag), with room to spare.
pub const MAX_FRAME: usize = 128 * 1024;

const LEN: usize = 4;

/// Encodes one frame.
///
/// # Errors
///
/// [`Error::FrameTooLarge`] above [`MAX_FRAME`].
pub fn encode(payload: &[u8]) -> Result<Vec<u8>, Error> {
    if payload.len() > MAX_FRAME {
        return Err(Error::FrameTooLarge);
    }
    let len = u32::try_from(payload.len()).map_err(|_| Error::FrameTooLarge)?;
    let mut out = Vec::with_capacity(payload.len().saturating_add(LEN));
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Incremental decoder: feed bytes as they arrive, take whole frames out.
#[derive(Debug, Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    /// An empty decoder.
    #[must_use]
    pub const fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Appends received bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete frame, if one has arrived.
    ///
    /// # Errors
    ///
    /// [`Error::FrameTooLarge`] if the announced length exceeds [`MAX_FRAME`]; the decoder is
    /// unusable afterwards.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, Error> {
        let Some(head) = self.buf.get(..LEN) else {
            return Ok(None);
        };
        let mut len = [0u8; LEN];
        len.copy_from_slice(head);
        let len = usize::try_from(u32::from_be_bytes(len)).map_err(|_| Error::FrameTooLarge)?;
        if len > MAX_FRAME {
            return Err(Error::FrameTooLarge);
        }
        let end = LEN.saturating_add(len);
        if self.buf.len() < end {
            return Ok(None);
        }
        let frame = self
            .buf
            .get(LEN..end)
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        self.buf.drain(..end);
        Ok(Some(frame))
    }

    /// Whether bytes of an incomplete frame are pending (the stream ended mid-frame).
    #[must_use]
    pub fn has_partial(&self) -> bool {
        !self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn half_frames_and_concatenated_frames() {
        let (a, b) = (encode(b"hello").unwrap(), encode(b"").unwrap());
        let mut d = Decoder::new();
        d.push(&a[..3]);
        assert_eq!(d.next_frame().unwrap(), None);
        d.push(&a[3..]);
        d.push(&b);
        assert_eq!(d.next_frame().unwrap(), Some(b"hello".to_vec()));
        assert_eq!(d.next_frame().unwrap(), Some(Vec::new()));
        assert_eq!(d.next_frame().unwrap(), None);
        assert!(!d.has_partial());
        d.push(&a[..6]);
        assert!(d.has_partial());
    }

    #[test]
    fn oversized_frames_are_rejected() {
        assert_eq!(
            encode(&vec![0; MAX_FRAME + 1]).unwrap_err(),
            Error::FrameTooLarge
        );
        assert!(encode(&vec![0; MAX_FRAME]).is_ok());
        let mut d = Decoder::new();
        d.push(&u32::try_from(MAX_FRAME + 1).unwrap().to_be_bytes());
        assert_eq!(d.next_frame().unwrap_err(), Error::FrameTooLarge);
    }

    proptest! {
        // Any split of the byte stream yields the same frames.
        #[test]
        fn any_split_yields_the_same_frames(
            frames in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..300), 0..6),
            cuts in proptest::collection::vec(0usize..2000, 0..10),
        ) {
            let stream: Vec<u8> = frames.iter().flat_map(|f| encode(f).unwrap()).collect();
            let mut cuts: Vec<usize> = cuts.into_iter().map(|c| c.checked_rem(stream.len().saturating_add(1)).unwrap()).collect();
            cuts.push(stream.len());
            cuts.sort_unstable();
            let mut d = Decoder::new();
            let (mut out, mut start) = (Vec::new(), 0);
            for end in cuts {
                d.push(&stream[start..end]);
                start = end;
                while let Some(f) = d.next_frame().unwrap() {
                    out.push(f);
                }
            }
            prop_assert_eq!(out, frames);
            prop_assert!(!d.has_partial());
        }
    }
}
