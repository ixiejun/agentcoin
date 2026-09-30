//! Prints `tests/vectors/sealed_channel.json`: a sealed-channel session from fixed inputs.
//!
//! Run once with
//! `cargo run -p ac-crypto --features sealed,deterministic --example gen_sealed_channel_vectors > tests/vectors/sealed_channel.json`.
//! The committed output is a regression vector and must never change (a new format needs a new
//! protocol version and new contexts).

use ac_crypto::kem::KemSecretKey;
use ac_crypto::sealed::{
    Acceptor, ReplayCache, accept_session, open_session_derandomized, recipient_id,
};
use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::{KemAlg, SigAlg, account_id};

const KEM_SEED: [u8; 32] = [0x11; 32];
const KEM_RANDOMNESS: [u8; 64] = [0x22; 64];
const SIGNING_SEED: [u8; 32] = [0x33; 32];
const NONCE: [u8; 32] = [0x44; 32];
const CREATED: u64 = 1_800_000_000;
const REQUEST: [(&str, bool); 2] = [("hello, ", false), ("sealed world", true)];
const RESPONSE: [(&str, bool); 1] = [("ok", true)];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn main() -> Result<(), ac_crypto::Error> {
    let kem = KemSecretKey::from_seed(KemAlg::XWing, &SecretSeed::new(KEM_SEED))?;
    let recipient = kem.public_key()?;
    let signer = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new(SIGNING_SEED))?;
    let sender = account_id(&signer.public_key()?);
    let (handshake, mut tx) = open_session_derandomized(
        &recipient,
        &signer,
        (sender, CREATED),
        NONCE,
        &KEM_RANDOMNESS,
    )?;
    let (k_req, k_resp) = tx.vector_keys();
    let key = signer.public_key()?;
    let mut rx = accept_session(
        &handshake,
        &Acceptor {
            secret: &kem,
            recipient: recipient_id(&recipient)?,
            now: CREATED,
        },
        |_| Some(key),
        &mut ReplayCache::default(),
    )?;
    let mut request = Vec::new();
    for (text, last) in REQUEST {
        let chunk = tx.request.seal(text.as_bytes(), last)?;
        rx.request.open(&chunk)?;
        request.push(format!(
            "    {{ \"plaintext\": \"{}\", \"last\": {last}, \"chunk\": \"{}\" }}",
            hex(text.as_bytes()),
            hex(&chunk)
        ));
    }
    let mut response = Vec::new();
    for (text, last) in RESPONSE {
        let chunk = rx.response.seal(text.as_bytes(), last)?;
        tx.response.open(&chunk)?;
        response.push(format!(
            "    {{ \"plaintext\": \"{}\", \"last\": {last}, \"chunk\": \"{}\" }}",
            hex(text.as_bytes()),
            hex(&chunk)
        ));
    }
    println!("{{");
    println!("  \"kem_seed\": \"{}\",", hex(&KEM_SEED));
    println!("  \"kem_randomness\": \"{}\",", hex(&KEM_RANDOMNESS));
    println!("  \"signing_alg_id\": {},", SigAlg::MlDsa44.id());
    println!("  \"signing_seed\": \"{}\",", hex(&SIGNING_SEED));
    println!("  \"nonce\": \"{}\",", hex(&NONCE));
    println!("  \"created\": {CREATED},");
    println!(
        "  \"recipient_id\": \"{}\",",
        hex(&recipient_id(&recipient)?)
    );
    println!("  \"handshake\": \"{}\",", hex(&handshake.encode()?));
    println!("  \"request_key\": \"{}\",", hex(&k_req));
    println!("  \"response_key\": \"{}\",", hex(&k_resp));
    println!("  \"request\": [\n{}\n  ],", request.join(",\n"));
    println!("  \"response\": [\n{}\n  ]", response.join(",\n"));
    println!("}}");
    Ok(())
}
