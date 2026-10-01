//! Randomness of the agent's choices (request times, models, gateways, payers, prompts). Live
//! choices come from the operating system's CSPRNG (AGENT.md §6.6); tests use a seeded stand-in
//! confined to `#[cfg(test)]`.

use rand_core::TryRng;

/// A source of uniform choices.
pub trait Rand: Send {
    /// A uniform integer in `0..n` (0 when `n` is 0).
    fn below(&mut self, n: u64) -> u64;
}

/// The operating system's CSPRNG.
pub struct OsRand(ac_crypto::OsRng);

impl OsRand {
    /// Seeds from the operating system.
    ///
    /// # Errors
    ///
    /// The operating system's random source fails.
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self(ac_crypto::OsRng::new()?))
    }
}

impl Rand for OsRand {
    fn below(&mut self, n: u64) -> u64 {
        uniform(n, || self.0.try_next_u64().unwrap_or_default())
    }
}

/// Rejection sampling of `0..n` from uniform 64-bit words, without modulo bias.
fn uniform(n: u64, mut next: impl FnMut() -> u64) -> u64 {
    if n == 0 {
        return 0;
    }
    // The largest multiple of `n` that fits: words at or above it are redrawn.
    let zone = u64::MAX - (u64::MAX % n);
    loop {
        let x = next();
        if x < zone {
            return x % n;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A deterministic generator for tests (SplitMix64).
    pub struct Seeded(u64);

    impl Seeded {
        pub fn new(seed: u64) -> Self {
            Self(seed)
        }
    }

    impl Rand for Seeded {
        fn below(&mut self, n: u64) -> u64 {
            uniform(n, || {
                self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = self.0;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^ (z >> 31)
            })
        }
    }

    #[test]
    fn uniform_stays_in_range_and_covers_it() {
        let mut r = OsRand::new().unwrap();
        let mut seen = [false; 7];
        for _ in 0..1_000 {
            let x = r.below(7);
            assert!(x < 7);
            seen[usize::try_from(x).unwrap()] = true;
        }
        assert!(seen.iter().all(|s| *s));
        assert_eq!(r.below(0), 0);
        assert_eq!(r.below(1), 0);
    }
}
