//! Public jobs (m6-public-jobs design D2; spec `market/public-jobs`).
//!
//! Jobs published by governance are cut into units. Three workers drawn from the round's roster
//! run each unit, commit to a small summary of their result and reveal it once the commit window
//! has closed; a unit passes when at least two summaries agree under the job's comparison rules.
//! Canary units, whose expected summaries the publisher committed to in a Merkle root, catch a
//! colluding majority.
//!
//! Everything here is integer-only and shared by the runtime, the runtime API, wallets and the
//! worker service, so all of them judge, draw and hash exactly alike.

use alloc::vec::Vec;

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::{ConstU32, H256};
use sp_runtime::BoundedVec;

use super::audit::{Drawing, RoundIndex, draw_distinct};
use super::model::ModelId;
use super::usd::{MICRO_USD_PER_USD, MicroUsd};
use super::work::JobKind;
use crate::emission::EpochIndex;

/// Identifier of a job, assigned in publication order from 0.
pub type JobId = u32;
/// Index of a unit within its job.
pub type UnitIndex = u32;

/// Hashing context of a worker's commitment.
pub const COMMIT_CONTEXT: &str = "agentcoin 2026-10 public-commit v1";
/// Hashing context of canary leaves (`0x00 ‖ …`) and inner nodes (`0x01 ‖ left ‖ right`).
pub const CANARY_CONTEXT: &str = "agentcoin 2026-10 public-canary v1";
/// Hashing context of the worker draw: `seed ‖ SCALE(job, unit, attempt) ‖ u32_le(i)`.
pub const ASSIGN_CONTEXT: &str = "agentcoin 2026-10 public-assign v1";
/// Hashing context of the embedding fingerprint directions: `SCALE(job, j, block)`.
pub const DIRECTION_CONTEXT: &str = "agentcoin 2026-10 public-direction v1";
/// Subject of a round's seed in the chain's randomness: this prefix ‖ `u32_le(round)`.
pub const ROUND_SEED_SUBJECT: &[u8] = b"agentcoin/public-round";

/// The only comparison rules version so far.
pub const RULES_V1: u8 = 1;
/// Workers per unit.
pub const REDUNDANCY: usize = 3;
/// Attempts before a unit is marked failed.
pub const MAX_ATTEMPTS: u8 = 3;
/// Consecutive misses that suspend a worker.
pub const MISSES_TO_SUSPEND: u8 = 3;
/// Most registered workers.
pub const MAX_WORKERS: u32 = 1_000;
/// Most models a worker declares.
pub const MAX_WORKER_MODELS: u32 = 8;
/// Most jobs in progress at once.
pub const MAX_ACTIVE_JOBS: u32 = 16;
/// Most units of a job.
pub const MAX_JOB_UNITS: u32 = 100_000;
/// Longest summary, in bytes.
pub const MAX_SUMMARY: u32 = 1_024;
/// Longest manifest or results URL, in bytes.
pub const MAX_URL: u32 = 256;
/// Deepest canary Merkle proof (about a million leaves).
pub const MAX_CANARY_DEPTH: u32 = 20;
/// Most units opened in one round (guardrail of `units_per_round`).
pub const MAX_UNITS_PER_ROUND: u32 = 256;
/// Highest per-unit price cap the administration may set: $10.
pub const MAX_PRICE_CAP: MicroUsd = MicroUsd(10 * MICRO_USD_PER_USD);

/// Most items of an evaluation unit and texts of an embedding unit.
pub const MAX_ITEMS: usize = 256;
/// Most choices of an evaluation item.
pub const MAX_CHOICES: u8 = 16;
/// Bits of an embedding fingerprint.
pub const FINGERPRINT_BITS: u32 = 32;
/// Hamming distance up to which two fingerprints of one text count as the same (rules v1).
pub const FINGERPRINT_TOLERANCE: u32 = 3;

