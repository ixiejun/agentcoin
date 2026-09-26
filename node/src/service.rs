//! Node service: client, networking, RPC and block production.
//!
//! Block production is temporarily driven by manual sealing on a 1 s timer; Aura-PQ replaces it
//! in m1-pq-chain task 7.1.

use std::{sync::Arc, time::Duration};

use ac_runtime::{RuntimeApi, opaque::Block};
use sc_executor::WasmExecutor;
use sc_service::{Configuration, TaskManager, error::Error as ServiceError};
use sc_telemetry::{Telemetry, TelemetryWorker};
use sp_runtime::traits::Block as BlockT;

type HostFunctions = sp_io::SubstrateHostFunctions;

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

/// Block time of the temporary manual-seal driver.
const BLOCK_TIME: Duration = Duration::from_millis(ac_runtime::MILLISECS_PER_BLOCK);

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

    let import_queue = sc_consensus_manual_seal::import_queue(
        Box::new(client.clone()),
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

/// Builds and starts a full node.
///
/// # Errors
///
/// Fails if any service component cannot be started.
pub fn new_full<Network: sc_network::NetworkBackend<Block, <Block as BlockT>::Hash>>(
    config: Configuration,
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
        sync_service,
        config,
        telemetry: telemetry.as_mut(),
        tracing_execute_block: None,
    })?;

    if is_authority {
        let proposer = sc_basic_authorship::ProposerFactory::new(
            task_manager.spawn_handle(),
            client.clone(),
            transaction_pool.clone(),
            prometheus_registry.as_ref(),
            telemetry.as_ref().map(|x| x.handle()),
        );

        let (mut sink, commands_stream) = futures::channel::mpsc::channel(1024);
        task_manager
            .spawn_handle()
            .spawn("block-timer", None, async move {
                loop {
                    futures_timer_delay(BLOCK_TIME).await;
                    let command = sc_consensus_manual_seal::EngineCommand::SealNewBlock {
                        create_empty: true,
                        finalize: false,
                        parent_hash: None,
                        sender: None,
                    };
                    // The receiver only disappears when the node shuts down.
                    if sink.try_send(command).is_err() {
                        break;
                    }
                }
            });

        let params = sc_consensus_manual_seal::ManualSealParams {
            block_import: client.clone(),
            env: proposer,
            client,
            pool: transaction_pool,
            select_chain,
            commands_stream: Box::pin(commands_stream),
            consensus_data_provider: None,
            create_inherent_data_providers: move |_, ()| async move {
                Ok(sp_timestamp::InherentDataProvider::from_system_time())
            },
        };
        task_manager.spawn_essential_handle().spawn_blocking(
            "manual-seal",
            None,
            sc_consensus_manual_seal::run_manual_seal(params),
        );
    }

    Ok(task_manager)
}

async fn futures_timer_delay(duration: Duration) {
    tokio::time::sleep(duration).await;
}
