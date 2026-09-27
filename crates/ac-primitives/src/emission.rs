//! Scheduled emission of ATC (plan §5.1; decisions D11, D14–D16, D18, D19; design D2–D3 of
//! `m3-economics`).
//!
//! The runtime settles emission with [`settle`], the node's invariant checker bounds minting
//! with [`EmissionSchedule`], and the economic simulation replays both: one implementation, so
//! all three compute the same numbers. Integer arithmetic only, rounding down; remainders are
//! never minted.
//!
//! - The first four-year period of [`BLOCKS_PER_PERIOD`] blocks emits
//!   [`FIRST_PERIOD_TOTAL`]; every later period emits half of the previous one.
//! - Emission is settled per *emission epoch* of `L` blocks (a genesis parameter that must
//!   divide [`BLOCKS_PER_PERIOD`], so halvings fall on epoch boundaries). Blocks `1..=L` form
//!   epoch 0; epoch `e` is settled in block `(e + 1) × L + 1`, and only settlement blocks mint.

/// Emission epoch index.
pub type EpochIndex = u64;

/// Smallest ATC units per ATC (18 decimals).
pub const UNITS: u128 = 1_000_000_000_000_000_000;

/// Total supply cap: 21,000,000 ATC (D14).
pub const CAP: u128 = 21_000_000 * UNITS;

/// Scheduled emission of the first four-year period: 10,500,000 ATC (D15).
pub const FIRST_PERIOD_TOTAL: u128 = 10_500_000 * UNITS;

/// Blocks per four-year period at one block per second: 4 × 365.25 × 86,400.
pub const BLOCKS_PER_PERIOD: u64 = 126_230_400;

/// Emission epoch length of live chains (about one hour).
pub const LIVE_EPOCH_LENGTH: u64 = 3_600;

/// Basis-point denominator.
pub const BPS: u128 = 10_000;

/// Security budget: 10% of the scheduled amount (D16).
pub const SECURITY_BPS: u128 = 1_000;

/// Market-work cap: 50% of the available amount (D16).
pub const MARKET_BPS: u128 = 5_000;

/// Public-work cap: 20% of the available amount (D16).
pub const PUBLIC_BPS: u128 = 2_000;

/// Treasury floor: 5% of the scheduled amount (D18).
pub const TREASURY_FLOOR_BPS: u128 = 500;

/// Blocks over which a treasury-floor batch vests linearly: 2 × 365.25 days.
pub const FLOOR_VESTING_BLOCKS: u64 = 63_115_200;

/// Length of a treasury-floor batch: 30 days.
pub const FLOOR_BATCH_BLOCKS: u64 = 2_592_000;

/// Why an emission epoch length is not allowed.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmissionError {
    /// The length is zero or does not divide [`BLOCKS_PER_PERIOD`].
    InvalidEpochLength(u64),
}

impl core::fmt::Display for EmissionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidEpochLength(l) => write!(
                f,
                "emission epoch length {l} must be non-zero and divide {BLOCKS_PER_PERIOD}"
            ),
        }
    }
}

/// Whether `length` is a valid emission epoch length.
#[must_use]
pub fn is_valid_epoch_length(length: u64) -> bool {
    length != 0 && BLOCKS_PER_PERIOD.checked_rem(length) == Some(0)
}

/// The emission curve for one epoch length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmissionSchedule {
    length: u64,
    epochs_per_period: u64,
}

impl EmissionSchedule {
    /// The schedule for epochs of `length` blocks.
    ///
    /// # Errors
    ///
    /// [`EmissionError::InvalidEpochLength`] if `length` is zero or does not divide
    /// [`BLOCKS_PER_PERIOD`].
    pub fn new(length: u64) -> Result<Self, EmissionError> {
        match BLOCKS_PER_PERIOD.checked_div(length) {
            Some(epochs_per_period) if is_valid_epoch_length(length) => Ok(Self {
                length,
                epochs_per_period,
            }),
            _ => Err(EmissionError::InvalidEpochLength(length)),
        }
    }

    /// Epoch length in blocks.
    #[must_use]
    pub fn epoch_length(&self) -> u64 {
        self.length
    }

    /// Epochs per four-year period.
    #[must_use]
    pub fn epochs_per_period(&self) -> u64 {
        self.epochs_per_period
    }

