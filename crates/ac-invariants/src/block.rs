//! Per-block checks (spec node/invariants; design D5 of `m3-economics`).
//!
//! The node reads two well-known values before and after each block — total issuance and the
//! cumulative burned amount — and calls [`check_block`]. Minting is measured as
//! `Δissuance + Δburned`, so burning never hides minting; a runtime that inflates the burned
//! counter only makes the cumulative check stricter.

use ac_primitives::emission::CAP;
use parity_scale_codec::Decode;

use crate::genesis::GenesisParams;
use crate::keys;

/// The two well-known values of one state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ledger {
    /// `Balances::TotalIssuance`.
    pub issuance: u128,
    /// `Emission::TotalBurned`.
    pub burned: u128,
}

impl Ledger {
    /// `issuance + burned`: everything ever minted, including genesis.
    fn gross(&self) -> u128 {
        self.issuance.saturating_add(self.burned)
    }
}

/// A broken invariant; the block must be rejected.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Violation {
    /// Total issuance above 21,000,000 ATC.
    SupplyCap {
        /// Issuance after the block.
        issuance: u128,
    },
    /// A block that settles no emission epoch minted.
    MintOutsideSettlement {
        /// Amount minted.
        minted: u128,
    },
    /// A settlement block minted more than twice the settled epoch's scheduled amount.
    EpochMint {
        /// Amount minted.
        minted: u128,
        /// Limit `2 × S(e)`.
        limit: u128,
    },
    /// Minting since genesis exceeds the cumulative schedule.
    CumulativeMint {
        /// Minted since genesis.
        minted: u128,
        /// Cumulative scheduled amount of the epochs settled so far.
        limit: u128,
    },
    /// A well-known key is missing (fail-closed).
    MissingKey(&'static str),
    /// A well-known value does not decode (fail-closed).
    Malformed(&'static str),
}

impl Violation {
    /// Short name of the broken rule, for logs.
    #[must_use]
    pub fn rule(&self) -> &'static str {
        match self {
            Self::SupplyCap { .. } => "supply cap",
            Self::MintOutsideSettlement { .. }
            | Self::EpochMint { .. }
            | Self::CumulativeMint { .. } => "minting bounded by the emission curve",
            Self::MissingKey(_) | Self::Malformed(_) => "well-known storage keys",
        }
    }
}

