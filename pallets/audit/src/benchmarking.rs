//! Benchmarks: every call at its worst case under the guardrails, the round start by number of
//! auditors, one pruning step by number of verdict lists handled, one step clearing statistical
//! states of another version than their model's profile by number of states, and one
//! spot-check step by verdict lists read and spot checks opened (m6-audit-hardening 5.9).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)] // Benchmark setup.

use frame_benchmarking::v2::benchmarks;

use crate::{Config, Pallet};

#[benchmarks]
mod benchmarks {
    use super::{Config, Pallet};
    use crate::{
        Adjustable, AuditProfiles, AuditorEndpoints, BenchmarkHelper, Call, ClearCursor,
        DefaultProfile, Disputes, Findings, Locks, ModelSprtStates, OpenDispute, OpenReviews,
        Params, Penalty, PruneNext, RetainedVerdicts, ReviewOf, ReviewRefs, Reviews, Seeds,
        SpotCheckCursor, StaleClearing, StatsEnabled, UnavailableFindings, VerdictList,
        VerdictSubmission, Verdicts,
    };
    use ac_crypto::SigAlg;
    use ac_crypto::sig::SigningKey;
    use ac_primitives::market::audit::{
        AdjustableParams, AuditMetric, AuditProfile, AuditStats, AuditorStatus, CURRENT_STATS,
        DisputeKind, FailReason, MAX_ASSIGN, MAX_AUDITORS, MAX_DISPUTE_ACCUSERS,
        MAX_SPOT_CHECK_BPS, MAX_SPRT_ENTRIES, PenaltyParams, ReviewOutcome, ReviewVote, RoundIndex,
        SprtEntry, SprtState, VerdictOutcome, VerdictRecord, VerdictStats, Vote, round_start,
        spot_checked,
    };
    use ac_primitives::market::receipt::{RECEIPT_CONTEXT, fee_for};
    use ac_primitives::market::work::JobKind;
    use ac_primitives::market::{MicroUsd, ModelId, PricePerMTok, ReceiptBody, SignedReceipt};
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    use frame_benchmarking::v2::{account, whitelisted_caller};
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;
    use frame_support::traits::Get;
    use frame_support::traits::Hooks;
    use frame_support::traits::fungible::Mutate;
    use frame_system::RawOrigin;
    use sp_core::H256;
    use sp_runtime::{AccountId32, SaturatedConversion, Saturating};

    const ATC: u128 = 1_000_000_000_000_000_000;
    const MODEL: ModelId = ModelId([0x42; 32]);
    const PRICE: PricePerMTok = PricePerMTok {
        input: MicroUsd(100_000),
        output: MicroUsd(300_000),
    };
    const FAIL: VerdictOutcome = VerdictOutcome::Fail(FailReason::Threshold {
        chunk: 0,
        metric: AuditMetric::MantissaMean,
    });
    /// Statistics contributing the largest amount (+3.0 nats under version 1).
    const INT8: AuditStats = AuditStats {
        prompt_tokens: 200,
        prefill_mean_centi: 125,
        decode_mean_centi: 250,
        decode_chunks: 4,
    };

    /// ML-DSA-87: the largest keys and signatures, the slowest verification.
    fn signer(name: &str) -> SigningKey {
        SigningKey::from_seed(SigAlg::MlDsa87, &ac_crypto::dev_seed(name).unwrap()).unwrap()
    }

    fn provider() -> AccountId32 {
        account("provider", 0, 0)
    }

    fn gateway() -> AccountId32 {
        account("gateway", 0, 0)
    }

    fn auditor(i: u32) -> AccountId32 {
        account("auditor", i, 0)
    }

