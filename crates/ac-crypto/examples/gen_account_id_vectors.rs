//! Prints `tests/vectors/account_id.json`: fixed seeds → public keys → account IDs.
//!
//! Run once with `cargo run -p ac-crypto --example gen_account_id_vectors > tests/vectors/account_id.json`.
//! The committed output is a consensus regression vector and must never change.

use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::{SigAlg, account_id};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn main() -> Result<(), ac_crypto::Error> {
    let cases = [
        (SigAlg::MlDsa44, [0x11u8; 32]),
        (SigAlg::MlDsa65, [0x22u8; 32]),
    ];
    let mut rows = Vec::new();
    for (alg, seed) in cases {
        let pk = SigningKey::from_seed(alg, &SecretSeed::new(seed))?.public_key()?;
        rows.push(format!(
            "  {{\n    \"alg_id\": {},\n    \"seed\": \"{}\",\n    \"public_key\": \"{}\",\n    \"account_id\": \"{}\"\n  }}",
            alg.id(),
            hex(&seed),
            hex(pk.as_bytes()),
            hex(account_id(&pk).as_bytes())
        ));
    }
    println!("[\n{}\n]", rows.join(",\n"));
    Ok(())
}
