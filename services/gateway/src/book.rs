//! The gateway's ledger of credit channels (spec "后付费额度核对", "自动提交工作报告与领取").
//!
//! Per user it keeps the channel number, the billed total, the latest accepted voucher (always
//! exactly the billed total at some point), the maximum fees of requests in flight (memory
//! only) and the unreported receipts in billing order, each with the cumulative total after it.
//! Every change is written to disk (write, sync, rename) before it is acknowledged. Nothing of
//! a request's content is ever stored.

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ac_primitives::market::usd::to_atc_threshold;
use ac_primitives::market::work::{MAX_REPORT_ENTRIES, MAX_REPORT_VOUCHERS};
use ac_primitives::market::{AtcPerUsd, MicroUsd, SignedReceipt, SignedVoucher};
use anyhow::{Context, Result, bail};
use parity_scale_codec::{Decode, Encode};
use serde_json::{Value, json};
use sp_runtime::AccountId32;

/// A billed receipt and the channel's billed total after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Billed {
    /// The double-signed receipt.
    pub receipt: SignedReceipt,
    /// Billed total including this receipt.
    pub cumulative: MicroUsd,
}

/// One channel's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelState {
    /// On-chain channel number the state belongs to.
    pub number: u32,
    /// Sum of all billed fees on this channel number.
    pub billed: MicroUsd,
    /// Latest accepted voucher.
    pub voucher: Option<SignedVoucher>,
    /// Maximum fees of requests in flight (not persisted).
    pub inflight: BTreeMap<[u8; 32], MicroUsd>,
    /// Billed receipts not yet reported, in billing order.
    pub unreported: Vec<Billed>,
}

/// Why a request or payment is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PayError {
    /// The voucher is not exactly the billed total.
    WrongAmount {
        /// The billed total it must cover.
        billed: MicroUsd,
        /// What it covers.
        offered: MicroUsd,
    },
    /// The escrow cannot cover the billed but unredeemed total plus requests in flight.
    Escrow {
        /// ATC needed.
        needed: u128,
        /// ATC in escrow.
        escrow: u128,
    },
    /// No reference rate: nothing can be priced.
    NoRate,
}

impl core::fmt::Display for PayError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WrongAmount { billed, offered } => write!(
                f,
                "the voucher must cover exactly the billed total of {} micro-USD (it covers {})",
                billed.0, offered.0
            ),
            Self::Escrow { needed, escrow } => {
                write!(f, "escrow too low: {needed} units needed, {escrow} held")
            }
            Self::NoRate => f.write_str("the reference rate is not set"),
        }
    }
}

/// What the chain says about a channel when a request is admitted.
#[derive(Clone, Copy, Debug)]
pub struct ChannelFacts {
    /// Current channel number.
    pub number: u32,
    /// Escrow in smallest ATC units.
    pub escrow: u128,
    /// Amount already redeemed on chain.
    pub redeemed: MicroUsd,
    /// The reference rate.
    pub rate: Option<AtcPerUsd>,
}

/// The ledger.
pub struct Book {
    dir: PathBuf,
    channels: Mutex<HashMap<AccountId32, ChannelState>>,
}