    /// Rate, randomness, a provider and a gateway with keys, a funded pot, and `n` auditors.
    fn world<T: Config>(n: u32) {
        T::BenchmarkHelper::set_rate(ATC);
        T::BenchmarkHelper::set_randomness(H256([7; 32]));
        T::BenchmarkHelper::register_provider(&provider(), MODEL, PRICE);
        T::BenchmarkHelper::register_gateway(&gateway());
        T::BenchmarkHelper::set_key(&provider(), &signer("bench-provider").public_key().unwrap());
        T::BenchmarkHelper::set_key(&gateway(), &signer("bench-gateway").public_key().unwrap());
        T::Currency::set_balance(&Pallet::<T>::pot(), 1_000_000 * ATC);
        T::BenchmarkHelper::register_bf16_model(MODEL);
        StatsEnabled::<T>::put(true);
        DefaultProfile::<T>::put(AuditProfile {
            thresholds_version: 4,
            stats_version: CURRENT_STATS.version,
        });
        let max_rate = AdjustableParams {
            spot_check_bps: MAX_SPOT_CHECK_BPS,
            ..Adjustable::<T>::get().unwrap()
        };
        Adjustable::<T>::put(max_rate);
        for i in 0..n {
            let a = auditor(i);
            T::Currency::set_balance(&a, 10_000 * ATC);
            Pallet::<T>::register(RawOrigin::Signed(a).into(), 1_000 * ATC).unwrap();
        }
    }

    /// Moves to the first block of the next round and starts it.
    fn next_round<T: Config>() -> RoundIndex {
        let params = Params::<T>::get().unwrap();
        let r = Pallet::<T>::current_round(&params).saturating_add(1);
        let at: u32 = round_start(r, params.round_blocks);
        frame_system::Pallet::<T>::set_block_number(at.saturated_into());
        Pallet::<T>::on_initialize(at.saturated_into());
        r
    }

    fn receipt<T: Config>(id: u32) -> SignedReceipt {
        let mut request_id = [0xaa; 32];
        request_id[..4].copy_from_slice(&id.to_le_bytes());
        let body = ReceiptBody {
            genesis: Pallet::<T>::genesis(),
            gateway: gateway(),
            provider: provider(),
            kind: JobKind::Inference,
            model: MODEL,
            request_id,
            in_tokens: 4_000,
            out_tokens: 4_000,
            fee: fee_for(&PRICE, 4_000, 4_000).unwrap(),
            toploc_commit: [9; 32],
            content_commit: [8; 32],
            ttft_ms: 100,
            total_ms: 9_000,
        };
        let payload = body.payload().unwrap();
        let (p, g) = (signer("bench-provider"), signer("bench-gateway"));
        SignedReceipt {
            provider_key: p.public_key().unwrap(),
            provider_sig: p.sign_deterministic(&payload, RECEIPT_CONTEXT).unwrap(),
            gateway_key: g.public_key().unwrap(),
            gateway_sig: g.sign_deterministic(&payload, RECEIPT_CONTEXT).unwrap(),
            body,
        }
    }

    fn verdict<T: Config>(
        round: RoundIndex,
        id: u32,
        outcome: VerdictOutcome,
    ) -> Box<VerdictSubmission> {
        Box::new(VerdictSubmission {
            provider: provider(),
            round,
            outcome,
            thresholds_version: 4,
            evidence: Some([1; 32]),
            receipt: receipt::<T>(id),
            stats: Some(VerdictStats {
                version: CURRENT_STATS.version,
                stats: INT8,
            }),
        })
    }

    /// A full statistical state of `who` on [`MODEL`] (MAX_SPRT_ENTRIES verdicts of distinct
    /// auditors in rounds `first..`), just below the bound, with the verdict lists it refers to
    /// stored in `store` (`true`: retained past their retention; `false`: still in `Verdicts`).
    fn full_state<T: Config>(who: &AccountId32, first: RoundIndex, retained: bool) {
        state_of::<T>(who, first, (retained, false), MAX_SPRT_ENTRIES);
    }

