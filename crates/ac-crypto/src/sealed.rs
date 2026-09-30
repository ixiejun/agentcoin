//! Sealed request channel (feature `sealed`): one request and its streamed response between
//! off-chain services, encrypted end to end and bound to the sender's account.
//!
//! The sender encapsulates to the recipient's X-Wing key, signs the handshake with its account's
//! ML-DSA key and derives one ChaCha20-Poly1305 key per direction from the shared secret and the
//! signed handshake. Each direction is a sequence of chunks whose nonce is the chunk number, so
//! tampering, reordering, replay within the stream and truncation are all detected. The
//! recipient refuses stale handshakes and handshakes it has already accepted.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use zeroize::Zeroizing;

use crate::account::AccountId;
use crate::error::{Error, SealedError};
use crate::hash::derive;
use crate::kem::KemSecretKey;
use crate::sig::verify;
use crate::tagged::{KemCiphertext, KemPublicKey, PqPublicKey, PqSignature};

/// Protocol version carried in every handshake.
pub const VERSION: u8 = 1;

/// Signing context of handshakes. Never reuse it for another purpose.
pub const SEALED_SIGNING_CONTEXT: &[u8] = b"agentcoin/sealed-channel/v1";

/// Hash context of a recipient's key digest.
pub const SEALED_RECIPIENT_CONTEXT: &str = "agentcoin 2026-09 sealed-recipient v1";

/// Hash context of the message signed in a handshake.
pub const SEALED_HANDSHAKE_CONTEXT: &str = "agentcoin 2026-09 sealed-handshake v1";

/// Key-derivation context of the per-direction keys.
pub const SEALED_KEY_CONTEXT: &str = "agentcoin 2026-09 sealed-key v1";

/// Largest plaintext of one chunk.
pub const MAX_CHUNK: usize = 64 * 1024;

/// Seconds a handshake stays acceptable on either side of its creation time.
pub const VALIDITY_SECS: u64 = 120;

const TAG_LEN: usize = 16;
const NONCE_LEN: usize = 32;

/// Direction of a chunk stream; each has its own key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Request = 1,
    Response = 2,
}

/// Digest of a recipient's encapsulation key, as named in handshakes.
///
/// # Errors
///
/// Never in practice (the context is valid); propagated for uniformity.
pub fn recipient_id(recipient: &KemPublicKey) -> Result<[u8; 32], Error> {
    derive(SEALED_RECIPIENT_CONTEXT, &recipient.to_canonical())
}

/// The unsigned handshake.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handshake {
    /// Digest of the recipient's encapsulation key ([`recipient_id`]).
    pub recipient: [u8; 32],
    /// KEM ciphertext encapsulated to the recipient.
    pub kem_ct: KemCiphertext,
    /// The sender's account.
    pub sender: AccountId,
    /// The sender's current public key.
    pub sender_key: PqPublicKey,
    /// Creation time, Unix seconds.
    pub created: u64,
    /// Fresh randomness.
    pub nonce: [u8; NONCE_LEN],
}

impl Handshake {
    /// Fixed-layout encoding: version, recipient, KEM ciphertext, sender, sender key, creation
    /// time, nonce; variable fields carry a 2-byte little-endian length.
    fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut out = Vec::with_capacity(4096);
        out.push(VERSION);
        out.extend_from_slice(&self.recipient);
        put_var(&mut out, &self.kem_ct.to_canonical())?;
        out.extend_from_slice(self.sender.as_bytes());
        put_var(&mut out, &self.sender_key.to_canonical())?;
        out.extend_from_slice(&self.created.to_le_bytes());
        out.extend_from_slice(&self.nonce);
        Ok(out)
    }

    fn decode(input: &mut &[u8]) -> Result<Self, Error> {
        if take::<1>(input)? != [VERSION] {
            return Err(SealedError::Malformed.into());
        }
        let recipient = take::<32>(input)?;
        let kem_ct = KemCiphertext::from_canonical(take_var(input)?)
            .map_err(|_| Error::from(SealedError::Malformed))?;
        let sender = AccountId(take::<32>(input)?);
        let sender_key = PqPublicKey::from_canonical(take_var(input)?)
            .map_err(|_| Error::from(SealedError::Malformed))?;
        let created = u64::from_le_bytes(take::<8>(input)?);
        let nonce = take::<NONCE_LEN>(input)?;
        Ok(Self {
            recipient,
            kem_ct,
            sender,
            sender_key,
            created,
            nonce,
        })
    }

    fn signing_message(&self) -> Result<[u8; 32], Error> {
        derive(SEALED_HANDSHAKE_CONTEXT, &self.encode()?)
    }
}

