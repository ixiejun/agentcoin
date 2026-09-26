//! Node service: client, networking, RPC, Aura-PQ block production and AC-BFT finality.
//!
//! Blocks are imported through [`AcBftBlockImport`], which verifies and applies finality
//! proofs; the AC-BFT gadget votes (validators) or follows (full nodes); double signing seen by
//! the Aura-PQ verifier or the gadget is reported on chain through the local transaction pool;
//! block authors supply their commit–reveal randomness secrets as inherent data.

use std::sync::{Arc, Mutex};

use ac_consensus_aura_pq::{
    EquivocationReporter, ImportQueueParams, SlotProvider, StartAuraPqParams,
};
use ac_consensus_bft::gadget::{self, GadgetParams, Reporter};
use ac_consensus_bft::import::{AcBftBlockImport, SharedTracker};
use ac_consensus_bft::tracker::{self, TrackerState};
use ac_primitives::ac_bft::BlockRef;
use ac_primitives::aura_pq::AuraPqApi;
use ac_primitives::offences::{Evidence, OffencesApi};
use ac_primitives::randomness::{INHERENT_IDENTIFIER, InherentSecrets};
use ac_primitives::validator_set::ValidatorSetApi;
use ac_runtime::{RuntimeApi, opaque::Block};
use sc_consensus_slots::SlotProportion;
use sc_executor::WasmExecutor;
use sc_service::{Configuration, SpawnTaskHandle, TaskManager, error::Error as ServiceError};
use sc_telemetry::{Telemetry, TelemetryWorker};
use sc_transaction_pool_api::{TransactionPool, TransactionSource};
use sp_api::ProvideRuntimeApi;
use sp_blockchain::HeaderBackend;
use sp_consensus_slots::SlotDuration;
use sp_runtime::traits::Block as BlockT;

use crate::keys::ValidatorKey;

#[cfg(not(feature = "runtime-benchmarks"))]
type HostFunctions = sp_io::SubstrateHostFunctions;
#[cfg(feature = "runtime-benchmarks")]
type HostFunctions = (
    sp_io::SubstrateHostFunctions,
    frame_benchmarking::benchmarking::HostFunctions,
);

/// Full client type.
pub type FullClient = sc_service::TFullClient<Block, RuntimeApi, WasmExecutor<HostFunctions>>;
type FullBackend = sc_service::TFullBackend<Block>;
type FullSelectChain = sc_consensus::LongestChain<FullBackend, Block>;
type FullPool = sc_transaction_pool::TransactionPoolHandle<Block, FullClient>;
type FullBlockImport = AcBftBlockImport<Block, FullBackend, Arc<FullClient>, FullClient>;

/// Node-specific parts built by [`new_partial`].
pub struct Extra {
    telemetry: Option<Telemetry>,
    block_import: FullBlockImport,
    tracker: SharedTracker,
    reporter: Arc<PoolReporter>,
}

/// Components shared by the node and the maintenance subcommands.
pub type Service = sc_service::PartialComponents<
    FullClient,
    FullBackend,
    FullSelectChain,
    sc_consensus::DefaultImportQueue<Block>,
    FullPool,
    Extra,
>;

/// Submits double-signing reports through the local transaction pool (task 7.6 of
/// `m2-finality`). The runtime checks the evidence and builds the unsigned report; invalid or
/// already recorded evidence yields nothing.
pub struct PoolReporter {
    client: Arc<FullClient>,
    pool: Arc<FullPool>,
    spawner: SpawnTaskHandle,
}

impl PoolReporter {
    fn submit(&self, evidence: Evidence) {
        let kind = match &evidence {
            Evidence::AuraEquivocation { .. } => "block seal",
            Evidence::BftEquivocation { .. } => "AC-BFT vote",
        };
        let best = self.client.info().best_hash;
        let offender = self.offender(best, &evidence);
        let xt = match self.client.runtime_api().report_extrinsic(best, evidence) {
            Ok(Some(xt)) => xt,
            Ok(None) => {
                // One offence per offender and set: a later one is refused, as the outcome is
                // already decided.
                if offender.is_some_and(|o| self.recorded_in_current_set(best, &o)) {
                    log::debug!(target: "ac-offences", "{kind} double signing not reported: the offender is already recorded in this set");
                } else {
                    log::debug!(target: "ac-offences", "{kind} double-signing evidence not reportable (stale or invalid)");
                }
                return;
            }
            Err(e) => {
                log::warn!(target: "ac-offences", "cannot build a {kind} double-signing report: {e}");
                return;
            }
        };
        let pool = self.pool.clone();
        self.spawner.spawn("ac-offence-report", None, async move {
            match pool.submit_one(best, TransactionSource::Local, xt).await {
                Ok(hash) => {
                    log::info!(target: "ac-offences", "reported {kind} double signing in {hash:?}")
                }
                Err(e) => {
                    log::warn!(target: "ac-offences", "{kind} double-signing report rejected: {e}")
                }
            }
        });
    }
}

