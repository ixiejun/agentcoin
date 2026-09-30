//! Background work against the chain: heartbeats, model and price refresh, claims and receipt
//! pruning (spec "自动心跳", "费用计算与收据签名").

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ac_crypto::KemPublicKey;
use ac_primitives::market::ModelId;
use ac_runtime::RuntimeCall;
use ac_wallet::NodeClient;
use ac_wallet::market::Provider;
use ac_wallet::ops::Signer;
use anyhow::{Result, bail};

use crate::logging::TARGET;
use crate::service::{ModelEntry, Service};

/// Whether a heartbeat is due: three quarters of an interval since the last one (rounded up),
/// so heartbeats are at least half an interval apart (free) and at most one interval apart
/// once included.
#[must_use]
pub fn heartbeat_due(now: u64, last: u64, interval: u64) -> bool {
    now.saturating_sub(last) >= interval.saturating_mul(3).div_ceil(4)
}

/// Checks the on-chain registration against the loaded key and the model mapping, and returns
/// the served models with their prices (spec "加密密钥管理", "只为活跃网关服务").
///
/// # Errors
///
/// A different registered key, or a mapped model the provider has not registered.
pub fn check_registration(
    record: &Provider,
    kem: &KemPublicKey,
    mapping: &[(ModelId, String)],
) -> Result<BTreeMap<ModelId, ModelEntry>> {
    if &record.kem_pk != kem {
        bail!("the encryption key registered on chain is not the loaded key file's key");
    }
    let mut out = BTreeMap::new();
    for (id, name) in mapping {
        let Some(m) = record.models.iter().find(|m| m.model == *id) else {
            bail!(
                "model 0x{} is mapped but not registered for this provider",
                hex::encode(id.0)
            );
        };
        out.insert(
            *id,
            ModelEntry {
                engine_name: name.clone(),
                price: m.price,
            },
        );
    }
    if out.is_empty() {
        bail!("no model mapped (use --model <model id>=<engine model name>)");
    }
    Ok(out)
}

/// Runs forever: once a second reads the best block; heartbeats when due, refreshes models and
/// prices every heartbeat check, claims matured payments once per emission epoch and prunes
/// receipts whose challenge period is over.
pub async fn run(
    node: NodeClient,
    signer: Signer,
    service: Arc<Service>,
    mapping: Vec<(ModelId, String)>,
) {
    let mut last_claim_epoch = None;
    loop {
        if let Err(e) = tick(&node, &signer, &service, &mapping, &mut last_claim_epoch).await {
            log::warn!(target: TARGET, "chain task: {e:#}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn tick(
    node: &NodeClient,
    signer: &Signer,
    service: &Service,
    mapping: &[(ModelId, String)],
    last_claim_epoch: &mut Option<u64>,
) -> Result<()> {
    let best = node.best_block().await?;
    service.set_block(best);
    let Some(record) = node.market_provider(&signer.account).await? else {
        bail!("the provider is not registered");
    };
    let served: BTreeMap<ModelId, ModelEntry> = mapping
        .iter()
        .filter_map(|(id, name)| {
            record.models.iter().find(|m| m.model == *id).map(|m| {
                (
                    *id,
                    ModelEntry {
                        engine_name: name.clone(),
                        price: m.price,
                    },
                )
            })
        })
        .collect();
    service.set_models(served);

    let interval = node.provider_params().await?.heartbeat_interval;
    if heartbeat_due(best, u64::from(record.last_heartbeat), interval) {
        signer
            .submit(
                node,
                RuntimeCall::Providers(pallet_providers::Call::heartbeat {}),
            )
            .await?;
        log::info!(target: TARGET, "heartbeat at block {best}");
    }

    let epoch_len = node.epoch_length().await?.max(1);
    let epoch = best / epoch_len;
    if *last_claim_epoch != Some(epoch) {
        let items = node.work_claimable(&signer.account, &[]).await?;
        if !items.is_empty() {
            let n = items.len();
            let call = RuntimeCall::Work(pallet_work::Call::claim {
                who: signer.account.clone(),
                items: items
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("too many claim items"))?,
            });
            signer.submit(node, call).await?;
            log::info!(target: TARGET, "claimed {n} matured item(s)");
        }
        *last_claim_epoch = Some(epoch);
        // Receipts stay until their report can no longer be challenged: a report is submitted
        // within an epoch of the receipt and matures `challenge_epochs` later.
        let challenge = u64::from(node.work_params().await?.challenge_epochs);
        let keep = challenge.saturating_add(3).saturating_mul(epoch_len);
        let pruned = service.store().prune(best.saturating_sub(keep))?;
        if pruned > 0 {
            log::info!(target: TARGET, "deleted {pruned} receipt(s) past their challenge period");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_crypto::kem::KemSecretKey;
    use ac_crypto::sig::SecretSeed;
    use ac_primitives::market::records::{ModelPrice, ProviderStatus, SlaMetrics, Tier};
    use ac_primitives::market::{MicroUsd, PricePerMTok};
    use sp_runtime::BoundedVec;

    fn kem(seed: u8) -> KemPublicKey {
        KemSecretKey::from_seed(ac_crypto::KemAlg::XWing, &SecretSeed::new([seed; 32]))
            .unwrap()
            .public_key()
            .unwrap()
    }

    fn record(kem_pk: KemPublicKey) -> Provider {
        let price = PricePerMTok {
            input: MicroUsd(1),
            output: MicroUsd(2),
        };
        Provider {
            tier: Tier::T2,
            endpoint: BoundedVec::truncate_from(b"http://p".to_vec()),
            kem_pk,
            models: BoundedVec::truncate_from(vec![ModelPrice {
                model: ModelId([1; 32]),
                price,
            }]),
            stake: 0,
            unlocking: BoundedVec::default(),
            status: ProviderStatus::Active,
            last_heartbeat: 0,
            metrics: SlaMetrics::default(),
            attestation: None,
            registered_at: 0,
        }
    }

    // Scenario "与链上登记不符", and the model mapping check.
    #[test]
    fn registration_must_match_the_key_and_models() {
        let mine = kem(1);
        let map = vec![(ModelId([1; 32]), "engine-name".to_string())];
        let served = check_registration(&record(mine.clone()), &mine, &map).unwrap();
        assert_eq!(served[&ModelId([1; 32])].engine_name, "engine-name");
        assert_eq!(served[&ModelId([1; 32])].price.output, MicroUsd(2));
        let err = check_registration(&record(kem(2)), &mine, &map).unwrap_err();
        assert!(err.to_string().contains("not the loaded key"), "{err}");
        let unknown = vec![(ModelId([3; 32]), "x".to_string())];
        assert!(check_registration(&record(mine.clone()), &mine, &unknown).is_err());
        assert!(check_registration(&record(mine.clone()), &mine, &[]).is_err());
    }

    // Scenario "持续可服务": driven by a simulated block source, heartbeats are between half and
    // one interval apart.
    #[test]
    fn heartbeats_stay_between_half_and_one_interval() {
        for interval in [10u64, 11, 600] {
            let mut last = 0u64;
            let mut sent = Vec::new();
            for now in 1..=interval * 6 {
                if heartbeat_due(now, last, interval) {
                    sent.push(now);
                    last = now;
                }
            }
            let mut prev = 0;
            for s in sent {
                let gap = s - prev;
                assert!(
                    gap * 2 >= interval && gap <= interval,
                    "interval {interval}: gap {gap}"
                );
                prev = s;
            }
        }
    }
}
