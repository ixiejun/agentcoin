//! The PoA → PoS switch as a node-enforced rule (spec node/invariants "PoA→PoS 切换受节点约束";
//! design D6 of `m3-pos`; decisions D19, D24, D30).
//!
//! For every block the node reads the switch state (phase, start of the qualified run, PoA
//! roster) before and after it. At a PoA epoch boundary it recomputes the checkpoint itself
//! from the parent state's stake ledger and the genesis parameters with the same
//! [`transition_step`] the runtime uses, and requires the runtime to have reached exactly the
//! same result: no early switch, no delayed switch, no way back. Everywhere else the switch
//! state must not change. Only published well-known keys are read, so the rule survives any
//! runtime upgrade.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use ac_primitives::epoch::is_boundary;
use ac_primitives::staking::{
    CandidateRecord, ChainPhase, LedgerHead, TransitionInputs, TransitionParams, min_self_bond,
    transition_step,
};
use parity_scale_codec::{Compact, Decode};

use crate::block::Violation;
use crate::keys;

/// Genesis parameters of the switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransitionGenesis {
    /// Switch parameters (`ValidatorSet::TransitionParams`).
    pub params: TransitionParams,
    /// Validator epoch length (`ValidatorSet::EpochLength`); checkpoints are its boundaries.
    pub epoch_length: u64,
}

/// The switch state of one block's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchState {
    /// `ValidatorSet::Phase`.
    pub phase: ChainPhase,
    /// `ValidatorSet::QualifiedSince`.
    pub qualified_since: Option<u64>,
    /// Number of keys in `ValidatorSet::PoaAuthorities`.
    pub poa_authorities: u32,
}

/// What a PoA checkpoint looks at, computed from the parent state's stake ledger.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StakeSnapshot {
    /// Sum of every ledger's active amount.
    pub total_active: u128,
    /// `Balances::TotalIssuance`.
    pub issuance: u128,
    /// Candidates that are not chilled and whose active self-stake reaches the minimum.
    pub qualified_candidates: u32,
}

fn decode_exact<T: Decode>(raw: &[u8], what: &'static str) -> Result<T, Violation> {
    let mut input = raw;
    let value = T::decode(&mut input).map_err(|_| Violation::Malformed(what))?;
    if input.is_empty() {
        Ok(value)
    } else {
        Err(Violation::Malformed(what))
    }
}

/// Reads the [`SwitchState`] of a state through `get` (storage lookup by key).
///
/// # Errors
///
/// [`Violation::MissingKey`] or [`Violation::Malformed`] if a well-known value is absent or does
/// not decode (fail-closed).
pub fn read_switch_state(get: impl Fn(&[u8]) -> Option<Vec<u8>>) -> Result<SwitchState, Violation> {
    let phase = get(&keys::PHASE).ok_or(Violation::MissingKey("ValidatorSet::Phase"))?;
    let since =
        get(&keys::QUALIFIED_SINCE).ok_or(Violation::MissingKey("ValidatorSet::QualifiedSince"))?;
    let roster =
        get(&keys::POA_AUTHORITIES).ok_or(Violation::MissingKey("ValidatorSet::PoaAuthorities"))?;
    // Only the roster's length matters; the keys themselves are not needed.
    let mut input = roster.as_slice();
    let Compact(count) = <Compact<u32>>::decode(&mut input)
        .map_err(|_| Violation::Malformed("ValidatorSet::PoaAuthorities"))?;
    Ok(SwitchState {
        phase: decode_exact(&phase, "ValidatorSet::Phase")?,
        qualified_since: decode_exact(&since, "ValidatorSet::QualifiedSince")?,
        poa_authorities: count,
    })
}