    /// [`full_state`] with `count` entries (at least 9: a contribution is at most 3.0 nats)
    /// summing to the same value just below the bound; with `registered`, the entries are those
    /// of registered auditors `0..`, who can be slashed (m6-audit-vote-weights 1.1).
    fn state_of<T: Config>(
        who: &AccountId32,
        first: RoundIndex,
        (retained, registered): (bool, bool),
        count: u32,
    ) {
        let total = CURRENT_STATS
            .bound
            .checked_div(i64::from(MAX_SPRT_ENTRIES))
            .unwrap()
            .saturating_sub(3)
            .saturating_mul(i64::from(MAX_SPRT_ENTRIES));
        let per = total.checked_div(i64::from(count)).unwrap();
        let rest = total.saturating_sub(per.saturating_mul(i64::from(count)));
        let mut state = SprtState::default();
        for i in 0..count {
            // The remainder goes one unit each to the first entries: the sum stays `total`.
            let contribution =
                i32::try_from(per.saturating_add(i64::from(i64::from(i) < rest))).unwrap();
            assert!(
                i64::from(contribution) <= 3_000,
                "a contribution is at most 3.0 nats"
            );
            let entry_auditor: AccountId32 = if registered {
                auditor(i)
            } else {
                account("entry", i, 0)
            };
            let round = first.saturating_add(i);
            state
                .entries
                .try_push(SprtEntry {
                    auditor: entry_auditor.clone(),
                    round,
                    contribution,
                })
                .unwrap();
            let list = VerdictList::truncate_from(alloc::vec![VerdictRecord {
                auditor: entry_auditor,
                outcome: VerdictOutcome::Pass,
                thresholds_version: 4,
                evidence: Some([1; 32]),
                receipt_hash: H256([2; 32]),
                request_id: [3; 32],
                gateway: account("entry-gateway", i, 0),
                stats: Some(VerdictStats {
                    version: CURRENT_STATS.version,
                    stats: INT8,
                }),
            }]);
            if retained {
                RetainedVerdicts::<T>::insert(round, who, list);
            } else {
                Verdicts::<T>::insert(round, who, list);
            }
        }
        state.cumulative = total;
        ModelSprtStates::<T>::insert(who, MODEL, (CURRENT_STATS.version, state));
    }

    /// A statistical dispute on the provider citing `n` verdicts of registered auditors (`n − 1`
    /// in a state just below the bound, retained past their retention, and one submitted now
    /// that crosses it). Returns the dispute.
    fn statistical_dispute<T: Config>(n: u32) -> u64 {
        world::<T>(MAX_AUDITORS);
        let r = next_round::<T>();
        state_of::<T>(&provider(), 0, (true, true), n.saturating_sub(1));
        let last_verdict = Pallet::<T>::assignment(r, &provider())
            .into_iter()
            .find(|a| (0..MAX_SPRT_ENTRIES).all(|i| auditor(i) != *a))
            .unwrap();
        Pallet::<T>::submit_verdict(
            RawOrigin::Signed(last_verdict).into(),
            verdict::<T>(r, 1_000, VerdictOutcome::Pass),
        )
        .unwrap();
        let id = OpenDispute::<T>::get(provider()).unwrap();
        assert_eq!(
            Disputes::<T>::get(id).unwrap().accusers.len(),
            usize::try_from(n).unwrap()
        );
        id
    }

    /// Every cited verdict of dispute `id` marked as evidence not obtained.
    fn all_marked<T: Config>(id: u64) -> u128 {
        let cited = u32::try_from(Disputes::<T>::get(id).unwrap().accusers.len()).unwrap();
        u128::MAX
            .checked_shr(128u32.saturating_sub(cited))
            .unwrap_or(u128::MAX)
    }

