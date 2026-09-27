//! Staking, election inputs and the PoA → PoS transition (design D1, D4, D6, D8 of `m3-pos`;
//! decisions D19, D24, D30).
//!
//! The runtime (`pallet-staking-pos`, `pallet-validator-set`) and the node's invariant checker
//! (`ac-invariants`) share these pure functions, so both compute the same switch decision,
//! minimum amounts, unbonding delays and reward splits. Integer arithmetic only, rounding
//! down unless stated otherwise; remainders are never paid out.
//!
//! The types with explicit field layouts here ([`ChainPhase`], [`TransitionParams`]) are the
//! values of published well-known storage keys: their encodings never change.

use alloc::vec::Vec;

use ac_crypto::PqPublicKey;

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

/// Basis-point denominator.
pub const BPS: u32 = 10_000;

/// Parts-per-million denominator (the minimum nomination, 0.001%, is below one basis point).
pub const PPM: u128 = 1_000_000;

/// Constitution: total active stake must reach 10% of the total issuance (D19, D24).
pub const TRANSITION_STAKE_BPS: u32 = 1_000;

/// Constitution: qualified candidates required to switch (D19).
pub const TRANSITION_MIN_CANDIDATES: u32 = 21;

/// Constitution: earliest switch height, two years of one-second blocks (2 × 365.25 days).
pub const TRANSITION_MIN_HEIGHT: u64 = 63_115_200;

/// Constitution: blocks the conditions must hold without interruption, seven days.
pub const TRANSITION_SUSTAIN_BLOCKS: u64 = 604_800;

/// Minimum candidate self-stake: 0.1% of the total issuance, in parts per million.
pub const MIN_SELF_BOND_PPM: u128 = 1_000;

/// Minimum nomination: 0.001% of the total issuance, in parts per million.
pub const MIN_NOMINATION_PPM: u128 = 10;

/// Most candidates one nominator may nominate.
pub const MAX_NOMINATIONS: u32 = 16;

/// Lowest commission: 5%.
pub const MIN_COMMISSION_BPS: u32 = 500;

/// Highest commission: 100%.
pub const MAX_COMMISSION_BPS: u32 = BPS;

/// Validator phase stored under the well-known key `ValidatorSet::Phase`. The discriminants
/// are part of the published format.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum ChainPhase {
    /// Proof of authority: the admin multisig manages the authority list.
    #[default]
    #[codec(index = 0)]
    Poa = 0,
    /// Proof of stake: the set is elected; there is no way back.
    #[codec(index = 1)]
    Pos = 1,
}

impl From<ChainPhase> for crate::emission::Phase {
    fn from(phase: ChainPhase) -> Self {
        match phase {
            ChainPhase::Poa => Self::Poa,
            ChainPhase::Pos => Self::Pos,
        }
    }
}

/// Switch parameters stored under the well-known key `ValidatorSet::TransitionParams`, set at
/// genesis. Live chains must use [`TransitionParams::CONSTITUTION`]. The field order is part of
/// the published format.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct TransitionParams {
    /// Active stake threshold in basis points of the total issuance.
    pub stake_bps: u32,
    /// Qualified candidates required.
    pub min_candidates: u32,
    /// Earliest switch height.
    pub min_height: u64,
    /// Blocks the conditions must hold without interruption.
    pub sustain_blocks: u64,
}

impl TransitionParams {
    /// The constitution values (10%, 21, 63,115,200, 604,800).
    pub const CONSTITUTION: Self = Self {
        stake_bps: TRANSITION_STAKE_BPS,
        min_candidates: TRANSITION_MIN_CANDIDATES,
        min_height: TRANSITION_MIN_HEIGHT,
        sustain_blocks: TRANSITION_SUSTAIN_BLOCKS,
    };
}

impl Default for TransitionParams {
    fn default() -> Self {
        Self::CONSTITUTION
    }
}

/// ML-DSA signing context of a validator key's proof of possession. Never change it.
pub const VALIDATOR_POP_CONTEXT: &[u8] = b"agentcoin/validator-pop/v1";

/// Domain tag at the start of every proof-of-possession statement.
pub const VALIDATOR_POP_TAG: &[u8] = b"agentcoin/validator-pop-statement";

