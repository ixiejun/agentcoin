//! What the gateway reads from the chain, behind a trait so tests can run without a node.

use ac_crypto::PqPublicKey;
use ac_primitives::market::{AtcPerUsd, ModelId, SignedVoucher, VoucherCheck, VoucherError};
use ac_wallet::NodeClient;
use ac_wallet::market::{Channel, Provider};
use anyhow::Result;
use async_trait::async_trait;
use sp_runtime::AccountId32;

/// Chain queries of the gateway.
#[async_trait]
pub trait Chain: Send + Sync {
    /// Current key of an account.
    async fn current_key(&self, who: &AccountId32) -> Result<Option<PqPublicKey>>;
    /// `user`'s channel with this gateway.
    async fn channel(&self, user: &AccountId32, gateway: &AccountId32) -> Result<Option<Channel>>;
    /// The reference rate.
    async fn rate(&self) -> Result<Option<AtcPerUsd>>;
    /// A voucher checked as redemption would.
    async fn check_voucher(
        &self,
        voucher: &SignedVoucher,
    ) -> Result<Result<VoucherCheck, VoucherError>>;
    /// Every registered model ID.
    async fn model_ids(&self) -> Result<Vec<ModelId>>;
    /// A model's name.
    async fn model_name(&self, id: ModelId) -> Result<Option<String>>;
    /// Serviceable providers of a model.
    async fn serviceable(&self, model: ModelId) -> Result<Vec<(AccountId32, Provider)>>;
}

#[async_trait]
impl Chain for NodeClient {
    async fn current_key(&self, who: &AccountId32) -> Result<Option<PqPublicKey>> {
        Ok(NodeClient::current_key(self, who).await?.map(|(k, _)| k))
    }

    async fn channel(&self, user: &AccountId32, gateway: &AccountId32) -> Result<Option<Channel>> {
        self.market_channel(user, gateway).await
    }

    async fn rate(&self) -> Result<Option<AtcPerUsd>> {
        Ok(self.market_rate().await?.map(|(r, _)| r))
    }

    async fn check_voucher(
        &self,
        voucher: &SignedVoucher,
    ) -> Result<Result<VoucherCheck, VoucherError>> {
        self.market_check_voucher(voucher).await
    }

    async fn model_ids(&self) -> Result<Vec<ModelId>> {
        self.market_model_ids().await
    }

    async fn model_name(&self, id: ModelId) -> Result<Option<String>> {
        Ok(self
            .market_model(id)
            .await?
            .map(|m| String::from_utf8_lossy(&m.manifest.name).into_owned()))
    }

    async fn serviceable(&self, model: ModelId) -> Result<Vec<(AccountId32, Provider)>> {
        self.market_serviceable(model).await
    }
}
