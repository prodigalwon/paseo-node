use crate::{
	benchmarking::{inherent_benchmark_data, RemarkBuilder, TransferKeepAliveBuilder},
	chain_spec,
	cli::{Cli, RelayChainCli, Subcommand},
	service,
};
use frame_benchmarking_cli::{BenchmarkCmd, ExtrinsicFactory, SUBSTRATE_REFERENCE_HARDWARE};
use log::info;
use paseo_runtime::{Block, EXISTENTIAL_DEPOSIT};
use sc_cli::{
	ChainSpec, CliConfiguration, DefaultConfigurationValues, ImportParams, KeystoreParams,
	NetworkParams, Result, RpcEndpoint, SharedParams, SubstrateCli,
};
use sc_service::{
	config::{BasePath, PrometheusConfig},
	PartialComponents,
};
use sp_keyring::Sr25519Keyring;
use std::path::PathBuf;

impl SubstrateCli for Cli {
	fn impl_name() -> String { "Paseo Unified Parachain Collator".into() }
	fn impl_version() -> String { env!("SUBSTRATE_CLI_IMPL_VERSION").into() }
	fn description() -> String { env!("CARGO_PKG_DESCRIPTION").into() }
	fn author() -> String { env!("CARGO_PKG_AUTHORS").into() }
	fn support_url() -> String { "https://github.com/prodigalwon".into() }
	fn copyright_start_year() -> i32 { 2025 }

	fn load_spec(&self, id: &str) -> std::result::Result<Box<dyn ChainSpec>, String> {
		Ok(match id {
			"dev" => Box::new(chain_spec::development_chain_spec()?),
			"" | "local" => Box::new(chain_spec::local_chain_spec()?),
			path => Box::new(chain_spec::ChainSpec::from_json_file(PathBuf::from(path))?),
		})
	}
}

impl SubstrateCli for RelayChainCli {
	fn impl_name() -> String { "Paseo Unified — Relay Chain".into() }
	fn impl_version() -> String { env!("SUBSTRATE_CLI_IMPL_VERSION").into() }
	fn description() -> String { "Relay chain arg parser".into() }
	fn author() -> String { env!("CARGO_PKG_AUTHORS").into() }
	fn support_url() -> String { "https://github.com/prodigalwon".into() }
	fn copyright_start_year() -> i32 { 2025 }

	fn load_spec(&self, id: &str) -> std::result::Result<Box<dyn ChainSpec>, String> {
		polkadot_cli::Cli::from_iter([RelayChainCli::executable_name()].iter()).load_spec(id)
	}
}

