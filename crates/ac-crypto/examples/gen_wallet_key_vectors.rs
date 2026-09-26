//! Prints `tests/vectors/wallet_keys.json`: fixed mnemonic and development names → account IDs.
//!
//! Run once with
//! `cargo run -p ac-crypto --features mnemonic --example gen_wallet_key_vectors > tests/vectors/wallet_keys.json`.
//! The committed output is a regression vector and must never change.

use ac_crypto::sig::SigningKey;
use ac_crypto::{SigAlg, account_id, dev_seed, mnemonic, wallet_key_seed};

/// BIP-39 test mnemonic with all-zero 256-bit entropy (public, never use for real funds).
const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn main() -> Result<(), ac_crypto::Error> {
    let entropy = mnemonic::from_mnemonic(MNEMONIC)?;
    let mut wallet = Vec::new();
    for (alg, index) in [
        (SigAlg::MlDsa44, 0u32),
        (SigAlg::MlDsa44, 1),
        (SigAlg::MlDsa65, 0),
    ] {
        let key = SigningKey::from_seed(alg, &wallet_key_seed(&entropy, alg, index)?)?;
        wallet.push(format!(
            "    {{ \"alg_id\": {}, \"index\": {index}, \"account_id\": \"{}\" }}",
            alg.id(),
            hex(account_id(&key.public_key()?).as_bytes())
        ));
    }
    let mut dev = Vec::new();
    for name in ["alice", "bob", "charlie", "dave"] {
        for alg in [SigAlg::MlDsa44, SigAlg::MlDsa65] {
            let key = SigningKey::from_seed(alg, &dev_seed(name)?)?;
            dev.push(format!(
                "    {{ \"name\": \"{name}\", \"alg_id\": {}, \"account_id\": \"{}\" }}",
                alg.id(),
                hex(account_id(&key.public_key()?).as_bytes())
            ));
        }
    }
    println!(
        "{{\n  \"mnemonic\": \"{MNEMONIC}\",\n  \"wallet\": [\n{}\n  ],\n  \"dev\": [\n{}\n  ]\n}}",
        wallet.join(",\n"),
        dev.join(",\n")
    );
    Ok(())
}