/// A URL stored on chain.
pub type Url = BoundedVec<u8, ConstU32<MAX_URL>>;
/// A unit summary (format per job kind, see [`check_summary`]).
pub type Summary = BoundedVec<u8, ConstU32<MAX_SUMMARY>>;
/// Models a worker declares.
pub type WorkerModels = BoundedVec<ModelId, ConstU32<MAX_WORKER_MODELS>>;
/// Siblings of a canary Merkle proof, leaf level first.
pub type CanarySiblings = BoundedVec<[u8; 32], ConstU32<MAX_CANARY_DEPTH>>;

/// What a job asks for (design D2).
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct JobSpec {
    /// Evaluation, data cleaning or embedding.
    pub kind: JobKind,
    /// The model evaluation and embedding units run; `None` for data cleaning.
    pub model: Option<ModelId>,
    /// Comparison rules version ([`RULES_V1`]).
    pub rules: u8,
    /// BLAKE3 of the data manifest.
    pub manifest_hash: [u8; 32],
    /// Where the manifest is served.
    pub manifest_url: Url,
    /// Where workers upload full results.
    pub results_url: Url,
    /// Number of units, `1..=MAX_JOB_UNITS`.
    pub units: u32,
    /// Price per unit and passing worker; positive and at most the current cap.
    pub price: MicroUsd,
    /// Merkle root of the canary leaves, if the job has canaries.
    pub canary_root: Option<[u8; 32]>,
}

/// Why a [`JobSpec`] is refused.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecError {
    /// Not evaluation, data cleaning or embedding (training kinds are reserved).
    Kind,
    /// Evaluation and embedding need a model, data cleaning takes none.
    Model,
    /// Unknown comparison rules version.
    Rules,
    /// Unit count outside `1..=MAX_JOB_UNITS`.
    Units,
    /// Price zero or above the cap.
    Price,
    /// An empty or non-UTF-8 URL.
    Url,
}

impl core::fmt::Display for SpecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Kind => "only evaluation, data cleaning and embedding jobs are accepted",
            Self::Model => "evaluation and embedding jobs need a model, data cleaning takes none",
            Self::Rules => "unknown comparison rules version",
            Self::Units => "a job has between 1 and 100,000 units",
            Self::Price => "the unit price must be positive and at most the cap",
            Self::Url => "URLs must be non-empty UTF-8",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for SpecError {}

/// Whether `kind` is a public job kind this version accepts.
#[must_use]
pub const fn is_public_kind(kind: JobKind) -> bool {
    matches!(kind, JobKind::Eval | JobKind::DataClean | JobKind::Embed)
}

/// Checks a job specification against the price cap.
///
/// # Errors
///
/// The first rule broken.
pub fn check_spec(spec: &JobSpec, price_cap: MicroUsd) -> Result<(), SpecError> {
    if !is_public_kind(spec.kind) {
        return Err(SpecError::Kind);
    }
    let needs_model = !matches!(spec.kind, JobKind::DataClean);
    if spec.model.is_some() != needs_model {
        return Err(SpecError::Model);
    }
    if spec.rules != RULES_V1 {
        return Err(SpecError::Rules);
    }
    if spec.units == 0 || spec.units > MAX_JOB_UNITS {
        return Err(SpecError::Units);
    }
    if spec.price == MicroUsd::ZERO || spec.price > price_cap {
        return Err(SpecError::Price);
    }
    for url in [&spec.manifest_url, &spec.results_url] {
        if url.is_empty() || core::str::from_utf8(url).is_err() {
            return Err(SpecError::Url);
        }
    }
    Ok(())
}

/// Why a summary does not have its kind's format.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryError {
    /// Not a public job kind, or an unknown rules version.
    Kind,
    /// Wrong length for the kind.
    Length,
    /// An evaluation answer index of 16 or more.
    Choice,
}

impl core::fmt::Display for SummaryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Kind => "no summary format for this job kind and rules version",
            Self::Length => "summary length does not fit the job kind",
            Self::Choice => "evaluation answers are below 16",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for SummaryError {}