impl PoolReporter {
    /// The key that signed both items of `evidence`, if its set is still known.
    fn offender(
        &self,
        at: <Block as BlockT>::Hash,
        evidence: &Evidence,
    ) -> Option<ac_crypto::PqPublicKey> {
        match evidence {
            Evidence::AuraEquivocation { offender, .. } => Some(offender.clone()),
            Evidence::BftEquivocation { first, .. } => self
                .client
                .runtime_api()
                .historical_set(at, first.set_id)
                .ok()??
                .get(usize::from(first.signer))
                .map(|a| a.key.clone()),
        }
    }

    fn recorded_in_current_set(
        &self,
        at: <Block as BlockT>::Hash,
        offender: &ac_crypto::PqPublicKey,
    ) -> bool {
        let api = self.client.runtime_api();
        api.authority_set(at)
            .and_then(|(set_id, _)| api.offences(at, set_id))
            .is_ok_and(|recorded| recorded.iter().any(|(who, _)| who == offender))
    }
}

impl EquivocationReporter for PoolReporter {
    fn report(&self, evidence: Evidence) {
        self.submit(evidence);
    }
}

impl Reporter for PoolReporter {
    fn report(&self, evidence: Evidence) {
        self.submit(evidence);
    }
}

/// Loads the AC-BFT tracker, or starts it from the genesis authority set.
fn load_tracker(client: &FullClient) -> Result<TrackerState, ServiceError> {
    if let Some(state) = tracker::load(client)? {
        return Ok(state);
    }
    let info = client.info();
    let (set_id, authorities) = client
        .runtime_api()
        .authority_set(info.genesis_hash)
        .map_err(|e| ServiceError::Other(format!("cannot read the genesis authority set: {e}")))?;
    let mut state = TrackerState::genesis(
        BlockRef {
            hash: info.genesis_hash,
            number: 0,
        },
        authorities,
    );
    state.set_id = set_id;
    Ok(state)
}

/// Reads the slot duration from the runtime at the best block.
fn slot_duration(client: &FullClient) -> Result<SlotDuration, ServiceError> {
    let best = client.info().best_hash;
    let millis = client
        .runtime_api()
        .slot_duration(best)
        .map_err(|e| ServiceError::Other(format!("cannot read the slot duration: {e}")))?;
    Ok(SlotDuration::from_millis(millis))
}

/// Inherent data providers used by Aura-PQ: the slot first (required by the slot machinery),
/// then the timestamp.
type AuraPqInherentProviders = (SlotProvider, sp_timestamp::InherentDataProvider);
type InherentProvidersResult =
    Result<AuraPqInherentProviders, Box<dyn std::error::Error + Send + Sync>>;

/// Supplies the author's commit–reveal secrets for the block's epoch and the previous one.
/// Never logged: [`InherentSecrets`] has a redacted `Debug`.
pub struct RandomnessProvider(Option<InherentSecrets>);

#[async_trait::async_trait]
impl sp_inherents::InherentDataProvider for RandomnessProvider {
    async fn provide_inherent_data(
        &self,
        inherent_data: &mut sp_inherents::InherentData,
    ) -> Result<(), sp_inherents::Error> {
        match &self.0 {
            Some(secrets) => inherent_data.put_data(INHERENT_IDENTIFIER, secrets),
            None => Ok(()),
        }
    }

    async fn try_handle_error(
        &self,
        _: &sp_inherents::InherentIdentifier,
        _: &[u8],
    ) -> Option<Result<(), sp_inherents::Error>> {
        None
    }
}

/// The secrets of `key` for the epoch of the block built on `parent` (task 7.7 of
/// `m2-finality`). `None` if the epoch cannot be determined; the block then carries no commit
/// or reveal, which only costs this author its contribution.
fn randomness_secrets(
    client: &FullClient,
    key: &ValidatorKey,
    parent: <Block as BlockT>::Hash,
) -> Option<InherentSecrets> {
    let parent_number = client.number(parent).ok()??;
    let length = client.runtime_api().epoch_length(parent).ok()?;
    let epoch = ac_primitives::epoch::epoch_of(u64::from(parent_number).checked_add(1)?, length)?;
    let genesis = client.info().genesis_hash.to_fixed_bytes();
    let current = *key.randomness_secret(&genesis, epoch).ok()?.expose();
    let previous = match epoch.checked_sub(1) {
        Some(previous) => Some(*key.randomness_secret(&genesis, previous).ok()?.expose()),
        None => None,
    };
    Some(InherentSecrets {
        epoch,
        current,
        previous,
    })
}