/// The statement a validator key signs (context [`VALIDATOR_POP_CONTEXT`]) to prove that the
/// registering account controls it: SCALE of (tag, genesis hash, account, key). Binding the
/// account stops anyone from registering someone else's validator key.
#[must_use]
pub fn pop_statement<H: Encode, A: Encode>(
    genesis_hash: &H,
    who: &A,
    key: &PqPublicKey,
) -> Vec<u8> {
    (VALIDATOR_POP_TAG, genesis_hash, who, key).encode()
}

/// Identifier of a validator key: the account ID it derives (`ac_crypto::account_id`). The
/// randomness pallet identifies validators the same way.
#[must_use]
pub fn validator_key_id(key: &PqPublicKey) -> [u8; 32] {
    *ac_crypto::account_id(key).as_bytes()
}

/// A candidate validator, stored under the well-known key `StakingPos::Candidates`
/// (`Identity`-hashed account → this record). Its self-stake is the account's
/// `StakingPos::Ledger` entry. The field order is part of the published format.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct CandidateRecord {
    /// ML-DSA-65 key for block seals, AC-BFT votes and randomness.
    pub key: PqPublicKey,
    /// Commission in basis points.
    pub commission_bps: u32,
    /// A scheduled commission change: new value and the block it takes effect at.
    pub pending_commission: Option<(u32, u64)>,
    /// Paused from elections (by itself, an offence or missed reveals).
    pub chilled: bool,
}

impl CandidateRecord {
    /// Commission in force at block `now`.
    #[must_use]
    pub fn commission_at(&self, now: u64) -> u32 {
        match self.pending_commission {
            Some((value, at)) if now >= at => value,
            _ => self.commission_bps,
        }
    }
}

/// Head of a staking ledger as the node reads it from the well-known key `StakingPos::Ledger`:
/// the first field is the active (bonded, not unbonding) amount. Decoding stops after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub struct LedgerHead {
    /// Active amount.
    pub active: u128,
}

/// What one epoch-boundary check of the switch conditions looks at.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransitionInputs {
    /// All active stake: self-stake plus nominations, excluding unbonding amounts.
    pub total_active: u128,
    /// Total issuance.
    pub issuance: u128,
    /// Candidates whose active self-stake reaches [`min_self_bond`] and that are not chilled.
    pub qualified_candidates: u32,
    /// Height of the boundary block.
    pub height: u64,
}

impl TransitionInputs {
    /// Inputs of the check at boundary block `height`.
    #[must_use]
    pub fn new(total_active: u128, issuance: u128, qualified_candidates: u32, height: u64) -> Self {
        Self {
            total_active,
            issuance,
            qualified_candidates,
            height,
        }
    }
}

/// Whether one checkpoint meets all switch conditions (spec consensus/pos-transition
/// "切换条件"): active stake ≥ `stake_bps` of the issuance, enough qualified candidates, and
/// the height at least `min_height`.
#[must_use]
pub fn transition_check(inputs: &TransitionInputs, params: &TransitionParams) -> bool {
    // Both sides are at most 2^84 × 10^4, far from u128::MAX; saturation keeps the comparison
    // conservative (a saturated threshold is never met by a smaller left side).
    let stake = inputs.total_active.saturating_mul(u128::from(BPS));
    let needed = inputs.issuance.saturating_mul(u128::from(params.stake_bps));
    stake >= needed
        && inputs.qualified_candidates >= params.min_candidates
        && inputs.height >= params.min_height
}

/// The start of the current qualified run after a checkpoint at `height`: cleared when the
/// checkpoint fails, set to `height` when it is the first passing one, kept otherwise.
#[must_use]
pub fn next_qualified_since(previous: Option<u64>, ok: bool, height: u64) -> Option<u64> {
    if !ok {
        return None;
    }
    Some(previous.unwrap_or(height))
}

/// Result of one PoA checkpoint.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransitionStep {
    /// Whether the checkpoint met the conditions.
    pub ok: bool,
    /// New value of `ValidatorSet::QualifiedSince`.
    pub qualified_since: Option<u64>,
    /// Whether this boundary switches to PoS.
    pub switch: bool,
}

