//! Prints `tests/vectors/poseidon2_hash.json`: Poseidon2 hashes of fixed inputs.
//!
//! Run once with
//! `cargo run -p ac-crypto --features poseidon2 --example gen_poseidon2_vectors > tests/vectors/poseidon2_hash.json`.
//! The committed output is a regression vector and must never change.

use ac_crypto::poseidon2::hash;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Input byte i = i mod 251, as in the BLAKE3 reference vectors.
fn input(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| u8::try_from(i % 251).unwrap_or_default())
        .collect()
}

fn main() -> Result<(), ac_crypto::Error> {
    let mut cases = Vec::new();
    for len in [0usize, 31, 32, 1000] {
        cases.push(format!(
            "    {{ \"input_len\": {len}, \"hash\": \"{}\" }}",
            hex(&hash(&input(len))?)
        ));
    }
    println!(
        "{{\n  \"input\": \"byte i = i mod 251\",\n  \"cases\": [\n{}\n  ]\n}}",
        cases.join(",\n")
    );
    Ok(())
}