/// Inherent data providers for authoring: the import providers plus the randomness secrets.
type AuthoringProvidersResult = Result<
    (
        SlotProvider,
        sp_timestamp::InherentDataProvider,
        RandomnessProvider,
    ),
    Box<dyn std::error::Error + Send + Sync>,
>;

/// Creates the inherent data providers for authoring.
fn authoring_providers(
    slot_duration: SlotDuration,
    client: Arc<FullClient>,
    key: Arc<ValidatorKey>,
) -> impl Fn(<Block as BlockT>::Hash, ()) -> futures::future::Ready<AuthoringProvidersResult>
+ Clone
+ Send
+ Sync
+ 'static {
    move |parent, ()| {
        let timestamp = sp_timestamp::InherentDataProvider::from_system_time();
        let slot = SlotProvider::from_timestamp(*timestamp, slot_duration);
        let randomness = RandomnessProvider(randomness_secrets(&client, &key, parent));
        futures::future::ready(Ok((slot, timestamp, randomness)))
    }
}

/// Creates the inherent data providers for import.
fn inherent_providers(
    slot_duration: SlotDuration,
) -> impl Fn(<Block as BlockT>::Hash, ()) -> futures::future::Ready<InherentProvidersResult>
+ Clone
+ Send
+ Sync
+ 'static {
    move |_, ()| {
        let timestamp = sp_timestamp::InherentDataProvider::from_system_time();
        let slot = SlotProvider::from_timestamp(*timestamp, slot_duration);
        futures::future::ready(Ok((slot, timestamp)))
    }
}

/// Builds the client, backend, import queue and transaction pool.
///
/// # Errors
///
/// Fails if the database or the runtime executor cannot be set up.
pub fn new_partial(config: &Configuration) -> Result<Service, ServiceError> {
    let telemetry = config
        .telemetry_endpoints
        .clone()
        .filter(|x| !x.is_empty())
        .map(|endpoints| -> Result<_, sc_telemetry::Error> {
            let worker = TelemetryWorker::new(16)?;
            let telemetry = worker.handle().new_telemetry(endpoints);
            Ok((worker, telemetry))
        })
        .transpose()?;

    let executor = sc_service::new_wasm_executor(&config.executor);
    let (client, backend, keystore_container, task_manager) =
        sc_service::new_full_parts::<Block, RuntimeApi, _>(
            config,
            telemetry.as_ref().map(|(_, telemetry)| telemetry.handle()),
            executor,
            Default::default(),
        )?;
    let client = Arc::new(client);

    let telemetry = telemetry.map(|(worker, telemetry)| {
        task_manager
            .spawn_handle()
            .spawn("telemetry", None, worker.run());
        telemetry
    });

    let select_chain = sc_consensus::LongestChain::new(backend.clone());

    let transaction_pool = Arc::from(
        sc_transaction_pool::Builder::new(
            task_manager.spawn_essential_handle(),
            client.clone(),
            config.role.is_authority().into(),
        )
        .with_options(config.transaction_pool.clone())
        .with_prometheus(config.prometheus_registry())
        .build(),
    );

    let tracker: SharedTracker = Arc::new(Mutex::new(load_tracker(&client)?));
    let block_import = AcBftBlockImport::new(client.clone(), client.clone(), tracker.clone());
    let reporter = Arc::new(PoolReporter {
        client: client.clone(),
        pool: transaction_pool.clone(),
        spawner: task_manager.spawn_handle(),
    });

    let import_queue = ac_consensus_aura_pq::import_queue(ImportQueueParams {
        client: client.clone(),
        block_import: Box::new(block_import.clone()),
        justification_import: Some(Box::new(block_import.clone())),
        create_inherent_data_providers: inherent_providers(slot_duration(&client)?),
        reporter: Some(reporter.clone()),
        spawner: &task_manager.spawn_essential_handle(),
        registry: config.prometheus_registry(),
    });

    Ok(sc_service::PartialComponents {
        client,
        backend,
        task_manager,
        import_queue,
        keystore_container,
        select_chain,
        transaction_pool,
        other: Extra {
            telemetry,
            block_import,
            tracker,
            reporter,
        },
    })
}