/// One PoA epoch-boundary checkpoint (design D6): checks the conditions, advances
/// `QualifiedSince`, and switches exactly when the conditions have held for `sustain_blocks`.
/// The runtime executes this and the node recomputes it for every boundary block.
#[must_use]
pub fn transition_step(
    previous: Option<u64>,
    inputs: &TransitionInputs,
    params: &TransitionParams,
) -> TransitionStep {
    let ok = transition_check(inputs, params);
    let qualified_since = next_qualified_since(previous, ok, inputs.height);
    let switch = qualified_since
        .is_some_and(|since| inputs.height.saturating_sub(since) >= params.sustain_blocks);
    TransitionStep {
        ok,
        qualified_since,
        switch,
    }
}

/// `ceil(issuance × ppm / 1,000,000)`.
fn ppm_ceil(issuance: u128, ppm: u128) -> u128 {
    // The issuance is at most the cap (about 2^84), so the product stays far below u128::MAX.
    let product = issuance.saturating_mul(ppm);
    let floor = product.checked_div(PPM).unwrap_or(0);
    if product.checked_rem(PPM).unwrap_or(0) == 0 {
        floor
    } else {
        floor.saturating_add(1)
    }
}

/// Minimum candidate self-stake: 0.1% of `issuance`, rounded up to the smallest unit.
#[must_use]
pub fn min_self_bond(issuance: u128) -> u128 {
    ppm_ceil(issuance, MIN_SELF_BOND_PPM)
}

/// Minimum nomination: 0.001% of `issuance`, rounded up to the smallest unit.
#[must_use]
pub fn min_nomination(issuance: u128) -> u128 {
    ppm_ceil(issuance, MIN_NOMINATION_PPM)
}

/// Bounds of the nomination unbonding queue, in blocks.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnbondingParams {
    /// Shortest wait (two days on live chains).
    pub min_blocks: u64,
    /// Longest wait (28 days on live chains); also the time the whole active stake needs to
    /// leave through the queue.
    pub max_blocks: u64,
}

impl UnbondingParams {
    /// Queue bounds `[min_blocks, max_blocks]`.
    #[must_use]
    pub fn new(min_blocks: u64, max_blocks: u64) -> Self {
        Self {
            min_blocks,
            max_blocks,
        }
    }
}

/// When a nomination of `amount` unbonding at block `now` unlocks, and the new end of the
/// network-wide queue (design D4, after Polkadot RFC-0097).
///
/// The queue drains `total_active / max_blocks` per block (at least 1), so the whole active
/// stake leaves in `max_blocks`. The request joins the queue's end; its unlock height is that
/// end clamped to `[now + min_blocks, now + max_blocks]`. `total_active` is the active stake
/// before this request. Returns `(unlock_at, queue_end)`.
#[must_use]
pub fn unbonding_unlock(
    now: u64,
    queue_end: u64,
    amount: u128,
    total_active: u128,
    params: &UnbondingParams,
) -> (u64, u64) {
    let rate = total_active
        .checked_div(u128::from(params.max_blocks))
        .unwrap_or(0)
        .max(1);
    let delay = u64::try_from(amount.checked_div(rate).unwrap_or(0)).unwrap_or(u64::MAX);
    let end = now.max(queue_end).saturating_add(delay);
    let earliest = now.saturating_add(params.min_blocks);
    let latest = now.saturating_add(params.max_blocks).max(earliest);
    (end.clamp(earliest, latest), end)
}

/// `amount × num / den`, rounded down; 0 when `den` is 0.
fn pro_rata(amount: u128, num: u128, den: u128) -> u128 {
    // A u128 product could overflow for amounts near the cap times large numerators; split
    // `amount = q × den + r` so every intermediate stays in range: amount × num / den =
    // q × num + r × num / den (exact for the floor since r < den).
    let Some(q) = amount.checked_div(den) else {
        return 0;
    };
    let r = amount.checked_rem(den).unwrap_or(0);
    q.saturating_mul(num)
        .saturating_add(r.saturating_mul(num).checked_div(den).unwrap_or(0))
}

