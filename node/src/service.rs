//! Node service: client, networking, RPC and Aura-PQ block production.
//!
//! There is no finality gadget in M1 (GRANDPA's signatures are hard-wired to Ed25519); fork
//! choice is the longest chain and AC-BFT finality arrives in M2.

use std::sync::Arc;

use ac_consensus_aura_pq::{SlotProvider, StartAuraPqParams};
use ac_crypto::sig::SigningKey;
use ac_primitives::aura_pq::AuraPqApi;
use ac_runtime::{RuntimeApi, opaque::Block};
use sc_consensus_slots::SlotProportion;
use sc_executor::WasmExecutor;
use sc_service::{Configuration, TaskManager, error::Error as ServiceError};
use sc_telemetry::{Telemetry, TelemetryWorker};
use sp_api::ProvideRuntimeApi;
use sp_blockchain::HeaderBackend;
use sp_consensus_slots::SlotDuration;
use sp_runtime::traits::Block as BlockT;

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

/// Components shared by the node and the maintenance subcommands.
pub type Service = sc_service::PartialComponents<
    FullClient,
    FullBackend,
    FullSelectChain,
    sc_consensus::DefaultImportQueue<Block>,
    sc_transaction_pool::TransactionPoolHandle<Block, FullClient>,
    Option<Telemetry>,
>;

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

/// Creates the inherent data providers for authoring and import.
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

    let import_queue = ac_consensus_aura_pq::import_queue(
        client.clone(),
        Box::new(client.clone()),
        inherent_providers(slot_duration(&client)?),
        &task_manager.spawn_essential_handle(),
        config.prometheus_registry(),
    );

    Ok(sc_service::PartialComponents {
        client,
        backend,
        task_manager,
        import_queue,
        keystore_container,
        select_chain,
        transaction_pool,
        other: telemetry,
    })
}

/// Builds and starts a full node; `authority_key` is the local Aura-PQ key of an authority.
///
/// # Errors
///
/// Fails if any service component cannot be started, if the chain has no Aura-PQ authorities,
/// or if the node runs as an authority without a key.
pub fn new_full<Network: sc_network::NetworkBackend<Block, <Block as BlockT>::Hash>>(
    config: Configuration,
    authority_key: Option<SigningKey>,
) -> Result<TaskManager, ServiceError> {
    let sc_service::PartialComponents {
        client,
        backend,
        mut task_manager,
        import_queue,
        keystore_container,
        select_chain,
        transaction_pool,
        other: mut telemetry,
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

    let net_config = sc_network::config::FullNetworkConfiguration::<
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
            block_import: client.clone(),
            proposer_factory: proposer,
            sync_oracle: sync_service.clone(),
            justification_sync_link: sync_service.clone(),
            create_inherent_data_providers: inherent_providers(slot_duration),
            force_authoring,
            key,
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