/// A handshake with the sender's signature; the first thing sent on a channel.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedHandshake {
    /// The handshake.
    pub handshake: Handshake,
    /// ML-DSA signature over its digest under [`SEALED_SIGNING_CONTEXT`].
    pub signature: PqSignature,
}

impl SignedHandshake {
    /// Wire encoding: the handshake followed by the length-prefixed signature.
    ///
    /// # Errors
    ///
    /// [`Error::InputTooLong`] never in practice (all fields are bounded).
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut out = self.handshake.encode()?;
        put_var(&mut out, &self.signature.to_canonical())?;
        Ok(out)
    }

    /// Decodes a wire encoding; trailing bytes are rejected.
    ///
    /// # Errors
    ///
    /// [`SealedError::Malformed`] for any malformed input.
    pub fn decode(mut input: &[u8]) -> Result<Self, Error> {
        let handshake = Handshake::decode(&mut input)?;
        let signature = PqSignature::from_canonical(take_var(&mut input)?)
            .map_err(|_| Error::from(SealedError::Malformed))?;
        if !input.is_empty() {
            return Err(SealedError::Malformed.into());
        }
        Ok(Self {
            handshake,
            signature,
        })
    }

    /// The KEM ciphertext's digest: the key of the replay cache.
    fn replay_key(&self) -> Result<[u8; 32], Error> {
        derive(
            SEALED_HANDSHAKE_CONTEXT,
            &self.handshake.kem_ct.to_canonical(),
        )
    }
}

/// Encrypts one direction of a channel, chunk by chunk.
pub struct Sealer {
    key: Zeroizing<[u8; 32]>,
    direction: Direction,
    seq: u64,
    done: bool,
}

impl core::fmt::Debug for Sealer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Sealer")
            .field("direction", &self.direction)
            .field("seq", &self.seq)
            .finish_non_exhaustive()
    }
}

impl Sealer {
    /// Encrypts the next chunk; `last` marks the end of the stream. The wire form is the
    /// final flag (one byte) followed by the ciphertext and tag.
    ///
    /// # Errors
    ///
    /// [`SealedError::ChunkTooLarge`] above [`MAX_CHUNK`], [`SealedError::AfterFinal`] after
    /// the last chunk.
    pub fn seal(&mut self, plaintext: &[u8], last: bool) -> Result<Vec<u8>, Error> {
        if self.done {
            return Err(SealedError::AfterFinal.into());
        }
        if plaintext.len() > MAX_CHUNK {
            return Err(SealedError::ChunkTooLarge.into());
        }
        let flag = u8::from(last);
        let aad = chunk_aad(self.direction, self.seq, flag);
        let ct = cipher(&self.key)
            .encrypt(
                &chunk_nonce(self.seq),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| Error::from(SealedError::Authentication))?;
        self.seq = self.seq.checked_add(1).ok_or(SealedError::AfterFinal)?;
        self.done = last;
        let mut out = Vec::with_capacity(ct.len().saturating_add(1));
        out.push(flag);
        out.extend_from_slice(&ct);
        Ok(out)
    }
}

/// Decrypts one direction of a channel, chunk by chunk, in order.
pub struct Opener {
    key: Zeroizing<[u8; 32]>,
    direction: Direction,
    seq: u64,
    done: bool,
    failed: bool,
}

impl core::fmt::Debug for Opener {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Opener")
            .field("direction", &self.direction)
            .field("seq", &self.seq)
            .finish_non_exhaustive()
    }
}