/// Splits `amount` between validators in proportion to their work points (reward rule R1,
/// independent of stake), rounding down. Entries with zero points get nothing; when all
/// points are zero nothing is paid. The shares sum to at most `amount`.
#[must_use]
pub fn split_by_points<A: Clone>(amount: u128, points: &[(A, u64)]) -> Vec<(A, u128)> {
    let total = points
        .iter()
        .fold(0u128, |acc, (_, p)| acc.saturating_add(u128::from(*p)));
    points
        .iter()
        .filter(|(_, p)| *p > 0)
        .map(|(who, p)| (who.clone(), pro_rata(amount, u128::from(*p), total)))
        .collect()
}

/// How one validator's reward is paid.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewardSplit<A> {
    /// Commission, paid to the validator on top of its stake share.
    pub commission: u128,
    /// Stake shares, in the order of the exposure.
    pub shares: Vec<(A, u128)>,
}

/// Splits one validator's `reward`: first `commission_bps` of it (rounded down, clamped to
/// 100%) as commission, then the rest in proportion to the backing `exposure` (the validator's
/// own stake and each nominator's part), rounding down. Commission plus shares never exceed
/// `reward`.
#[must_use]
pub fn split_reward<A: Clone>(
    reward: u128,
    commission_bps: u32,
    exposure: &[(A, u128)],
) -> RewardSplit<A> {
    let commission = pro_rata(reward, u128::from(commission_bps.min(BPS)), u128::from(BPS));
    let rest = reward.saturating_sub(commission);
    let total = exposure
        .iter()
        .fold(0u128, |acc, (_, s)| acc.saturating_add(*s));
    let shares = exposure
        .iter()
        .map(|(who, stake)| (who.clone(), pro_rata(rest, *stake, total)))
        .collect();
    RewardSplit { commission, shares }
}

/// One candidate as seen by the election (design D5, D12). `workscore` is reserved for the
/// full version's work-weighted election (`ValidatorElection(stake, workscore)`, full plan
/// §11) and is always 0 in M3.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElectionCandidate<A> {
    /// Candidate account.
    pub who: A,
    /// Active self-stake, counted as a vote for itself.
    pub self_stake: u128,
    /// Reserved; 0 in M3.
    pub workscore: u128,
}

impl<A> ElectionCandidate<A> {
    /// A candidate with no work score.
    #[must_use]
    pub fn new(who: A, self_stake: u128) -> Self {
        Self {
            who,
            self_stake,
            workscore: 0,
        }
    }
}

/// One nominator as seen by the election.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElectionVoter<A> {
    /// Nominator account.
    pub who: A,
    /// Active nominated stake.
    pub stake: u128,
    /// Nominated candidates (at most [`MAX_NOMINATIONS`]).
    pub targets: Vec<A>,
}

impl<A> ElectionVoter<A> {
    /// A nominator backing `targets` with `stake`.
    #[must_use]
    pub fn new(who: A, stake: u128, targets: Vec<A>) -> Self {
        Self {
            who,
            stake,
            targets,
        }
    }
}

/// Input of one election: eligible candidates, nominators and the number of seats `K`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElectionInput<A> {
    /// Eligible candidates (qualified and not chilled).
    pub candidates: Vec<ElectionCandidate<A>>,
    /// Nominators.
    pub voters: Vec<ElectionVoter<A>>,
    /// Seats to fill.
    pub seats: u32,
}

impl<A> ElectionInput<A> {
    /// An election of `seats` validators.
    #[must_use]
    pub fn new(
        candidates: Vec<ElectionCandidate<A>>,
        voters: Vec<ElectionVoter<A>>,
        seats: u32,
    ) -> Self {
        Self {
            candidates,
            voters,
            seats,
        }
    }
}

/// Backing per unit of AC-BFT voting weight: 10^12 smallest units (10^-6 ATC). The whole
/// supply (2.1 × 10^25 units) maps to about 2.1 × 10^13 weight units, far inside `u64`.
pub const BACKING_PER_WEIGHT: u128 = 1_000_000_000_000;

/// AC-BFT voting weight of a validator with `backing` (spec consensus/npos-election
/// "按支撑额的投票权重"): `max(1, backing / 10^12)`, so every member has a vote and ratios
/// are kept to within one unit.
#[must_use]
pub fn backing_to_weight(backing: u128) -> u64 {
    let units = backing.checked_div(BACKING_PER_WEIGHT).unwrap_or(0);
    u64::try_from(units).unwrap_or(u64::MAX).max(1)
}