/// Checks a summary's format (spec "摘要一致的判定"): evaluation 1–256 answer indices below 16,
/// embedding 1–256 fingerprints of 4 bytes, data cleaning a 32-byte hash.
///
/// # Errors
///
/// What is wrong with the format.
pub fn check_summary(kind: JobKind, rules: u8, summary: &[u8]) -> Result<(), SummaryError> {
    if rules != RULES_V1 {
        return Err(SummaryError::Kind);
    }
    let len = summary.len();
    match kind {
        JobKind::Eval => {
            if len == 0 || len > MAX_ITEMS {
                return Err(SummaryError::Length);
            }
            if summary.iter().any(|c| *c >= MAX_CHOICES) {
                return Err(SummaryError::Choice);
            }
            Ok(())
        }
        JobKind::Embed => {
            if len == 0 || !len.is_multiple_of(4) || len / 4 > MAX_ITEMS {
                return Err(SummaryError::Length);
            }
            Ok(())
        }
        JobKind::DataClean => {
            if len != 32 {
                return Err(SummaryError::Length);
            }
            Ok(())
        }
        _ => Err(SummaryError::Kind),
    }
}

/// Outliers two summaries of `n` items may differ in: `max(1, ⌊n × 2 / 100⌋)`.
#[must_use]
pub fn tolerance(n: usize) -> usize {
    n.saturating_mul(2).checked_div(100).unwrap_or(0).max(1)
}

/// Whether two summaries agree under the rules (spec "摘要一致的判定"). Summaries of the wrong
/// format never agree.
#[must_use]
pub fn agree(kind: JobKind, rules: u8, a: &[u8], b: &[u8]) -> bool {
    if check_summary(kind, rules, a).is_err() || check_summary(kind, rules, b).is_err() {
        return false;
    }
    if a.len() != b.len() {
        return false;
    }
    match kind {
        JobKind::Eval => {
            let differ = a.iter().zip(b).filter(|(x, y)| x != y).count();
            differ <= tolerance(a.len())
        }
        JobKind::Embed => {
            let far = a
                .chunks_exact(4)
                .zip(b.chunks_exact(4))
                .filter(|(x, y)| {
                    let x = u32::from_le_bytes((*x).try_into().unwrap_or([0; 4]));
                    let y = u32::from_le_bytes((*y).try_into().unwrap_or([0; 4]));
                    (x ^ y).count_ones() > FINGERPRINT_TOLERANCE
                })
                .count();
            far <= tolerance(a.len() / 4)
        }
        JobKind::DataClean => a == b,
        _ => false,
    }
}

/// How a unit's revealed summaries settle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Majority {
    /// Slot (assignment order) of the reference summary.
    pub reference: u8,
    /// Slots whose summaries agree with the reference, the reference included.
    pub members: [bool; REDUNDANCY],
}

/// Picks the reference summary and the majority (design D2; spec "单元结算"): the revealed
/// summary agreeing with most others, ties to the earlier slot; `None` when no two agree.
#[must_use]
pub fn reference(
    kind: JobKind,
    rules: u8,
    revealed: &[Option<&[u8]>; REDUNDANCY],
) -> Option<Majority> {
    let mut best: Option<(usize, usize)> = None;
    for (i, a) in revealed.iter().enumerate() {
        let Some(a) = a else { continue };
        let count = revealed
            .iter()
            .enumerate()
            .filter(|(j, b)| *j != i && b.is_some_and(|b| agree(kind, rules, a, b)))
            .count();
        if count > 0 && best.is_none_or(|(_, c)| count > c) {
            best = Some((i, count));
        }
    }
    let (r, _) = best?;
    let reference = revealed.get(r).copied().flatten()?;
    let mut members = [false; REDUNDANCY];
    for (j, slot) in members.iter_mut().enumerate() {
        *slot = j == r
            || revealed
                .get(j)
                .copied()
                .flatten()
                .is_some_and(|b| agree(kind, rules, reference, b));
    }
    Some(Majority {
        reference: u8::try_from(r).ok()?,
        members,
    })
}