/// Computes the [`StakeSnapshot`] of a state from its issuance and the `(key suffix, value)`
/// entries of the `StakingPos::Ledger` and `StakingPos::Candidates` maps (the suffix is the
/// account ID; both maps use the `Identity` hasher).
///
/// # Errors
///
/// [`Violation::Malformed`] if an entry does not decode (fail-closed).
pub fn stake_snapshot<'a>(
    issuance: u128,
    ledgers: impl IntoIterator<Item = (&'a [u8], &'a [u8])>,
    candidates: impl IntoIterator<Item = (&'a [u8], &'a [u8])>,
) -> Result<StakeSnapshot, Violation> {
    let mut total_active = 0u128;
    let mut active: BTreeMap<&[u8], u128> = BTreeMap::new();
    for (account, value) in ledgers {
        let mut input = value;
        let head = LedgerHead::decode(&mut input)
            .map_err(|_| Violation::Malformed("StakingPos::Ledger"))?;
        total_active = total_active.saturating_add(head.active);
        active.insert(account, head.active);
    }
    let minimum = min_self_bond(issuance);
    let mut qualified = 0u32;
    for (account, value) in candidates {
        let record: CandidateRecord = decode_exact(value, "StakingPos::Candidates")?;
        let stake = active.get(account).copied().unwrap_or(0);
        if !record.chilled && stake >= minimum {
            qualified = qualified.saturating_add(1);
        }
    }
    Ok(StakeSnapshot {
        total_active,
        issuance,
        qualified_candidates: qualified,
    })
}

