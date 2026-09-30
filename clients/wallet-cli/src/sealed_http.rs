//! Sealed channels over HTTP (spec `crypto/sealed-channel`, `ac-market-proto` transport).
//!
//! The request body is the framed signed handshake followed by framed sealed chunks; the response
//! body is framed sealed chunks, streamed. Inside each direction the plaintext is a sequence of
//! messages, each prefixed with its 4-byte big-endian length, so a message may span chunks and a
//! chunk may end a message; the final chunk ends the stream.

use std::time::{SystemTime, UNIX_EPOCH};

use ac_crypto::sealed::{
    Acceptor, Opener, RecipientSession, ReplayCache, Sealer, SignedHandshake, accept_session,
    open_session,
};
use ac_crypto::sig::SigningKey;
use ac_crypto::{AccountId, KemPublicKey, PqPublicKey};
use ac_market_proto::frame;
use anyhow::{Context, Result, anyhow, bail};
use bytes::Bytes;
use hyper::body::Incoming;
use sp_runtime::AccountId32;

use crate::http::{self, BodySender, Client, Response};

/// Content type of sealed bodies.
pub const CONTENT_TYPE: &str = "application/x-agentcoin-sealed";

/// Path of the sealed endpoint.
pub const PATH: &str = "/ac/v1/sealed";

/// Largest plaintext message accepted (a long prompt or a whole non-streamed response).
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;

const CHUNK: usize = ac_crypto::sealed::MAX_CHUNK;

/// Seconds since the Unix epoch (handshake times).
#[must_use]
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Converts an account between the crypto and the runtime representations.
#[must_use]
pub fn to_account32(a: &AccountId) -> AccountId32 {
    AccountId32::new(*a.as_bytes())
}

fn to_account(a: &AccountId32) -> AccountId {
    let bytes: &[u8; 32] = a.as_ref();
    AccountId(*bytes)
}

/// Seals a stream of length-prefixed messages.
struct MessageSealer {
    sealer: Sealer,
}

impl MessageSealer {
    /// The sealed chunks carrying `msg`; `last` ends the stream after it.
    fn seal(&mut self, msg: &[u8], last: bool) -> Result<Vec<Vec<u8>>> {
        let len = u32::try_from(msg.len()).context("message too large")?;
        let mut plain = Vec::with_capacity(msg.len().saturating_add(4));
        plain.extend_from_slice(&len.to_be_bytes());
        plain.extend_from_slice(msg);
        let pieces: Vec<&[u8]> = plain.chunks(CHUNK).collect();
        let n = pieces.len();
        pieces
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let final_chunk = last && i.saturating_add(1) == n;
                self.sealer.seal(p, final_chunk).map_err(|e| anyhow!("{e}"))
            })
            .collect()
    }
}

/// Reassembles length-prefixed messages from opened chunks.
struct MessageOpener {
    opener: Opener,
    buf: Vec<u8>,
    ended: bool,
}

impl MessageOpener {
    fn open(&mut self, chunk: &[u8]) -> Result<()> {
        let (plain, last) = self.opener.open(chunk).map_err(|e| anyhow!("{e}"))?;
        self.buf.extend_from_slice(&plain);
        if self.buf.len() > MAX_MESSAGE.saturating_add(4) {
            bail!("message too large");
        }
        self.ended = last;
        Ok(())
    }

    /// The next complete message, if buffered.
    fn next_message(&mut self) -> Result<Option<Vec<u8>>> {
        let Some(head) = self.buf.get(..4) else {
            return Ok(None);
        };
        let mut len = [0u8; 4];
        len.copy_from_slice(head);
        let len = usize::try_from(u32::from_be_bytes(len))?;
        if len > MAX_MESSAGE {
            bail!("message too large");
        }
        let end = len.saturating_add(4);
        if self.buf.len() < end {
            return Ok(None);
        }
        let msg = self.buf.get(4..end).map(<[u8]>::to_vec).unwrap_or_default();
        self.buf.drain(..end);
        Ok(Some(msg))
    }
}

/// The client's view of a sealed exchange: the response's messages in order.
pub struct SealedResponse {
    body: Incoming,
    frames: frame::Decoder,
    messages: MessageOpener,
}

impl core::fmt::Debug for SealedResponse {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SealedResponse")
    }
}

