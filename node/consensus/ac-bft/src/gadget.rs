//! The AC-BFT node task (design D3–D5, D11 of `m2-finality`).
//!
//! [`run`] drives a [`Voter`] with the client's import and finality notifications, the
//! AC-BFT notification protocol and a timer, and carries out its actions: the voter state is
//! written to auxiliary storage **before** any message is signed, finality proofs are applied
//! through the client (and stored for set-change blocks and every [`PROOF_INTERVAL`] blocks),
//! double signing is handed to the reporter, and the voter is rebuilt when a set change is
//! finalized. Nodes without a validator key follow finality without signing anything.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ac_crypto::sig::SigningKey;
use ac_primitives::ac_bft::{
    Authority, AuthorityIndex, BlockRef, ENGINE_ID, FinalityProof, SignedMessage, VOTE_CONTEXT,
    VersionedFinalityProof, VersionedMessage, signing_payload,
};
use ac_primitives::offences::Evidence;
use futures::StreamExt;
use parity_scale_codec::{Decode, Encode};
use sc_client_api::{AuxStore, Backend, BlockchainEvents, Finalizer};
use sc_network::service::traits::{
    MessageSink, NotificationEvent, NotificationService, ValidationResult,
};
use sc_network::{NetworkPeers, PeerId, ReputationChange};
use sp_blockchain::{HeaderBackend, HeaderMetadata};
use sp_core::H256;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT};
use substrate_prometheus_endpoint::{
    Gauge, Histogram, HistogramOpts, PrometheusError, Registry, U64, exponential_buckets, register,
};

use crate::LOG_TARGET;
use crate::chain::{ClientAncestry, ClientChain};
use crate::import::SharedTracker;
use crate::network::{MessageFilter, Verdict};
use crate::protocol::{Action, Config, Voter, VoterState};
use crate::tracker;

/// Auxiliary-storage key of the persisted voter state.
pub const VOTER_AUX_KEY: &[u8] = b"acbft:voter";
/// Finality proofs are stored at least this often (in blocks), besides set-change blocks.
pub const PROOF_INTERVAL: u32 = 64;
/// Reputation cost of an invalid AC-BFT message.
const INVALID_MESSAGE: ReputationChange =
    ReputationChange::new(-(1 << 12), "AC-BFT: invalid message");
/// Import times kept for the latency metric.
const MAX_TRACKED_IMPORTS: usize = 4096;

/// Submits double-signing evidence on chain.
pub trait Reporter: Send + Sync {
    /// Reports `evidence` (best effort).
    fn report(&self, evidence: Evidence);
}

/// Parameters of [`run`].
pub struct GadgetParams<C, N: ?Sized> {
    /// The client.
    pub client: Arc<C>,
    /// Shared tracker (also used by the block import).
    pub tracker: SharedTracker,
    /// Validator key; `None` on nodes that only follow finality.
    pub key: Option<Arc<SigningKey>>,
    /// The AC-BFT notification protocol.
    pub notifications: Box<dyn NotificationService>,
    /// Peer reputation.
    pub network: Arc<N>,
    /// Double-signing reports.
    pub reporter: Arc<dyn Reporter>,
    /// Aura slot duration in milliseconds.
    pub slot_ms: u64,
    /// Prometheus registry for the gadget's metrics.
    pub prometheus: Option<Registry>,
}

/// Gadget metrics (task 7.8 of `m2-finality`).
#[derive(Clone)]
struct Metrics {
    round: Gauge<U64>,
    finalized: Gauge<U64>,
    latency: Histogram,
}

impl Metrics {
    fn register(registry: &Registry) -> Result<Self, PrometheusError> {
        Ok(Self {
            round: register(Gauge::new("acbft_round", "Current AC-BFT round")?, registry)?,
            finalized: register(
                Gauge::new(
                    "acbft_finalized_number",
                    "Number of the last block finalized by AC-BFT",
                )?,
                registry,
            )?,
            latency: register(
                Histogram::with_opts(
                    HistogramOpts::new(
                        "acbft_finality_latency_seconds",
                        "Time from importing a block to finalizing it",
                    )
                    .buckets(exponential_buckets(0.05, 2.0, 10)?),
                )?,
                registry,
            )?,
        })
    }
}

/// Loads the persisted voter state.
fn load_voter_state(aux: &impl AuxStore) -> Option<VoterState> {
    let bytes = aux.get_aux(VOTER_AUX_KEY).ok()??;
    VoterState::decode(&mut &bytes[..]).ok()
}