impl core::fmt::Display for Violation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "constitution invariant violated ({}): ", self.rule())?;
        match self {
            Self::SupplyCap { issuance } => {
                write!(f, "issuance {issuance} exceeds the cap {CAP}")
            }
            Self::MintOutsideSettlement { minted } => {
                write!(
                    f,
                    "minted {minted} in a block that settles no emission epoch"
                )
            }
            Self::EpochMint { minted, limit } => {
                write!(f, "minted {minted} in a settlement block, limit {limit}")
            }
            Self::CumulativeMint { minted, limit } => {
                write!(
                    f,
                    "minted {minted} since genesis, cumulative schedule {limit}"
                )
            }
            Self::MissingKey(what) => write!(f, "{what} is missing"),
            Self::Malformed(what) => write!(f, "{what} does not decode"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Violation {}

fn read_u128(
    get: &impl Fn(&[u8]) -> Option<alloc::vec::Vec<u8>>,
    key: &[u8],
    what: &'static str,
) -> Result<u128, Violation> {
    let raw = get(key).ok_or(Violation::MissingKey(what))?;
    let mut input = raw.as_slice();
    let value = u128::decode(&mut input).map_err(|_| Violation::Malformed(what))?;
    if input.is_empty() {
        Ok(value)
    } else {
        Err(Violation::Malformed(what))
    }
}

/// Reads the [`Ledger`] of a state through `get` (storage lookup by key).
///
/// # Errors
///
/// [`Violation::MissingKey`] or [`Violation::Malformed`] if a well-known value is absent or
/// does not decode: the block is rejected (fail-closed).
pub fn read_ledger(
    get: impl Fn(&[u8]) -> Option<alloc::vec::Vec<u8>>,
) -> Result<Ledger, Violation> {
    Ok(Ledger {
        issuance: read_u128(&get, &keys::TOTAL_ISSUANCE, "Balances::TotalIssuance")?,
        burned: read_u128(&get, &keys::TOTAL_BURNED, "Emission::TotalBurned")?,
    })
}

/// Checks block `number` given the ledgers of its parent state (`pre`) and its own state
/// (`post`):
/// - issuance ≤ 21,000,000 ATC;
/// - a block that settles no epoch mints nothing (`gross` does not grow);
/// - a settlement block of epoch `e` mints at most `2 × S(e)`;
/// - minted since genesis ≤ cumulative scheduled amount of the epochs settled so far.
///
/// # Errors
///
/// The first broken rule as a [`Violation`].
pub fn check_block(
    params: &GenesisParams,
    number: u64,
    pre: Ledger,
    post: Ledger,
) -> Result<(), Violation> {
    if post.issuance > CAP {
        return Err(Violation::SupplyCap {
            issuance: post.issuance,
        });
    }
    let minted = post.gross().saturating_sub(pre.gross());
    let schedule = &params.schedule;
    match schedule.settled_epoch(number) {
        None if minted > 0 => return Err(Violation::MintOutsideSettlement { minted }),
        None => {}
        Some(epoch) => {
            let limit = schedule.scheduled(epoch).saturating_mul(2);
            if minted > limit {
                return Err(Violation::EpochMint { minted, limit });
            }
        }
    }
    let since_genesis = post.gross().saturating_sub(params.genesis_issuance);
    let limit = schedule.cumulative_scheduled(schedule.settled_epochs(number));
    if since_genesis > limit {
        return Err(Violation::CumulativeMint {
            minted: since_genesis,
            limit,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::arithmetic_side_effects)]

    use super::*;
    use ac_primitives::emission::{EmissionSchedule, EpochInput, Phase, UNITS, settle};
    use parity_scale_codec::Encode;
    use proptest::prelude::*;

    const L: u64 = 10;

    fn params(genesis_issuance: u128) -> GenesisParams {
        GenesisParams {
            schedule: EmissionSchedule::new(L).unwrap(),
            genesis_issuance,
        }
    }

    fn ledger(issuance: u128, burned: u128) -> Ledger {
        Ledger { issuance, burned }
    }

    // Scenario "超过总量".
    #[test]
    fn supply_cap() {
        let err = check_block(&params(0), 5, ledger(CAP, 0), ledger(CAP + 1, 0)).unwrap_err();
        assert_eq!(err, Violation::SupplyCap { issuance: CAP + 1 });
        assert!(err.to_string().contains("supply cap"));
    }

    // Scenario "非结算区块铸币": one smallest unit is enough.
    #[test]
    fn mint_outside_settlement() {
        let p = params(100);
        let err = check_block(&p, 5, ledger(100, 0), ledger(101, 0)).unwrap_err();
        assert_eq!(err, Violation::MintOutsideSettlement { minted: 1 });
        assert!(err.to_string().contains("emission curve"));
        // Burning (issuance down, burned up by the same amount) is not minting.
        assert!(check_block(&p, 5, ledger(100, 0), ledger(90, 10)).is_ok());
        // Dust removal (issuance down alone) is fine too.
        assert!(check_block(&p, 5, ledger(100, 0), ledger(99, 0)).is_ok());
    }

    #[test]
    fn settlement_limits() {
        let p = params(0);
        let s0 = p.schedule.scheduled(0);
        // Block 11 settles epoch 0: up to the cumulative schedule S(0) is fine.
        assert!(check_block(&p, 11, ledger(0, 0), ledger(s0, 0)).is_ok());
        // More than the cumulative schedule is rejected even though below 2 × S.
        assert_eq!(
            check_block(&p, 11, ledger(0, 0), ledger(s0 + 1, 0)).unwrap_err(),
            Violation::CumulativeMint {
                minted: s0 + 1,
                limit: s0
            }
        );
        // With a reserve built up, a settlement block may mint up to 2 × S(e) but no more.
        let before = p.schedule.cumulative_scheduled(5) - 2 * s0;
        let block = 5 * L + 1; // settles epoch 4
        assert!(check_block(&p, block, ledger(before, 0), ledger(before + 2 * s0, 0)).is_ok());
        assert_eq!(
            check_block(&p, block, ledger(before - 1, 0), ledger(before + 2 * s0, 0)).unwrap_err(),
            Violation::EpochMint {
                minted: 2 * s0 + 1,
                limit: 2 * s0
            }
        );
    }

    // A runtime that inflates the burned counter only makes the checks stricter.
    #[test]
    fn inflated_burn_counter_is_stricter() {
        let p = params(0);
        let s0 = p.schedule.scheduled(0);
        assert!(check_block(&p, 11, ledger(0, 0), ledger(s0 / 2, 0)).is_ok());
        assert!(check_block(&p, 11, ledger(0, 0), ledger(s0 / 2, s0)).is_err());
    }

    // Scenario "固定存储键被移除": fail-closed.
    #[test]
    fn missing_or_malformed_keys() {
        let issuance_only = |key: &[u8]| (key == keys::TOTAL_ISSUANCE).then(|| 5u128.encode());
        assert_eq!(
            read_ledger(issuance_only).unwrap_err(),
            Violation::MissingKey("Emission::TotalBurned")
        );
        let short = |_: &[u8]| Some(vec![1u8, 2]);
        assert!(matches!(read_ledger(short), Err(Violation::Malformed(_))));
        let both = |key: &[u8]| {
            if key == keys::TOTAL_ISSUANCE {
                Some(7u128.encode())
            } else if key == keys::TOTAL_BURNED {
                Some(3u128.encode())
            } else {
                None
            }
        };
        assert_eq!(read_ledger(both).unwrap(), ledger(7, 3));
    }

    proptest! {
        // Scenario "正常排放": any honest settlement sequence, with fees burned in between,
        // passes every check.
        #[test]
        fn honest_settlement_passes(
            genesis in 0u128..(1_000_000 * UNITS),
            work in proptest::collection::vec((any::<u64>(), any::<u64>()), 1..40),
            burns in proptest::collection::vec(0u128..1_000, 1..40),
        ) {
            let p = params(genesis);
            let mut state = ledger(genesis, 0);
            let mut reserve = 0u128;
            for number in 1..=(40 * L) {
                let mut next = state;
                if let Some(epoch) = p.schedule.settled_epoch(number) {
                    let w = work.get(usize::try_from(epoch).unwrap() % work.len()).copied().unwrap();
                    let out = settle(&EpochInput::new(
                        p.schedule.scheduled(epoch),
                        reserve,
                        (u128::from(w.0) * 1_000, u128::from(w.1) * 1_000),
                        Phase::Pos,
                    ));
                    reserve = out.reserve;
                    next.issuance += out.total;
                }
                let burn = burns[usize::try_from(number).unwrap() % burns.len()].min(next.issuance);
                next.issuance -= burn;
                next.burned += burn;
                prop_assert!(check_block(&p, number, state, next).is_ok(), "block {}", number);
                state = next;
            }
        }
    }
}