/// Parse and run command line arguments
pub fn run() -> Result<()> {
	let cli = Cli::from_args();

	match &cli.subcommand {
		Some(Subcommand::Key(cmd)) => cmd.run(&cli),
		#[allow(deprecated)]
		Some(Subcommand::BuildSpec(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.sync_run(|config| cmd.run(config.chain_spec, config.network))
		},
		Some(Subcommand::CheckBlock(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|config| {
				let PartialComponents { client, task_manager, import_queue, .. } =
					service::new_partial(&config)?;
				Ok((cmd.run(client, import_queue), task_manager))
			})
		},
		Some(Subcommand::ExportChainSpec(cmd)) => {
			let chain_spec = cli.load_spec(&cmd.chain)?;
			cmd.run(chain_spec)
		},
		Some(Subcommand::ExportBlocks(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|config| {
				let PartialComponents { client, task_manager, .. } = service::new_partial(&config)?;
				Ok((cmd.run(client, config.database), task_manager))
			})
		},
		Some(Subcommand::ExportState(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|config| {
				let PartialComponents { client, task_manager, .. } = service::new_partial(&config)?;
				Ok((cmd.run(client, config.chain_spec), task_manager))
			})
		},
		Some(Subcommand::ImportBlocks(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|config| {
				let PartialComponents { client, task_manager, import_queue, .. } =
					service::new_partial(&config)?;
				Ok((cmd.run(client, import_queue), task_manager))
			})
		},
		Some(Subcommand::PurgeChain(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.sync_run(|config| {
				let polkadot_cli = RelayChainCli::new(
					&config,
					[RelayChainCli::executable_name()].iter().chain(cli.relay_chain_args.iter()),
				);
				let polkadot_config = SubstrateCli::create_configuration(
					&polkadot_cli,
					&polkadot_cli,
					config.tokio_handle.clone(),
				)
				.map_err(|err| format!("Relay chain argument error: {err}"))?;
				cmd.run(config, polkadot_config)
			})
		},
		Some(Subcommand::Revert(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|config| {
				let PartialComponents { client, task_manager, backend, .. } =
					service::new_partial(&config)?;
				Ok((cmd.run(client, backend, None), task_manager))
			})
		},
		Some(Subcommand::ExportGenesisHead(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.sync_run(|config| {
				let partials = service::new_partial(&config)?;
				cmd.run(partials.client)
			})
		},
		Some(Subcommand::ExportGenesisWasm(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.sync_run(|_config| {
				let spec = cli.load_spec(&cmd.shared_params.chain.clone().unwrap_or_default())?;
				cmd.run(&*spec)
			})
		},
		Some(Subcommand::Benchmark(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.sync_run(|config| {
				match cmd {
					BenchmarkCmd::Pallet(cmd) => {
						if !cfg!(feature = "runtime-benchmarks") {
							return Err("Runtime benchmarking not enabled.".into());
						}
						cmd.run_with_spec::<sp_runtime::traits::HashingFor<Block>, ()>(Some(config.chain_spec))
					},
					BenchmarkCmd::Block(cmd) => {
						let PartialComponents { client, .. } = service::new_partial(&config)?;
						cmd.run(client)
					},
					#[cfg(not(feature = "runtime-benchmarks"))]
					BenchmarkCmd::Storage(_) => Err("Storage benchmarking requires `--features runtime-benchmarks`.".into()),
					#[cfg(feature = "runtime-benchmarks")]
					BenchmarkCmd::Storage(cmd) => {
						let PartialComponents { client, backend, .. } = service::new_partial(&config)?;
						let db = backend.expose_db();
						let storage = backend.expose_storage();
						let shared_cache = backend.expose_shared_trie_cache();
						cmd.run(config, client, db, storage, shared_cache)
					},
					BenchmarkCmd::Overhead(cmd) => {
						let PartialComponents { client, .. } = service::new_partial(&config)?;
						let ext_builder = RemarkBuilder::new(client.clone());
						cmd.run(config.chain_spec.name().into(), client, inherent_benchmark_data()?, Vec::new(), &ext_builder, false)
					},
					BenchmarkCmd::Extrinsic(cmd) => {
						let PartialComponents { client, .. } = service::new_partial(&config)?;
						let ext_factory = ExtrinsicFactory(vec![
							Box::new(RemarkBuilder::new(client.clone())),
							Box::new(TransferKeepAliveBuilder::new(client.clone(), Sr25519Keyring::Alice.to_account_id(), EXISTENTIAL_DEPOSIT)),
						]);
						cmd.run(client, inherent_benchmark_data()?, Vec::new(), &ext_factory)
					},
					BenchmarkCmd::Machine(cmd) => cmd.run(&config, SUBSTRATE_REFERENCE_HARDWARE.clone()),
				}
			})
		},
		Some(Subcommand::ChainInfo(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.sync_run(|config| cmd.run::<Block>(&config))
		},
		None => {
			let runner = cli.create_runner(&cli.run.normalize())?;
			let collator_options = cli.run.collator_options();

			runner.run_node_until_exit(|config| async move {
				let hwbench = (!cli.no_hardware_benchmarks)
					.then(|| {
						config.database.path().map(|database_path| {
							let _ = std::fs::create_dir_all(database_path);
							sc_sysinfo::gather_hwbench(
								Some(database_path),
								&SUBSTRATE_REFERENCE_HARDWARE,
							)
						})
					})
					.flatten();

				let polkadot_cli = RelayChainCli::new(
					&config,
					[RelayChainCli::executable_name()].iter().chain(cli.relay_chain_args.iter()),
				);

				let tokio_handle = config.tokio_handle.clone();
				let polkadot_config = SubstrateCli::create_configuration(
					&polkadot_cli,
					&polkadot_cli,
					tokio_handle,
				)
				.map_err(|err| format!("Relay chain argument error: {err}"))?;

				info!("Is collating: {}", if config.role.is_authority() { "yes" } else { "no" });

				service::start_parachain_node(
					config,
					polkadot_config,
					collator_options,
					hwbench,
				)
				.await
				.map(|r| r.0)
				.map_err(Into::into)
			})
		},
	}
}

impl DefaultConfigurationValues for RelayChainCli {
	fn p2p_listen_port() -> u16 { 30334 }
	fn rpc_listen_port() -> u16 { 9945 }
	fn prometheus_listen_port() -> u16 { 9616 }
}

impl CliConfiguration<Self> for RelayChainCli {
	fn shared_params(&self) -> &SharedParams { self.base.base.shared_params() }
	fn import_params(&self) -> Option<&ImportParams> { self.base.base.import_params() }
	fn network_params(&self) -> Option<&NetworkParams> { self.base.base.network_params() }
	fn keystore_params(&self) -> Option<&KeystoreParams> { self.base.base.keystore_params() }

	fn base_path(&self) -> Result<Option<BasePath>> {
		Ok(self.shared_params().base_path()?.or_else(|| self.base_path.clone().map(Into::into)))
	}

	fn rpc_addr(&self, default_listen_port: u16) -> Result<Option<Vec<RpcEndpoint>>> {
		self.base.base.rpc_addr(default_listen_port)
	}

	fn prometheus_config(
		&self,
		default_listen_port: u16,
		chain_spec: &Box<dyn ChainSpec>,
	) -> Result<Option<PrometheusConfig>> {
		self.base.base.prometheus_config(default_listen_port, chain_spec)
	}

	fn init<F>(&self, _support_url: &String, _impl_version: &String, _logger_hook: F) -> Result<()>
	where
		F: FnOnce(&mut sc_cli::LoggerBuilder),
	{
		unreachable!("PolkadotCli is never initialized; qed");
	}

	fn chain_id(&self, is_dev: bool) -> Result<String> {
		let chain_id = self.base.base.chain_id(is_dev)?;
		Ok(if chain_id.is_empty() { self.chain_id.clone().unwrap_or_default() } else { chain_id })
	}

	fn role(&self, is_dev: bool) -> Result<sc_service::Role> { self.base.base.role(is_dev) }

	fn transaction_pool(&self, is_dev: bool) -> Result<sc_service::config::TransactionPoolOptions> {
		self.base.base.transaction_pool(is_dev)
	}

	fn trie_cache_maximum_size(&self) -> Result<Option<usize>> { self.base.base.trie_cache_maximum_size() }
	fn rpc_methods(&self) -> Result<sc_service::config::RpcMethods> { self.base.base.rpc_methods() }
	fn rpc_max_connections(&self) -> Result<u32> { self.base.base.rpc_max_connections() }
	fn rpc_cors(&self, is_dev: bool) -> Result<Option<Vec<String>>> { self.base.base.rpc_cors(is_dev) }
	fn default_heap_pages(&self) -> Result<Option<u64>> { self.base.base.default_heap_pages() }
	fn force_authoring(&self) -> Result<bool> { self.base.base.force_authoring() }
	fn disable_grandpa(&self) -> Result<bool> { self.base.base.disable_grandpa() }
	fn max_runtime_instances(&self) -> Result<Option<usize>> { self.base.base.max_runtime_instances() }
	fn announce_block(&self) -> Result<bool> { self.base.base.announce_block() }

	fn telemetry_endpoints(
		&self,
		chain_spec: &Box<dyn ChainSpec>,
	) -> Result<Option<sc_telemetry::TelemetryEndpoints>> {
		self.base.base.telemetry_endpoints(chain_spec)
	}

	fn node_name(&self) -> Result<String> { self.base.base.node_name() }
}