/// What a worker commits to.
#[derive(Clone, Copy, Debug)]
pub struct Reveal<'a, A> {
    /// The job.
    pub job: JobId,
    /// The unit.
    pub unit: UnitIndex,
    /// The attempt (1 for the first).
    pub attempt: u8,
    /// The worker.
    pub worker: &'a A,
    /// The summary.
    pub summary: &'a [u8],
    /// BLAKE3 of the full result.
    pub result_hash: &'a [u8; 32],
    /// The worker's random salt.
    pub salt: &'a [u8; 32],
}

/// A worker's commitment: `derive(COMMIT_CONTEXT, SCALE(job, unit, attempt, worker, summary,
/// result_hash, salt))`, where the summary is encoded as a byte vector.
#[must_use]
pub fn commitment<A: Encode>(r: &Reveal<'_, A>) -> H256 {
    let data = (
        r.job,
        r.unit,
        r.attempt,
        r.worker,
        r.summary,
        r.result_hash,
        r.salt,
    )
        .encode();
    // The context is well formed, so `derive` cannot fail; the zero hash never matches a
    // commitment made with the same function.
    H256(ac_crypto::hash::derive(COMMIT_CONTEXT, &data).unwrap_or([0; 32]))
}

/// A canary leaf: `derive(CANARY_CONTEXT, 0x00 ‖ SCALE(job, unit, summary, salt))`.
#[must_use]
pub fn canary_leaf(job: JobId, unit: UnitIndex, summary: &[u8], salt: &[u8; 32]) -> [u8; 32] {
    let mut data = Vec::with_capacity(summary.len().saturating_add(48));
    data.push(0u8);
    (job, unit, summary, salt).encode_to(&mut data);
    ac_crypto::hash::derive(CANARY_CONTEXT, &data).unwrap_or([0; 32])
}

fn canary_node(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut data = Vec::with_capacity(65);
    data.push(1u8);
    data.extend_from_slice(left);
    data.extend_from_slice(right);
    ac_crypto::hash::derive(CANARY_CONTEXT, &data).unwrap_or([0; 32])
}

fn next_level(level: &[[u8; 32]]) -> Vec<[u8; 32]> {
    level
        .chunks(2)
        .map(|pair| match pair {
            [l, r] => canary_node(l, r),
            [single] => *single,
            _ => [0; 32],
        })
        .collect()
}

/// Merkle root of canary leaves: pairs hash up level by level and the last node of an odd level
/// rises unchanged. `None` for no leaves.
#[must_use]
pub fn canary_root(leaves: &[[u8; 32]]) -> Option<[u8; 32]> {
    let mut level = leaves.to_vec();
    if level.is_empty() {
        return None;
    }
    while level.len() > 1 {
        level = next_level(&level);
    }
    level.first().copied()
}

/// A proof that a leaf is in a canary tree.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct CanaryProof {
    /// Position of the leaf.
    pub index: u32,
    /// Number of leaves of the tree.
    pub leaves: u32,
    /// Siblings on the way up, leaf level first; levels where the node rises alone have none.
    pub siblings: CanarySiblings,
}

/// The proof for leaf `index`; `None` when out of range or too deep.
#[must_use]
pub fn canary_proof(leaves: &[[u8; 32]], index: usize) -> Option<CanaryProof> {
    if index >= leaves.len() {
        return None;
    }
    let mut siblings = Vec::new();
    let mut level = leaves.to_vec();
    let mut pos = index;
    while level.len() > 1 {
        let sibling = pos ^ 1;
        if let Some(s) = level.get(sibling) {
            siblings.push(*s);
        }
        level = next_level(&level);
        pos /= 2;
    }
    Some(CanaryProof {
        index: u32::try_from(index).ok()?,
        leaves: u32::try_from(leaves.len()).ok()?,
        siblings: BoundedVec::try_from(siblings).ok()?,
    })
}

