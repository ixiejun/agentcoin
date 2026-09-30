//! Serviceable providers per model, refreshed from the chain, and the routing order (spec
//! "提供者选择与故障切换", design D5).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use ac_market_proto::ErrorCode;
use ac_market_proto::route::{Candidate, PriceThenLatency, RouteScore, ewma, max_fee};
use ac_primitives::market::records::Tier;
use ac_primitives::market::{MicroUsd, ModelId, PricePerMTok};
use ac_wallet::market::Provider;
use anyhow::Result;
use sp_runtime::AccountId32;

use crate::chain::Chain;

/// A model that has serviceable providers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    /// On-chain ID.
    pub id: ModelId,
    /// Manifest name.
    pub name: String,
    /// Serviceable providers with their records.
    pub providers: Vec<(AccountId32, Provider)>,
}

/// Routing state.
pub struct Router {
    models: RwLock<BTreeMap<ModelId, ModelInfo>>,
    ttft: Mutex<HashMap<AccountId32, u32>>,
    suspended: Mutex<HashMap<AccountId32, Instant>>,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

/// A provider chosen for a request.
#[derive(Clone, Debug)]
pub struct Choice {
    /// The provider.
    pub provider: AccountId32,
    /// Its record (endpoint, KEM key, price).
    pub record: Provider,
    /// Its price for the model.
    pub price: PricePerMTok,
}

impl Router {
    /// Empty state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            models: RwLock::new(BTreeMap::new()),
            ttft: Mutex::new(HashMap::new()),
            suspended: Mutex::new(HashMap::new()),
        }
    }

    /// Re-reads every model and its serviceable providers. T0 providers and providers with an
    /// attestation are left out in this phase (confidential inference is the full version's).
    ///
    /// # Errors
    ///
    /// Chain failures (the previous state is kept).
    pub async fn refresh(&self, chain: &dyn Chain) -> Result<()> {
        let mut next = BTreeMap::new();
        for id in chain.model_ids().await? {
            let providers: Vec<(AccountId32, Provider)> = chain
                .serviceable(id)
                .await?
                .into_iter()
                .filter(|(_, p)| p.tier != Tier::T0 && p.attestation.is_none())
                .collect();
            if providers.is_empty() {
                continue;
            }
            let name = chain.model_name(id).await?.unwrap_or_default();
            next.insert(
                id,
                ModelInfo {
                    id,
                    name,
                    providers,
                },
            );
        }
        if let Ok(mut m) = self.models.write() {
            *m = next;
        }
        Ok(())
    }

    /// Models with serviceable providers.
    #[must_use]
    pub fn models(&self) -> Vec<ModelInfo> {
        self.models
            .read()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Resolves a requested model: a `0x`-prefixed ID, or a name unique among serviceable
    /// models.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::ModelNotFound`] or [`ErrorCode::AmbiguousModel`] with a message listing
    /// the candidate IDs.
    pub fn resolve(&self, requested: &str) -> Result<ModelId, (ErrorCode, String)> {
        let models = self.models();
        if let Some(hex) = requested.strip_prefix("0x") {
            let id = hex::decode(hex)
                .ok()
                .and_then(|b| <[u8; 32]>::try_from(b).ok())
                .map(ModelId);
            return match id {
                Some(id) if models.iter().any(|m| m.id == id) => Ok(id),
                _ => Err((
                    ErrorCode::ModelNotFound,
                    "no serviceable model with this ID".into(),
                )),
            };
        }
        let matches: Vec<&ModelInfo> = models.iter().filter(|m| m.name == requested).collect();
        match matches.as_slice() {
            [one] => Ok(one.id),
            [] => Err((
                ErrorCode::ModelNotFound,
                "no serviceable model with this name".into(),
            )),
            many => Err((
                ErrorCode::AmbiguousModel,
                format!(
                    "several models share this name; use one of the IDs: {}",
                    many.iter()
                        .map(|m| format!("0x{}", hex::encode(m.id.0)))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
        }
    }

    /// Serviceable, not suspended providers of `model`, best first, and the largest maximum fee
    /// among them (what the escrow must cover).
    #[must_use]
    pub fn candidates(
        &self,
        model: ModelId,
        input_bound: u64,
        max_output: u32,
    ) -> (Vec<Choice>, Option<MicroUsd>) {
        let info = self.models.read().ok().and_then(|m| m.get(&model).cloned());
        let Some(info) = info else {
            return (Vec::new(), None);
        };
        let now = Instant::now();
        let suspended = self.suspended.lock().map(|s| s.clone()).unwrap_or_default();
        let ttft = self.ttft.lock().map(|t| t.clone()).unwrap_or_default();
        let mut cands = Vec::new();
        let mut choices = HashMap::new();
        let mut worst: Option<MicroUsd> = None;
        for (who, record) in info.providers {
            if suspended.get(&who).is_some_and(|until| *until > now) {
                continue;
            }
            let Some(price) = record
                .models
                .iter()
                .find(|m| m.model == model)
                .map(|m| m.price)
            else {
                continue;
            };
            let Some(fee) = max_fee(&price, input_bound, max_output) else {
                continue;
            };
            worst = Some(worst.map_or(fee, |w| w.max(fee)));
            cands.push(Candidate {
                provider: who.clone(),
                max_fee: fee,
                ttft_ms: ttft.get(&who).copied(),
            });
            choices.insert(
                who.clone(),
                Choice {
                    provider: who,
                    record,
                    price,
                },
            );
        }
        PriceThenLatency.rank(&mut cands);
        let ordered = cands
            .into_iter()
            .filter_map(|c| choices.remove(&c.provider))
            .collect();
        (ordered, worst)
    }

    /// Records an observed time to first token.
    pub fn observe(&self, provider: &AccountId32, ttft_ms: u32) {
        if let Ok(mut t) = self.ttft.lock() {
            let prev = t.get(provider).copied();
            t.insert(provider.clone(), ewma(prev, ttft_ms));
        }
    }

    /// Stops choosing `provider` for `period`.
    pub fn suspend(&self, provider: &AccountId32, period: Duration) {
        if let Ok(mut s) = self.suspended.lock()
            && let Some(until) = Instant::now().checked_add(period)
        {
            s.insert(provider.clone(), until);
        }
    }
}
