//! Which verdicts are spot-checked (OpenSpec change m6-audit-hardening design D5; spec
//! `market/audit` "裁决抽查").
//!
//! A verdict judged by the thresholds is spot-checked with probability `ρ`. The draw hashes the
//! seed of the round *after* the verdict's and the receipt's request ID, so an auditor cannot
//! know when submitting whether this verdict will be checked.

use sp_core::H256;

/// Hashing context of the spot-check draw: `next round seed ‖ request ID`.
pub const SPOT_CHECK_CONTEXT: &str = "agentcoin 2026-10 audit-spot-check v1";

/// Highest spot-check rate, in basis points (the guardrail 0–50%).
pub const MAX_SPOT_CHECK_BPS: u16 = 5_000;

/// Basis points in one.
const BPS: u128 = 10_000;

/// Whether the verdict on the receipt with `request_id` is spot-checked at `rate_bps` basis
/// points under the next round's seed: the first 8 bytes of the hash, little endian, scaled to
/// `0..10_000`, are below the rate. A rate of 0 never draws, 10,000 or more always does.
#[must_use]
pub fn spot_checked(next_seed: &H256, request_id: &[u8; 32], rate_bps: u16) -> bool {
    let data = [next_seed.0, *request_id].concat();
    // The context is well formed, so `derive` cannot fail; an error draws nothing rather than
    // panicking.
    let Ok(h) = ac_crypto::hash::derive(SPOT_CHECK_CONTEXT, &data) else {
        return false;
    };
    let mut word = [0u8; 8];
    if let Some(head) = h.get(..8) {
        word.copy_from_slice(head);
    }
    // `word × 10,000 / 2⁶⁴` is in `0..10,000` and fits a u128.
    let scaled = u128::from(u64::from_le_bytes(word)).saturating_mul(BPS) >> 64;
    scaled < u128::from(rate_bps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(i: u32) -> [u8; 32] {
        let mut r = [0u8; 32];
        r[..4].copy_from_slice(&i.to_le_bytes());
        r
    }

    #[test]
    fn extreme_rates() {
        let seed = H256::repeat_byte(7);
        for i in 0..1_000 {
            assert!(!spot_checked(&seed, &id(i), 0));
            assert!(spot_checked(&seed, &id(i), 10_000));
        }
    }

    // Spec "裁决抽查", scenario "抽中与否可复算": a fixed vector, recomputed independently with
    // Python's `blake3` (`derive_key_context` = the context, input `seed ‖ request ID`).
    #[test]
    fn vector() {
        let seed = H256::repeat_byte(0x11);
        let mask = (0..16u32).fold(0u16, |m, i| {
            m | (u16::from(spot_checked(&seed, &id(i), 2_500)) << i)
        });
        assert_eq!(mask, 0xfd40);
    }

    #[test]
    fn rate_is_the_frequency() {
        let seed = H256::repeat_byte(0x5a);
        let n = 100_000u32;
        let hits = (0..n).filter(|&i| spot_checked(&seed, &id(i), 500)).count();
        // 5% of 100,000: mean 5,000, σ ≈ 69; within 3σ.
        assert!((4_793..=5_207).contains(&hits), "{hits}");
    }

    #[test]
    fn the_seed_matters() {
        let a = (0..2_000).filter(|&i| spot_checked(&H256::repeat_byte(1), &id(i), 5_000));
        let b = (0..2_000).filter(|&i| spot_checked(&H256::repeat_byte(2), &id(i), 5_000));
        assert_ne!(a.collect::<Vec<_>>(), b.collect::<Vec<_>>());
    }
}
