//! Interfaces between the market pallets (m5-market-registry design D1).
//!
//! Each market pallet depends on the others only through these traits, wired together in the
//! runtime. `Credit` is the unified credit interface of D20: the transparent credits of α and the
//! shielded vouchers of β are two implementations of it.

use sp_runtime::{DispatchError, DispatchResult, Perbill};

use super::model::ModelId;
use super::usd::{AtcPerUsd, MicroUsd, PriceError, PricePerMTok, Rounding, to_atc};

/// Source of the ATC/USD rate (\[reserved\] a multi-source oracle in the full version).
pub trait PriceSource {
    /// Current rate, if set.
    fn atc_per_usd() -> Option<AtcPerUsd>;

    /// Converts `amount` at the current rate.
    ///
    /// # Errors
    ///
    /// [`PriceError::NotSet`] without a rate; [`PriceError::Overflow`].
    fn to_atc(amount: MicroUsd, rounding: Rounding) -> Result<u128, PriceError> {
        to_atc(
            amount,
            Self::atc_per_usd().ok_or(PriceError::NotSet)?,
            rounding,
        )
    }
}

/// Whether a model is registered.
pub trait ModelLookup {
    /// `true` if `id` is registered.
    fn exists(id: &ModelId) -> bool;
}

/// What other modules need to know about gateways.
pub trait GatewayLookup<AccountId> {
    /// `true` if `who` is a registered, active gateway.
    fn is_active(who: &AccountId) -> bool;
    /// Fee of gateway `who`, in basis points, if registered.
    fn fee_bps(who: &AccountId) -> Option<u16>;
    /// `true` if `who` is a registered gateway, active or exiting: an exiting gateway accepts no
    /// new escrow but still settles the requests it served.
    fn is_registered(who: &AccountId) -> bool;
}

/// What settlement needs to know about providers.
pub trait ProviderLookup<AccountId> {
    /// `true` if `who` is a registered provider, not jailed, that lists `model`: work it did
    /// for that model can be settled (an exiting provider still settles finished work).
    fn can_settle(who: &AccountId, model: &ModelId) -> bool;
}

/// What audits need to know about providers (m6-audit-chain).
pub trait ProviderAudit<AccountId> {
    /// `true` if `who` is a registered provider, in any state.
    fn is_registered(who: &AccountId) -> bool;
    /// `who`'s current price for `model`, if it lists it.
    fn price(who: &AccountId, model: &ModelId) -> Option<PricePerMTok>;
}

/// The key an account signs with now.
pub trait AccountKeys<AccountId> {
    /// Fingerprint of `who`'s registered public key (see `voucher::key_fingerprint`).
    fn key_fingerprint(who: &AccountId) -> Option<[u8; 32]>;
}

/// Outcome of redeeming one voucher.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Redemption<Balance> {
    /// Moved from the user's escrow to the payee.
    pub paid: Balance,
    /// Value of the increment the escrow could not cover; borne by the gateway.
    pub shortfall: Balance,
    /// The authorized increment redeemed: cumulative amount minus the amount redeemed before
    /// (zero for a replay). Settlement checks report totals against it.
    pub usd: MicroUsd,
}

/// The unified credit interface (D20). Market and settlement modules use only this.
pub trait Credit<AccountId, Balance> {
    /// A payment voucher.
    type Voucher;

    /// Redeems `voucher` for `gateway`, moving the payment to `payee` (an existing account).
    /// A voucher that authorizes nothing new redeems for zero.
    ///
    /// # Errors
    ///
    /// Any reason the voucher is invalid, or the transfer to `payee` failing; nothing changes
    /// then.
    fn redeem(
        gateway: &AccountId,
        voucher: &Self::Voucher,
        payee: &AccountId,
    ) -> Result<Redemption<Balance>, DispatchError>;
}

/// Penalties for providers, for the audit module (M6). No transaction or human origin may reach
/// it (D6).
pub trait ProviderPenalty<AccountId, Balance> {
    /// Slashes `ratio` of `who`'s stake, bonded first then unbonding by unlock order; the
    /// slashed amount is burned. Returns the amount slashed.
    fn slash(who: &AccountId, ratio: Perbill) -> Balance;

    /// Jails `who`: never serviceable again until an audit decision (M6) releases it.
    ///
    /// # Errors
    ///
    /// If `who` is not a provider.
    fn jail(who: &AccountId) -> DispatchResult;
}

/// Called when a provider is jailed, so settlement can void its unmatured earnings
/// (m5-work-settlement design D7). Reached only through [`ProviderPenalty::jail`], which no
/// transaction reaches (D6).
pub trait OnJail<AccountId> {
    /// `who` was just jailed.
    fn on_jail(who: &AccountId);
}

impl<AccountId> OnJail<AccountId> for () {
    fn on_jail(_who: &AccountId) {}
}

/// Whether a model is calibrated: it has an audit profile, its own or the chain's default
/// (m6-audit-hardening design D1; spec `market/audit` "按模型的审计档案"). Settlement pays for
/// an uncalibrated model's receipts but credits no market work for them.
pub trait ModelCalibration {
    /// `true` if `model` has an audit profile.
    fn is_calibrated(model: &ModelId) -> bool;
}

/// Every model counts as calibrated: for chains and tests without audits.
pub struct AllCalibrated;

impl ModelCalibration for AllCalibrated {
    fn is_calibrated(_model: &ModelId) -> bool {
        true
    }
}

/// A provider's recent revenue, which its required stake grows with (m6-audit-hardening design
/// D3, D4; spec `market/work-settlement` "近期结算额").
pub trait SettlementHistory<AccountId> {
    /// `who`'s settled dollars per epoch over the recent window, rounded up.
    fn avg_per_epoch(who: &AccountId) -> MicroUsd;
}

/// No settlement history: every provider's recent revenue is zero.
impl<AccountId> SettlementHistory<AccountId> for () {
    fn avg_per_epoch(_who: &AccountId) -> MicroUsd {
        MicroUsd::ZERO
    }
}

/// Whether an account is a party to an open audit dispute or spot check, so its unbonding stake
/// must stay (m6-audit-hardening design D8; spec `market/audit` "争议当事方的罚没锁").
pub trait DisputeLock<AccountId> {
    /// `true` while `who` may still be slashed by an open dispute or spot check.
    fn is_locked(who: &AccountId) -> bool;
}

/// Nothing is ever locked: for chains and tests without audits.
impl<AccountId> DisputeLock<AccountId> for () {
    fn is_locked(_who: &AccountId) -> bool {
        false
    }
}