/// What a worker reveals for a unit (one call argument, G.FUD.01).
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct WorkerReveal {
    /// The summary it committed to.
    pub summary: Summary,
    /// BLAKE3 of its full result.
    pub result_hash: [u8; 32],
    /// The salt of its commitment.
    pub salt: [u8; 32],
}

/// A canary leaf's content and its proof (one call argument, G.FUD.01).
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct CanaryReveal {
    /// The expected summary.
    pub summary: Summary,
    /// The leaf's salt.
    pub salt: [u8; 32],
    /// The Merkle proof.
    pub proof: CanaryProof,
}

/// Whether `leaf` is in the tree with `root` according to `proof`.
#[must_use]
pub fn verify_canary(root: &[u8; 32], leaf: &[u8; 32], proof: &CanaryProof) -> bool {
    if proof.index >= proof.leaves {
        return false;
    }
    let mut node = *leaf;
    let mut pos = proof.index;
    let mut width = proof.leaves;
    let mut siblings = proof.siblings.iter();
    while width > 1 {
        if pos % 2 == 1 {
            let Some(s) = siblings.next() else {
                return false;
            };
            node = canary_node(s, &node);
        } else if pos.saturating_add(1) < width {
            let Some(s) = siblings.next() else {
                return false;
            };
            node = canary_node(&node, s);
        }
        pos /= 2;
        width = width.div_ceil(2);
    }
    siblings.next().is_none() && node == *root
}

/// Draws a unit's workers: up to [`REDUNDANCY`] distinct roster members for which `eligible`
/// holds, by `seed ‖ SCALE(job, unit, attempt) ‖ u32_le(i)` under [`ASSIGN_CONTEXT`].
#[must_use]
pub fn assign_unit<A: PartialEq + Clone>(
    roster: &[A],
    seed: &H256,
    unit: (JobId, UnitIndex, u8),
    eligible: impl Fn(&A) -> bool,
) -> Vec<A> {
    let prefix = unit.encode();
    draw_distinct(
        Drawing {
            context: ASSIGN_CONTEXT,
            seed,
            prefix: &prefix,
        },
        roster,
        eligible,
        REDUNDANCY,
    )
}

/// Most units one worker is drawn for in a round: `⌈3 × opened / roster⌉ + 1`.
#[must_use]
pub fn max_per_worker(units_per_round: u32, roster: usize) -> u32 {
    let roster = u32::try_from(roster).unwrap_or(u32::MAX).max(1);
    units_per_round
        .saturating_mul(3)
        .div_ceil(roster)
        .saturating_add(1)
}

/// Block `b` of fingerprint direction `j` of `job`: 256 signs, bit `d` of the hash giving the
/// sign of dimension `256 × b + d` (set: +1).
#[must_use]
pub fn direction_block(job: JobId, j: u32, block: u32) -> [u8; 32] {
    ac_crypto::hash::derive(DIRECTION_CONTEXT, &(job, j, block).encode()).unwrap_or([0; 32])
}

/// Parameters fixed at genesis, plus the price cap the administration adjusts (design D8).
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
pub struct PublicParams {
    /// Blocks per round.
    pub round_blocks: u32,
    /// Units opened per round, `1..=MAX_UNITS_PER_ROUND`.
    pub units_per_round: u32,
    /// Blocks from opening to the commit deadline.
    pub commit_blocks: u32,
    /// Blocks from the commit deadline to the reveal deadline.
    pub reveal_blocks: u32,
    /// Emission epochs before a unit's work counts.
    pub challenge_epochs: u32,
    /// Blocks claimed rewards stay locked.
    pub lock_blocks: u32,
    /// Blocks a suspended worker stays out of the roster.
    pub suspend_blocks: u32,
    /// Blocks a settled unit's record is kept.
    pub retention_blocks: u32,
    /// Highest unit price, `0..=MAX_PRICE_CAP`.
    pub price_cap: MicroUsd,
}

