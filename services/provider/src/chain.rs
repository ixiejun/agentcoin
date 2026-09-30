//! What the agent needs to know about the chain on the request path, cached so no request waits
//! for an RPC round trip more than once per gateway and cache period.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ac_crypto::PqPublicKey;
use ac_primitives::market::records::GatewayStatus;
use ac_wallet::NodeClient;
use anyhow::Result;
use async_trait::async_trait;
use sp_runtime::AccountId32;

/// A gateway as the chain knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayView {
    /// The account's current key.
    pub key: PqPublicKey,
    /// Registered and active.
    pub active: bool,
}

/// Looks gateways up.
#[async_trait]
pub trait Directory: Send + Sync {
    /// The gateway `who`, or `None` if it has no key or no registration.
    async fn gateway(&self, who: &AccountId32) -> Result<Option<GatewayView>>;
}

#[async_trait]
impl Directory for NodeClient {
    async fn gateway(&self, who: &AccountId32) -> Result<Option<GatewayView>> {
        let Some((key, _)) = self.current_key(who).await? else {
            return Ok(None);
        };
        let active = self
            .market_gateway(who)
            .await?
            .is_some_and(|g| g.status == GatewayStatus::Active);
        Ok(Some(GatewayView { key, active }))
    }
}

#[async_trait]
impl Directory for Box<dyn Directory> {
    async fn gateway(&self, who: &AccountId32) -> Result<Option<GatewayView>> {
        (**self).gateway(who).await
    }
}

/// A fixed directory (tests and tools).
#[derive(Debug, Default)]
pub struct StaticDirectory(pub HashMap<AccountId32, GatewayView>);

#[async_trait]
impl Directory for StaticDirectory {
    async fn gateway(&self, who: &AccountId32) -> Result<Option<GatewayView>> {
        Ok(self.0.get(who).cloned())
    }
}

type CacheMap = HashMap<AccountId32, (Instant, Option<GatewayView>)>;

/// Caches lookups for `ttl`.
pub struct Cached<D> {
    inner: D,
    ttl: Duration,
    cache: Mutex<CacheMap>,
}

impl<D: Directory> Cached<D> {
    /// Wraps `inner`.
    pub fn new(inner: D, ttl: Duration) -> Self {
        Self {
            inner,
            ttl,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// A cached or fresh lookup.
    ///
    /// # Errors
    ///
    /// Lookup failures (not cached).
    pub async fn gateway(&self, who: &AccountId32) -> Result<Option<GatewayView>> {
        let hit = self.cache.lock().ok().and_then(|c| {
            c.get(who)
                .filter(|(at, _)| at.elapsed() < self.ttl)
                .map(|(_, v)| v.clone())
        });
        if let Some(v) = hit {
            return Ok(v);
        }
        let fresh = self.inner.gateway(who).await?;
        if let Ok(mut c) = self.cache.lock() {
            c.insert(who.clone(), (Instant::now(), fresh.clone()));
        }
        Ok(fresh)
    }
}