    /// Scheduled amount of any epoch in period `n` (0 from period 128 on).
    #[must_use]
    pub fn period_epoch_amount(&self, period: u64) -> u128 {
        let Ok(shift) = u32::try_from(period) else {
            return 0;
        };
        let total = FIRST_PERIOD_TOTAL.checked_shr(shift).unwrap_or(0);
        // total ≤ 1.05e25 and length ≤ 1.3e8, so the product cannot overflow u128.
        total
            .saturating_mul(u128::from(self.length))
            .checked_div(u128::from(BLOCKS_PER_PERIOD))
            .unwrap_or(0)
    }

    /// Scheduled amount `S(e)` of epoch `epoch`.
    #[must_use]
    pub fn scheduled(&self, epoch: EpochIndex) -> u128 {
        let period = epoch.checked_div(self.epochs_per_period).unwrap_or(0);
        self.period_epoch_amount(period)
    }

    /// Sum of the scheduled amounts of the first `epochs` epochs (`S(0) + … + S(epochs − 1)`).
    #[must_use]
    pub fn cumulative_scheduled(&self, epochs: u64) -> u128 {
        let full = epochs.checked_div(self.epochs_per_period).unwrap_or(0);
        let partial = epochs.checked_rem(self.epochs_per_period).unwrap_or(0);
        let per_period = u128::from(self.epochs_per_period);
        let mut sum = 0u128;
        // Amounts are zero from period 128 on; stop there.
        for period in 0..full.min(128) {
            sum = sum.saturating_add(self.period_epoch_amount(period).saturating_mul(per_period));
        }
        sum.saturating_add(
            self.period_epoch_amount(full)
                .saturating_mul(u128::from(partial)),
        )
    }

    /// Number of epochs settled up to and including block `number`: `⌊(number − 1) / L⌋`
    /// (0 for genesis).
    #[must_use]
    pub fn settled_epochs(&self, number: u64) -> u64 {
        number
            .saturating_sub(1)
            .checked_div(self.length)
            .unwrap_or(0)
    }

    /// The epoch settled in block `number`, if it is a settlement block: block
    /// `(e + 1) × L + 1` settles epoch `e`.
    #[must_use]
    pub fn settled_epoch(&self, number: u64) -> Option<EpochIndex> {
        let offset = number.checked_sub(1)?;
        if offset.checked_rem(self.length)? != 0 {
            return None;
        }
        offset.checked_div(self.length)?.checked_sub(1)
    }
}

/// Validator phase (D19): PoA validators receive no security budget.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Proof of authority: the security budget rolls over into the reserve.
    Poa,
    /// Proof of stake: the security budget is paid to validators.
    Pos,
}

/// Inputs of one epoch's settlement. All amounts are in smallest ATC units.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EpochInput {
    /// Scheduled amount `S` of the epoch.
    pub scheduled: u128,
    /// Rollover reserve before settlement.
    pub reserve: u128,
    /// Verified paid market work.
    pub market_work: u128,
    /// Verified consumption of the public-job budget.
    pub public_work: u128,
    /// Validator phase.
    pub phase: Phase,
}

impl EpochInput {
    /// Settlement inputs.
    #[must_use]
    pub fn new(scheduled: u128, reserve: u128, work: (u128, u128), phase: Phase) -> Self {
        Self {
            scheduled,
            reserve,
            market_work: work.0,
            public_work: work.1,
            phase,
        }
    }
}

/// Result of one epoch's settlement. `total` is minted; the parts add up to it exactly.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EpochOutcome {
    /// Security budget paid to validators (0 in PoA).
    pub security: u128,
    /// Market-work emission.
    pub market: u128,
    /// Public-work emission.
    pub public: u128,
    /// Treasury share proportional to work emission (to the two treasury accounts).
    pub treasury_proportional: u128,
    /// Top-up to the treasury floor (to the vesting floor account).
    pub treasury_floor_topup: u128,
    /// Total minted this epoch.
    pub total: u128,
    /// Amount taken from the reserve (at most the scheduled amount).
    pub drawn: u128,
    /// Reserve after settlement.
    pub reserve: u128,
}

impl EpochOutcome {
    /// Treasury total: `max(floor, proportional)`.
    #[must_use]
    pub fn treasury(&self) -> u128 {
        self.treasury_proportional
            .saturating_add(self.treasury_floor_topup)
    }
}

/// `amount × bps / 10,000`, rounded down. Saturates instead of overflowing; amounts are at
/// most the cap (about 2^84), so the product never gets near `u128::MAX`.
#[must_use]
pub fn bps_of(amount: u128, bps: u128) -> u128 {
    amount.saturating_mul(bps).checked_div(BPS).unwrap_or(0)
}