/// Why [`PublicParams`] break the guardrails.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamsError {
    /// A block count is zero.
    ZeroBlocks,
    /// Units per round outside `1..=256`.
    UnitsPerRound,
    /// The challenge period is shorter than one epoch.
    Challenge,
    /// The price cap is above $10.
    PriceCap,
}

impl core::fmt::Display for ParamsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::ZeroBlocks => {
                "round, commit, reveal, lock, suspension and retention blocks must be positive"
            }
            Self::UnitsPerRound => "units per round must be between 1 and 256",
            Self::Challenge => "the challenge period is at least one epoch",
            Self::PriceCap => "the unit price cap is at most $10",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ParamsError {}

impl PublicParams {
    /// Draft values of live chains.
    pub const LIVE: Self = Self {
        round_blocks: 600,
        units_per_round: 64,
        commit_blocks: 1_800,
        reveal_blocks: 300,
        challenge_epochs: 2,
        lock_blocks: 604_800,
        suspend_blocks: 86_400,
        retention_blocks: 604_800,
        price_cap: MicroUsd(MICRO_USD_PER_USD),
    };

    /// Development and local chains: a unit goes from opening to withdrawal in minutes.
    pub const DEV: Self = Self {
        round_blocks: 10,
        units_per_round: 8,
        commit_blocks: 20,
        reveal_blocks: 10,
        challenge_epochs: 1,
        lock_blocks: 30,
        suspend_blocks: 30,
        retention_blocks: 200,
        price_cap: MicroUsd(MICRO_USD_PER_USD),
    };

    /// Checks the guardrails.
    ///
    /// # Errors
    ///
    /// The first guardrail broken.
    pub fn check(&self) -> Result<(), ParamsError> {
        if [
            self.round_blocks,
            self.commit_blocks,
            self.reveal_blocks,
            self.lock_blocks,
            self.suspend_blocks,
            self.retention_blocks,
        ]
        .contains(&0)
        {
            return Err(ParamsError::ZeroBlocks);
        }
        if self.units_per_round == 0 || self.units_per_round > MAX_UNITS_PER_ROUND {
            return Err(ParamsError::UnitsPerRound);
        }
        if self.challenge_epochs == 0 {
            return Err(ParamsError::Challenge);
        }
        if self.price_cap > MAX_PRICE_CAP {
            return Err(ParamsError::PriceCap);
        }
        Ok(())
    }
}

/// A registered worker.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct WorkerRecord<BlockNumber> {
    /// Models it can run.
    pub models: WorkerModels,
    /// Last round it declared itself ready in.
    pub last_ready: Option<RoundIndex>,
    /// Consecutive misses.
    pub misses: u8,
    /// Block its suspension ends at, if suspended.
    pub suspended_until: Option<BlockNumber>,
    /// Units it passed in the majority.
    pub accepted: u32,
    /// Misses in total.
    pub missed: u32,
}

/// A published job.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct JobRecord<BlockNumber> {
    /// What it asks for.
    pub spec: JobSpec,
    /// Block it was published in.
    pub published_at: BlockNumber,
    /// Units opened for the first time so far (units `0..opened`).
    pub opened: u32,
    /// Units that passed.
    pub accepted: u32,
    /// Units that failed.
    pub failed: u32,
    /// Whether it was cancelled.
    pub cancelled: bool,
}

/// Where a unit stands.
///
/// Wire-format enum: variant indices are explicit and never reused.
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
)]
pub enum UnitState {
    /// Assigned, collecting commitments and reveals.
    #[codec(index = 0)]
    Open,
    /// Passed: the reference slot and the majority.
    #[codec(index = 1)]
    Accepted {
        /// Slot of the reference summary.
        reference: u8,
        /// Majority slots.
        majority: [bool; REDUNDANCY],
    },
    /// Did not pass; waiting to be reopened.
    #[codec(index = 2)]
    Retry,
    /// Failed for good (attempts exhausted or caught by its canary).
    #[codec(index = 3)]
    Failed,
}

