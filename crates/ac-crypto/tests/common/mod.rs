//! Shared test helpers.
#![allow(dead_code)] // Each integration test binary uses a different subset.

use core::convert::Infallible;

/// Deterministic, seeded test RNG (BLAKE3 XOF). Test-only; never use outside tests.
pub struct TestRng(blake3::OutputReader);

impl TestRng {
    pub fn new(label: &str) -> Self {
        Self(
            blake3::Hasher::new_derive_key("agentcoin 2026-09 test-rng v1")
                .update(label.as_bytes())
                .finalize_xof(),
        )
    }
}

impl rand_core::TryRng for TestRng {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        let mut b = [0u8; 4];
        self.0.fill(&mut b);
        Ok(u32::from_le_bytes(b))
    }

    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        let mut b = [0u8; 8];
        self.0.fill(&mut b);
        Ok(u64::from_le_bytes(b))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        self.0.fill(dst);
        Ok(())
    }
}

impl rand_core::TryCryptoRng for TestRng {}

pub fn unhex(s: &str) -> Vec<u8> {
    hex::decode(s).unwrap()
}

pub fn load(name: &str) -> serde_json::Value {
    let path = format!("{}/tests/vectors/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}
