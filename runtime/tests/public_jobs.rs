//! Public jobs in the assembled runtime (m6-public-jobs 8.1).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use ac_crypto::SigAlg;
use ac_primitives::market::MicroUsd;
use ac_primitives::market::public::runtime_decl_for_public_jobs_api::PublicJobsApiV1;
use ac_primitives::market::public::{JobSpec, RULES_V1, Url, WorkerModels};
use ac_primitives::market::work::JobKind;
use ac_runtime::{ATC, Runtime, RuntimeCall, RuntimeOrigin};
use common::{Signer, apply, dev_ext, free, signed};
use frame_support::traits::fungible::Mutate;
use sp_runtime::traits::Dispatchable;

fn clean_spec() -> JobSpec {
    JobSpec {
        kind: JobKind::DataClean,
        model: None,
        rules: RULES_V1,
        manifest_hash: [1; 32],
        manifest_url: Url::truncate_from(b"https://data.example/m.json".to_vec()),
        results_url: Url::truncate_from(b"https://results.example".to_vec()),
        units: 1,
        price: MicroUsd(10_000),
        canary_root: None,
    }
}

fn publish(
    origin: RuntimeOrigin,
) -> sp_runtime::DispatchResultWithInfo<frame_support::dispatch::PostDispatchInfo> {
    RuntimeCall::PublicJobs(pallet_public_jobs::Call::publish {
        spec: Box::new(clean_spec()),
    })
    .dispatch(origin)
}

// Spec market/public-jobs "任务发布与取消": only the admin origin publishes.
#[test]
fn only_the_admin_publishes() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        assert!(publish(RuntimeOrigin::signed(alice.account.clone())).is_err());
        assert!(<Runtime as PublicJobsApiV1<_, _, _>>::jobs().is_empty());
        publish(RuntimeOrigin::root()).unwrap();
        assert_eq!(<Runtime as PublicJobsApiV1<_, _, _>>::jobs(), vec![0]);
        let job = <Runtime as PublicJobsApiV1<_, _, _>>::job(0).unwrap();
        assert_eq!(job.spec, clean_spec());
    });
}

// Spec market/public-jobs "工作者登记与就绪": a registered worker declares itself ready without a
// fee, and the runtime API returns its record and the dev parameters.
#[test]
fn ready_is_free() {
    dev_ext().execute_with(|| {
        let worker = Signer::fresh(9, SigAlg::MlDsa44);
        ac_runtime::Balances::set_balance(&worker.account, 10 * ATC);
        let result = apply(signed(
            &worker,
            RuntimeCall::PublicJobs(pallet_public_jobs::Call::register {
                models: WorkerModels::default(),
            }),
        ))
        .unwrap();
        assert!(result.is_ok(), "{result:?}");
        let before = free(&worker.account);
        let result = apply(signed(
            &worker,
            RuntimeCall::PublicJobs(pallet_public_jobs::Call::ready {}),
        ))
        .unwrap();
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(free(&worker.account), before);
        let record = <Runtime as PublicJobsApiV1<_, _, _>>::worker(worker.account.clone()).unwrap();
        assert!(record.last_ready.is_some());
        assert_eq!(
            <Runtime as PublicJobsApiV1<_, _, _>>::params(),
            Some(ac_primitives::market::public::PublicParams::DEV)
        );
    });
}
