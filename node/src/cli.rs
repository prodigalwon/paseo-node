use std::path::PathBuf;

#[derive(Debug, clap::Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum Subcommand {
	#[command(subcommand)]
	Key(sc_cli::KeySubcommand),

	#[deprecated(note = "build-spec will be removed. Use export-chain-spec instead")]
	BuildSpec(sc_cli::BuildSpecCmd),

	ExportChainSpec(sc_cli::ExportChainSpecCmd),
	CheckBlock(sc_cli::CheckBlockCmd),
	ExportBlocks(sc_cli::ExportBlocksCmd),
	ExportState(sc_cli::ExportStateCmd),
	ImportBlocks(sc_cli::ImportBlocksCmd),
	PurgeChain(cumulus_client_cli::PurgeChainCmd),
	Revert(sc_cli::RevertCmd),

	/// Export parachain genesis head (submitted to relay registrar).
	ExportGenesisHead(cumulus_client_cli::ExportGenesisHeadCommand),
	/// Export runtime wasm blob (submitted to relay registrar).
	ExportGenesisWasm(cumulus_client_cli::ExportGenesisWasmCommand),

	#[command(subcommand)]
	Benchmark(frame_benchmarking_cli::BenchmarkCmd),

	ChainInfo(sc_cli::ChainInfoCmd),
}

#[derive(Debug, clap::Parser)]
#[command(
	propagate_version = true,
	args_conflicts_with_subcommands = true,
	subcommand_negates_reqs = true
)]
pub struct Cli {
	#[command(subcommand)]
	pub subcommand: Option<Subcommand>,

	#[command(flatten)]
	pub run: cumulus_client_cli::RunCmd,

	/// Disable automatic hardware benchmarks on startup.
	#[arg(long)]
	pub no_hardware_benchmarks: bool,

	/// Relay chain args (after `--`), e.g. `-- --chain paseo
	/// --relay-chain-rpc-urls wss://rpc.substrate.node`.
	#[arg(raw = true)]
	pub relay_chain_args: Vec<String>,
}

/// Wrapper around `polkadot_cli::RunCmd` so we can impl `SubstrateCli`
/// + `CliConfiguration` for the relay-chain side of the collator.
#[derive(Debug)]
pub struct RelayChainCli {
	/// The actual relay chain cli object.
	pub base: polkadot_cli::RunCmd,
	/// Optional chain id passed to the relay chain.
	pub chain_id: Option<String>,
	/// Base path the relay chain uses (derived from para base path).
	pub base_path: Option<PathBuf>,
}

impl RelayChainCli {
	/// Parse relay chain CLI args using the paraconfig's spec extension.
	pub fn new<'a>(
		para_config: &sc_service::Configuration,
		relay_chain_args: impl Iterator<Item = &'a String>,
	) -> Self {
		let extension = crate::chain_spec::Extensions::try_get(&*para_config.chain_spec);
		let chain_id = extension.map(|e| e.relay_chain.clone());
		let base_path = para_config.base_path.path().join("polkadot");
		Self {
			base_path: Some(base_path),
			chain_id,
			base: clap::Parser::parse_from(relay_chain_args),
		}
	}
}