    /// All the dispute's reviewers but the last vote `vote`, every cited verdict marked; returns
    /// the last reviewer, whose ballot decides.
    fn all_but_one_vote<T: Config>(id: u64, vote: Vote) -> AccountId32 {
        let marks = all_marked::<T>(id);
        let d = Disputes::<T>::get(id).unwrap();
        let reviewers: Vec<AccountId32> = d.reviewers.iter().map(|(x, _)| x.clone()).collect();
        let quorum = usize::from(Params::<T>::get().unwrap().quorum);
        for x in reviewers.iter().take(quorum.saturating_sub(1)) {
            Pallet::<T>::vote_dispute(
                RawOrigin::Signed(x.clone()).into(),
                provider(),
                id,
                vote,
                marks,
            )
            .unwrap();
        }
        reviewers[quorum.saturating_sub(1)].clone()
    }

    #[benchmark]
    fn register() {
        world::<T>(0);
        let caller: AccountId32 = whitelisted_caller();
        T::Currency::set_balance(&caller, 10_000 * ATC);
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 1_000 * ATC);
        assert!(Pallet::<T>::auditor(&caller).is_some());
    }

    #[benchmark]
    fn bond_extra() {
        world::<T>(1);
        #[extrinsic_call]
        _(RawOrigin::Signed(auditor(0)), 10 * ATC);
    }

    #[benchmark]
    fn unbond() {
        world::<T>(1);
        Pallet::<T>::bond_extra(RawOrigin::Signed(auditor(0)).into(), 10 * ATC).unwrap();
        #[extrinsic_call]
        _(RawOrigin::Signed(auditor(0)), 10 * ATC);
    }

    #[benchmark]
    fn exit() {
        world::<T>(1);
        #[extrinsic_call]
        _(RawOrigin::Signed(auditor(0)));
    }

    #[benchmark]
    fn withdraw_unbonded() {
        world::<T>(1);
        Pallet::<T>::exit(RawOrigin::Signed(auditor(0)).into()).unwrap();
        let params = Params::<T>::get().unwrap();
        let later = frame_system::Pallet::<T>::block_number()
            .saturating_add(params.unbond_blocks.saturated_into());
        frame_system::Pallet::<T>::set_block_number(later);
        #[extrinsic_call]
        _(RawOrigin::Signed(auditor(0)));
        assert!(Pallet::<T>::auditor(&auditor(0)).is_none());
    }

    /// The worst verdict: a lone failure judged by the thresholds (looks for a failure
    /// dispute in two rounds, reading their verdicts and judgments), with the largest receipt, a full roster and a payment, whose
    /// statistics fill a full state (the oldest entry dropped, the rest replayed) past the
    /// bound and open a statistical dispute on all its verdicts (drawing the reviewers).
    #[benchmark]
    fn submit_verdict() {
        world::<T>(MAX_AUDITORS);
        let r = next_round::<T>();
        full_state::<T>(&provider(), 0, true);
        let last = Pallet::<T>::assignment(r, &provider())[0].clone();
        let v = verdict::<T>(r, 1_000, FAIL);
        #[extrinsic_call]
        _(RawOrigin::Signed(last), v);
        let id = OpenDispute::<T>::get(provider()).unwrap();
        assert!(matches!(
            Disputes::<T>::get(id).unwrap().kind,
            DisputeKind::ModelStatistical { .. }
        ));
    }

    /// A vote that decides nothing (m6-audit-vote-weights design D2): the first reviewer of a
    /// statistical dispute on MAX_DISPUTE_ACCUSERS verdicts, marking every one, on the largest
    /// dispute record.
    #[benchmark]
    fn vote_dispute_open() {
        let id = statistical_dispute::<T>(MAX_DISPUTE_ACCUSERS);
        let marks = all_marked::<T>(id);
        let first = Disputes::<T>::get(id).unwrap().reviewers[0].0.clone();
        #[extrinsic_call]
        vote_dispute(
            RawOrigin::Signed(first),
            provider(),
            id,
            Vote::Reject,
            marks,
        );
        assert_eq!(OpenDispute::<T>::get(provider()), Some(id));
    }

    /// The deciding vote confirming a statistical dispute on `n` verdicts of registered
    /// auditors (m6-audit-vote-weights 2.2: confirming is heavier than rejecting at every `n`),
    /// every one marked unavailable by every ballot, each auditor at its last finding below the
    /// limit: the provider is slashed and jailed, each auditor takes the full penalty and exits,
    /// the winning reviewers are paid, the marks are cleared.
    #[benchmark]
    fn vote_dispute_close(n: Linear<10, MAX_DISPUTE_ACCUSERS>) {
        let id = statistical_dispute::<T>(n);
        let params = Params::<T>::get().unwrap();
        let limit = Penalty::<T>::get()
            .unwrap_or(PenaltyParams::LIVE)
            .unavailable_limit;
        let now = Pallet::<T>::current_round(&params);
        for a in &Disputes::<T>::get(id).unwrap().accusers {
            UnavailableFindings::<T>::insert(
                &a.auditor,
                Findings::truncate_from(alloc::vec![now; usize::from(limit.saturating_sub(1))]),
            );
        }
        let last = all_but_one_vote::<T>(id, Vote::Confirm);
        let marks = all_marked::<T>(id);
        #[extrinsic_call]
        vote_dispute(
            RawOrigin::Signed(last),
            provider(),
            id,
            Vote::Confirm,
            marks,
        );
        assert!(OpenDispute::<T>::get(provider()).is_none());
        let first = Disputes::<T>::get(id).unwrap().accusers[0].auditor.clone();
        assert_eq!(
            Pallet::<T>::auditor(&first).unwrap().status,
            AuditorStatus::Exiting
        );
    }

    /// A spot check of auditor `n`'s verdict on the provider in round 0, with the first `N`
    /// registered auditors as reviewers, open until `deadline`.
    fn spot_check_of<T: Config>(n: u32) -> u64 {
        let params = Params::<T>::get().unwrap();
        let verdict_auditor = auditor(n);
        Verdicts::<T>::insert(
            0,
            provider(),
            VerdictList::truncate_from(alloc::vec![VerdictRecord {
                auditor: verdict_auditor.clone(),
                outcome: VerdictOutcome::Pass,
                thresholds_version: 4,
                evidence: Some([1; 32]),
                receipt_hash: H256([2; 32]),
                request_id: [3; 32],
                gateway: gateway(),
                stats: Some(VerdictStats {
                    version: CURRENT_STATS.version,
                    stats: INT8,
                }),
            }]),
        );
        let reviewers = (0..u32::from(params.reviewers))
            .map(|i| (auditor(i), None))
            .collect::<Vec<_>>();
        let deadline = frame_system::Pallet::<T>::block_number()
            .saturating_add(params.vote_blocks.saturated_into());
        Reviews::<T>::insert(
            0,
            ReviewOf::<T> {
                provider: provider(),
                round: 0,
                auditor: verdict_auditor.clone(),
                reviewers: frame_support::BoundedVec::truncate_from(reviewers),
                deadline,
                outcome: None,
                closed_at: None,
            },
        );
        OpenReviews::<T>::insert(0, ());
        ReviewRefs::<T>::insert(0, provider(), 1);
        Locks::<T>::insert(&verdict_auditor, 1);
        0
    }

    /// The deciding ballot overturning a verdict with the evidence at hand: its auditor is
    /// slashed and made to exit, the winning reviewers are paid.
    #[benchmark]
    fn vote_review() {
        world::<T>(16);
        let params = Params::<T>::get().unwrap();
        let id = spot_check_of::<T>(u32::from(params.reviewers));
        let quorum = u32::from(params.quorum);
        for i in 0..quorum.saturating_sub(1) {
            Pallet::<T>::vote_review(
                RawOrigin::Signed(auditor(i)).into(),
                id,
                ReviewVote::Overturn,
                true,
            )
            .unwrap();
        }
        let last = auditor(quorum.saturating_sub(1));
        #[extrinsic_call]
        _(RawOrigin::Signed(last), id, ReviewVote::Overturn, true);
        assert_eq!(
            Reviews::<T>::get(id).unwrap().outcome,
            Some(ReviewOutcome::Overturned)
        );
    }

    /// Closing an undecided spot check: every reviewer missed its ballot.
    #[benchmark]
    fn close_review() {
        world::<T>(16);
        let params = Params::<T>::get().unwrap();
        let id = spot_check_of::<T>(u32::from(params.reviewers));
        let deadline = Reviews::<T>::get(id).unwrap().deadline;
        frame_system::Pallet::<T>::set_block_number(deadline.saturating_add(1u32.into()));
        let caller: AccountId32 = whitelisted_caller();
        #[extrinsic_call]
        close_spot_check(RawOrigin::Signed(caller), id);
        assert!(!OpenReviews::<T>::contains_key(id));
    }

    /// One spot-check step reading `l` verdict lists of the previous round, each full, and
    /// opening `o` spot checks (the request IDs are chosen so that exactly `o` are drawn).
    #[benchmark]
    fn spot_check(
        l: Linear<1, { crate::SPOT_CHECK_BATCH }>,
        o: Linear<0, { crate::MAX_SPOT_CHECKS_PER_STEP }>,
    ) {
        world::<T>(MAX_AUDITORS);
        let previous = next_round::<T>();
        let r = next_round::<T>();
        SpotCheckCursor::<T>::kill();
        let seed = Seeds::<T>::get(r).unwrap();
        let mut candidate = 0u32;
        let mut next_id = |drawn: bool| loop {
            candidate = candidate.saturating_add(1);
            let mut id = [0xbb; 32];
            id[..4].copy_from_slice(&candidate.to_le_bytes());
            if spot_checked(&seed, &id, MAX_SPOT_CHECK_BPS) == drawn {
                break id;
            }
        };
        let mut to_draw = o;
        for p in 0..l {
            let who: AccountId32 = account("listed", p, 0);
            let list: Vec<VerdictRecord<AccountId32>> = (0..u32::from(MAX_ASSIGN))
                .map(|k| {
                    let drawn = to_draw > 0;
                    to_draw = to_draw.saturating_sub(u32::from(drawn));
                    VerdictRecord {
                        auditor: auditor(k),
                        outcome: VerdictOutcome::Pass,
                        thresholds_version: 4,
                        evidence: Some([1; 32]),
                        receipt_hash: H256([2; 32]),
                        request_id: next_id(drawn),
                        gateway: gateway(),
                        stats: Some(VerdictStats {
                            version: CURRENT_STATS.version,
                            stats: INT8,
                        }),
                    }
                })
                .collect();
            Verdicts::<T>::insert(previous, who, VerdictList::truncate_from(list));
        }
        SpotCheckCursor::<T>::put((previous, ClearCursor::new()));
        let params = Params::<T>::get().unwrap();
        #[block]
        {
            Pallet::<T>::spot_check_step(&params, r);
        }
    }

    /// Closing an undecided statistical dispute on `n` verdicts retained past their retention:
    /// every reviewer missed its vote, every auditor is unlocked and the verdicts are released.
    #[benchmark]
    fn close_dispute(n: Linear<10, MAX_DISPUTE_ACCUSERS>) {
        let id = statistical_dispute::<T>(n);
        let deadline = Disputes::<T>::get(id).unwrap().deadline;
        frame_system::Pallet::<T>::set_block_number(deadline.saturating_add(1u32.into()));
        let caller: AccountId32 = whitelisted_caller();
        #[extrinsic_call]
        _(RawOrigin::Signed(caller), provider(), id);
        assert!(OpenDispute::<T>::get(provider()).is_none());
    }

    /// Enabling the statistical judgment.
    #[benchmark]
    fn set_stats_enabled() {
        world::<T>(0);
        StatsEnabled::<T>::put(false);
        #[extrinsic_call]
        _(RawOrigin::Root, true);
        assert!(StatsEnabled::<T>::get());
    }

    /// Setting the profile of a bf16 model that has one already.
    #[benchmark]
    fn set_audit_profile() {
        world::<T>(0);
        let p = AuditProfile {
            thresholds_version: 4,
            stats_version: CURRENT_STATS.version,
        };
        AuditProfiles::<T>::insert(MODEL, p);
        #[extrinsic_call]
        _(RawOrigin::Root, MODEL, p);
        assert_eq!(AuditProfiles::<T>::get(MODEL), Some(p));
    }

    /// One clearing step removing `n` full states of another version than their model's
    /// profile, each releasing the MAX_SPRT_ENTRIES verdict lists it retained.
    #[benchmark]
    fn clear_stale(n: Linear<1, { crate::STALE_CLEAR_LIMIT }>) {
        world::<T>(0);
        for i in 0..n {
            let who: AccountId32 = account("stale", i, 0);
            full_state::<T>(&who, 0, true);
            let (_, state) = ModelSprtStates::<T>::get(&who, MODEL).unwrap();
            ModelSprtStates::<T>::insert(
                &who,
                MODEL,
                (CURRENT_STATS.version.saturating_sub(1), state),
            );
        }
        StaleClearing::<T>::put(ClearCursor::new());
        #[block]
        {
            Pallet::<T>::clear_stale_step();
        }
        assert_eq!(ModelSprtStates::<T>::iter().count(), 0);
    }

    #[benchmark]
    fn set_params() {
        world::<T>(0);
        let mut p = Adjustable::<T>::get().unwrap();
        p.spot_check_bps = p.spot_check_bps.saturating_sub(1);
        #[extrinsic_call]
        _(RawOrigin::Root, p);
    }

    /// Setting the longest endpoint with an X-Wing key.
    #[benchmark]
    fn set_endpoint() {
        world::<T>(1);
        let url = frame_support::BoundedVec::truncate_from(alloc::vec![
            b'e';
            ac_primitives::market::records::MAX_ENDPOINT_LEN as usize
        ]);
        let kem = ac_crypto::KemPublicKey::new(ac_crypto::KemAlg::XWing, &[7; 1216]).unwrap();
        #[extrinsic_call]
        _(RawOrigin::Signed(auditor(0)), Some((url, kem)));
        assert!(AuditorEndpoints::<T>::contains_key(auditor(0)));
    }

    /// A round's first block with `n` registered auditors.
    #[benchmark]
    fn start_round(n: Linear<0, MAX_AUDITORS>) {
        world::<T>(n);
        let params = Params::<T>::get().unwrap();
        let r = Pallet::<T>::current_round(&params).saturating_add(1);
        #[block]
        {
            Pallet::<T>::start_round(r);
        }
    }

    /// One pruning step handling `n` verdict lists of an old round, each one that a full
    /// statistical state still refers to and so moves to the retained lists (heavier per entry
    /// than removing a used request ID).
    #[benchmark]
    fn prune(n: Linear<1, { T::PruneLimit::get() }>) {
        world::<T>(0);
        for i in 0..n {
            full_state::<T>(&account("pruned", i, 0), 0, false);
        }
        PruneNext::<T>::put(0);
        let current = T::RetentionRounds::get().saturating_add(1);
        #[block]
        {
            Pallet::<T>::prune_step(current);
        }
        assert_eq!(
            RetainedVerdicts::<T>::iter_key_prefix(0).count(),
            usize::try_from(n).unwrap()
        );
    }

    frame_benchmarking::impl_benchmark_test_suite!(
        Pallet,
        crate::mock::new_test_ext(0),
        crate::mock::Test
    );
}