/// A worker's reveal as stored: the summary and the result hash.
pub type Revealed = (Summary, [u8; 32]);

/// A unit's current attempt.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct UnitRecord<AccountId, BlockNumber> {
    /// Attempt number, from 1.
    pub attempt: u8,
    /// Block of the current attempt's opening.
    pub opened_at: BlockNumber,
    /// Last block commitments are accepted in.
    pub commit_by: BlockNumber,
    /// Last block reveals are accepted in.
    pub reveal_by: BlockNumber,
    /// The workers, in draw order.
    pub assigned: [AccountId; REDUNDANCY],
    /// Workers of earlier attempts, never drawn again for this unit.
    pub tried: BoundedVec<AccountId, ConstU32<{ (MAX_ATTEMPTS as u32 - 1) * REDUNDANCY as u32 }>>,
    /// Their commitments.
    pub commits: [Option<H256>; REDUNDANCY],
    /// Their reveals.
    pub reveals: [Option<Revealed>; REDUNDANCY],
    /// Where it stands.
    pub state: UnitState,
    /// Block it settled in.
    pub settled_at: Option<BlockNumber>,
    /// Whether its canary was revealed.
    pub canary_revealed: bool,
}

/// Public work and emission of an epoch.
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
)]
pub struct EpochPublic<Balance> {
    /// Verified public work maturing in the epoch.
    pub verified: Balance,
    /// Public emission minted for it, once settled.
    pub emission: Option<Balance>,
}

/// Locked rewards of a worker: `(unlock block, amount)` segments.
pub type LockedList<BlockNumber> = BoundedVec<(BlockNumber, u128), ConstU32<32>>;

/// An open unit as a worker sees it.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct Assignment<BlockNumber> {
    /// The job.
    pub job: JobId,
    /// The unit.
    pub unit: UnitIndex,
    /// Its attempt.
    pub attempt: u8,
    /// Commit deadline.
    pub commit_by: BlockNumber,
    /// Reveal deadline.
    pub reveal_by: BlockNumber,
    /// Whether this worker committed.
    pub committed: bool,
    /// Whether this worker revealed.
    pub revealed: bool,
}

sp_api::decl_runtime_apis! {
    /// Read access to the public jobs (design D9).
    pub trait PublicJobsApi<AccountId, BlockNumber>
    where
        AccountId: parity_scale_codec::Codec,
        BlockNumber: parity_scale_codec::Codec,
    {
        /// The current round and its first and last block (`None` before genesis parameters).
        fn round() -> Option<(RoundIndex, u32, u32)>;
        /// The current round's roster and seed.
        fn roster() -> (Vec<AccountId>, Option<H256>);
        /// A worker's record.
        fn worker(who: AccountId) -> Option<WorkerRecord<BlockNumber>>;
        /// A job.
        fn job(id: JobId) -> Option<JobRecord<BlockNumber>>;
        /// Jobs in progress, in publication order.
        fn jobs() -> Vec<JobId>;
        /// A unit's current attempt.
        fn unit(job: JobId, unit: UnitIndex) -> Option<UnitRecord<AccountId, BlockNumber>>;
        /// A worker's unsettled units.
        fn assigned(who: AccountId) -> Vec<Assignment<BlockNumber>>;
        /// An epoch's public work and emission.
        fn epoch(epoch: EpochIndex) -> EpochPublic<u128>;
        /// A worker's unclaimed public work by maturity epoch.
        fn pending(who: AccountId) -> Vec<(EpochIndex, u128)>;
        /// A worker's locked rewards.
        fn locked(who: AccountId) -> Vec<(BlockNumber, u128)>;
        /// Balance of the public payout account.
        fn pot_balance() -> u128;
        /// Current parameters.
        fn params() -> Option<PublicParams>;
    }
}

#[cfg(test)]
mod tests;