/// Checks block `number` given the switch state of its parent (`pre`) and its own (`post`).
/// `stake` computes the parent state's [`StakeSnapshot`]; it is called only at PoA
/// checkpoints.
///
/// - The phase never goes back from PoS to PoA, and in PoS the PoA roster is empty.
/// - Outside epoch boundaries the phase and `QualifiedSince` do not change; in PoS they do not
///   change at all.
/// - At a PoA boundary, `QualifiedSince` and the phase are exactly what
///   [`transition_step`] gives for the parent state: the switch happens neither early nor
///   late.
///
/// # Errors
///
/// The first broken rule as a [`Violation`], or what `stake` returns.
pub fn check_transition(
    genesis: &TransitionGenesis,
    number: u64,
    pre: &SwitchState,
    post: &SwitchState,
    stake: impl FnOnce() -> Result<StakeSnapshot, Violation>,
) -> Result<(), Violation> {
    if pre.phase == ChainPhase::Pos && post.phase != ChainPhase::Pos {
        return Err(Violation::SwitchReverted);
    }
    if post.phase == ChainPhase::Pos && post.poa_authorities != 0 {
        return Err(Violation::PoaAuthoritiesInPos {
            count: post.poa_authorities,
        });
    }
    let checkpoint = pre.phase == ChainPhase::Poa && is_boundary(number, genesis.epoch_length);
    if !checkpoint {
        if post.phase != pre.phase {
            return Err(Violation::SwitchEarly);
        }
        if post.qualified_since != pre.qualified_since {
            return Err(Violation::QualifiedSinceMismatch {
                expected: pre.qualified_since,
                found: post.qualified_since,
            });
        }
        return Ok(());
    }
    let snapshot = stake()?;
    let inputs = TransitionInputs::new(
        snapshot.total_active,
        snapshot.issuance,
        snapshot.qualified_candidates,
        number,
    );
    let step = transition_step(pre.qualified_since, &inputs, &genesis.params);
    if post.qualified_since != step.qualified_since {
        return Err(Violation::QualifiedSinceMismatch {
            expected: step.qualified_since,
            found: post.qualified_since,
        });
    }
    match (step.switch, post.phase) {
        (false, ChainPhase::Pos) => Err(Violation::SwitchEarly),
        (true, ChainPhase::Poa) => Err(Violation::SwitchDelayed),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::panic,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing
    )]

    use super::*;
    use ac_crypto::sig::SigningKey;
    use ac_crypto::{SigAlg, dev_seed};
    use parity_scale_codec::Encode;
    use proptest::prelude::*;

    fn genesis() -> TransitionGenesis {
        TransitionGenesis {
            params: TransitionParams {
                stake_bps: 1_000,
                min_candidates: 3,
                min_height: 40,
                sustain_blocks: 20,
            },
            epoch_length: 10,
        }
    }

    fn state(phase: ChainPhase, since: Option<u64>, roster: u32) -> SwitchState {
        SwitchState {
            phase,
            qualified_since: since,
            poa_authorities: roster,
        }
    }

    fn qualified() -> Result<StakeSnapshot, Violation> {
        Ok(StakeSnapshot {
            total_active: 100,
            issuance: 1_000,
            qualified_candidates: 3,
        })
    }

    fn unqualified() -> Result<StakeSnapshot, Violation> {
        Ok(StakeSnapshot {
            total_active: 99,
            issuance: 1_000,
            qualified_candidates: 3,
        })
    }

    fn poa(since: Option<u64>) -> SwitchState {
        state(ChainPhase::Poa, since, 4)
    }

    fn pos() -> SwitchState {
        state(ChainPhase::Pos, Some(41), 0)
    }

    // Scenario "提前切换被拒绝": a switch before T_min, or outside a boundary.
    #[test]
    fn early_switch_is_rejected() {
        let g = genesis();
        let err = check_transition(&g, 31, &poa(None), &pos(), qualified).unwrap_err();
        assert!(matches!(
            err,
            Violation::QualifiedSinceMismatch { .. } | Violation::SwitchEarly
        ));
        let switched_early = state(ChainPhase::Pos, None, 0);
        let err = check_transition(&g, 31, &poa(None), &switched_early, qualified).unwrap_err();
        assert_eq!(err, Violation::SwitchEarly);
        assert!(err.to_string().contains("PoA → PoS transition"));
        // Qualified since 41, but only 10 blocks later.
        let err = check_transition(&g, 51, &poa(Some(41)), &pos(), qualified).unwrap_err();
        assert_eq!(err, Violation::SwitchEarly);
        // Outside a boundary.
        let err = check_transition(&g, 45, &poa(Some(41)), &pos(), qualified).unwrap_err();
        assert_eq!(err, Violation::SwitchEarly);
    }

    // Scenario "阻止切换被拒绝".
    #[test]
    fn delayed_switch_is_rejected() {
        let g = genesis();
        let err = check_transition(&g, 61, &poa(Some(41)), &poa(Some(41)), qualified).unwrap_err();
        assert_eq!(err, Violation::SwitchDelayed);
    }

    // Scenario "退回 PoA 被拒绝".
    #[test]
    fn reverting_is_rejected() {
        let g = genesis();
        let back = state(ChainPhase::Poa, Some(41), 0);
        assert_eq!(
            check_transition(&g, 71, &pos(), &back, qualified).unwrap_err(),
            Violation::SwitchReverted
        );
        let roster = state(ChainPhase::Pos, Some(41), 2);
        assert_eq!(
            check_transition(&g, 75, &pos(), &roster, qualified).unwrap_err(),
            Violation::PoaAuthoritiesInPos { count: 2 }
        );
    }

    // QualifiedSince must follow the checkpoint and never change elsewhere.
    #[test]
    fn qualified_since_is_checked() {
        let g = genesis();
        // A passing checkpoint must start the run.
        assert!(check_transition(&g, 41, &poa(None), &poa(Some(41)), qualified).is_ok());
        assert!(check_transition(&g, 41, &poa(None), &poa(None), qualified).is_err());
        // A failing one must clear it.
        assert!(check_transition(&g, 51, &poa(Some(41)), &poa(None), unqualified).is_ok());
        assert!(check_transition(&g, 51, &poa(Some(41)), &poa(Some(41)), unqualified).is_err());
        // Outside boundaries nothing changes.
        assert!(check_transition(&g, 45, &poa(Some(41)), &poa(None), qualified).is_err());
        assert!(check_transition(&g, 45, &poa(Some(41)), &poa(Some(41)), qualified).is_ok());
        // The ledger is only read at PoA checkpoints.
        let never = || -> Result<StakeSnapshot, Violation> { panic!("read outside a checkpoint") };
        assert!(check_transition(&g, 45, &poa(None), &poa(None), never).is_ok());
        assert!(check_transition(&g, 81, &pos(), &pos(), never).is_ok());
    }

    // Scenario "正常切换".
    #[test]
    fn honest_switch_is_accepted() {
        let g = genesis();
        assert!(check_transition(&g, 61, &poa(Some(41)), &pos(), qualified).is_ok());
    }

    #[test]
    fn well_known_values_are_read_fail_closed() {
        let key = SigningKey::from_seed(SigAlg::MlDsa65, &dev_seed("alice").unwrap())
            .unwrap()
            .public_key()
            .unwrap();
        let values = move |k: &[u8]| {
            if k == keys::PHASE {
                Some(ChainPhase::Pos.encode())
            } else if k == keys::QUALIFIED_SINCE {
                Some(Some(41u64).encode())
            } else if k == keys::POA_AUTHORITIES {
                Some(vec![key.clone()].encode())
            } else {
                None
            }
        };
        assert_eq!(
            read_switch_state(&values).unwrap(),
            state(ChainPhase::Pos, Some(41), 1)
        );
        let missing = |k: &[u8]| (k != keys::QUALIFIED_SINCE).then(|| values(k)).flatten();
        assert_eq!(
            read_switch_state(missing).unwrap_err(),
            Violation::MissingKey("ValidatorSet::QualifiedSince")
        );
        let bad_phase = |k: &[u8]| {
            if k == keys::PHASE {
                Some(vec![7])
            } else {
                values(k)
            }
        };
        assert_eq!(
            read_switch_state(bad_phase).unwrap_err(),
            Violation::Malformed("ValidatorSet::Phase")
        );
    }

    #[test]
    fn snapshot_counts_qualified_candidates() {
        let key = SigningKey::from_seed(SigAlg::MlDsa65, &dev_seed("alice").unwrap())
            .unwrap()
            .public_key()
            .unwrap();
        let record = |chilled| CandidateRecord {
            key: key.clone(),
            commission_bps: 1_000,
            pending_commission: None,
            chilled,
        };
        // Ledger values carry unlocking chunks after the active amount.
        let ledger = |active: u128| (active, Vec::<(u128, u64)>::new()).encode();
        let ledgers = [
            ([1u8; 32], ledger(1_000)),
            ([2u8; 32], ledger(999)),
            ([3u8; 32], ledger(5_000)),
            ([4u8; 32], ledger(700)),
        ];
        let candidates = [
            ([1u8; 32], record(false).encode()),
            ([2u8; 32], record(false).encode()),
            ([3u8; 32], record(true).encode()),
        ];
        // Issuance 1,000,000: the minimum self-stake is 1,000.
        let snapshot = stake_snapshot(
            1_000_000,
            ledgers.iter().map(|(k, v)| (k.as_slice(), v.as_slice())),
            candidates.iter().map(|(k, v)| (k.as_slice(), v.as_slice())),
        )
        .unwrap();
        assert_eq!(snapshot.total_active, 7_699);
        assert_eq!(snapshot.qualified_candidates, 1);
        let bad = [([1u8; 32], vec![1u8])];
        assert!(matches!(
            stake_snapshot(0, bad.iter().map(|(k, v)| (k.as_slice(), v.as_slice())), []),
            Err(Violation::Malformed("StakingPos::Ledger"))
        ));
    }

    proptest! {
        // Any sequence produced by the honest state machine passes every check.
        #[test]
        fn honest_state_machine_passes(
            stakes in proptest::collection::vec((0u128..200, 0u32..6), 1..30),
        ) {
            let g = genesis();
            let mut state = poa(None);
            for number in 1..=300u64 {
                let (active, cands) = stakes[(number as usize / 10) % stakes.len()];
                let snapshot = StakeSnapshot {
                    total_active: active,
                    issuance: 1_000,
                    qualified_candidates: cands,
                };
                let mut next = state;
                if state.phase == ChainPhase::Poa && is_boundary(number, g.epoch_length) {
                    let step = transition_step(
                        state.qualified_since,
                        &TransitionInputs::new(active, 1_000, cands, number),
                        &g.params,
                    );
                    next.qualified_since = step.qualified_since;
                    if step.switch {
                        next.phase = ChainPhase::Pos;
                        next.poa_authorities = 0;
                    }
                }
                prop_assert!(check_transition(&g, number, &state, &next, || Ok(snapshot)).is_ok());
                state = next;
            }
        }
    }
}
