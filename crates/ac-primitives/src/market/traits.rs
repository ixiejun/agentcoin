//! Interfaces between the market pallets (m5-market-registry design D1).
//!
//! Each market pallet depends on the others only through these traits, wired together in the
//! runtime. `Credit` is the unified credit interface of D20: the transparent credits of α and the
//! shielded vouchers of β are two implementations of it.

use sp_runtime::{DispatchError, DispatchResult, Perbill};

use super::model::ModelId;
use super::usd::{AtcPerUsd, MicroUsd, PriceError, Rounding, to_atc};

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
