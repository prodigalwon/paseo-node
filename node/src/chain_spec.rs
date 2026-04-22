use sc_chain_spec::{ChainSpecExtension, ChainSpecGroup};
use sc_service::ChainType;
use paseo_runtime::WASM_BINARY;
use serde::{Deserialize, Serialize};

/// Chain-spec extensions: the parachain service and collator need the
/// relay chain ID and the parachain ID from the spec before they can
/// connect to the relay. These ride in the `extensions` field of the
/// JSON chain spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ChainSpecGroup, ChainSpecExtension)]
#[serde(deny_unknown_fields)]
pub struct Extensions {
	/// Relay chain id to connect to (e.g. "paseo", "rococo-local").
	pub relay_chain: String,
	/// Parachain ID assigned on the relay.
	pub para_id: u32,
}

impl Extensions {
	/// Read the extensions from a `ChainSpec`.
	pub fn try_get(chain_spec: &dyn sc_service::ChainSpec) -> Option<&Self> {
		sc_chain_spec::get_extension(chain_spec.extensions())
	}
}

pub type ChainSpec = sc_service::GenericChainSpec<Extensions>;

/// TODO: swap this for the reserved parachain ID before Paseo launch.
const PARA_ID_PLACEHOLDER: u32 = 2000;

pub fn development_chain_spec() -> Result<ChainSpec, String> {
	Ok(ChainSpec::builder(
		WASM_BINARY.ok_or_else(|| "Development wasm not available".to_string())?,
		Extensions { relay_chain: "rococo-local".into(), para_id: PARA_ID_PLACEHOLDER },
	)
	.with_name("Development")
	.with_id("dev")
	.with_chain_type(ChainType::Development)
	.with_genesis_config_preset_name(sp_genesis_builder::DEV_RUNTIME_PRESET)
	.build())
}

pub fn local_chain_spec() -> Result<ChainSpec, String> {
	Ok(ChainSpec::builder(
		WASM_BINARY.ok_or_else(|| "Development wasm not available".to_string())?,
		Extensions { relay_chain: "paseo".into(), para_id: PARA_ID_PLACEHOLDER },
	)
	.with_name("Paseo Unified Local")
	.with_id("paseo_unified_local")
	.with_chain_type(ChainType::Local)
	.with_genesis_config_preset_name(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET)
	.build())
}