/// Scales `part` by `num / den`, rounding down.
fn scale(part: u128, num: u128, den: u128) -> u128 {
    part.saturating_mul(num).checked_div(den).unwrap_or(0)
}

/// Settles one epoch (design D3 of `m3-economics`).
///
/// - `avail = S + min(R, S)`; security `10% × S` (paid only in PoS); market
///   `min(50% × avail, W_m)`; public `min(20% × avail, W_p)`; treasury
///   `max(5% × S, 20/70 × (market + public))` — the larger, never the sum (red line 5).
/// - If the parts exceed `avail` they are scaled down pro rata (never happens with the
///   D16 shares, kept as a safeguard).
/// - What exceeds `S` is drawn from the reserve (at most `S`); what stays below `S` goes to it,
///   so `reserve' + total = reserve + S`.
#[must_use]
pub fn settle(input: &EpochInput) -> EpochOutcome {
    let s = input.scheduled;
    let avail = s.saturating_add(input.reserve.min(s));
    let security = match input.phase {
        Phase::Poa => 0,
        Phase::Pos => bps_of(s, SECURITY_BPS),
    };
    let market = bps_of(avail, MARKET_BPS).min(input.market_work);
    let public = bps_of(avail, PUBLIC_BPS).min(input.public_work);
    let proportional = scale(market.saturating_add(public), 20, 70);
    let floor = bps_of(s, TREASURY_FLOOR_BPS);
    let topup = floor.saturating_sub(proportional);

    let mut parts = [security, market, public, proportional, topup];
    let sum = parts.iter().fold(0u128, |acc, p| acc.saturating_add(*p));
    if sum > avail {
        for part in &mut parts {
            *part = scale(*part, avail, sum);
        }
    }
    let [security, market, public, proportional, topup] = parts;
    let total = parts.iter().fold(0u128, |acc, p| acc.saturating_add(*p));
    let drawn = total.saturating_sub(s);
    let reserve = input
        .reserve
        .saturating_sub(drawn)
        .saturating_add(s.saturating_sub(total));
    EpochOutcome {
        security,
        market,
        public,
        treasury_proportional: proportional,
        treasury_floor_topup: topup,
        total,
        drawn,
        reserve,
    }
}

/// Splits the proportional treasury share: `community_bps` of it (rounded down) to community
/// grants, the rest to the holder treasury.
#[must_use]
pub fn split_treasury(proportional: u128, community_bps: u128) -> (u128, u128) {
    let community = bps_of(proportional, community_bps.min(BPS));
    (community, proportional.saturating_sub(community))
}

/// Vested part of a treasury-floor batch of `amount` that closed at block `batch_end`, at
/// block `now`: linear over [`FLOOR_VESTING_BLOCKS`] from `batch_end`, rounded down.
#[must_use]
pub fn vested(amount: u128, batch_end: u64, now: u64) -> u128 {
    let elapsed = now.saturating_sub(batch_end);
    if elapsed >= FLOOR_VESTING_BLOCKS {
        return amount;
    }
    scale(
        amount,
        u128::from(elapsed),
        u128::from(FLOOR_VESTING_BLOCKS),
    )
}

/// Source of verified work for the market and public shares (M5 `pallet-work`, M6
/// `pallet-public-jobs`). `()` reports no work, as in M3.
pub trait WorkSource {
    /// Verified `(market, public)` work of `epoch`, in smallest ATC units.
    fn verified_work(epoch: EpochIndex) -> (u128, u128);
}

impl WorkSource for () {
    fn verified_work(_epoch: EpochIndex) -> (u128, u128) {
        (0, 0)
    }
}

/// Recipients of the security budget. In PoA ([`PoaPhase`]) nothing is paid (D19); `m3-pos`
/// pays the validator set.
pub trait SecurityBudget<AccountId> {
    /// Current validator phase.
    fn phase() -> Phase;
    /// How `amount` is paid out: `(account, amount)` pairs summing to at most `amount`; the
    /// rest rolls over into the reserve.
    fn recipients(amount: u128) -> alloc::vec::Vec<(AccountId, u128)>;
}

/// Proof-of-authority phase: no security budget is paid.
pub struct PoaPhase;

impl<AccountId> SecurityBudget<AccountId> for PoaPhase {
    fn phase() -> Phase {
        Phase::Poa
    }

