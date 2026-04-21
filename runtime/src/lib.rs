#![cfg_attr(not(feature = "std"), no_std)]

// SECURITY GUARD — `test-attestation` exposes bypass-crypto verifiers
// that accept caller-controlled EK hashes. `production` marks a build
// handling real value. Enabling both simultaneously would ship a
// production runtime with Sybil resistance disabled. See
// `pki/SECURITY.md` for the pre-Kusama checklist.
#[cfg(all(feature = "test-attestation", feature = "production"))]
compile_error!(
    "paseo-runtime: `test-attestation` and `production` features are mutually \
     exclusive. `test-attestation` enables bypass-crypto attestation verifiers \
     (NoopBindingProofVerifier, TpmTestAttestationVerifier) that accept \
     attacker-controlled EK hashes. Enabling it alongside `production` would \
     ship a runtime with no hardware-attestation Sybil resistance. Build with \
     `--no-default-features --features std,production` for a production build."
);

#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

pub mod apis;
#[cfg(feature = "runtime-benchmarks")]
mod benchmarks;
pub mod configs;
pub mod proxy_validator;

extern crate alloc;
use alloc::vec::Vec;
use sp_runtime::{
	generic, impl_opaque_keys,
	traits::{BlakeTwo256, IdentifyAccount, Verify},
	MultiAddress, MultiSignature,
};
#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;

pub use frame_system::Call as SystemCall;
pub use pallet_balances::Call as BalancesCall;
pub use pallet_timestamp::Call as TimestampCall;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;

pub mod genesis_config_presets;

/// Opaque types for the CLI.
pub mod opaque {
	use super::*;
	use sp_runtime::{
		generic,
		traits::{BlakeTwo256, Hash as HashT},
	};

	pub use sp_runtime::OpaqueExtrinsic as UncheckedExtrinsic;

	pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
	pub type Block = generic::Block<Header, UncheckedExtrinsic>;
	pub type BlockId = generic::BlockId<Block>;
	pub type Hash = <BlakeTwo256 as HashT>::Output;
}

impl_opaque_keys! {
	pub struct SessionKeys {
		pub aura: Aura,
	}
}

#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: alloc::borrow::Cow::Borrowed("paseo-runtime"),
	impl_name: alloc::borrow::Cow::Borrowed("paseo-runtime"),
	authoring_version: 1,
	spec_version: 200,
	impl_version: 1,
	apis: apis::RUNTIME_API_VERSIONS,
	transaction_version: 1,
	system_version: 1,
};

mod block_times {
	pub const MILLI_SECS_PER_BLOCK: u64 = 6000;
	pub const SLOT_DURATION: u64 = MILLI_SECS_PER_BLOCK;
}
pub use block_times::*;

pub const MINUTES: BlockNumber = 60_000 / (MILLI_SECS_PER_BLOCK as BlockNumber);
pub const HOURS: BlockNumber = MINUTES * 60;
pub const DAYS: BlockNumber = HOURS * 24;

pub const BLOCK_HASH_COUNT: BlockNumber = 2400;

pub const UNIT: Balance = 1_000_000_000_000;
pub const MILLI_UNIT: Balance = 1_000_000_000;
pub const MICRO_UNIT: Balance = 1_000_000;

pub const EXISTENTIAL_DEPOSIT: Balance = MILLI_UNIT;

#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
	NativeVersion { runtime_version: VERSION, can_author_with: Default::default() }
}

pub type Signature = MultiSignature;
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;
pub type Balance = u128;
pub type Nonce = u32;
pub type Hash = sp_core::H256;
pub type BlockNumber = u32;
pub type Address = MultiAddress<AccountId, ()>;
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
pub type SignedBlock = generic::SignedBlock<Block>;
pub type BlockId = generic::BlockId<Block>;

pub type TxExtension = (
	frame_system::AuthorizeCall<Runtime>,
	frame_system::CheckNonZeroSender<Runtime>,
	frame_system::CheckSpecVersion<Runtime>,
	frame_system::CheckTxVersion<Runtime>,
	frame_system::CheckGenesis<Runtime>,
	frame_system::CheckEra<Runtime>,
	frame_system::CheckNonce<Runtime>,
	frame_system::CheckWeight<Runtime>,
	pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
	frame_metadata_hash_extension::CheckMetadataHash<Runtime>,
	frame_system::WeightReclaim<Runtime>,
);

pub type UncheckedExtrinsic =
	generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, TxExtension>;

pub type SignedPayload = generic::SignedPayload<RuntimeCall, TxExtension>;

// ═══════════════════════════════════════════════════════════════
// construct_runtime! — all pallets
// ═══════════════════════════════════════════════════════════════

#[frame_support::runtime]
mod runtime {
	#[runtime::runtime]
	#[runtime::derive(
		RuntimeCall,
		RuntimeEvent,
		RuntimeError,
		RuntimeOrigin,
		RuntimeFreezeReason,
		RuntimeHoldReason,
		RuntimeSlashReason,
		RuntimeLockId,
		RuntimeTask,
		RuntimeViewFunction
	)]
	pub struct Runtime;

	// ── Standard ──
	#[runtime::pallet_index(0)]
	pub type System = frame_system;

	#[runtime::pallet_index(1)]
	pub type Timestamp = pallet_timestamp;

	#[runtime::pallet_index(2)]
	pub type Aura = pallet_aura;

	#[runtime::pallet_index(3)]
	pub type Grandpa = pallet_grandpa;

	#[runtime::pallet_index(4)]
	pub type Balances = pallet_balances;

	#[runtime::pallet_index(5)]
	pub type TransactionPayment = pallet_transaction_payment;

	#[runtime::pallet_index(6)]
	pub type Sudo = pallet_sudo;

	#[runtime::pallet_index(7)]
	pub type Proxy = pallet_proxy;

	// ── PNS (indices 8–13) ──
	#[runtime::pallet_index(8)]
	pub type PnsNft = pns_registrar::nft;

	#[runtime::pallet_index(9)]
	pub type PnsPriceOracle = pns_registrar::price_oracle;

	#[runtime::pallet_index(10)]
	pub type PnsRegistry = pns_registrar::registry;

	#[runtime::pallet_index(11)]
	pub type PnsRegistrar = pns_registrar::registrar;

	#[runtime::pallet_index(12)]
	pub type PnsResolvers = pns_resolvers::resolvers;

	#[runtime::pallet_index(13)]
	pub type PnsMarketplace = pns_marketplace;

	// ── Infrastructure for SecretSquirrel ──
	#[runtime::pallet_index(20)]
	pub type Scheduler = pallet_scheduler;

	#[runtime::pallet_index(21)]
	pub type Authorship = pallet_authorship;

	/// TESTNET ONLY — production must use relay-chain randomness.
	#[runtime::pallet_index(22)]
	pub type InsecureRandomness = pallet_insecure_randomness_collective_flip;

	// ── ZK-PKI ──
	#[runtime::pallet_index(30)]
	pub type ZkPki = zk_pki_pallet;

	// ── Secret Squirrel Messaging ──
	#[runtime::pallet_index(40)]
	pub type SecretSquirrel = pallet_secret_squirrel;
}

/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
	Runtime,
	Block,
	frame_system::ChainContext<Runtime>,
	Runtime,
	AllPalletsWithSystem,
>;