impl SealedResponse {
    /// The next response message, or `None` after the final one.
    ///
    /// # Errors
    ///
    /// Transport errors, tampering or a truncated stream.
    pub async fn next(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            if let Some(m) = self.messages.next_message()? {
                return Ok(Some(m));
            }
            if self.messages.ended {
                if !self.messages.buf.is_empty() {
                    bail!("sealed stream ended inside a message");
                }
                return Ok(None);
            }
            if let Some(f) = self.frames.next_frame()? {
                self.messages.open(&f)?;
                continue;
            }
            match http::next_chunk(&mut self.body).await? {
                Some(bytes) => self.frames.push(&bytes),
                None => {
                    self.messages.opener.finish().map_err(|e| anyhow!("{e}"))?;
                    bail!("sealed stream truncated");
                }
            }
        }
    }
}

/// Seals `message` to `recipient` (whose endpoint is `base`) as `sender`, posts it and returns
/// the response stream.
///
/// # Errors
///
/// Transport errors or an HTTP error status (whose short text is included).
pub async fn post(
    client: &Client,
    base: &str,
    recipient: &KemPublicKey,
    (sender, key): (&AccountId32, &SigningKey),
    message: &[u8],
) -> Result<SealedResponse> {
    let mut rng = ac_crypto::OsRng::new()?;
    let (handshake, session) =
        open_session(recipient, key, to_account(sender), now_secs(), &mut rng)?;
    let mut body = frame::encode(&handshake.encode()?)?;
    let mut sealer = MessageSealer {
        sealer: session.request,
    };
    for chunk in sealer.seal(message, true)? {
        body.extend_from_slice(&frame::encode(&chunk)?);
    }
    let resp = client
        .post(&http::join(base, PATH), CONTENT_TYPE, body)
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let text = http::read_body(resp.into_body(), 4096)
            .await
            .unwrap_or_default();
        bail!("HTTP {status}: {}", String::from_utf8_lossy(&text));
    }
    Ok(SealedResponse {
        body: resp.into_body(),
        frames: frame::Decoder::new(),
        messages: MessageOpener {
            opener: session.response,
            buf: Vec::new(),
            ended: false,
        },
    })
}

/// A request accepted by a recipient.
pub struct SealedRequest {
    /// The authenticated sender.
    pub sender: AccountId32,
    /// The request message.
    pub message: Vec<u8>,
    response: Sealer,
}

impl core::fmt::Debug for SealedRequest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SealedRequest")
            .field("sender", &self.sender)
            .finish_non_exhaustive()
    }
}

/// Why a request was refused before decryption; mapped to plain HTTP statuses.
#[derive(Debug)]
pub enum Refusal {
    /// Malformed body (400).
    Malformed(String),
    /// Authentication, freshness or replay failure (401).
    Unauthenticated(String),
}

impl Refusal {
    /// The plain response for the refusal (short reason, no request content).
    #[must_use]
    pub fn response(&self) -> Response {
        match self {
            Self::Malformed(m) => http::full(400, "text/plain", m.clone()),
            Self::Unauthenticated(m) => http::full(401, "text/plain", m.clone()),
        }
    }
}

/// Parses the handshake of a request body without decrypting anything.
///
/// # Errors
///
/// A malformed body.
pub fn handshake_of(body: &[u8]) -> Result<SignedHandshake, Refusal> {
    let mut frames = frame::Decoder::new();
    frames.push(body);
    let first = frames
        .next_frame()
        .map_err(|e| Refusal::Malformed(e.to_string()))?
        .ok_or_else(|| Refusal::Malformed("missing handshake".into()))?;
    SignedHandshake::decode(&first).map_err(|e| Refusal::Malformed(e.to_string()))
}