impl Opener {
    /// Decrypts the next chunk and says whether it was the last one. After any error the
    /// opener stays failed.
    ///
    /// # Errors
    ///
    /// [`SealedError::Authentication`] for a tampered, reordered or foreign chunk,
    /// [`SealedError::AfterFinal`] for data after the last chunk,
    /// [`SealedError::Malformed`] for a chunk shorter than a tag.
    pub fn open(&mut self, chunk: &[u8]) -> Result<(Vec<u8>, bool), Error> {
        if self.failed {
            return Err(SealedError::Authentication.into());
        }
        if self.done {
            self.failed = true;
            return Err(SealedError::AfterFinal.into());
        }
        let result = self.open_inner(chunk);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn open_inner(&mut self, chunk: &[u8]) -> Result<(Vec<u8>, bool), Error> {
        let (flag, ct) = chunk.split_first().ok_or(SealedError::Malformed)?;
        if *flag > 1 || ct.len() < TAG_LEN || ct.len() > MAX_CHUNK.saturating_add(TAG_LEN) {
            return Err(SealedError::Malformed.into());
        }
        let aad = chunk_aad(self.direction, self.seq, *flag);
        let plain = cipher(&self.key)
            .decrypt(&chunk_nonce(self.seq), Payload { msg: ct, aad: &aad })
            .map_err(|_| Error::from(SealedError::Authentication))?;
        self.seq = self.seq.checked_add(1).ok_or(SealedError::AfterFinal)?;
        self.done = *flag == 1;
        Ok((plain, self.done))
    }

    /// Call when the transport ends: succeeds only if the last chunk was seen.
    ///
    /// # Errors
    ///
    /// [`SealedError::Truncated`] if the stream ended before its last chunk.
    pub fn finish(&self) -> Result<(), Error> {
        if self.done && !self.failed {
            Ok(())
        } else {
            Err(SealedError::Truncated.into())
        }
    }
}

/// The sender's side of a channel: seals the request, opens the response.
#[derive(Debug)]
pub struct SenderSession {
    /// Encrypts the request.
    pub request: Sealer,
    /// Decrypts the response.
    pub response: Opener,
}

impl SenderSession {
    /// The derived (request, response) keys, for regression vectors only.
    #[cfg(feature = "deterministic")]
    #[doc(hidden)]
    #[must_use]
    pub fn vector_keys(&self) -> ([u8; 32], [u8; 32]) {
        (*self.request.key, *self.response.key)
    }
}

/// The recipient's side of a channel: opens the request, seals the response.
#[derive(Debug)]
pub struct RecipientSession {
    /// The authenticated sender.
    pub sender: AccountId,
    /// Decrypts the request.
    pub request: Opener,
    /// Encrypts the response.
    pub response: Sealer,
}

/// Opens a channel to `recipient` as the holder of `signer` (whose account is `sender`).
///
/// # Errors
///
/// KEM, signing or randomness failures.
#[cfg(feature = "rand")]
pub fn open_session<R: rand_core::CryptoRng + ?Sized>(
    recipient: &KemPublicKey,
    signer: &crate::sig::SigningKey,
    sender: AccountId,
    created: u64,
    rng: &mut R,
) -> Result<(SignedHandshake, SenderSession), Error> {
    let (kem_ct, ss) = crate::kem::encapsulate(recipient, rng)?;
    let mut nonce = [0u8; NONCE_LEN];
    rng.fill_bytes(&mut nonce);
    let handshake = Handshake {
        recipient: recipient_id(recipient)?,
        kem_ct,
        sender,
        sender_key: signer.public_key()?,
        created,
        nonce,
    };
    let signature = signer.sign(&handshake.signing_message()?, SEALED_SIGNING_CONTEXT, rng)?;
    finish_sender(handshake, signature, ss.expose())
}

/// Deterministic [`open_session`] for test vectors: caller-supplied KEM randomness and nonce,
/// deterministic signing. Never use it outside tests and tooling.
///
/// # Errors
///
/// As [`open_session`].
#[cfg(feature = "deterministic")]
pub fn open_session_derandomized(
    recipient: &KemPublicKey,
    signer: &crate::sig::SigningKey,
    (sender, created): (AccountId, u64),
    nonce: [u8; NONCE_LEN],
    kem_randomness: &[u8; crate::kem::ENCAPSULATION_RANDOMNESS_LEN],
) -> Result<(SignedHandshake, SenderSession), Error> {
    let (kem_ct, ss) = crate::kem::encapsulate_derandomized(recipient, kem_randomness)?;
    let handshake = Handshake {
        recipient: recipient_id(recipient)?,
        kem_ct,
        sender,
        sender_key: signer.public_key()?,
        created,
        nonce,
    };
    let signature =
        signer.sign_deterministic(&handshake.signing_message()?, SEALED_SIGNING_CONTEXT)?;
    finish_sender(handshake, signature, ss.expose())
}

#[cfg(any(feature = "rand", feature = "deterministic"))]
fn finish_sender(
    handshake: Handshake,
    signature: PqSignature,
    shared: &[u8; 32],
) -> Result<(SignedHandshake, SenderSession), Error> {
    let signed = SignedHandshake {
        handshake,
        signature,
    };
    let (req, resp) = derive_keys(shared, &signed)?;
    Ok((
        signed,
        SenderSession {
            request: sealer(req, Direction::Request),
            response: opener(resp, Direction::Response),
        },
    ))
}

/// What the recipient needs to accept a handshake.
pub struct Acceptor<'a> {
    /// The recipient's decapsulation key.
    pub secret: &'a KemSecretKey,
    /// Digest of the matching encapsulation key ([`recipient_id`]).
    pub recipient: [u8; 32],
    /// Current time, Unix seconds.
    pub now: u64,
}