impl core::fmt::Debug for Book {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Book")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl Book {
    /// Opens the ledger in `dir`, loading every saved channel.
    ///
    /// # Errors
    ///
    /// Unreadable directories or malformed files.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let mut channels = HashMap::new();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path)?;
            let (user, state) =
                decode_state(&text).with_context(|| format!("reading {}", path.display()))?;
            channels.insert(user, state);
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            channels: Mutex::new(channels),
        })
    }

    fn with<T>(
        &self,
        f: impl FnOnce(&mut HashMap<AccountId32, ChannelState>) -> Result<T>,
    ) -> Result<T> {
        let mut guard = self
            .channels
            .lock()
            .map_err(|_| anyhow::anyhow!("ledger lock poisoned"))?;
        f(&mut guard)
    }

    /// The state of `user`'s channel (a copy).
    #[must_use]
    pub fn get(&self, user: &AccountId32) -> Option<ChannelState> {
        self.channels.lock().ok()?.get(user).cloned()
    }

    /// The latest accepted voucher of `user`, if any.
    #[must_use]
    pub fn voucher(&self, user: &AccountId32) -> Option<SignedVoucher> {
        self.get(user).and_then(|s| s.voucher)
    }

    /// Admits a request: the voucher (already checked on chain) must be exactly the billed
    /// total, and the escrow must cover the billed but unredeemed total plus every request in
    /// flight including this one. On success the request is in flight.
    ///
    /// # Errors
    ///
    /// A [`PayError`] (outer `Ok`), or storage failures.
    pub fn admit(
        &self,
        user: &AccountId32,
        facts: ChannelFacts,
        voucher: &SignedVoucher,
        (request_id, max_fee): ([u8; 32], MicroUsd),
    ) -> Result<Result<(), PayError>> {
        self.with(|channels| {
            let state = channels.entry(user.clone()).or_default();
            if state.number != facts.number {
                // A reset channel voids earlier vouchers (D54); unreported receipts of the old
                // number can no longer be paid and are dropped.
                *state = ChannelState {
                    number: facts.number,
                    ..ChannelState::default()
                };
            }
            if voucher.body.cumulative != state.billed {
                return Ok(Err(PayError::WrongAmount {
                    billed: state.billed,
                    offered: voucher.body.cumulative,
                }));
            }
            let Some(rate) = facts.rate else {
                return Ok(Err(PayError::NoRate));
            };
            let inflight = state
                .inflight
                .values()
                .fold(MicroUsd::ZERO, |a, f| MicroUsd(a.0.saturating_add(f.0)));
            let owed = MicroUsd(
                state
                    .billed
                    .0
                    .saturating_sub(facts.redeemed.0)
                    .saturating_add(inflight.0)
                    .saturating_add(max_fee.0),
            );
            let needed = to_atc_threshold(owed, rate).unwrap_or(u128::MAX);
            if needed > facts.escrow {
                return Ok(Err(PayError::Escrow {
                    needed,
                    escrow: facts.escrow,
                }));
            }
            if state.voucher.as_ref() != Some(voucher) {
                state.voucher = Some(voucher.clone());
                self.persist(user, state)?;
            }
            state.inflight.insert(request_id, max_fee);
            Ok(Ok(()))
        })
    }

    /// Drops a request from flight without billing it.
    pub fn release(&self, user: &AccountId32, request_id: &[u8; 32]) {
        if let Ok(mut c) = self.channels.lock()
            && let Some(s) = c.get_mut(user)
        {
            s.inflight.remove(request_id);
        }
    }

    /// Bills a completed request: persists the receipt and returns the new billed total.
    ///
    /// # Errors
    ///
    /// Storage failures (the request is then not billed).
    pub fn bill(&self, user: &AccountId32, receipt: SignedReceipt) -> Result<MicroUsd> {
        self.with(|channels| {
            let state = channels.get_mut(user).context("no channel state")?;
            state.inflight.remove(&receipt.body.request_id);
            let cumulative = MicroUsd(
                state
                    .billed
                    .0
                    .checked_add(receipt.body.fee.0)
                    .context("billed total overflow")?,
            );
            let mut next = state.clone();
            next.billed = cumulative;
            next.unreported.push(Billed {
                receipt,
                cumulative,
            });
            self.persist(user, &next)?;
            *state = next;
            Ok(cumulative)
        })
    }

    /// Records a payment voucher (already checked on chain) if it is exactly the billed total.
    ///
    /// # Errors
    ///
    /// A [`PayError`] (outer `Ok`), or storage failures.
    pub fn pay(
        &self,
        user: &AccountId32,
        number: u32,
        voucher: &SignedVoucher,
    ) -> Result<Result<MicroUsd, PayError>> {
        self.with(|channels| {
            let state = channels.entry(user.clone()).or_default();
            if state.number != number || voucher.body.cumulative != state.billed {
                return Ok(Err(PayError::WrongAmount {
                    billed: state.billed,
                    offered: voucher.body.cumulative,
                }));
            }
            if state.voucher.as_ref() != Some(voucher) {
                let mut next = state.clone();
                next.voucher = Some(voucher.clone());
                self.persist(user, &next)?;
                *state = next;
            }
            Ok(Ok(state.billed))
        })
    }

    /// Every channel with its latest voucher and unreported receipts (for report planning).
    #[must_use]
    pub fn snapshot(&self) -> Vec<(AccountId32, ChannelState)> {
        self.channels
            .lock()
            .map(|c| c.iter().map(|(u, s)| (u.clone(), s.clone())).collect())
            .unwrap_or_default()
    }

    /// Removes reported receipts (by request ID) from their channels.
    ///
    /// # Errors
    ///
    /// Storage failures.
    pub fn mark_reported(&self, reported: &[(AccountId32, Vec<[u8; 32]>)]) -> Result<()> {
        self.with(|channels| {
            for (user, ids) in reported {
                if let Some(state) = channels.get_mut(user) {
                    let mut next = state.clone();
                    next.unreported
                        .retain(|b| !ids.contains(&b.receipt.body.request_id));
                    self.persist(user, &next)?;
                    *state = next;
                }
            }
            Ok(())
        })
    }

    fn persist(&self, user: &AccountId32, state: &ChannelState) -> Result<()> {
        let name = hex::encode(AsRef::<[u8]>::as_ref(user));
        let tmp = self.dir.join(format!("{name}.tmp"));
        let mut f =
            std::fs::File::create(&tmp).with_context(|| format!("writing {}", tmp.display()))?;
        f.write_all(encode_state(user, state).as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, self.dir.join(format!("{name}.json")))?;
        Ok(())
    }
}