/// Builds and starts a full node; `authority_key` is the local validator key, used for block
/// seals, AC-BFT votes and randomness alike. A node with a key votes in AC-BFT whenever the key
/// is in the current authority set; a node without one only follows finality.
///
/// # Errors
///
/// Fails if any service component cannot be started, if the chain has no Aura-PQ authorities,
/// or if the node runs as an authority without a key.
pub fn new_full<Network: sc_network::NetworkBackend<Block, <Block as BlockT>::Hash>>(
    config: Configuration,
    authority_key: Option<ValidatorKey>,
) -> Result<TaskManager, ServiceError> {
    let sc_service::PartialComponents {
        client,
        backend,
        mut task_manager,
        import_queue,
        keystore_container,
        select_chain,
        transaction_pool,
        other:
            Extra {
                mut telemetry,
                block_import,
                tracker,
                reporter,
            },
    } = new_partial(&config)?;

    // An empty authority set means no block can ever be authored (pallet-aura-pq accepts it only
    // as the SDK's default genesis); refuse to run such a chain (design D6).
    let best = client.info().best_hash;
    let authorities = client
        .runtime_api()
        .authorities(best)
        .map_err(|e| ServiceError::Other(format!("cannot read Aura-PQ authorities: {e}")))?;
    if authorities.is_empty() {
        return Err(ServiceError::Other(
            "the chain has no Aura-PQ authorities; refusing to start".into(),
        ));
    }

    let mut net_config = sc_network::config::FullNetworkConfiguration::<
        Block,
        <Block as BlockT>::Hash,
        Network,
    >::new(
        &config.network,
        config
            .prometheus_config
            .as_ref()
            .map(|cfg| cfg.registry.clone()),
    );
    let metrics = Network::register_notification_metrics(
        config.prometheus_config.as_ref().map(|cfg| &cfg.registry),
    );
    let (acbft_config, acbft_notifications) = gadget::notification_protocol::<Block, Network>(
        &client.info().genesis_hash,
        metrics.clone(),
        net_config.peer_store_handle(),
    );
    net_config.add_notification_protocol(acbft_config);

    let (network, system_rpc_tx, tx_handler_controller, sync_service) =
        sc_service::build_network(sc_service::BuildNetworkParams {
            config: &config,
            net_config,
            client: client.clone(),
            transaction_pool: transaction_pool.clone(),
            spawn_handle: task_manager.spawn_handle(),
            spawn_essential_handle: task_manager.spawn_essential_handle(),
            import_queue,
            block_announce_validator_builder: None,
            warp_sync_config: None,
            block_relay: None,
            metrics,
        })?;

    let rpc_extensions_builder = {
        let client = client.clone();
        let pool = transaction_pool.clone();
        Box::new(move |_| {
            let deps = crate::rpc::FullDeps {
                client: client.clone(),
                pool: pool.clone(),
            };
            crate::rpc::create_full(deps).map_err(Into::into)
        })
    };

    let prometheus_registry = config.prometheus_registry().cloned();
    let is_authority = config.role.is_authority();
    let force_authoring = config.force_authoring;

    let slot_ms = slot_duration(&client)?.as_millis();
    let authority_key = authority_key.map(Arc::new);
    task_manager.spawn_essential_handle().spawn(
        "ac-bft",
        Some("finality"),
        gadget::run::<Block, FullBackend, _, _>(GadgetParams {
            client: client.clone(),
            tracker,
            key: authority_key.as_ref().map(|k| Arc::new(k.signing.clone())),
            notifications: acbft_notifications,
            network: network.clone(),
            reporter,
            slot_ms,
            prometheus: prometheus_registry.clone(),
        }),
    );

    sc_service::spawn_tasks(sc_service::SpawnTasksParams {
        network,
        client: client.clone(),
        keystore: keystore_container.keystore(),
        task_manager: &mut task_manager,
        transaction_pool: transaction_pool.clone(),
        rpc_builder: rpc_extensions_builder,
        backend,
        system_rpc_tx,
        tx_handler_controller,
        sync_service: sync_service.clone(),
        config,
        telemetry: telemetry.as_mut(),
        tracing_execute_block: None,
    })?;

    if is_authority {
        let key = authority_key.ok_or_else(|| {
            ServiceError::Other(
                "running as an authority requires --pq-key-file or --dev-key".into(),
            )
        })?;
        let proposer = sc_basic_authorship::ProposerFactory::new(
            task_manager.spawn_handle(),
            client.clone(),
            transaction_pool.clone(),
            prometheus_registry.as_ref(),
            telemetry.as_ref().map(|x| x.handle()),
        );
        let slot_duration = slot_duration(&client)?;
        let aura = ac_consensus_aura_pq::start_aura_pq(StartAuraPqParams {
            slot_duration,
            client: client.clone(),
            select_chain,
            block_import,
            proposer_factory: proposer,
            sync_oracle: sync_service.clone(),
            justification_sync_link: sync_service.clone(),
            create_inherent_data_providers: authoring_providers(
                slot_duration,
                client.clone(),
                key.clone(),
            ),
            force_authoring,
            key: key.signing.clone(),
            block_proposal_slot_portion: SlotProportion::new(2f32 / 3f32),
            telemetry: telemetry.as_ref().map(|x| x.handle()),
        })
        .map_err(|e| ServiceError::Other(format!("cannot start Aura-PQ: {e}")))?;
        task_manager.spawn_essential_handle().spawn_blocking(
            "aura-pq",
            Some("block-authoring"),
            aura,
        );
    }

    Ok(task_manager)
}