/// Accepts a whole request body: checks the handshake (`key_of` returns the sender's current
/// on-chain key), decrypts the request and prepares the response sealer.
///
/// # Errors
///
/// A [`Refusal`]; nothing is decrypted unless the handshake passes every check.
pub fn accept(
    body: &[u8],
    acceptor: &Acceptor<'_>,
    key_of: impl FnOnce(&AccountId) -> Option<PqPublicKey>,
    replay: &mut ReplayCache,
) -> Result<SealedRequest, Refusal> {
    let mut frames = frame::Decoder::new();
    frames.push(body);
    let first = frames
        .next_frame()
        .map_err(|e| Refusal::Malformed(e.to_string()))?
        .ok_or_else(|| Refusal::Malformed("missing handshake".into()))?;
    let handshake =
        SignedHandshake::decode(&first).map_err(|e| Refusal::Malformed(e.to_string()))?;
    let RecipientSession {
        sender,
        request,
        response,
    } = accept_session(&handshake, acceptor, key_of, replay)
        .map_err(|e| Refusal::Unauthenticated(e.to_string()))?;
    let mut messages = MessageOpener {
        opener: request,
        buf: Vec::new(),
        ended: false,
    };
    while let Some(f) = frames
        .next_frame()
        .map_err(|e| Refusal::Malformed(e.to_string()))?
    {
        messages
            .open(&f)
            .map_err(|e| Refusal::Malformed(e.to_string()))?;
    }
    if frames.has_partial() || !messages.ended {
        return Err(Refusal::Malformed("truncated request".into()));
    }
    let message = messages
        .next_message()
        .map_err(|e| Refusal::Malformed(e.to_string()))?
        .ok_or_else(|| Refusal::Malformed("empty request".into()))?;
    if !messages.buf.is_empty() {
        return Err(Refusal::Malformed("trailing data".into()));
    }
    Ok(SealedRequest {
        sender: to_account32(&sender),
        message,
        response,
    })
}

/// Streams sealed response messages.
pub struct SealedWriter {
    tx: BodySender,
    sealer: MessageSealer,
}

impl core::fmt::Debug for SealedWriter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SealedWriter")
    }
}

impl SealedWriter {
    /// Sends one message; `last` ends the stream. `false` once the peer is gone or the message
    /// cannot be sealed.
    pub async fn send(&mut self, message: &[u8], last: bool) -> bool {
        let Ok(chunks) = self.sealer.seal(message, last) else {
            return false;
        };
        for c in chunks {
            let Ok(framed) = frame::encode(&c) else {
                return false;
            };
            if !self.tx.send(Bytes::from(framed)).await {
                return false;
            }
        }
        true
    }
}

impl SealedRequest {
    /// Starts the streamed sealed response.
    #[must_use]
    pub fn respond(self) -> (SealedWriter, Response, AccountId32, Vec<u8>) {
        let (tx, resp) = http::streaming(200, CONTENT_TYPE);
        (
            SealedWriter {
                tx,
                sealer: MessageSealer {
                    sealer: self.response,
                },
            },
            resp,
            self.sender,
            self.message,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_crypto::kem::KemSecretKey;
    use ac_crypto::sealed::recipient_id;
    use ac_crypto::sig::SecretSeed;
    use ac_crypto::{KemAlg, SigAlg, account_id};
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn round_trip_over_http_with_large_messages() {
        let kem =
            Arc::new(KemSecretKey::from_seed(KemAlg::XWing, &SecretSeed::new([3; 32])).unwrap());
        let recipient = kem.public_key().unwrap();
        let signer = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([4; 32])).unwrap();
        let sender = to_account32(&account_id(&signer.public_key().unwrap()));
        let key = signer.public_key().unwrap();
        let replay = Arc::new(Mutex::new(ReplayCache::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let rid = recipient_id(&recipient).unwrap();
        tokio::spawn(http::serve(listener, move |req: http::Request| {
            let (kem, replay, key) = (Arc::clone(&kem), Arc::clone(&replay), key.clone());
            async move {
                let body = http::read_body(req.into_body(), 32 << 20).await.unwrap();
                let acceptor = Acceptor {
                    secret: &kem,
                    recipient: rid,
                    now: now_secs(),
                };
                let accepted = accept(&body, &acceptor, |_| Some(key), &mut replay.lock().unwrap());
                match accepted {
                    Err(r) => r.response(),
                    Ok(req) => {
                        let (mut w, resp, _, msg) = req.respond();
                        tokio::spawn(async move {
                            w.send(&msg, false).await;
                            w.send(&vec![7u8; 200_000], false).await;
                            w.send(b"end", true).await;
                        });
                        resp
                    }
                }
            }
        }));
        let client = Client::new().unwrap();
        let big: Vec<u8> = (0..150_000u32)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect();
        let mut resp = post(&client, &base, &recipient, (&sender, &signer), &big)
            .await
            .unwrap();
        assert_eq!(resp.next().await.unwrap().unwrap(), big);
        assert_eq!(resp.next().await.unwrap().unwrap(), vec![7u8; 200_000]);
        assert_eq!(resp.next().await.unwrap().unwrap(), b"end");
        assert_eq!(resp.next().await.unwrap(), None);

        // A handshake from an account whose key the recipient does not accept.
        let other = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([5; 32])).unwrap();
        let err = post(&client, &base, &recipient, (&sender, &other), b"x")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("401"), "{err}");
    }
}
