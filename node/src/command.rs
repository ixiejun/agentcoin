//! Command dispatch.

use sc_cli::SubstrateCli;
use sc_service::PartialComponents;

use ac_crypto::keystore::{EncryptedSecret, SecretKind};
use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_primitives::aura_pq::AUTHORITY_ALG;

use crate::{
    chain_spec,
    cli::{Cli, PqKeyCmd, Subcommand},
    keys, service,
};

/// Runs `pq-key` subcommands.
fn pq_key(cmd: &PqKeyCmd) -> Result<(), String> {
    match cmd {
        PqKeyCmd::Generate {
            output,
            password_file,
        } => {
            if output.exists() {
                return Err(format!(
                    "{} already exists; refusing to overwrite",
                    output.display()
                ));
            }
            let password = keys::read_password(password_file)?;
            let mut rng = ac_crypto::OsRng::new().map_err(|e| e.to_string())?;
            let seed = SecretSeed::generate(&mut rng);
            let public = SigningKey::from_seed(AUTHORITY_ALG, &seed)
                .and_then(|k| k.public_key())
                .map_err(|e| e.to_string())?;
            let file = EncryptedSecret::encrypt(
                seed.expose(),
                SecretKind::SigningSeed,
                Some(&public),
                &password,
                &mut rng,
            )
            .map_err(|e| e.to_string())?;
            std::fs::write(output, file.to_json())
                .map_err(|e| format!("cannot write {}: {e}", output.display()))?;
            println!("0x{}", hex_encode(&public.to_canonical()));
            Ok(())
        }
        PqKeyCmd::Inspect { file } => {
            let json = std::fs::read_to_string(file)
                .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let parsed = EncryptedSecret::from_json(&json).map_err(|e| e.to_string())?;
            let public = parsed.public_key().ok_or("not a signing key file")?;
            println!("0x{}", hex_encode(&public.to_canonical()));
            Ok(())
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl SubstrateCli for Cli {
    fn impl_name() -> String {
        "AgentCoin Node".into()
    }

    fn impl_version() -> String {
        env!("SUBSTRATE_CLI_IMPL_VERSION").into()
    }

    fn description() -> String {
        env!("CARGO_PKG_DESCRIPTION").into()
    }

    fn author() -> String {
        env!("CARGO_PKG_AUTHORS").into()
    }

    fn support_url() -> String {
        "https://github.com/ixiejun/agentcoin/issues".into()
    }

    fn copyright_start_year() -> i32 {
        2026
    }

    fn load_spec(&self, id: &str) -> Result<Box<dyn sc_service::ChainSpec>, String> {
        Ok(match id {
            "dev" => Box::new(chain_spec::development()?),
            "" | "local" => Box::new(chain_spec::local_testnet()?),
            path => Box::new(chain_spec::ChainSpec::from_json_file(
                std::path::PathBuf::from(path),
            )?),
        })
    }
}

/// Parses the command line and runs the node or a subcommand.
///
/// # Errors
///
/// Returns any error of the selected command.
pub fn run() -> sc_cli::Result<()> {
    let cli = Cli::from_args();

    match &cli.subcommand {
        Some(Subcommand::PqKey(cmd)) => pq_key(cmd).map_err(sc_cli::Error::Input),
        Some(Subcommand::ExportChainSpec(cmd)) => {
            let chain_spec = cli.load_spec(&cmd.chain)?;
            cmd.run(chain_spec)
        }
        Some(Subcommand::CheckBlock(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    import_queue,
                    ..
                } = service::new_partial(&config)?;
                Ok((cmd.run(client, import_queue), task_manager))
            })
        }
        Some(Subcommand::ExportBlocks(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    ..
                } = service::new_partial(&config)?;
                Ok((cmd.run(client, config.database), task_manager))
            })
        }
        Some(Subcommand::ExportState(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    ..
                } = service::new_partial(&config)?;
                Ok((cmd.run(client, config.chain_spec), task_manager))
            })
        }
        Some(Subcommand::ImportBlocks(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    import_queue,
                    ..
                } = service::new_partial(&config)?;
                Ok((cmd.run(client, import_queue), task_manager))
            })
        }
        Some(Subcommand::PurgeChain(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.sync_run(|config| cmd.run(config.database))
        }
        Some(Subcommand::Revert(cmd)) => {
            let runner = cli.create_runner(cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    backend,
                    ..
                } = service::new_partial(&config)?;
                Ok((cmd.run(client, backend, None), task_manager))
            })
        }
        None => {
            let runner = cli.create_runner(&cli.run)?;
            let dev_flag = cli.run.shared_params.is_dev();
            let key_args = cli.pq.clone();
            runner.run_node_until_exit(|config| async move {
                let key = keys::resolve(&key_args, &config.chain_spec.chain_type(), dev_flag)
                    .map_err(sc_cli::Error::Input)?;
                match config.network.network_backend {
                    sc_network::config::NetworkBackendType::Libp2p => {
                        service::new_full::<sc_network::NetworkWorker<_, _>>(config, key)
                            .map_err(sc_cli::Error::Service)
                    }
                    sc_network::config::NetworkBackendType::Litep2p => {
                        service::new_full::<sc_network::Litep2pNetworkBackend>(config, key)
                            .map_err(sc_cli::Error::Service)
                    }
                }
            })
        }
    }
}
