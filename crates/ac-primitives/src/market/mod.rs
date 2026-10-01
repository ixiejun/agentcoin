//! Inference-market types shared by the runtime, wallets and gateways (M5, m5-market-registry).
//!
//! - [`audit`]: on-chain audit types and the assignment draw (m6-audit-chain).
//! - [`usd`]: dollar amounts, the reference rate and conversions with explicit rounding (D23).
//! - [`model`]: weight manifests and model IDs.
//! - [`voucher`]: cumulative transparent vouchers and [`voucher::check_voucher`], the single
//!   implementation of the redemption rules used on and off chain.
//! - [`receipt`]: inference receipts signed by provider and gateway (m5-work-settlement).
//! - [`receipt_tree`]: the Merkle tree committing a report's receipts.
//! - [`work`]: work settlement types: job kinds, report entries and the fee allocation.
//! - [`records`]: provider, gateway and channel records as stored on chain.
//! - [`traits`]: the interfaces between the market pallets, including the `Credit` interface
//!   (D20).
//! - [`MarketApi`]: the runtime API clients query the market through.

use alloc::vec::Vec;

use crate::emission::EpochIndex;

pub mod audit;
pub mod model;
pub mod receipt;
pub mod receipt_tree;
pub mod records;
pub mod traits;
pub mod usd;
pub mod voucher;
pub mod work;

pub use model::{ModelId, ModelManifest, ModelRecord};
pub use receipt::{ReceiptBody, ReceiptError, SignedReceipt};
pub use records::{ChannelRecord, GatewayRecord, ProviderRecord, Tier};
pub use usd::{AtcPerUsd, MicroUsd, PriceError, PricePerMTok};
pub use voucher::{SignedVoucher, VoucherBody, VoucherCheck, VoucherError};
pub use work::{
    EpochWork, Held, JobKind, LifetimeWork, ProviderWork, ReportEntry, ReportLine, ReportRecord,
    WorkParams,
};

sp_api::decl_runtime_apis! {
    /// Read access to the inference market.
    pub trait MarketApi<AccountId, Balance, BlockNumber>
    where
        AccountId: parity_scale_codec::Codec,
        Balance: parity_scale_codec::Codec,
        BlockNumber: parity_scale_codec::Codec,
    {
        /// The reference rate and the block it was set at.
        fn atc_per_usd() -> Option<(AtcPerUsd, BlockNumber)>;
        /// A registered model.
        fn model(id: ModelId) -> Option<ModelRecord<AccountId, Balance, BlockNumber>>;
        /// A registered provider.
        fn provider(who: AccountId) -> Option<ProviderRecord<Balance, BlockNumber>>;
        /// Whether `who` is serviceable now: active, staked above the threshold, heartbeating.
        fn is_serviceable(who: AccountId) -> bool;
        /// Serviceable providers of `model`, ordered by account, starting after `start_after`,
        /// at most `limit` (capped at 256).
        fn serviceable_providers(
            model: ModelId,
            start_after: Option<AccountId>,
            limit: u32,
        ) -> Vec<(AccountId, ProviderRecord<Balance, BlockNumber>)>;
        /// Stake a provider of `tier` needs now, in smallest ATC units.
        fn provider_threshold(tier: Tier) -> Result<Balance, PriceError>;
        /// A registered gateway.
        fn gateway(who: AccountId) -> Option<GatewayRecord<Balance, BlockNumber>>;
        /// Stake a gateway needs now, in smallest ATC units.
        fn gateway_threshold() -> Result<Balance, PriceError>;
        /// `user`'s channel with `gateway`.
        fn channel(user: AccountId, gateway: AccountId) -> Option<ChannelRecord<Balance, BlockNumber>>;
        /// Checks a voucher exactly as redemption would, against the current state.
        fn check_voucher(voucher: SignedVoucher) -> Result<VoucherCheck, VoucherError>;
    }

    /// Read access to work settlement (m5-work-settlement).
    pub trait WorkApi<AccountId, Balance>
    where
        AccountId: parity_scale_codec::Codec,
        Balance: parity_scale_codec::Codec,
    {
        /// Settlement parameters.
        fn params() -> WorkParams;
        /// An accepted report still within its retention period.
        fn report(id: u64) -> Option<ReportRecord<AccountId, Balance>>;
        /// What gateways hold for `who`, by maturity epoch and gateway.
        fn held(who: AccountId) -> Vec<(EpochIndex, AccountId, Held<Balance>)>;
        /// `who`'s market work not yet claimed, by maturity epoch.
        fn work(who: AccountId) -> Vec<(EpochIndex, ProviderWork)>;
        /// Verified market work of `epoch` and its emission once settled.
        fn epoch_work(epoch: EpochIndex) -> EpochWork<Balance>;
        /// `who`'s lifetime work.
        fn lifetime(who: AccountId) -> LifetimeWork<Balance>;
    }
}
