//! The collector against a stand-in chain (m6-public-jobs 7.2).

#![allow(clippy::unwrap_used, clippy::expect_used)] // Test code.

use std::collections::BTreeMap;
use std::sync::Mutex as StdMutex;

use ac_primitives::encode_address;
use ac_primitives::market::public::{Summary, UnitRecord};
use ac_wallet::http::Client;

use super::*;
use crate::canary::CanaryFile;

#[derive(Default)]
struct FakeChain {
    units: StdMutex<BTreeMap<(JobId, UnitIndex), Unit>>,
    canaries: StdMutex<Vec<(JobId, UnitIndex, CanaryReveal)>>,
}

#[async_trait]
impl CollectChain for FakeChain {
    async fn unit(&self, job: JobId, unit: UnitIndex) -> Result<Option<Unit>> {
        Ok(self.units.lock().unwrap().get(&(job, unit)).cloned())
    }
    async fn reveal_canary(
        &self,
        job: JobId,
        unit: UnitIndex,
        canary: CanaryReveal,
    ) -> Result<bool> {
        self.canaries.lock().unwrap().push((job, unit, canary));
        if let Some(u) = self.units.lock().unwrap().get_mut(&(job, unit)) {
            u.canary_revealed = true;
        }
        Ok(true)
    }
}

fn w(b: u8) -> AccountId32 {
    AccountId32::new([b; 32])
}

/// Unit (0, `unit`), settled with `state`; workers 1 and 2 revealed `good`, worker 3 `bad`.
fn unit(state: UnitState, good: &[u8], bad: &[u8]) -> Unit {
    let s = Summary::truncate_from(vec![0; 32]);
    UnitRecord {
        attempt: 1,
        opened_at: 1,
        commit_by: 20,
        reveal_by: 30,
        assigned: [w(1), w(2), w(3)],
        tried: Default::default(),
        commits: [None; 3],
        reveals: [
            Some((s.clone(), blake3(good))),
            Some((s.clone(), blake3(good))),
            Some((s, blake3(bad))),
        ],
        state,
        settled_at: Some(30),
        canary_revealed: false,
    }
}

const ACCEPTED: UnitState = UnitState::Accepted {
    reference: 0,
    majority: [true, true, false],
};

fn collector(name: &str, chain: Arc<FakeChain>, canaries: Vec<CanaryFile>) -> (Collector, PathBuf) {
    let dir = std::env::temp_dir().join(format!("ac-collect-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    (Collector::new(chain, &dir, canaries), dir)
}

// Spec "哈希不符的上传被拒绝", and the other checks of D11.
#[tokio::test]
async fn only_the_majoritys_revealed_result_is_saved_once() {
    let chain = Arc::new(FakeChain::default());
    let (good, bad) = (b"the result".to_vec(), b"another result".to_vec());
    chain
        .units
        .lock()
        .unwrap()
        .insert((0, 1), unit(ACCEPTED, &good, &bad));
    chain
        .units
        .lock()
        .unwrap()
        .insert((0, 2), unit(UnitState::Open, &good, &bad));
    let (c, dir) = collector("checks", Arc::clone(&chain), vec![]);

    // A majority worker whose body does not match its revealed hash.
    assert_eq!(
        c.accept(0, 1, &w(1), b"tampered").await.unwrap(),
        Outcome::Refused
    );
    // The minority, even with its own revealed result; a stranger.
    assert_eq!(c.accept(0, 1, &w(3), &bad).await.unwrap(), Outcome::Refused);
    assert_eq!(
        c.accept(0, 1, &w(9), &good).await.unwrap(),
        Outcome::Refused
    );
    // A unit that has not passed, or does not exist.
    assert_eq!(
        c.accept(0, 2, &w(1), &good).await.unwrap(),
        Outcome::Conflict
    );
    assert_eq!(
        c.accept(0, 9, &w(1), &good).await.unwrap(),
        Outcome::Conflict
    );
    assert!(!c.path(0, 1).exists());

    // Over HTTP: the majority's matching upload is saved, the second copy is a conflict.
    let c = Arc::new(c);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = Arc::clone(&c);
    tokio::spawn(http::serve(listener, move |req| {
        let c = Arc::clone(&served);
        async move { handle(req, c).await }
    }));
    let client = Client::new().unwrap();
    let put = |who: AccountId32, body: Vec<u8>| {
        let client = &client;
        async move {
            let address = encode_address(who.as_ref());
            client
                .put_with(
                    &format!("http://{addr}/0/1"),
                    "application/octet-stream",
                    &[(WORKER_HEADER, &address)],
                    body,
                )
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    assert_eq!(put(w(1), b"tampered".to_vec()).await, 403);
    assert_eq!(put(w(1), good.clone()).await, 201);
    assert_eq!(put(w(2), good.clone()).await, 409);
    assert_eq!(std::fs::read(c.path(0, 1)).unwrap(), good);
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec "自动公开金丝雀": a canary is revealed once its unit passes, and only once.
#[tokio::test]
async fn canaries_are_revealed_when_their_units_pass() {
    let chain = Arc::new(FakeChain::default());
    let file = CanaryFile::build(0, &[(1, vec![5; 32]), (2, vec![6; 32])], || Ok([3; 32])).unwrap();
    chain
        .units
        .lock()
        .unwrap()
        .insert((0, 1), unit(ACCEPTED, b"a", b"b"));
    chain
        .units
        .lock()
        .unwrap()
        .insert((0, 2), unit(UnitState::Open, b"a", b"b"));
    let (c, dir) = collector("canary", Arc::clone(&chain), vec![file.clone()]);
    c.reveal_canaries().await.unwrap();
    c.reveal_canaries().await.unwrap();
    {
        let sent = chain.canaries.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!((sent[0].0, sent[0].1), (0, 1));
        assert_eq!(sent[0].2, file.get(1).unwrap().reveal().unwrap());
    }
    chain.units.lock().unwrap().get_mut(&(0, 2)).unwrap().state = ACCEPTED;
    c.reveal_canaries().await.unwrap();
    assert_eq!(chain.canaries.lock().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}