/// One validator chosen by an election.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct ElectedValidator<A> {
    /// Candidate account.
    pub who: A,
    /// Its validator key.
    pub key: PqPublicKey,
    /// Total backing: self-stake plus the nominations assigned to it.
    pub backing: u128,
    /// Composition of the backing: the validator itself and each nominator, with amounts.
    pub exposure: Vec<(A, u128)>,
}

impl<A> ElectedValidator<A> {
    /// An elected validator.
    #[must_use]
    pub fn new(who: A, key: PqPublicKey, backing: u128, exposure: Vec<(A, u128)>) -> Self {
        Self {
            who,
            key,
            backing,
            exposure,
        }
    }
}

/// The latest election as reported to clients.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct ElectionInfo<A> {
    /// Block whose execution ran the election.
    pub block: u64,
    /// Whether it was a preview during the switch buffer (does not change the PoA set).
    pub preview: bool,
    /// Elected validators, in set order.
    pub elected: Vec<ElectedValidator<A>>,
}

impl<A> ElectionInfo<A> {
    /// Election information.
    #[must_use]
    pub fn new(block: u64, preview: bool, elected: Vec<ElectedValidator<A>>) -> Self {
        Self {
            block,
            preview,
            elected,
        }
    }
}

/// Progress of the PoA → PoS switch (spec consensus/pos-transition "阶段可查询").
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct TransitionProgress {
    /// Current phase.
    pub phase: ChainPhase,
    /// Block of the switch, once it happened.
    pub switched_at: Option<u64>,
    /// Start of the current qualified run, if any.
    pub qualified_since: Option<u64>,
    /// All active stake now.
    pub total_active: u128,
    /// Active stake needed now: the threshold share of the total issuance.
    pub stake_needed: u128,
    /// Qualified candidates now.
    pub qualified_candidates: u32,
    /// The switch parameters.
    pub params: TransitionParams,
    /// Current height.
    pub height: u64,
}

impl TransitionProgress {
    /// Progress report from its parts; `stake_needed` is derived from `issuance`.
    #[must_use]
    pub fn new(
        phase: ChainPhase,
        runs: (Option<u64>, Option<u64>),
        inputs: &TransitionInputs,
        params: TransitionParams,
    ) -> Self {
        let stake_needed = pro_rata(
            inputs.issuance,
            u128::from(params.stake_bps),
            u128::from(BPS),
        );
        Self {
            phase,
            switched_at: runs.0,
            qualified_since: runs.1,
            total_active: inputs.total_active,
            stake_needed,
            qualified_candidates: inputs.qualified_candidates,
            params,
            height: inputs.height,
        }
    }
}

/// Stake of one account (spec consensus/staking "质押查询与守恒").
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct AccountStake {
    /// Bonded and counting.
    pub active: u128,
    /// Unbonding, not yet unlocked.
    pub unlocking: u128,
    /// Unlocked and ready to withdraw.
    pub withdrawable: u128,
}

impl AccountStake {
    /// Stake split into its three parts.
    #[must_use]
    pub fn new(active: u128, unlocking: u128, withdrawable: u128) -> Self {
        Self {
            active,
            unlocking,
            withdrawable,
        }
    }
}

/// A candidate as reported to clients.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct CandidateInfo<A> {
    /// Validator key.
    pub key: PqPublicKey,
    /// Active self-stake.
    pub self_active: u128,
    /// Commission in force now, in basis points.
    pub commission_bps: u32,
    /// Scheduled commission change: value and effective block.
    pub pending_commission: Option<(u32, u64)>,
    /// Paused from elections.
    pub chilled: bool,
    /// Nominators naming this candidate and their active nominated amounts.
    pub nominators: Vec<(A, u128)>,
}

impl<A> CandidateInfo<A> {
    /// Candidate information.
    #[must_use]
    pub fn new(
        record: CandidateRecord,
        now: u64,
        self_active: u128,
        nominators: Vec<(A, u128)>,
    ) -> Self {
        Self {
            commission_bps: record.commission_at(now),
            pending_commission: record.pending_commission.filter(|(_, at)| *at > now),
            key: record.key,
            self_active,
            chilled: record.chilled,
            nominators,
        }
    }
}