impl core::fmt::Debug for Acceptor<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Acceptor")
            .field("now", &self.now)
            .finish_non_exhaustive()
    }
}

/// Accepts a handshake: checks recipient, freshness, the sender's on-chain key (`key_of`
/// returns the account's current key), the signature and replay, then derives the keys.
///
/// # Errors
///
/// [`SealedError::WrongRecipient`], [`SealedError::Expired`], [`SealedError::UnknownSender`],
/// [`SealedError::KeyMismatch`], [`Error::InvalidSignature`], [`SealedError::Replayed`] or
/// [`SealedError::ReplayCacheFull`]; nothing is decrypted before all checks pass.
pub fn accept_session(
    signed: &SignedHandshake,
    acceptor: &Acceptor<'_>,
    key_of: impl FnOnce(&AccountId) -> Option<PqPublicKey>,
    replay: &mut ReplayCache,
) -> Result<RecipientSession, Error> {
    let hs = &signed.handshake;
    if hs.recipient != acceptor.recipient {
        return Err(SealedError::WrongRecipient.into());
    }
    if acceptor.now.abs_diff(hs.created) > VALIDITY_SECS {
        return Err(SealedError::Expired.into());
    }
    let current = key_of(&hs.sender).ok_or(SealedError::UnknownSender)?;
    if current != hs.sender_key {
        return Err(SealedError::KeyMismatch.into());
    }
    verify(
        &hs.sender_key,
        &hs.signing_message()?,
        SEALED_SIGNING_CONTEXT,
        &signed.signature,
    )?;
    replay.insert(signed.replay_key()?, hs.created, acceptor.now)?;
    let shared = acceptor.secret.decapsulate(&hs.kem_ct)?;
    let (req, resp) = derive_keys(shared.expose(), signed)?;
    Ok(RecipientSession {
        sender: hs.sender,
        request: opener(req, Direction::Request),
        response: sealer(resp, Direction::Response),
    })
}

/// Handshakes accepted within the validity window, so a captured request cannot run twice.
///
/// Entries expire once their handshake could no longer pass the freshness check. When full,
/// new handshakes are refused rather than evicting live entries.
#[derive(Debug)]
pub struct ReplayCache {
    seen: BTreeSet<[u8; 32]>,
    by_expiry: BTreeMap<(u64, [u8; 32]), ()>,
    capacity: usize,
}

impl ReplayCache {
    /// Default capacity.
    pub const DEFAULT_CAPACITY: usize = 1_000_000;

    /// An empty cache holding at most `capacity` live entries.
    #[must_use]
    pub const fn new(capacity: usize) -> Self {
        Self {
            seen: BTreeSet::new(),
            by_expiry: BTreeMap::new(),
            capacity,
        }
    }

    /// Number of live entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    fn insert(&mut self, key: [u8; 32], created: u64, now: u64) -> Result<(), Error> {
        // Drop entries whose handshakes are already too old to pass the freshness check.
        while let Some((&(expiry, old), ())) = self.by_expiry.first_key_value() {
            if expiry >= now {
                break;
            }
            self.by_expiry.remove(&(expiry, old));
            self.seen.remove(&old);
        }
        if self.seen.contains(&key) {
            return Err(SealedError::Replayed.into());
        }
        if self.seen.len() >= self.capacity {
            return Err(SealedError::ReplayCacheFull.into());
        }
        let expiry = created.saturating_add(VALIDITY_SECS);
        self.seen.insert(key);
        self.by_expiry.insert((expiry, key), ());
        Ok(())
    }
}