/// Everything the loop mutates besides the client and the notification stream.
struct Gadget<B, BE, C, N: ?Sized> {
    client: Arc<C>,
    tracker: SharedTracker,
    key: Option<Arc<SigningKey>>,
    network: Arc<N>,
    reporter: Arc<dyn Reporter>,
    slot_ms: u64,
    metrics: Option<Metrics>,
    genesis: H256,
    voter: Voter,
    filter: MessageFilter,
    peers: HashMap<PeerId, Box<dyn MessageSink>>,
    imported_at: BTreeMap<u32, Instant>,
    start: Instant,
    _marker: std::marker::PhantomData<(B, BE)>,
}

impl<B, BE, C, N> Gadget<B, BE, C, N>
where
    B: BlockT<Hash = H256>,
    B::Header: HeaderT<Number = u32>,
    BE: Backend<B>,
    C: HeaderBackend<B>
        + HeaderMetadata<B, Error = sp_blockchain::Error>
        + AuxStore
        + Finalizer<B, BE>,
    N: NetworkPeers + Send + Sync + ?Sized,
{
    fn now(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn me(&self, authorities: &[Authority]) -> Option<AuthorityIndex> {
        let public = self.key.as_ref()?.public_key().ok()?;
        let index = authorities.iter().position(|a| a.key == public)?;
        AuthorityIndex::try_from(index).ok()
    }

    /// A voter for the tracker's current set, resuming from the persisted state.
    fn build_voter(&self, set_id: u64, authorities: Vec<Authority>) -> Voter {
        let info = self.client.info();
        let finalized = BlockRef {
            hash: info.finalized_hash,
            number: info.finalized_number,
        };
        let me = self.me(&authorities);
        if me.is_none() && self.key.is_some() {
            log::info!(target: LOG_TARGET, "not a member of authority set {set_id}: following finality only");
        }
        Voter::new(
            Config {
                set_id,
                authorities,
                me,
                slot_ms: self.slot_ms,
            },
            load_voter_state(&*self.client),
            finalized,
        )
    }

    fn tracker_snapshot(&self) -> Option<tracker::TrackerState> {
        self.tracker.lock().ok().map(|t| t.clone())
    }

    /// Rebuilds the voter if the tracker moved to another set (a change was finalized).
    fn follow_set_changes(&mut self) {
        let Some(t) = self.tracker_snapshot() else {
            return;
        };
        if t.set_id == self.voter.config().set_id {
            return;
        }
        log::info!(target: LOG_TARGET, "switching to authority set {} ({} members)", t.set_id, t.authorities.len());
        self.voter = self.build_voter(t.set_id, t.authorities.clone());
        let held = self.filter.set_changed(t.set_id, t.authorities);
        let mut actions = Vec::new();
        for bytes in held {
            if let Verdict::Accept(message) = self.filter.recheck(&bytes) {
                actions.extend(self.feed(message));
            }
        }
        self.execute(actions);
    }

    fn feed(&mut self, message: SignedMessage) -> Vec<Action> {
        let Some(t) = self.tracker_snapshot() else {
            return Vec::new();
        };
        let now = self.now();
        self.voter
            .on_message(message, &ClientChain::<B, C>::new(&*self.client, &t), now)
    }

    fn on_imported(&mut self, number: u32) {
        self.imported_at.entry(number).or_insert_with(Instant::now);
        while self.imported_at.len() > MAX_TRACKED_IMPORTS {
            self.imported_at.pop_first();
        }
        // The block import may have applied a finality proof that changed the set; during
        // initial sync the client does not always emit a finality notification for it.
        self.follow_set_changes();
        let Some(t) = self.tracker_snapshot() else {
            return;
        };
        let now = self.now();
        let actions = self
            .voter
            .on_block_imported(&ClientChain::<B, C>::new(&*self.client, &t), now);
        self.execute(actions);
    }

    fn on_finalized(&mut self, block: BlockRef) {
        self.observe_finality(block.number);
        self.follow_set_changes();
        let Some(t) = self.tracker_snapshot() else {
            return;
        };
        let now = self.now();
        let actions =
            self.voter
                .on_finalized(block, &ClientChain::<B, C>::new(&*self.client, &t), now);
        self.execute(actions);
    }

    fn on_tick(&mut self) {
        self.follow_set_changes();
        let Some(t) = self.tracker_snapshot() else {
            return;
        };
        let now = self.now();
        let actions = self
            .voter
            .on_tick(&ClientChain::<B, C>::new(&*self.client, &t), now);
        self.execute(actions);
    }

    fn observe_finality(&mut self, number: u32) {
        if let Some(m) = &self.metrics {
            m.finalized.set(u64::from(number));
            for (_, at) in self.imported_at.range(..=number) {
                m.latency.observe(at.elapsed().as_secs_f64());
            }
        }
        self.imported_at.retain(|n, _| *n > number);
    }

    fn on_notification(&mut self, peer: PeerId, bytes: &[u8]) {
        match self.filter.incoming(bytes) {
            Verdict::Accept(message) => {
                self.send_to_peers(bytes, Some(peer));
                let actions = self.feed(message);
                self.execute(actions);
            }
            Verdict::Invalid => self.network.report_peer(peer, INVALID_MESSAGE),
            Verdict::Duplicate | Verdict::Held | Verdict::Stale => {}
        }
    }

    fn send_to_peers(&self, bytes: &[u8], except: Option<PeerId>) {
        for (peer, sink) in &self.peers {
            if Some(*peer) != except {
                sink.send_sync_notification(bytes.to_vec());
            }
        }
    }

    fn on_event(
        &mut self,
        event: NotificationEvent,
        notifications: &mut Box<dyn NotificationService>,
    ) {
        match event {
            NotificationEvent::ValidateInboundSubstream { result_tx, .. } => {
                let _ = result_tx.send(ValidationResult::Accept);
            }
            NotificationEvent::NotificationStreamOpened { peer, .. } => {
                if let Some(sink) = notifications.message_sink(&peer) {
                    self.peers.insert(peer, sink);
                }
            }
            NotificationEvent::NotificationStreamClosed { peer } => {
                self.peers.remove(&peer);
            }
            NotificationEvent::NotificationReceived { peer, notification } => {
                self.on_notification(peer, &notification);
            }
        }
    }

    /// Carries out `actions` in order, including those produced by our own messages.
    fn execute(&mut self, actions: Vec<Action>) {
        let mut queue: VecDeque<Action> = actions.into();
        // A failed persist blocks every later broadcast of this batch: never sign unrecorded.
        let mut persisted = true;
        while let Some(action) = queue.pop_front() {
            match action {
                Action::Persist(state) => {
                    persisted = self
                        .client
                        .insert_aux(&[(VOTER_AUX_KEY, state.encode().as_slice())], &[])
                        .inspect_err(|e| log::error!(target: LOG_TARGET, "cannot persist the voter state: {e}"))
                        .is_ok();
                }
                Action::Broadcast(message) => {
                    if !persisted {
                        continue;
                    }
                    if let Some(signed) = self.sign(message) {
                        // Machine-readable for the restart tests: one line per signed message.
                        log::debug!(
                            target: LOG_TARGET,
                            "signed set={} round={} kind={:?} target={}",
                            signed.set_id,
                            signed.message.round(),
                            signed.message.kind(),
                            signed.message.target().map_or_else(|| "-".into(), |t| format!("{:?}", t.hash)),
                        );
                        let bytes = self.filter.outgoing(&signed);
                        self.send_to_peers(&bytes, None);
                        queue.extend(self.feed(signed));
                    }
                }
                Action::Finalize(proof) => self.finalize(proof),
                Action::Report(evidence) => self.reporter.report(*evidence),
                Action::Relay(messages) => {
                    for message in messages {
                        let bytes = VersionedMessage::V1(message).encode();
                        self.send_to_peers(&bytes, None);
                    }
                }
            }
        }
        self.filter.set_round(self.voter.round());
        if let Some(m) = &self.metrics {
            m.round.set(self.voter.round());
        }
    }

    fn sign(&self, message: ac_primitives::ac_bft::Message) -> Option<SignedMessage> {
        let key = self.key.as_ref()?;
        let signer = self.voter.config().me?;
        let set_id = self.voter.config().set_id;
        let payload = signing_payload(&self.genesis, set_id, &message).ok()?;
        let mut rng = ac_crypto::OsRng::new()
            .inspect_err(|e| log::error!(target: LOG_TARGET, "no randomness for signing: {e}"))
            .ok()?;
        let signature = key
            .sign(&payload, VOTE_CONTEXT, &mut rng)
            .inspect_err(|e| log::error!(target: LOG_TARGET, "cannot sign: {e}"))
            .ok()?;
        Some(SignedMessage {
            set_id,
            signer,
            message,
            signature,
        })
    }

    /// Applies a commit certificate: finalizes its target and stores the proof when the
    /// target announces a set change or [`PROOF_INTERVAL`] blocks passed since the last one.
    fn finalize(&mut self, proof: FinalityProof) {
        let target = proof.target;
        if target.number <= self.client.info().finalized_number {
            return;
        }
        let store = match self.tracker.lock() {
            Ok(t) => {
                t.is_change_block(&target)
                    || target.number >= t.last_stored_proof.saturating_add(PROOF_INTERVAL)
            }
            Err(_) => return,
        };
        let justification = store.then(|| (ENGINE_ID, VersionedFinalityProof::V1(proof).encode()));
        if let Err(e) = self.client.finalize_block(target.hash, justification, true) {
            log::warn!(target: LOG_TARGET, "cannot finalize #{}: {e}", target.number);
            return;
        }
        log::debug!(target: LOG_TARGET, "finalized #{} ({:?})", target.number, target.hash);
        if let Ok(mut t) = self.tracker.lock() {
            t.on_finalized(target, &ClientAncestry::<B, C>::new(&*self.client));
            if store {
                t.last_stored_proof = target.number;
            }
            if let Err(e) = tracker::store(&*self.client, &t) {
                log::warn!(target: LOG_TARGET, "cannot persist the AC-BFT tracker: {e}");
            }
        }
        self.observe_finality(target.number);
        self.follow_set_changes();
    }
}

/// Runs the gadget until the client or the network shuts down.
pub async fn run<B, BE, C, N>(params: GadgetParams<C, N>)
where
    B: BlockT<Hash = H256>,
    B::Header: HeaderT<Number = u32>,
    BE: Backend<B>,
    C: HeaderBackend<B>
        + HeaderMetadata<B, Error = sp_blockchain::Error>
        + AuxStore
        + Finalizer<B, BE>
        + BlockchainEvents<B>
        + Send
        + Sync
        + 'static,
    N: NetworkPeers + Send + Sync + ?Sized,
{
    let GadgetParams {
        client,
        tracker,
        key,
        mut notifications,
        network,
        reporter,
        slot_ms,
        prometheus,
    } = params;
    let metrics = prometheus.as_ref().and_then(|registry| {
        Metrics::register(registry)
            .inspect_err(|e| log::warn!(target: LOG_TARGET, "cannot register metrics: {e}"))
            .ok()
    });
    let Some(state) = tracker.lock().ok().map(|t| t.clone()) else {
        log::error!(target: LOG_TARGET, "AC-BFT tracker unavailable; finality gadget not started");
        return;
    };
    let genesis = client.info().genesis_hash;
    let placeholder = Voter::new(
        Config {
            set_id: state.set_id,
            authorities: state.authorities.clone(),
            me: None,
            slot_ms,
        },
        None,
        state.finalized,
    );
    let mut gadget = Gadget::<B, BE, C, N> {
        client: client.clone(),
        tracker,
        key,
        network,
        reporter,
        slot_ms,
        metrics,
        genesis,
        voter: placeholder,
        filter: MessageFilter::new(genesis, state.set_id, state.authorities.clone()),
        peers: HashMap::new(),
        imported_at: BTreeMap::new(),
        start: Instant::now(),
        _marker: std::marker::PhantomData,
    };
    gadget.voter = gadget.build_voter(state.set_id, state.authorities);
    log::info!(
        target: LOG_TARGET,
        "AC-BFT started: set {}, round {}, {}",
        gadget.voter.config().set_id,
        gadget.voter.round(),
        if gadget.voter.config().me.is_some() { "voting" } else { "following" }
    );

    let mut imports = client.import_notification_stream();
    let mut finality = client.finality_notification_stream();
    loop {
        let wait = gadget
            .voter
            .next_deadline()
            .map_or(Duration::from_secs(3600), |d| {
                Duration::from_millis(d.saturating_sub(gadget.now()))
            });
        tokio::select! {
            imported = imports.next() => match imported {
                Some(n) => gadget.on_imported(*n.header.number()),
                None => break,
            },
            finalized = finality.next() => match finalized {
                Some(f) => gadget.on_finalized(BlockRef { hash: f.hash, number: *f.header.number() }),
                None => break,
            },
            event = notifications.next_event() => match event {
                Some(event) => gadget.on_event(event, &mut notifications),
                None => break,
            },
            () = tokio::time::sleep(wait) => gadget.on_tick(),
        }
    }
    log::info!(target: LOG_TARGET, "AC-BFT stopped");
}

/// The AC-BFT notification protocol for a node's network configuration: flooding among up to
/// 25 inbound and 25 outbound peers, any peer accepted.
pub fn notification_protocol<B, N>(
    genesis: &H256,
    metrics: sc_network::NotificationMetrics,
    peer_store: Arc<dyn sc_network::peer_store::PeerStoreProvider>,
) -> (N::NotificationProtocolConfig, Box<dyn NotificationService>)
where
    B: BlockT<Hash = H256>,
    N: sc_network::NetworkBackend<B, H256>,
{
    N::notification_config(
        crate::network::protocol_name(genesis).into(),
        Vec::new(),
        crate::network::MAX_MESSAGE_SIZE,
        None,
        sc_network::config::SetConfig {
            in_peers: 25,
            out_peers: 25,
            reserved_nodes: Vec::new(),
            non_reserved_mode: sc_network::config::NonReservedPeerMode::Accept,
        },
        metrics,
        peer_store,
    )
}
