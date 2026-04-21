#[derive(Debug, clap::Parser)]
pub struct Cli {
	#[command(subcommand)]
	pub subcommand: Option<Subcommand>,

	#[clap(flatten)]
	pub run: sc_cli::RunCmd,
}

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
	PurgeChain(sc_cli::PurgeChainCmd),
	Revert(sc_cli::RevertCmd),

	#[command(subcommand)]
	Benchmark(frame_benchmarking_cli::BenchmarkCmd),

	ChainInfo(sc_cli::ChainInfoCmd),
}
