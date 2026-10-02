//! Background work against the chain: provider refresh, work reports, fee claims and receipt
//! deletion (spec "提供者选择与故障切换", "自动提交工作报告与领取").

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ac_market_proto::toploc::ToplocProofs;
use ac_primitives::market::SignedReceipt;
use ac_runtime::{RuntimeCall, RuntimeEvent};
use ac_wallet::NodeClient;
use ac_wallet::ops::Signer;
use ac_wallet::work::{build_report, check_totals};
use anyhow::{Context, Result};
use parity_scale_codec::{Decode, Encode};
use serde_json::{Value, json};
use sp_runtime::{AccountId32, BoundedVec};

use crate::book::{Portion, check_portion, plan};
use crate::logging::TARGET;
use crate::routing::Router;
use crate::service::Gateway;

/// Schedule of the background work, in blocks.
#[derive(Clone, Copy, Debug)]
pub struct Schedule {
    /// Blocks between report rounds (at most an emission epoch).
    pub report_interval: u64,
}

/// Runs forever: once a second reads the best block; refreshes providers every heartbeat
/// interval, submits reports every `report_interval` blocks, and once per emission epoch claims
/// the gateway's matured fees and deletes the receipts of matured reports.
pub async fn run(
    (node, signer): (NodeClient, Signer),
    gateway: Arc<Gateway>,
    router: Arc<Router>,
    data_dir: PathBuf,
    schedule: Schedule,
) {
    let (mut last_refresh, mut last_report, mut last_epoch) =
        (None::<u64>, None::<u64>, None::<u64>);
    loop {
        let result: Result<()> = async {
            let best = node.best_block().await?;
            let interval = node.provider_params().await?.heartbeat_interval.max(1);
            if last_refresh.is_none_or(|b| best.saturating_sub(b) >= interval) {
                router.refresh(&node).await?;
                last_refresh = Some(best);
            }
            if last_report.is_none_or(|b| best.saturating_sub(b) >= schedule.report_interval) {
                last_report = Some(best);
                report_round(&node, &signer, &gateway, &data_dir).await?;
            }
            let epoch = best / node.epoch_length().await?.max(1);
            if last_epoch != Some(epoch) {
                last_epoch = Some(epoch);
                claim_and_clean(&node, &signer, &data_dir).await?;
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            log::warn!(target: TARGET, "chain task: {e:#}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Plans and submits reports for every paid, unreported receipt.
///
/// # Errors
///
/// Chain failures; a failed report keeps its receipts for the next round.
pub async fn report_round(
    node: &NodeClient,
    signer: &Signer,
    gateway: &Gateway,
    data_dir: &Path,
) -> Result<()> {
    let (reports, problems) = plan(&gateway.book().snapshot());
    for p in problems {
        log::warn!(target: TARGET, "report planning: {p}");
    }
    for portions in reports {
        let mut kept = Vec::new();
        for p in portions {
            let redeemed = node
                .market_channel(&p.user, gateway.account())
                .await?
                .map(|c| c.redeemed)
                .unwrap_or_default();
            match check_portion(&p, redeemed) {
                Ok(()) => kept.push(p),
                Err(e) => log::warn!(target: TARGET, "skipping a channel: {e:#}"),
            }
        }
        if kept.is_empty() {
            continue;
        }
        if let Err(e) = submit(node, signer, gateway, data_dir, &kept).await {
            log::warn!(target: TARGET, "report not accepted, retrying next round: {e:#}");
        }
    }
    Ok(())
}

/// The report of `portions`: their receipts' root and totals, with the fees of receipts without
/// proofs counted as unproven (spec `market/gateway-service` "自动提交工作报告与领取").
fn report_of(gateway: &AccountId32, portions: &[Portion]) -> Result<ac_wallet::work::Report> {
    let named: Vec<(String, SignedReceipt)> = portions
        .iter()
        .flat_map(|p| {
            p.receipts
                .iter()
                .map(|r| (hex::encode(r.body.request_id), r.clone()))
        })
        .collect();
    build_report(gateway, &named)
}

async fn submit(
    node: &NodeClient,
    signer: &Signer,
    gateway: &Gateway,
    data_dir: &Path,
    portions: &[Portion],
) -> Result<()> {
    let report = report_of(gateway.account(), portions)?;
    let mut increments = Vec::new();
    for p in portions {
        let check = node
            .market_check_voucher(&p.voucher)
            .await?
            .map_err(|e| anyhow::anyhow!("voucher: {e}"))?;
        increments.push((hex::encode(AsRef::<[u8]>::as_ref(&p.user)), check.increment));
    }
    check_totals(&report, &increments)?;
    let call = RuntimeCall::Work(pallet_work::Call::submit_report {
        root: report.root,
        receipt_count: report.receipt_count,
        entries: BoundedVec::try_from(report.entries)
            .map_err(|_| anyhow::anyhow!("too many entries"))?,
        vouchers: BoundedVec::try_from(
            portions
                .iter()
                .map(|p| p.voucher.clone())
                .collect::<Vec<_>>(),
        )
        .map_err(|_| anyhow::anyhow!("too many vouchers"))?,
    });
    let inclusion = signer.submit(node, call).await?;
    let (id, matures) = node
        .extrinsic_events(inclusion.block_hash, inclusion.index)
        .await?
        .into_iter()
        .find_map(|e| match e {
            RuntimeEvent::Work(pallet_work::Event::ReportAccepted { id, matures, .. }) => {
                Some((id, matures))
            }
            _ => None,
        })
        .context("no ReportAccepted event")?;
    save_report(data_dir, id, matures, portions)?;
    gateway.book().mark_reported(
        &portions
            .iter()
            .map(|p| {
                (
                    p.user.clone(),
                    p.receipts.iter().map(|r| r.body.request_id).collect(),
                )
            })
            .collect::<Vec<(AccountId32, Vec<[u8; 32]>)>>(),
    )?;
    log::info!(target: TARGET, "report {id} accepted: {} receipt(s), {} voucher(s), matures in epoch {matures}", report.receipt_count, portions.len());
    Ok(())
}

fn reports_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("reports")
}

/// Keeps a report's receipts and their TOPLOC proofs until the report matures.
fn save_report(data_dir: &Path, id: u64, matures: u64, portions: &[Portion]) -> Result<()> {
    let dir = reports_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let receipts: Vec<String> = portions
        .iter()
        .flat_map(|p| p.receipts.iter())
        .map(|r| hex::encode(r.encode()))
        .collect();
    let proofs: serde_json::Map<String, Value> = portions
        .iter()
        .flat_map(|p| p.proofs.iter())
        .map(|(rid, p)| (hex::encode(rid), Value::from(hex::encode(p.encode()))))
        .collect();
    let body = json!({
        "id": id,
        "matures": matures,
        "receipts": receipts,
        "toploc": proofs,
    });
    let tmp = dir.join(format!("{id}.tmp"));
    std::fs::write(&tmp, body.to_string())?;
    std::fs::rename(&tmp, dir.join(format!("{id}.json")))?;
    Ok(())
}

/// A report kept until it matures: its maturity epoch, receipts and their TOPLOC proofs (by
/// request ID), for audits and disputes.
pub type SavedReport = (u64, Vec<SignedReceipt>, Vec<([u8; 32], ToplocProofs)>);

/// Receipts and proofs of a saved report (for audits and disputes until it matures).
///
/// # Errors
///
/// Unreadable or malformed files.
pub fn load_report(data_dir: &Path, id: u64) -> Result<SavedReport> {
    let text = std::fs::read_to_string(reports_dir(data_dir).join(format!("{id}.json")))?;
    let v: Value = serde_json::from_str(&text)?;
    let matures = v
        .get("matures")
        .and_then(Value::as_u64)
        .context("matures")?;
    let mut out = Vec::new();
    for h in v
        .get("receipts")
        .and_then(Value::as_array)
        .context("receipts")?
    {
        out.push(SignedReceipt::decode(
            &mut &hex::decode(h.as_str().context("receipt")?)?[..],
        )?);
    }
    let mut proofs = Vec::new();
    if let Some(map) = v.get("toploc").and_then(Value::as_object) {
        for (rid, h) in map {
            let rid: [u8; 32] = hex::decode(rid)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("bad request ID"))?;
            let bytes = hex::decode(h.as_str().context("proofs")?)?;
            proofs.push((rid, ToplocProofs::decode(&mut &bytes[..])?));
        }
    }
    Ok((matures, out, proofs))
}

/// Claims the gateway's matured fees, then deletes the receipts of reports whose maturity epoch
/// is settled.
///
/// # Errors
///
/// Chain or storage failures.
pub async fn claim_and_clean(node: &NodeClient, signer: &Signer, data_dir: &Path) -> Result<()> {
    let items = node.work_claimable(&signer.account, &[]).await?;
    if !items.is_empty() {
        let n = items.len();
        let call = RuntimeCall::Work(pallet_work::Call::claim {
            who: signer.account.clone(),
            items: BoundedVec::try_from(items)
                .map_err(|_| anyhow::anyhow!("too many claim items"))?,
        });
        signer.submit(node, call).await?;
        log::info!(target: TARGET, "claimed the gateway fee of {n} matured item(s)");
    }
    let dir = reports_dir(data_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(id) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let (matures, _, _) = load_report(data_dir, id)?;
        if node.work_epoch(matures).await?.market.is_some() {
            std::fs::remove_file(&path)?;
            log::info!(target: TARGET, "deleted the receipts and proofs of report {id} (challenge period over)");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_crypto::sig::{SecretSeed, SigningKey};
    use ac_crypto::{OsRng, SigAlg};
    use ac_primitives::market::receipt::RECEIPT_CONTEXT;
    use ac_primitives::market::voucher::VOUCHER_CONTEXT;
    use ac_primitives::market::work::JobKind;
    use ac_primitives::market::{MicroUsd, ModelId, ReceiptBody, SignedVoucher, VoucherBody};
    use sp_core::H256;

    fn receipt(id: u8) -> SignedReceipt {
        receipt_with(id, [0; 32], 1)
    }

    fn receipt_with(id: u8, toploc_commit: [u8; 32], fee: u128) -> SignedReceipt {
        let key = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([1; 32])).unwrap();
        let body = ReceiptBody {
            genesis: H256([0; 32]),
            gateway: AccountId32::new([2; 32]),
            provider: AccountId32::new([3; 32]),
            kind: JobKind::Inference,
            model: ModelId([4; 32]),
            request_id: [id; 32],
            in_tokens: 1,
            out_tokens: 1,
            fee: MicroUsd(fee),
            toploc_commit,
            ttft_ms: 1,
            total_ms: 2,
        };
        let sig = key
            .sign(
                &body.payload().unwrap(),
                RECEIPT_CONTEXT,
                &mut OsRng::new().unwrap(),
            )
            .unwrap();
        SignedReceipt {
            body,
            provider_key: key.public_key().unwrap(),
            provider_sig: sig.clone(),
            gateway_key: key.public_key().unwrap(),
            gateway_sig: sig,
        }
    }

    fn voucher() -> SignedVoucher {
        let key = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([5; 32])).unwrap();
        let body = VoucherBody {
            genesis: H256([0; 32]),
            user: AccountId32::new([6; 32]),
            gateway: AccountId32::new([2; 32]),
            channel: 0,
            cumulative: MicroUsd(2),
        };
        SignedVoucher {
            signature: key
                .sign(
                    &body.payload().unwrap(),
                    VOUCHER_CONTEXT,
                    &mut OsRng::new().unwrap(),
                )
                .unwrap(),
            public_key: key.public_key().unwrap(),
            body,
        }
    }

    // m5-engine-toploc 5.1: a report keeps its receipts' proofs until it is deleted at maturity.
    #[test]
    fn reports_keep_their_proofs() {
        let dir = std::env::temp_dir().join(format!("ac-gateway-reports-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let proofs = ToplocProofs {
            decode_batching_size: 32,
            topk: 128,
            skip_prefill: false,
            proofs: vec![vec![0xff, 0xd9, 0, 7]],
        };
        // Signatures are randomized: compare with the very receipts saved.
        let saved = vec![receipt(1), receipt(2)];
        let portion = Portion {
            user: AccountId32::new([6; 32]),
            voucher: voucher(),
            receipts: saved.clone(),
            proofs: vec![([2; 32], proofs.clone())],
        };
        save_report(&dir, 9, 40, &[portion]).unwrap();
        let (matures, receipts, kept) = load_report(&dir, 9).unwrap();
        assert_eq!(matures, 40);
        assert_eq!(receipts, saved);
        assert_eq!(kept, [([2; 32], proofs)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Spec market/gateway-service "自动提交工作报告与领取" / "无证明的收据单独累计".
    #[test]
    fn unproven_receipts_are_totalled_apart() {
        let portion = Portion {
            user: AccountId32::new([6; 32]),
            voucher: voucher(),
            receipts: vec![
                receipt_with(1, [9; 32], 300_000),
                receipt_with(2, [0; 32], 100_000),
                receipt_with(3, [9; 32], 200_000),
            ],
            proofs: Vec::new(),
        };
        let report = report_of(&AccountId32::new([2; 32]), &[portion]).unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].usd, MicroUsd(600_000));
        assert_eq!(report.entries[0].unproven, MicroUsd(100_000));
    }
}
