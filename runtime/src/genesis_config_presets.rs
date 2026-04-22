use crate::{AccountId, BalancesConfig, RuntimeGenesisConfig};
use alloc::{vec, vec::Vec};
use cumulus_primitives_core::ParaId;
use frame_support::build_struct_json_patch;
use serde_json::Value;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_core::Pair;
use sp_genesis_builder::{self, PresetId};
use sp_keyring::Sr25519Keyring;

/// One token (12 decimal places).
const UNIT: u128 = 1_000_000_000_000;

/// TODO: replace with the real parachain ID before submitting the
/// parachain registration extrinsic on Paseo. `2000` is a dev
/// placeholder — Paseo's registrar will reject it if anything else
/// has already reserved that slot.
pub const PARA_ID_PLACEHOLDER: u32 = 2000;

/// Returns the genesis config preset populated with given parameters.
fn testnet_genesis(
	initial_authorities: Vec<AuraId>,
	endowed_accounts: Vec<AccountId>,
	root: AccountId,
	para_id: ParaId,
) -> Value {
	build_struct_json_patch!(RuntimeGenesisConfig {
		balances: BalancesConfig {
			balances: endowed_accounts
				.iter()
				.cloned()
				.map(|k| (k, 1u128 << 60))
				.collect::<Vec<_>>(),
		},
		aura: pallet_aura::GenesisConfig {
			authorities: initial_authorities.clone(),
		},
		parachain_info: parachain_info::GenesisConfig {
			parachain_id: para_id,
			..Default::default()
		},
		// `pallet-sudo` removed. PNS admin path is via the
		// `PnsCustodian` multisig account (see `configs/mod.rs`).
		// Runtime upgrades go through Paseo relay-chain governance
		// via `parachain-registrar::force_schedule_code_upgrade`.
		// ── PNS bootstrap ──
		// Mint the native TLD base node NFT (class 0) to the initial
		// basenode owner (dev preset: Alice; prod: the custodian).
		pns_nft: pns_registrar::nft::GenesisConfig {
			tokens: vec![(
				root.clone(),
				vec![],
				(),
				vec![(
					root.clone(),
					vec![],
					pns_types::Record::default(),
					pns_types::NATIVE_BASENODE,
				)],
			)],
		},
		// Registration and renewal fees indexed by label length.
		pns_price_oracle: pns_registrar::price_oracle::GenesisConfig {
			base_prices: [
				1000 * UNIT, // 1 char
				100 * UNIT,  // 2 chars
				45 * UNIT,   // 3 chars
				25 * UNIT,   // 4 chars
				10 * UNIT,   // 5 chars
				UNIT / 2,    // 6 chars
				UNIT / 2,    // 7 chars
				UNIT / 2,    // 8 chars
				UNIT / 2,    // 9 chars
				UNIT / 2,    // 10 chars
				UNIT / 2,    // 11+ chars
			],
			rent_prices: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
			init_rate: 1,
		},
		// Set the official account.
		pns_registry: pns_registrar::registry::GenesisConfig {
			official: Some(root.clone()),
			origin: vec![],
		},
		pns_registrar: pns_registrar::registrar::GenesisConfig {
			infos: Default::default(),
			reserved_list: Default::default(),
			reserved_names: vec![
				b"polkadot".to_vec(),
				b"kusama".to_vec(),
				b"paseo".to_vec(),
				b"westend".to_vec(),
				b"fellowship".to_vec(),
				b"hub".to_vec(),
				b"polkadothub".to_vec(),
				b"assethub".to_vec(),
				b"collectives".to_vec(),
				b"pusd".to_vec(),
				b"pop".to_vec(),
				b"revive".to_vec(),
				b"jam".to_vec(),
				b"people".to_vec(),
				b"dap".to_vec(),
			],
		},
		// ZkPki, SecretSquirrel, Scheduler, Authorship, ParachainSystem,
		// AuraExt, MessageQueue, XcmpQueue, PolkadotXcm, CumulusXcm
		// have no non-default genesis state.
	})
}

/// Return the development genesis config.
pub fn development_config_genesis() -> Value {
	testnet_genesis(
		vec![aura_key_from_seed("Alice")],
		vec![
			Sr25519Keyring::Alice.to_account_id(),
			Sr25519Keyring::Bob.to_account_id(),
			Sr25519Keyring::AliceStash.to_account_id(),
			Sr25519Keyring::BobStash.to_account_id(),
		],
		Sr25519Keyring::Alice.to_account_id(),
		PARA_ID_PLACEHOLDER.into(),
	)
}

/// Return the local genesis config preset.
pub fn local_config_genesis() -> Value {
	testnet_genesis(
		vec![
			aura_key_from_seed("Alice"),
			aura_key_from_seed("Bob"),
		],
		Sr25519Keyring::iter()
			.filter(|v| v != &Sr25519Keyring::One && v != &Sr25519Keyring::Two)
			.map(|v| v.to_account_id())
			.collect::<Vec<_>>(),
		Sr25519Keyring::Alice.to_account_id(),
		PARA_ID_PLACEHOLDER.into(),
	)
}

/// Generate an `AuraId` (sr25519::Public) from a seed string.
fn aura_key_from_seed(s: &str) -> AuraId {
	use alloc::format;
	sp_core::sr25519::Pair::from_string(&format!("//{s}"), None).unwrap().public().into()
}

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
	let patch = match id.as_ref() {
		sp_genesis_builder::DEV_RUNTIME_PRESET => development_config_genesis(),
		sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET => local_config_genesis(),
		_ => return None,
	};
	Some(
		serde_json::to_string(&patch)
			.expect("serialization to json is expected to work. qed.")
			.into_bytes(),
	)
}

/// List of supported presets.
pub fn preset_names() -> Vec<PresetId> {
	vec![
		PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET),
		PresetId::from(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET),
	]
}
