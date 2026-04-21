//! Paseo Unified Parachain Node CLI.
#![warn(missing_docs)]

mod benchmarking;
mod chain_spec;
mod cli;
mod command;
mod pns_rpc;
mod rpc;
mod service;

fn main() -> sc_cli::Result<()> {
	command::run()
}