sp_api::decl_runtime_apis! {
    /// Staking queries (specs consensus/staking, consensus/npos-election,
    /// consensus/pos-transition).
    pub trait StakingApi<AccountId> where AccountId: parity_scale_codec::Codec {
        /// Active, unbonding and withdrawable stake of `who`.
        fn stake(who: AccountId) -> AccountStake;
        /// Candidate record, commission in force and received nominations.
        fn candidate(who: AccountId) -> Option<CandidateInfo<AccountId>>;
        /// All active stake.
        fn total_active() -> u128;
        /// Current minimum self-stake and minimum nomination.
        fn minimums() -> (u128, u128);
        /// The latest election or preview, with each validator's backing and its composition.
        fn last_election() -> Option<ElectionInfo<AccountId>>;
        /// Phase, switch conditions and their current values.
        fn transition() -> TransitionProgress;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::arithmetic_side_effects, clippy::unwrap_used)]

    use super::*;
    use crate::emission::{CAP, UNITS};
    use proptest::prelude::*;

    fn params() -> TransitionParams {
        TransitionParams::CONSTITUTION
    }

    fn inputs(stake_pct_x10: u128, candidates: u32, height: u64) -> TransitionInputs {
        let issuance = 1_000_000 * UNITS;
        TransitionInputs::new(
            issuance * stake_pct_x10 / 1000,
            issuance,
            candidates,
            height,
        )
    }

    #[test]
    fn constitution_values() {
        assert_eq!(params().stake_bps, 1_000);
        assert_eq!(params().min_candidates, 21);
        assert_eq!(params().min_height, 63_115_200);
        assert_eq!(params().sustain_blocks, 604_800);
        assert_eq!(TransitionParams::default(), params());
    }

    // Published formats: phase and parameters encode exactly like this forever.
    #[test]
    fn well_known_encodings() {
        assert_eq!(ChainPhase::Poa.encode(), [0]);
        assert_eq!(ChainPhase::Pos.encode(), [1]);
        let encoded = params().encode();
        let mut expected = Vec::new();
        expected.extend_from_slice(&1_000u32.to_le_bytes());
        expected.extend_from_slice(&21u32.to_le_bytes());
        expected.extend_from_slice(&63_115_200u64.to_le_bytes());
        expected.extend_from_slice(&604_800u64.to_le_bytes());
        assert_eq!(encoded, expected);
        assert_eq!(
            TransitionParams::decode(&mut &encoded[..]).unwrap(),
            params()
        );
    }

    #[test]
    fn minimums_round_up() {
        assert_eq!(min_self_bond(0), 0);
        assert_eq!(min_nomination(0), 0);
        // Exact: 0.1% of 1,000,000 units.
        assert_eq!(min_self_bond(1_000_000), 1_000);
        assert_eq!(min_nomination(1_000_000), 10);
        // Not exact: rounded up.
        assert_eq!(min_self_bond(1_000_001), 1_001);
        assert_eq!(min_nomination(1), 1);
        assert_eq!(min_self_bond(CAP), CAP / 1_000);
        assert_eq!(min_nomination(CAP), CAP / 100_000);
        assert_eq!(min_self_bond(u128::MAX), u128::MAX / PPM + 1);
    }

    // Scenario "质押不足".
    #[test]
    fn stake_below_threshold() {
        assert!(!transition_check(&inputs(90, 30, 70_000_000), &params()));
        assert!(transition_check(&inputs(100, 30, 70_000_000), &params()));
    }

    #[test]
    fn candidates_below_threshold() {
        assert!(!transition_check(&inputs(200, 20, 70_000_000), &params()));
        assert!(transition_check(&inputs(200, 21, 70_000_000), &params()));
    }

    // Scenario "时间未到".
    #[test]
    fn height_below_threshold() {
        assert!(!transition_check(&inputs(200, 30, 63_115_199), &params()));
        assert!(transition_check(&inputs(200, 30, 63_115_200), &params()));
    }

    // Scenario "中途跌破重新计时" and "连续保持 7 天后切换".
    #[test]
    fn interrupted_run_restarts() {
        let p = params();
        let h0 = 63_115_200;
        let day = 86_400;
        let s = transition_step(None, &inputs(120, 30, h0), &p);
        assert_eq!((s.qualified_since, s.switch), (Some(h0), false));
        let s = transition_step(s.qualified_since, &inputs(120, 30, h0 + 3 * day), &p);
        assert_eq!((s.qualified_since, s.switch), (Some(h0), false));
        // Drops to 9%: cleared.
        let s = transition_step(s.qualified_since, &inputs(90, 30, h0 + 3 * day + 3600), &p);
        assert_eq!((s.ok, s.qualified_since, s.switch), (false, None, false));
        // Qualifies again: a new run starts.
        let h1 = h0 + 4 * day;
        let s = transition_step(s.qualified_since, &inputs(120, 30, h1), &p);
        assert_eq!(s.qualified_since, Some(h1));
        // Seven days after the first run started is not enough.
        let s = transition_step(s.qualified_since, &inputs(120, 30, h0 + 7 * day), &p);
        assert!(!s.switch);
        let s = transition_step(s.qualified_since, &inputs(120, 30, h1 + 7 * day - 1), &p);
        assert!(!s.switch);
        let s = transition_step(s.qualified_since, &inputs(120, 30, h1 + 7 * day), &p);
        assert!(s.switch);
    }

    #[test]
    fn zero_sustain_switches_at_first_pass() {
        let p = TransitionParams {
            sustain_blocks: 0,
            ..params()
        };
        assert!(transition_step(None, &inputs(120, 30, 63_115_200), &p).switch);
    }

    // Scenario "少量退出等待下限".
    #[test]
    fn small_exit_waits_the_minimum() {
        let p = UnbondingParams::new(172_800, 2_419_200);
        let total = 5_000_000 * UNITS;
        let (unlock, end) = unbonding_unlock(1_000, 0, UNITS, total, &p);
        assert_eq!(unlock, 1_000 + 172_800);
        assert!(end < unlock);
    }

    // Scenario "集中退出等待变长但不超过上限".
    #[test]
    fn mass_exit_waits_longer_up_to_the_maximum() {
        let p = UnbondingParams::new(172_800, 2_419_200);
        let total = 5_000_000 * UNITS;
        let mut end = 0;
        let mut last = 0;
        for _ in 0..10 {
            let (unlock, e) = unbonding_unlock(1_000, end, total / 10, total, &p);
            assert!(unlock >= last);
            assert!(unlock <= 1_000 + 2_419_200);
            last = unlock;
            end = e;
        }
        assert!(last > 1_000 + 172_800);
        assert_eq!(last, 1_000 + 2_419_200);
    }

    #[test]
    fn empty_stake_drains_one_unit_per_block() {
        let p = UnbondingParams::new(10, 20);
        assert_eq!(unbonding_unlock(100, 0, 5, 0, &p), (110, 105));
        assert_eq!(unbonding_unlock(100, 0, 50, 0, &p), (120, 150));
    }

    // Scenario "支撑额不同、工作量相同" and "漏块少分".
    #[test]
    fn split_by_work_points() {
        let equal = split_by_points(1_000, &[("a", 50), ("b", 50)]);
        assert_eq!(equal, vec![("a", 500), ("b", 500)]);
        let uneven = split_by_points(1_000, &[("a", 60), ("b", 30)]);
        assert_eq!(uneven, vec![("a", 666), ("b", 333)]);
        assert!(split_by_points(1_000, &[("a", 0), ("b", 0)]).is_empty());
        assert_eq!(
            split_by_points(1_000, &[("a", 0), ("b", 7)]),
            vec![("b", 1_000)]
        );
    }

    // Scenario "佣金与按比例分配".
    #[test]
    fn commission_then_pro_rata() {
        let split = split_reward(100 * UNITS, 1_000, &[("v", 100), ("n", 200)]);
        assert_eq!(split.commission, 10 * UNITS);
        assert_eq!(split.shares, vec![("v", 30 * UNITS), ("n", 60 * UNITS)]);
        let all = split_reward(100, 10_000, &[("v", 1), ("n", 2)]);
        assert_eq!(all.commission, 100);
        assert_eq!(all.shares, vec![("v", 0), ("n", 0)]);
        let none = split_reward::<&str>(100, 500, &[]);
        assert_eq!(none.commission, 5);
        assert!(none.shares.is_empty());
    }

    // Scenario "权重与支撑额成比例".
    #[test]
    fn weights_follow_backing() {
        let three = backing_to_weight(3_000_000 * UNITS);
        let one = backing_to_weight(1_000_000 * UNITS);
        assert_eq!(three, 3 * one);
        assert_eq!(backing_to_weight(0), 1);
        assert_eq!(backing_to_weight(BACKING_PER_WEIGHT - 1), 1);
        assert_eq!(backing_to_weight(CAP), 21_000_000_000_000);
        assert_eq!(backing_to_weight(u128::MAX), u64::MAX);
    }

    #[test]
    fn pro_rata_does_not_overflow() {
        assert_eq!(pro_rata(CAP, CAP, CAP), CAP);
        assert_eq!(pro_rata(u128::MAX, 3, 4), u128::MAX / 4 * 3 + 2);
        assert_eq!(pro_rata(5, 1, 0), 0);
    }

    proptest! {
        #[test]
        fn failing_checkpoint_clears_and_never_switches(
            stake in 0u128..CAP, issuance in 0u128..CAP, cands in 0u32..100,
            height in 0u64..200_000_000, prev in proptest::option::of(0u64..200_000_000),
        ) {
            let i = TransitionInputs::new(stake, issuance, cands, height);
            let s = transition_step(prev, &i, &params());
            if !transition_check(&i, &params()) {
                prop_assert_eq!(s.qualified_since, None);
                prop_assert!(!s.switch);
            } else {
                prop_assert_eq!(s.qualified_since, Some(prev.unwrap_or(height)));
                prop_assert!(stake * 10 >= issuance && cands >= 21 && height >= 63_115_200);
            }
        }

        #[test]
        fn weight_ratio_error_is_at_most_one_unit(a in 0u128..CAP, b in 1u128..CAP) {
            let (wa, wb) = (backing_to_weight(a), backing_to_weight(b));
            // |wa/wb − a/b| ≤ 1 unit: wa·b and a·wb/… compared through the exact quotient.
            let exact = a / BACKING_PER_WEIGHT;
            prop_assert!(u128::from(wa) <= exact.max(1) && u128::from(wa) + 1 > exact);
            prop_assert!(wb >= 1);
        }

        #[test]
        fn unlock_is_bounded_and_queue_monotone(
            now in 0u64..1_000_000_000, queue in 0u64..2_000_000_000,
            amount in 0u128..CAP, small in 0u128..CAP, total in 0u128..CAP,
            min in 0u64..1_000_000, extra in 0u64..3_000_000,
        ) {
            let p = UnbondingParams::new(min, min + extra);
            let (unlock, end) = unbonding_unlock(now, queue, amount, total, &p);
            prop_assert!(unlock >= now + min && unlock <= now + min + extra);
            prop_assert!(end >= queue && end >= now);
            let small = small.min(amount);
            let (unlock_small, _) = unbonding_unlock(now, queue, small, total, &p);
            prop_assert!(unlock_small <= unlock);
        }

        #[test]
        fn splits_never_create_atc(
            amount in 0u128..CAP,
            points in proptest::collection::vec(0u64..10_000, 0..20),
            commission in 0u32..20_000,
            stakes in proptest::collection::vec(0u128..CAP, 0..20),
        ) {
            let points: Vec<(usize, u64)> = points.into_iter().enumerate().collect();
            let shares = split_by_points(amount, &points);
            let paid: u128 = shares.iter().map(|(_, a)| a).sum();
            prop_assert!(paid <= amount);
            if points.iter().all(|(_, p)| *p == 0) {
                prop_assert_eq!(paid, 0);
            }
            let exposure: Vec<(usize, u128)> = stakes.into_iter().enumerate().collect();
            let split = split_reward(amount, commission, &exposure);
            let sum: u128 = split.shares.iter().map(|(_, a)| a).sum();
            prop_assert!(split.commission + sum <= amount);
            if commission >= BPS {
                prop_assert_eq!(split.commission, amount);
            }
        }
    }
}