impl Default for ReplayCache {
    fn default() -> Self {
        Self::new(Self::DEFAULT_CAPACITY)
    }
}

type KeyPair = (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>);

fn derive_keys(shared: &[u8; 32], signed: &SignedHandshake) -> Result<KeyPair, Error> {
    let transcript = signed.encode()?;
    let key = |dir: Direction| -> Result<Zeroizing<[u8; 32]>, Error> {
        let mut input = Zeroizing::new(Vec::with_capacity(transcript.len().saturating_add(33)));
        input.push(dir as u8);
        input.extend_from_slice(shared);
        input.extend_from_slice(&transcript);
        Ok(Zeroizing::new(derive(SEALED_KEY_CONTEXT, &input)?))
    };
    Ok((key(Direction::Request)?, key(Direction::Response)?))
}

const fn sealer(key: Zeroizing<[u8; 32]>, direction: Direction) -> Sealer {
    Sealer {
        key,
        direction,
        seq: 0,
        done: false,
    }
}

const fn opener(key: Zeroizing<[u8; 32]>, direction: Direction) -> Opener {
    Opener {
        key,
        direction,
        seq: 0,
        done: false,
        failed: false,
    }
}

fn cipher(key: &[u8; 32]) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(&Key::from(*key))
}

/// 96-bit nonce: four zero bytes and the big-endian chunk number. Each key serves one
/// direction of one session, so nonces never repeat under a key.
fn chunk_nonce(seq: u64) -> Nonce {
    let s = seq.to_be_bytes();
    Nonce::from([0, 0, 0, 0, s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
}

fn chunk_aad(direction: Direction, seq: u64, flag: u8) -> [u8; 10] {
    let s = seq.to_be_bytes();
    [
        direction as u8,
        s[0],
        s[1],
        s[2],
        s[3],
        s[4],
        s[5],
        s[6],
        s[7],
        flag,
    ]
}

fn put_var(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Error> {
    let len = u16::try_from(bytes.len()).map_err(|_| Error::InputTooLong)?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

fn take<const N: usize>(input: &mut &[u8]) -> Result<[u8; N], Error> {
    let (head, rest) = input.split_at_checked(N).ok_or(SealedError::Malformed)?;
    *input = rest;
    head.try_into().map_err(|_| SealedError::Malformed.into())
}

fn take_var<'a>(input: &mut &'a [u8]) -> Result<&'a [u8], Error> {
    let len = usize::from(u16::from_le_bytes(take::<2>(input)?));
    let (head, rest) = input.split_at_checked(len).ok_or(SealedError::Malformed)?;
    *input = rest;
    Ok(head)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::account_id;
    use crate::alg::{KemAlg, SigAlg};
    use crate::sig::{SecretSeed, SigningKey};
    use proptest::prelude::*;

    /// Deterministic test RNG (never used outside tests).
    struct TestRng(u64);
    impl rand_core::TryRng for TestRng {
        type Error = core::convert::Infallible;
        fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
            Ok(u32::try_from(self.try_next_u64()? >> 32).unwrap())
        }
        fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            Ok(self.0)
        }
        fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
            for b in dst {
                *b = u8::try_from(self.try_next_u64()? >> 56).unwrap();
            }
            Ok(())
        }
    }
    impl rand_core::TryCryptoRng for TestRng {}

    const NOW: u64 = 1_800_000_000;

    struct Parties {
        recipient_sk: KemSecretKey,
        recipient_pk: KemPublicKey,
        signer: SigningKey,
        sender: AccountId,
    }

    fn parties(seed: u8) -> Parties {
        let recipient_sk =
            KemSecretKey::from_seed(KemAlg::XWing, &SecretSeed::new([seed; 32])).unwrap();
        let recipient_pk = recipient_sk.public_key().unwrap();
        let signer = SigningKey::from_seed(
            SigAlg::MlDsa44,
            &SecretSeed::new([seed.wrapping_add(1); 32]),
        )
        .unwrap();
        let sender = account_id(&signer.public_key().unwrap());
        Parties {
            recipient_sk,
            recipient_pk,
            signer,
            sender,
        }
    }

    fn open(p: &Parties, rng: u64, created: u64) -> (SignedHandshake, SenderSession) {
        open_session(
            &p.recipient_pk,
            &p.signer,
            p.sender,
            created,
            &mut TestRng(rng),
        )
        .unwrap()
    }

    fn accept(
        p: &Parties,
        hs: &SignedHandshake,
        now: u64,
        cache: &mut ReplayCache,
    ) -> Result<RecipientSession, Error> {
        let key = p.signer.public_key().unwrap();
        let sender = p.sender;
        accept_session(
            hs,
            &Acceptor {
                secret: &p.recipient_sk,
                recipient: recipient_id(&p.recipient_pk).unwrap(),
                now,
            },
            |who| (*who == sender).then_some(key),
            cache,
        )
    }

    fn sealed(e: SealedError) -> Error {
        Error::Sealed(e)
    }

    // Scenario "双方得到相同密钥": both directions work end to end with distinct keys.
    #[test]
    fn both_sides_derive_the_same_keys() {
        let p = parties(1);
        let (hs, mut tx) = open(&p, 1, NOW);
        let wire = SignedHandshake::decode(&hs.encode().unwrap()).unwrap();
        assert_eq!(wire, hs);
        let mut rx = accept(&p, &wire, NOW, &mut ReplayCache::default()).unwrap();
        assert_eq!(rx.sender, p.sender);
        assert_eq!(*tx.request.key, *rx.request.key);
        assert_eq!(*tx.response.key, *rx.response.key);
        assert_ne!(*tx.request.key, *tx.response.key);

        let c = tx.request.seal(b"hello", true).unwrap();
        assert_eq!(rx.request.open(&c).unwrap(), (b"hello".to_vec(), true));
        rx.request.finish().unwrap();
        let r1 = rx.response.seal(b"wor", false).unwrap();
        let r2 = rx.response.seal(b"ld", true).unwrap();
        assert_eq!(tx.response.open(&r1).unwrap(), (b"wor".to_vec(), false));
        assert_eq!(tx.response.open(&r2).unwrap(), (b"ld".to_vec(), true));
        tx.response.finish().unwrap();
    }

    // Scenario "发往错误的接收方".
    #[test]
    fn wrong_recipient_is_rejected() {
        let (p, q) = (parties(1), parties(9));
        let (hs, _) = open(&p, 2, NOW);
        let err = accept_session(
            &hs,
            &Acceptor {
                secret: &q.recipient_sk,
                recipient: recipient_id(&q.recipient_pk).unwrap(),
                now: NOW,
            },
            |_| Some(p.signer.public_key().unwrap()),
            &mut ReplayCache::default(),
        )
        .unwrap_err();
        assert_eq!(err, sealed(SealedError::WrongRecipient));
    }

    // Scenario "冒用他人账户": the handshake claims account A but is signed with another key.
    #[test]
    fn impersonation_is_rejected() {
        let p = parties(1);
        let mallory = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([77; 32])).unwrap();
        // Signed by Mallory's key, naming Alice's account: the key is not Alice's.
        let (hs, _) =
            open_session(&p.recipient_pk, &mallory, p.sender, NOW, &mut TestRng(3)).unwrap();
        let err = accept(&p, &hs, NOW, &mut ReplayCache::default()).unwrap_err();
        assert_eq!(err, sealed(SealedError::KeyMismatch));

        // Alice's key in the handshake but Mallory's signature: the signature fails.
        let (mut forged, _) = open(&p, 4, NOW);
        let (other, _) =
            open_session(&p.recipient_pk, &mallory, p.sender, NOW, &mut TestRng(4)).unwrap();
        forged.signature = other.signature;
        let err = accept(&p, &forged, NOW, &mut ReplayCache::default()).unwrap_err();
        assert!(matches!(
            err,
            Error::InvalidSignature | Error::AlgorithmMismatch
        ));

        // An account without a registered key.
        let err = accept_session(
            &hs,
            &Acceptor {
                secret: &p.recipient_sk,
                recipient: recipient_id(&p.recipient_pk).unwrap(),
                now: NOW,
            },
            |_| None,
            &mut ReplayCache::default(),
        )
        .unwrap_err();
        assert_eq!(err, sealed(SealedError::UnknownSender));
    }

    // Scenario "响应来自持钥方": a third party without the recipient key cannot forge a response.
    #[test]
    fn forged_response_is_rejected() {
        let p = parties(1);
        let (_, mut tx) = open(&p, 5, NOW);
        let mut fake = sealer(Zeroizing::new([42; 32]), Direction::Response);
        let chunk = fake.seal(b"trust me", true).unwrap();
        assert_eq!(
            tx.response.open(&chunk).unwrap_err(),
            sealed(SealedError::Authentication)
        );
        assert_eq!(
            tx.response.finish().unwrap_err(),
            sealed(SealedError::Truncated)
        );
    }

    // Scenario "篡改一个块".
    #[test]
    fn tampered_chunk_is_rejected() {
        let p = parties(1);
        let (hs, mut tx) = open(&p, 6, NOW);
        let mut rx = accept(&p, &hs, NOW, &mut ReplayCache::default()).unwrap();
        let mut c = tx.request.seal(b"prompt", true).unwrap();
        if let Some(b) = c.get_mut(3) {
            *b ^= 1;
        }
        assert_eq!(
            rx.request.open(&c).unwrap_err(),
            sealed(SealedError::Authentication)
        );
        // The final flag is authenticated too.
        let (hs, mut tx) = open(&p, 7, NOW);
        let mut rx = accept(&p, &hs, NOW, &mut ReplayCache::default()).unwrap();
        let mut c = tx.request.seal(b"prompt", false).unwrap();
        if let Some(b) = c.first_mut() {
            *b = 1;
        }
        assert_eq!(
            rx.request.open(&c).unwrap_err(),
            sealed(SealedError::Authentication)
        );
    }

    // Scenario "截断".
    #[test]
    fn truncation_is_detected() {
        let p = parties(1);
        let (hs, mut tx) = open(&p, 8, NOW);
        let mut rx = accept(&p, &hs, NOW, &mut ReplayCache::default()).unwrap();
        let c = tx.request.seal(b"part one", false).unwrap();
        rx.request.open(&c).unwrap();
        assert_eq!(
            rx.request.finish().unwrap_err(),
            sealed(SealedError::Truncated)
        );
    }

    // Scenario "块被重排", plus data after the last chunk and oversize chunks.
    #[test]
    fn reordering_and_extra_data_are_rejected() {
        let p = parties(1);
        let (hs, mut tx) = open(&p, 9, NOW);
        let mut rx = accept(&p, &hs, NOW, &mut ReplayCache::default()).unwrap();
        let a = tx.request.seal(b"a", false).unwrap();
        let b = tx.request.seal(b"b", true).unwrap();
        assert_eq!(
            rx.request.open(&b).unwrap_err(),
            sealed(SealedError::Authentication)
        );
        // A failed opener stays failed.
        assert!(rx.request.open(&a).is_err());

        let (hs, mut tx) = open(&p, 10, NOW);
        let mut rx = accept(&p, &hs, NOW, &mut ReplayCache::default()).unwrap();
        let a = tx.request.seal(b"a", true).unwrap();
        rx.request.open(&a).unwrap();
        assert_eq!(
            rx.request.open(&a).unwrap_err(),
            sealed(SealedError::AfterFinal)
        );
        assert_eq!(
            tx.request.seal(b"x", true).unwrap_err(),
            sealed(SealedError::AfterFinal)
        );

        let (_, mut tx) = open(&p, 11, NOW);
        let big = alloc::vec![0u8; MAX_CHUNK + 1];
        assert_eq!(
            tx.request.seal(&big, true).unwrap_err(),
            sealed(SealedError::ChunkTooLarge)
        );
        assert!(tx.request.seal(&big[..MAX_CHUNK], true).is_ok());
    }

    // Scenario "重放同一请求".
    #[test]
    fn replayed_handshake_is_rejected() {
        let p = parties(1);
        let (hs, _) = open(&p, 12, NOW);
        let mut cache = ReplayCache::default();
        accept(&p, &hs, NOW, &mut cache).unwrap();
        assert_eq!(
            accept(&p, &hs, NOW + 5, &mut cache).unwrap_err(),
            sealed(SealedError::Replayed)
        );
    }

    // Scenario "过期握手": 121 seconds old (or in the future) is refused; 120 is accepted.
    #[test]
    fn stale_handshakes_are_rejected() {
        let p = parties(1);
        let (hs, _) = open(&p, 13, NOW - 121);
        assert_eq!(
            accept(&p, &hs, NOW, &mut ReplayCache::default()).unwrap_err(),
            sealed(SealedError::Expired)
        );
        let (hs, _) = open(&p, 14, NOW + 121);
        assert_eq!(
            accept(&p, &hs, NOW, &mut ReplayCache::default()).unwrap_err(),
            sealed(SealedError::Expired)
        );
        let (hs, _) = open(&p, 15, NOW - 120);
        assert!(accept(&p, &hs, NOW, &mut ReplayCache::default()).is_ok());
    }

    // "缓存满时拒绝新握手" and "过期项被清除后同一密文仍因时效被拒".
    #[test]
    fn replay_cache_capacity_and_expiry() {
        let p = parties(1);
        let mut cache = ReplayCache::new(2);
        let hs: Vec<_> = (0..3).map(|i| open(&p, 20 + i, NOW).0).collect();
        accept(&p, &hs[0], NOW, &mut cache).unwrap();
        accept(&p, &hs[1], NOW, &mut cache).unwrap();
        assert_eq!(
            accept(&p, &hs[2], NOW, &mut cache).unwrap_err(),
            sealed(SealedError::ReplayCacheFull)
        );
        assert_eq!(cache.len(), 2);
        // Once the entries expire the cache has room again, and the old handshake is still
        // refused, now by the freshness check.
        let late = NOW + VALIDITY_SECS + 1;
        let fresh = open(&p, 30, late).0;
        accept(&p, &fresh, late, &mut cache).unwrap();
        assert_eq!(cache.len(), 1);
        assert_eq!(
            accept(&p, &hs[0], late, &mut cache).unwrap_err(),
            sealed(SealedError::Expired)
        );
    }

    #[test]
    fn malformed_handshakes_are_rejected() {
        let p = parties(1);
        let (hs, _) = open(&p, 40, NOW);
        let bytes = hs.encode().unwrap();
        let mut extra = bytes.clone();
        extra.push(0);
        assert_eq!(
            SignedHandshake::decode(&extra).unwrap_err(),
            sealed(SealedError::Malformed)
        );
        assert!(SignedHandshake::decode(&bytes[..bytes.len() - 1]).is_err());
        let mut v2 = bytes;
        v2[0] = 2;
        assert_eq!(
            SignedHandshake::decode(&v2).unwrap_err(),
            sealed(SealedError::Malformed)
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        // Any split of a message into chunks round-trips.
        #[test]
        fn any_chunking_round_trips(
            data in proptest::collection::vec(any::<u8>(), 0..4096),
            cuts in proptest::collection::vec(0usize..4096, 0..8),
        ) {
            let p = parties(2);
            let (hs, mut tx) = open(&p, 50, NOW);
            let mut rx = accept(&p, &hs, NOW, &mut ReplayCache::default()).unwrap();
            let mut cuts: Vec<usize> = cuts.into_iter().map(|c| c % (data.len() + 1)).collect();
            cuts.push(data.len());
            cuts.sort_unstable();
            let mut start = 0;
            let mut out = Vec::new();
            for (i, &end) in cuts.iter().enumerate() {
                let last = i + 1 == cuts.len();
                let c = tx.request.seal(&data[start..end], last).unwrap();
                let (plain, fin) = rx.request.open(&c).unwrap();
                prop_assert_eq!(fin, last);
                out.extend_from_slice(&plain);
                start = end;
            }
            rx.request.finish().unwrap();
            prop_assert_eq!(out, data);
        }

        // Any single-bit flip in a chunk is detected.
        #[test]
        fn any_bit_flip_is_detected(data in proptest::collection::vec(any::<u8>(), 0..256), bit in 0usize..8192) {
            let p = parties(3);
            let (hs, mut tx) = open(&p, 60, NOW);
            let mut rx = accept(&p, &hs, NOW, &mut ReplayCache::default()).unwrap();
            let mut c = tx.request.seal(&data, true).unwrap();
            let bit = bit % (c.len() * 8);
            c[bit / 8] ^= 1 << (bit % 8);
            prop_assert!(rx.request.open(&c).is_err());
        }
    }
}