fn encode_state(user: &AccountId32, s: &ChannelState) -> String {
    json!({
        "user": hex::encode(AsRef::<[u8]>::as_ref(user)),
        "number": s.number,
        "billed": s.billed.0.to_string(),
        "voucher": s.voucher.as_ref().map(|v| hex::encode(v.encode())),
        "unreported": s.unreported.iter().map(|b| json!({
            "receipt": hex::encode(b.receipt.encode()),
            "cumulative": b.cumulative.0.to_string(),
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

fn decode_state(text: &str) -> Result<(AccountId32, ChannelState)> {
    let v: Value = serde_json::from_str(text)?;
    let field = |k: &str| v.get(k).context("missing field");
    let user: [u8; 32] = hex::decode(field("user")?.as_str().context("user")?)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("bad user"))?;
    let number = u32::try_from(field("number")?.as_u64().context("number")?)?;
    let billed = MicroUsd(field("billed")?.as_str().context("billed")?.parse()?);
    let voucher = match v.get("voucher").and_then(Value::as_str) {
        Some(h) => Some(SignedVoucher::decode(&mut &hex::decode(h)?[..])?),
        None => None,
    };
    let mut unreported = Vec::new();
    for b in field("unreported")?.as_array().context("unreported")? {
        let receipt = SignedReceipt::decode(
            &mut &hex::decode(
                b.get("receipt")
                    .and_then(Value::as_str)
                    .context("receipt")?,
            )?[..],
        )?;
        let cumulative = MicroUsd(
            b.get("cumulative")
                .and_then(Value::as_str)
                .context("cumulative")?
                .parse()?,
        );
        unreported.push(Billed {
            receipt,
            cumulative,
        });
    }
    Ok((
        AccountId32::new(user),
        ChannelState {
            number,
            billed,
            voucher,
            inflight: BTreeMap::new(),
            unreported,
        },
    ))
}

/// One channel's part of a report: its latest voucher and the receipts it pays for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Portion {
    /// The user.
    pub user: AccountId32,
    /// The voucher to redeem.
    pub voucher: SignedVoucher,
    /// Receipts whose fees sum to the voucher's increment.
    pub receipts: Vec<SignedReceipt>,
}

/// Plans reports: per channel, the receipts covered by the latest voucher (cumulative at most
/// the voucher's) — receipts after it wait for the next voucher. Portions are packed greedily
/// into reports of at most [`MAX_REPORT_VOUCHERS`] vouchers and [`MAX_REPORT_ENTRIES`]
/// (provider, model) totals; a portion that alone exceeds the entry limit is skipped with an
/// error message rather than blocking the others.
#[must_use]
pub fn plan(channels: &[(AccountId32, ChannelState)]) -> (Vec<Vec<Portion>>, Vec<String>) {
    let mut reports: Vec<Vec<Portion>> = Vec::new();
    let mut current: Vec<Portion> = Vec::new();
    let mut entries: std::collections::BTreeSet<(AccountId32, [u8; 32])> =
        std::collections::BTreeSet::new();
    let mut problems = Vec::new();
    for (user, state) in channels {
        let Some(voucher) = &state.voucher else {
            continue;
        };
        let covered: Vec<SignedReceipt> = state
            .unreported
            .iter()
            .filter(|b| b.cumulative <= voucher.body.cumulative)
            .map(|b| b.receipt.clone())
            .collect();
        if covered.is_empty() {
            continue;
        }
        let keys: std::collections::BTreeSet<(AccountId32, [u8; 32])> = covered
            .iter()
            .map(|r| (r.body.provider.clone(), r.body.model.0))
            .collect();
        if keys.len() > usize::try_from(MAX_REPORT_ENTRIES).unwrap_or(usize::MAX) {
            problems.push(format!(
                "channel {} needs more than {MAX_REPORT_ENTRIES} report entries",
                hex::encode(AsRef::<[u8]>::as_ref(user))
            ));
            continue;
        }
        let merged = entries.union(&keys).count();
        let full = current.len() >= usize::try_from(MAX_REPORT_VOUCHERS).unwrap_or(usize::MAX)
            || merged > usize::try_from(MAX_REPORT_ENTRIES).unwrap_or(usize::MAX);
        if full && !current.is_empty() {
            reports.push(core::mem::take(&mut current));
            entries.clear();
        }
        entries.extend(keys);
        current.push(Portion {
            user: user.clone(),
            voucher: voucher.clone(),
            receipts: covered,
        });
    }
    if !current.is_empty() {
        reports.push(current);
    }
    (reports, problems)
}

/// Fails if a report's receipts do not sum to its vouchers' increments over `redeemed`.
///
/// # Errors
///
/// A mismatch (a bug: the ledger only accepts exact vouchers).
pub fn check_portion(p: &Portion, redeemed: MicroUsd) -> Result<()> {
    let sum = p
        .receipts
        .iter()
        .fold(0u128, |a, r| a.saturating_add(r.body.fee.0));
    let increment = p.voucher.body.cumulative.0.saturating_sub(redeemed.0);
    if sum != increment {
        bail!("receipts sum to {sum} but the voucher adds {increment} micro-USD");
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
    use ac_primitives::market::{ModelId, ReceiptBody, VoucherBody};
    use sp_core::H256;

    const RATE: AtcPerUsd = AtcPerUsd(1_000_000_000_000_000_000); // 1 ATC per USD
    const ATC: u128 = 1_000_000_000_000_000_000;

    fn key() -> SigningKey {
        SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([1; 32])).unwrap()
    }

    fn user(i: u8) -> AccountId32 {
        AccountId32::new([i; 32])
    }

    fn voucher(u: &AccountId32, cumulative: u128) -> SignedVoucher {
        let k = key();
        let body = VoucherBody {
            genesis: H256([0; 32]),
            user: u.clone(),
            gateway: AccountId32::new([99; 32]),
            channel: 0,
            cumulative: MicroUsd(cumulative),
        };
        SignedVoucher {
            signature: k
                .sign(
                    &body.payload().unwrap(),
                    VOUCHER_CONTEXT,
                    &mut OsRng::new().unwrap(),
                )
                .unwrap(),
            public_key: k.public_key().unwrap(),
            body,
        }
    }

    fn receipt(id: u8, provider: u8, fee: u128) -> SignedReceipt {
        let k = key();
        let body = ReceiptBody {
            genesis: H256([0; 32]),
            gateway: AccountId32::new([99; 32]),
            provider: AccountId32::new([provider; 32]),
            kind: JobKind::Inference,
            model: ModelId([4; 32]),
            request_id: [id; 32],
            in_tokens: 1,
            out_tokens: 1,
            fee: MicroUsd(fee),
            toploc_commit: [0; 32],
            ttft_ms: 1,
            total_ms: 1,
        };
        let sig = k
            .sign(
                &body.payload().unwrap(),
                RECEIPT_CONTEXT,
                &mut OsRng::new().unwrap(),
            )
            .unwrap();
        SignedReceipt {
            body,
            provider_key: k.public_key().unwrap(),
            provider_sig: sig.clone(),
            gateway_key: k.public_key().unwrap(),
            gateway_sig: sig,
        }
    }

    fn facts(escrow: u128) -> ChannelFacts {
        ChannelFacts {
            number: 0,
            escrow,
            redeemed: MicroUsd::ZERO,
            rate: Some(RATE),
        }
    }

    fn dir(label: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("ac-gateway-book-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    // Scenario "未付清上一请求", then paying exactly; scenario "多付的凭证被拒绝".
    #[test]
    fn vouchers_must_equal_the_billed_total() {
        let book = Book::open(&dir("exact")).unwrap();
        let u = user(1);
        book.admit(&u, facts(ATC), &voucher(&u, 0), ([1; 32], MicroUsd(50)))
            .unwrap()
            .unwrap();
        assert_eq!(
            book.bill(&u, receipt(1, 7, 10_000)).unwrap(),
            MicroUsd(10_000)
        );
        // The next request still carries the old voucher: refused.
        let err = book
            .admit(&u, facts(ATC), &voucher(&u, 0), ([2; 32], MicroUsd(50)))
            .unwrap()
            .unwrap_err();
        assert_eq!(
            err,
            PayError::WrongAmount {
                billed: MicroUsd(10_000),
                offered: MicroUsd(0)
            }
        );
        // Overpaying is refused and the latest voucher is unchanged.
        assert!(book.pay(&u, 0, &voucher(&u, 50_000)).unwrap().is_err());
        assert_eq!(book.voucher(&u).unwrap().body.cumulative, MicroUsd(0));
        // Paying exactly works, and then the request is admitted.
        assert_eq!(
            book.pay(&u, 0, &voucher(&u, 10_000)).unwrap().unwrap(),
            MicroUsd(10_000)
        );
        book.admit(
            &u,
            facts(ATC),
            &voucher(&u, 10_000),
            ([2; 32], MicroUsd(50)),
        )
        .unwrap()
        .unwrap();
    }

    // Scenario "托管不足": unredeemed bills plus requests in flight count against the escrow.
    #[test]
    fn escrow_covers_bills_and_requests_in_flight() {
        let book = Book::open(&dir("escrow")).unwrap();
        let u = user(2);
        // $0.01 escrow; a request that could cost $0.02 is refused.
        let escrow = ATC / 100;
        let err = book
            .admit(
                &u,
                facts(escrow),
                &voucher(&u, 0),
                ([1; 32], MicroUsd(20_000)),
            )
            .unwrap()
            .unwrap_err();
        assert!(matches!(err, PayError::Escrow { .. }));
        // Two requests of $0.006 cannot be in flight together.
        book.admit(
            &u,
            facts(escrow),
            &voucher(&u, 0),
            ([1; 32], MicroUsd(6_000)),
        )
        .unwrap()
        .unwrap();
        assert!(
            book.admit(
                &u,
                facts(escrow),
                &voucher(&u, 0),
                ([2; 32], MicroUsd(6_000))
            )
            .unwrap()
            .is_err()
        );
        book.release(&u, &[1; 32]);
        book.admit(
            &u,
            facts(escrow),
            &voucher(&u, 0),
            ([2; 32], MicroUsd(6_000)),
        )
        .unwrap()
        .unwrap();
        // No rate: nothing can be priced.
        let no_rate = ChannelFacts {
            rate: None,
            ..facts(escrow)
        };
        assert_eq!(
            book.admit(&u, no_rate, &voucher(&u, 0), ([3; 32], MicroUsd(1)))
                .unwrap()
                .unwrap_err(),
            PayError::NoRate
        );
    }

    // Crash recovery: the ledger reopens with the same bills, voucher and receipts.
    #[test]
    fn reopens_with_the_same_state() {
        let d = dir("reopen");
        let u = user(3);
        {
            let book = Book::open(&d).unwrap();
            book.admit(&u, facts(ATC), &voucher(&u, 0), ([1; 32], MicroUsd(50)))
                .unwrap()
                .unwrap();
            book.bill(&u, receipt(1, 7, 300)).unwrap();
            book.pay(&u, 0, &voucher(&u, 300)).unwrap().unwrap();
        }
        let again = Book::open(&d).unwrap().get(&u).unwrap();
        assert_eq!(again.billed, MicroUsd(300));
        assert_eq!(again.voucher.unwrap().body.cumulative, MicroUsd(300));
        assert_eq!(again.unreported.len(), 1);
        assert!(again.inflight.is_empty());
    }

    // Scenario "未付清的收据暂不报告": only receipts covered by the latest voucher are planned.
    #[test]
    fn uncovered_receipts_wait_for_the_next_voucher() {
        let u = user(4);
        let state = ChannelState {
            number: 0,
            billed: MicroUsd(30),
            voucher: Some(voucher(&u, 20)),
            inflight: BTreeMap::new(),
            unreported: vec![
                Billed {
                    receipt: receipt(1, 7, 10),
                    cumulative: MicroUsd(10),
                },
                Billed {
                    receipt: receipt(2, 7, 10),
                    cumulative: MicroUsd(20),
                },
                Billed {
                    receipt: receipt(3, 7, 10),
                    cumulative: MicroUsd(30),
                },
            ],
        };
        let (reports, problems) = plan(&[(u, state)]);
        assert!(problems.is_empty());
        assert_eq!(reports.len(), 1);
        let ids: Vec<u8> = reports[0][0]
            .receipts
            .iter()
            .map(|r| r.body.request_id[0])
            .collect();
        assert_eq!(ids, [1, 2]);
        check_portion(&reports[0][0], MicroUsd::ZERO).unwrap();
        assert!(check_portion(&reports[0][0], MicroUsd(5)).is_err());
    }

    // 17 paid channels need two reports (at most 16 vouchers each).
    #[test]
    fn reports_split_at_the_voucher_limit() {
        let channels: Vec<(AccountId32, ChannelState)> = (0..17u8)
            .map(|i| {
                let u = user(i.saturating_add(10));
                let state = ChannelState {
                    number: 0,
                    billed: MicroUsd(5),
                    voucher: Some(voucher(&u, 5)),
                    inflight: BTreeMap::new(),
                    unreported: vec![Billed {
                        receipt: receipt(i, 7, 5),
                        cumulative: MicroUsd(5),
                    }],
                };
                (u, state)
            })
            .collect();
        let (reports, _) = plan(&channels);
        let sizes: Vec<usize> = reports.iter().map(Vec::len).collect();
        assert_eq!(sizes, [16, 1]);
    }
}