    fn recipients(_amount: u128) -> alloc::vec::Vec<(AccountId, u128)> {
        alloc::vec::Vec::new()
    }
}

/// Where the treasury part of an epoch goes (`pallet-treasury-dual`).
pub trait TreasuryDeposit<AccountId> {
    /// Accounts and amounts receiving the proportional share and the floor top-up; called once
    /// per settlement, so an implementation may record the floor batch here. The amounts sum to
    /// `proportional + floor_topup`.
    fn recipients(proportional: u128, floor_topup: u128) -> [(AccountId, u128); 3];
}

sp_api::decl_runtime_apis! {
    /// Emission queries (spec economics/emission "排放结果可复算与查询").
    pub trait EmissionApi {
        /// Emission epoch length in blocks.
        fn epoch_length() -> u64;
        /// Emission epoch of the next block.
        fn current_epoch() -> EpochIndex;
        /// Scheduled amount of `epoch`.
        fn scheduled(epoch: EpochIndex) -> u128;
        /// Rollover reserve.
        fn reserve() -> u128;
        /// Total minted by emission since genesis.
        fn total_minted() -> u128;
        /// Total burned since genesis.
        fn total_burned() -> u128;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::arithmetic_side_effects, clippy::unwrap_used)]

    use super::*;
    use proptest::prelude::*;

    fn live() -> EmissionSchedule {
        EmissionSchedule::new(LIVE_EPOCH_LENGTH).unwrap()
    }

    // Scenario "首个纪元的计划量".
    #[test]
    fn first_epoch_amount() {
        assert_eq!(
            live().scheduled(0),
            FIRST_PERIOD_TOTAL * 3_600 / 126_230_400
        );
        assert_eq!(live().epochs_per_period(), 35_064);
    }

    // Scenario "减半": epoch 35,064 is the first of the second period.
    #[test]
    fn halving() {
        let s = live();
        assert_eq!(
            s.scheduled(35_063),
            FIRST_PERIOD_TOTAL * 3_600 / 126_230_400
        );
        assert_eq!(
            s.scheduled(35_064),
            (FIRST_PERIOD_TOTAL / 2) * 3_600 / 126_230_400
        );
        assert_eq!(
            s.period_epoch_amount(127),
            (FIRST_PERIOD_TOTAL >> 127) * 3_600 / 126_230_400
        );
        assert_eq!(s.period_epoch_amount(128), 0);
        assert_eq!(s.scheduled(u64::MAX), 0);
    }

    // Scenario "非法纪元长度".
    #[test]
    fn invalid_epoch_lengths() {
        assert_eq!(
            EmissionSchedule::new(7),
            Err(EmissionError::InvalidEpochLength(7))
        );
        assert!(EmissionSchedule::new(0).is_err());
        for l in [1, 10, 20, 3_600, BLOCKS_PER_PERIOD] {
            assert!(EmissionSchedule::new(l).is_ok(), "{l}");
        }
    }

    #[test]
    fn settlement_blocks() {
        let s = EmissionSchedule::new(10).unwrap();
        assert_eq!(s.settled_epoch(0), None);
        assert_eq!(s.settled_epoch(1), None);
        assert_eq!(s.settled_epoch(10), None);
        assert_eq!(s.settled_epoch(11), Some(0));
        assert_eq!(s.settled_epoch(12), None);
        assert_eq!(s.settled_epoch(21), Some(1));
        assert_eq!(
            [0, 1, 10, 11, 20, 21].map(|n| s.settled_epochs(n)),
            [0, 0, 0, 1, 1, 2]
        );
    }

    // The whole curve stays below the cap.
    #[test]
    fn cumulative_is_below_cap() {
        let s = live();
        let all = s.cumulative_scheduled(u64::MAX);
        assert!(all <= CAP);
        assert!(CAP - all < UNITS, "{}", CAP - all);
    }

    const S: u128 = 1_000_000;

    // Scenario "无工作量的纪元" and "PoA 阶段不发放": only the 5% floor is minted, the rest
    // (including the security budget) rolls over.
    #[test]
    fn epoch_without_work() {
        let out = settle(&EpochInput::new(S, 0, (0, 0), Phase::Poa));
        assert_eq!(out.security, 0);
        assert_eq!(
            (out.market, out.public, out.treasury_proportional),
            (0, 0, 0)
        );
        assert_eq!(out.treasury_floor_topup, S * 5 / 100);
        assert_eq!(out.total, S * 5 / 100);
        assert_eq!(out.reserve, S * 95 / 100);
        assert_eq!(out.drawn, 0);
    }

    // Scenario "满负荷纪元": every cap is reached; the treasury is exactly 20/70 of work.
    #[test]
    fn full_load_epoch() {
        let reserve = 10 * S;
        let avail = 2 * S;
        let out = settle(&EpochInput::new(
            S,
            reserve,
            (u128::MAX, u128::MAX),
            Phase::Pos,
        ));
        assert_eq!(out.security, S / 10);
        assert_eq!(out.market, avail / 2);
        assert_eq!(out.public, avail / 5);
        assert_eq!(out.treasury_proportional, (avail / 2 + avail / 5) * 20 / 70);
        assert_eq!(out.treasury_floor_topup, 0);
        assert_eq!(
            out.total,
            out.security + out.market + out.public + out.treasury()
        );
        assert!(out.total <= avail);
    }

    // Scenario "储备取用上限": at most 2 × S is minted and at most S drawn.
    #[test]
    fn reserve_draw_cap() {
        let out = settle(&EpochInput::new(
            S,
            u128::MAX / 4,
            (u128::MAX, u128::MAX),
            Phase::Pos,
        ));
        assert!(out.total <= 2 * S);
        assert!(out.drawn <= S);
        assert_eq!(out.reserve, u128::MAX / 4 - out.drawn);
    }

    // Treasury: the larger of floor and proportional share, never both (red line 5).
    #[test]
    fn treasury_takes_the_larger() {
        // Small work: proportional 20/70 × 70 = 20 < floor 50_000 → topped up to the floor.
        let small = settle(&EpochInput::new(S, 0, (70, 0), Phase::Poa));
        assert_eq!(small.treasury(), S / 20);
        assert_eq!(small.treasury_proportional, 20);
        // Large work: proportional above the floor → no top-up.
        let large = settle(&EpochInput::new(S, 0, (350_000, 0), Phase::Poa));
        assert_eq!(large.treasury_proportional, 100_000);
        assert_eq!(large.treasury_floor_topup, 0);
    }

    #[test]
    fn treasury_split() {
        assert_eq!(split_treasury(1_000, 4_000), (400, 600));
        // The rounding remainder goes to the holder treasury.
        assert_eq!(split_treasury(7, 4_000), (2, 5));
        assert_eq!(split_treasury(7, 20_000), (7, 0));
    }

    // Scenarios "两年后全部解锁" and "解锁进度线性".
    #[test]
    fn floor_vesting() {
        let amount = 1_000_001;
        assert_eq!(vested(amount, 100, 50), 0);
        assert_eq!(vested(amount, 100, 100), 0);
        assert_eq!(
            vested(amount, 100, 100 + FLOOR_VESTING_BLOCKS / 2),
            amount / 2
        );
        assert_eq!(vested(amount, 100, 100 + FLOOR_VESTING_BLOCKS), amount);
        assert_eq!(vested(amount, 100, u64::MAX), amount);
    }

    proptest! {
        #[test]
        fn cumulative_matches_sum(k in 0u64..200, pick in 0usize..4) {
            // Short epoch lengths make periods short enough to cross halvings.
            let l = [BLOCKS_PER_PERIOD / 4, BLOCKS_PER_PERIOD / 3, BLOCKS_PER_PERIOD / 50, BLOCKS_PER_PERIOD][pick];
            let s = EmissionSchedule::new(l).unwrap();
            let expected: u128 = (0..k).map(|e| s.scheduled(e)).sum();
            prop_assert_eq!(s.cumulative_scheduled(k), expected);
        }

        #[test]
        fn settlement_conserves(
            s in 0u128..10u128.pow(25),
            reserve in 0u128..10u128.pow(26),
            m in 0u128..10u128.pow(26),
            p in 0u128..10u128.pow(26),
            pos in any::<bool>(),
        ) {
            let phase = if pos { Phase::Pos } else { Phase::Poa };
            let out = settle(&EpochInput::new(s, reserve, (m, p), phase));
            let avail = s + reserve.min(s);
            prop_assert!(out.total <= avail);
            prop_assert!(out.drawn <= s);
            prop_assert_eq!(out.reserve + out.total, reserve + s);
            prop_assert_eq!(
                out.total,
                out.security + out.market + out.public + out.treasury()
            );
            prop_assert!(out.treasury() >= s * 5 / 100);
        }
    }
}
