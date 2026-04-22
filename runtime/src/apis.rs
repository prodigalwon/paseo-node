// External crates imports
use alloc::vec::Vec;
use frame_support::{
	genesis_builder_helper::{build_state, get_preset},
	weights::Weight,
};
use sp_api::impl_runtime_apis;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_core::{crypto::KeyTypeId, OpaqueMetadata};
use sp_runtime::{
	traits::Block as BlockT,
	transaction_validity::{TransactionSource, TransactionValidity},
	ApplyExtrinsicResult,
};
use sp_session::OpaqueGeneratedSessionKeys;
use sp_version::RuntimeVersion;

// Local module imports
use crate::InherentDataExt;
use super::{
	AccountId, Aura, Balance, Block, Executive, Nonce, Runtime,
	RuntimeCall, RuntimeGenesisConfig, SessionKeys, System, TransactionPayment, VERSION,
};
use super::configs::OfferWindow;

impl_runtime_apis! {
	impl sp_api::Core<Block> for Runtime {
		fn version() -> RuntimeVersion {
			VERSION
		}

		fn execute_block(block: <Block as BlockT>::LazyBlock) {
			Executive::execute_block(block);
		}

		fn initialize_block(header: &<Block as BlockT>::Header) -> sp_runtime::ExtrinsicInclusionMode {
			Executive::initialize_block(header)
		}
	}

	impl sp_api::Metadata<Block> for Runtime {
		fn metadata() -> OpaqueMetadata {
			OpaqueMetadata::new(Runtime::metadata().into())
		}

		fn metadata_at_version(version: u32) -> Option<OpaqueMetadata> {
			Runtime::metadata_at_version(version)
		}

		fn metadata_versions() -> Vec<u32> {
			Runtime::metadata_versions()
		}
	}

	impl frame_support::view_functions::runtime_api::RuntimeViewFunction<Block> for Runtime {
		fn execute_view_function(id: frame_support::view_functions::ViewFunctionId, input: Vec<u8>) -> Result<Vec<u8>, frame_support::view_functions::ViewFunctionDispatchError> {
			Runtime::execute_view_function(id, input)
		}
	}

	impl sp_block_builder::BlockBuilder<Block> for Runtime {
		fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
			Executive::apply_extrinsic(extrinsic)
		}

		fn finalize_block() -> <Block as BlockT>::Header {
			Executive::finalize_block()
		}

		fn inherent_extrinsics(data: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
			data.create_extrinsics()
		}

		fn check_inherents(
			block: <Block as BlockT>::LazyBlock,
			data: sp_inherents::InherentData,
		) -> sp_inherents::CheckInherentsResult {
			data.check_extrinsics(&block)
		}
	}

	impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
		fn validate_transaction(
			source: TransactionSource,
			tx: <Block as BlockT>::Extrinsic,
			block_hash: <Block as BlockT>::Hash,
		) -> TransactionValidity {
			Executive::validate_transaction(source, tx, block_hash)
		}
	}

	impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
		fn offchain_worker(header: &<Block as BlockT>::Header) {
			Executive::offchain_worker(header)
		}
	}

	impl sp_consensus_aura::AuraApi<Block, AuraId> for Runtime {
		fn slot_duration() -> sp_consensus_aura::SlotDuration {
			sp_consensus_aura::SlotDuration::from_millis(Aura::slot_duration())
		}

		fn authorities() -> Vec<AuraId> {
			pallet_aura::Authorities::<Runtime>::get().into_inner()
		}
	}

	impl sp_session::SessionKeys<Block> for Runtime {
		fn generate_session_keys(owner: Vec<u8>, seed: Option<Vec<u8>>) -> OpaqueGeneratedSessionKeys {
			SessionKeys::generate(&owner, seed).into()
		}

		fn decode_session_keys(
			encoded: Vec<u8>,
		) -> Option<Vec<(Vec<u8>, KeyTypeId)>> {
			SessionKeys::decode_into_raw_public_keys(&encoded)
		}
	}

	// CollectCollationInfo — relay validators call this to get the PoV
	// and downward-message queue state when validating a parachain
	// block. Parachain equivalent of GrandpaApi.
	impl cumulus_primitives_core::CollectCollationInfo<Block> for Runtime {
		fn collect_collation_info(
			header: &<Block as BlockT>::Header,
		) -> cumulus_primitives_core::CollationInfo {
			super::ParachainSystem::collect_collation_info(header)
		}
	}

	// AuraUnincludedSegmentApi — cumulus aura-ext uses this to decide
	// whether to author given the unincluded segment capacity.
	impl cumulus_primitives_aura::AuraUnincludedSegmentApi<Block> for Runtime {
		fn can_build_upon(
			included_hash: <Block as BlockT>::Hash,
			slot: cumulus_primitives_aura::Slot,
		) -> bool {
			super::configs::ConsensusHook::can_build_upon(included_hash, slot)
		}
	}

	// GetParachainInfo — collator node reads the para_id from runtime
	// state via this API (replaces reading from chain-spec extension
	// in newer stable releases).
	impl cumulus_primitives_core::GetParachainInfo<Block> for Runtime {
		fn parachain_id() -> cumulus_primitives_core::ParaId {
			parachain_info::Pallet::<Runtime>::parachain_id()
		}
	}

	// KeyToIncludeInRelayProof — lets parachain request extra relay
	// state to be included in the PoV proof. Default empty is fine
	// unless we start reading relay BABE randomness via state proof.
	impl cumulus_primitives_core::KeyToIncludeInRelayProof<Block> for Runtime {
		fn keys_to_prove() -> cumulus_primitives_core::RelayProofRequest {
			Default::default()
		}
	}

	impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
		fn account_nonce(account: AccountId) -> Nonce {
			System::account_nonce(account)
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance> for Runtime {
		fn query_info(
			uxt: <Block as BlockT>::Extrinsic,
			len: u32,
		) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_info(uxt, len)
		}
		fn query_fee_details(
			uxt: <Block as BlockT>::Extrinsic,
			len: u32,
		) -> pallet_transaction_payment::FeeDetails<Balance> {
			TransactionPayment::query_fee_details(uxt, len)
		}
		fn query_weight_to_fee(weight: Weight) -> Balance {
			TransactionPayment::weight_to_fee(weight)
		}
		fn query_length_to_fee(length: u32) -> Balance {
			TransactionPayment::length_to_fee(length)
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentCallApi<Block, Balance, RuntimeCall>
		for Runtime
	{
		fn query_call_info(
			call: RuntimeCall,
			len: u32,
		) -> pallet_transaction_payment::RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_call_info(call, len)
		}
		fn query_call_fee_details(
			call: RuntimeCall,
			len: u32,
		) -> pallet_transaction_payment::FeeDetails<Balance> {
			TransactionPayment::query_call_fee_details(call, len)
		}
		fn query_weight_to_fee(weight: Weight) -> Balance {
			TransactionPayment::weight_to_fee(weight)
		}
		fn query_length_to_fee(length: u32) -> Balance {
			TransactionPayment::length_to_fee(length)
		}
	}

	// ════════════════════════════════════════════════════════════
	// PNS Runtime API
	// ════════════════════════════════════════════════════════════

	impl pns_runtime_api::PnsStorageApi<Block, u64, Balance, AccountId> for Runtime {
		fn get_info(id: pns_types::DomainHash) -> Option<pns_types::NameRecord<AccountId, u64, Balance>> {
			use frame_support::traits::Time;
			if let Some(offer) = pns_registrar::registrar::OfferedNames::<Runtime>::get(id) {
				let now = pallet_timestamp::Pallet::<Runtime>::now();
				if now < offer.offered_at.saturating_add(OfferWindow::get()) {
					return None;
				}
			}
			let info = pns_registrar::registrar::Pallet::<Runtime>::get_info(id)?;
			if pallet_timestamp::Pallet::<Runtime>::now() >= info.expire {
				return None;
			}
			let token = pns_registrar::nft::Pallet::<Runtime>::tokens(0u32, id)?;
			let for_sale = pns_marketplace::Listings::<Runtime>::contains_key(id);
			Some(pns_types::NameRecord {
				owner: token.owner,
				expire: info.expire,
				capacity: info.capacity,
				register_fee: info.register_fee,
				for_sale,
				last_block: info.last_block,
				read_block_number: 0,
				read_block_hash: Default::default(),
			})
		}

		fn lookup(id: pns_types::DomainHash, record_types: Vec<pns_types::ddns::codec_type::RecordType>) -> Vec<(pns_types::ddns::codec_type::RecordType, Vec<u8>)> {
			pns_resolvers::resolvers::Pallet::<Runtime>::lookup(id, record_types)
		}

		fn get_listing(name: Vec<u8>) -> Option<pns_types::ListingInfo<AccountId, Balance, u64>> {
			use pns_registrar::traits::Label;
			let (label, _) = Label::new_with_len(&name)?;
			let node = label.encode_with_node(&pns_types::NATIVE_BASENODE);
			let l = pns_marketplace::Listings::<Runtime>::get(node)?;
			Some(pns_types::ListingInfo {
				seller: l.seller,
				price: l.price,
				expires_at: l.expires_at,
				read_block_number: 0,
				read_block_hash: Default::default(),
			})
		}

		fn resolve_name(name: Vec<u8>) -> Option<pns_types::NameRecord<AccountId, u64, Balance>> {
			use frame_support::traits::Time;
			use pns_registrar::traits::Label;
			let (label, _) = Label::new_with_len(&name)?;
			let base_node = pns_types::NATIVE_BASENODE;
			let node = label.encode_with_node(&base_node);
			if let Some(offer) = pns_registrar::registrar::OfferedNames::<Runtime>::get(node) {
				let now = pallet_timestamp::Pallet::<Runtime>::now();
				if now < offer.offered_at.saturating_add(OfferWindow::get()) {
					return None;
				}
			}
			let info = pns_registrar::registrar::Pallet::<Runtime>::get_info(node)?;
			if pallet_timestamp::Pallet::<Runtime>::now() >= info.expire {
				return None;
			}
			let token = pns_registrar::nft::Pallet::<Runtime>::tokens(0u32, node)?;
			let for_sale = pns_marketplace::Listings::<Runtime>::contains_key(node);
			Some(pns_types::NameRecord {
				owner: token.owner,
				expire: info.expire,
				capacity: info.capacity,
				register_fee: info.register_fee,
				for_sale,
				last_block: info.last_block,
				read_block_number: 0,
				read_block_hash: Default::default(),
			})
		}

		fn lookup_by_name(name: Vec<u8>, record_types: Vec<pns_types::ddns::codec_type::RecordType>) -> Vec<(pns_types::ddns::codec_type::RecordType, Vec<u8>)> {
			let node = pns_types::parse_name_to_node(&name, &pns_types::NATIVE_BASENODE)
				.unwrap_or_default();
			pns_resolvers::resolvers::Pallet::<Runtime>::lookup(node, record_types)
		}

	}

	// ════════════════════════════════════════════════════════════
	// Secret Squirrel Runtime API
	// ════════════════════════════════════════════════════════════

	impl secret_squirrel_runtime_api::SecretSquirrelApi<Block> for Runtime {
		fn retrieve_slot(slot_hash: sp_core::H256) -> Option<Vec<u8>> {
			pallet_secret_squirrel::Slots::<Runtime>::get(slot_hash)
				.map(|slot| slot.payload.into_inner())
		}

		fn slot_exists(slot_hash: sp_core::H256) -> bool {
			pallet_secret_squirrel::Slots::<Runtime>::contains_key(slot_hash)
		}
	}

	// ════════════════════════════════════════════════════════════
	// ZK-PKI Runtime API
	// ════════════════════════════════════════════════════════════
	//
	// Forwards every query to the pallet's `query_*` pub-fns, which
	// do the storage reads + RPC-shape translation. The pallet
	// generics changed in the TODO-5 / optimization passes — the
	// trait now takes just `<AccountId>` and surfaces block numbers
	// as `u64` at the boundary via `UniqueSaturatedInto`.

	impl zk_pki_primitives::runtime_api::ZkPkiApi<Block, AccountId> for Runtime {
		fn cert_status(
			thumbprint: [u8; 32],
		) -> Option<zk_pki_primitives::runtime_api::CertStatusResponse<AccountId>> {
			zk_pki_pallet::Pallet::<Runtime>::query_cert_status(thumbprint)
		}

		fn certs_by_issuer(
			issuer: AccountId,
		) -> Vec<zk_pki_primitives::runtime_api::CertSummary> {
			zk_pki_pallet::Pallet::<Runtime>::query_certs_by_issuer(issuer)
		}

		fn certs_by_user(
			user: AccountId,
		) -> Vec<zk_pki_primitives::runtime_api::CertSummary> {
			zk_pki_pallet::Pallet::<Runtime>::query_certs_by_user(user)
		}

		fn certs_by_root(
			root: AccountId,
		) -> Vec<zk_pki_primitives::runtime_api::CertSummary> {
			zk_pki_pallet::Pallet::<Runtime>::query_certs_by_root(root)
		}

		fn entity_status(
			address: AccountId,
		) -> Option<zk_pki_primitives::runtime_api::EntityStatusResponse<AccountId>> {
			zk_pki_pallet::Pallet::<Runtime>::query_entity_status(address)
		}

		fn ek_lookup(root: AccountId, ek_hash: [u8; 32]) -> Option<[u8; 32]> {
			zk_pki_pallet::Pallet::<Runtime>::query_ek_lookup(root, ek_hash)
		}

		fn chain_valid_at(thumbprint: [u8; 32], block_number: u64) -> bool {
			zk_pki_pallet::Pallet::<Runtime>::query_chain_valid_at(thumbprint, block_number)
		}
	}

	// ════════════════════════════════════════════════════════════
	// Benchmarking & Try-Runtime
	// ════════════════════════════════════════════════════════════

	#[cfg(feature = "runtime-benchmarks")]
	impl frame_benchmarking::Benchmark<Block> for Runtime {
		fn benchmark_metadata(extra: bool) -> (
			Vec<frame_benchmarking::BenchmarkList>,
			Vec<frame_support::traits::StorageInfo>,
		) {
			use frame_benchmarking::{baseline, BenchmarkList};
			use frame_support::traits::StorageInfoTrait;
			use frame_system_benchmarking::Pallet as SystemBench;
			use frame_system_benchmarking::extensions::Pallet as SystemExtensionsBench;
			use baseline::Pallet as BaselineBench;
			use super::*;

			let mut list = Vec::<BenchmarkList>::new();
			list_benchmarks!(list, extra);

			let storage_info = AllPalletsWithSystem::storage_info();

			(list, storage_info)
		}

		#[allow(non_local_definitions)]
		fn dispatch_benchmark(
			config: frame_benchmarking::BenchmarkConfig
		) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, alloc::string::String> {
			use frame_benchmarking::{baseline, BenchmarkBatch};
			use sp_storage::TrackedStorageKey;
			use frame_system_benchmarking::Pallet as SystemBench;
			use frame_system_benchmarking::extensions::Pallet as SystemExtensionsBench;
			use baseline::Pallet as BaselineBench;
			use super::*;

			impl frame_system_benchmarking::Config for Runtime {}
			impl baseline::Config for Runtime {}

			use frame_support::traits::WhitelistedStorageKeys;
			let whitelist: Vec<TrackedStorageKey> = AllPalletsWithSystem::whitelisted_storage_keys();

			let mut batches = Vec::<BenchmarkBatch>::new();
			let params = (&config, &whitelist);
			add_benchmarks!(params, batches);

			Ok(batches)
		}
	}

	#[cfg(feature = "try-runtime")]
	impl frame_try_runtime::TryRuntime<Block> for Runtime {
		fn on_runtime_upgrade(checks: frame_try_runtime::UpgradeCheckSelect) -> (Weight, Weight) {
			let weight = Executive::try_runtime_upgrade(checks).unwrap();
			(weight, super::configs::RuntimeBlockWeights::get().max_block)
		}

		fn execute_block(
			block: <Block as BlockT>::LazyBlock,
			state_root_check: bool,
			signature_check: bool,
			select: frame_try_runtime::TryStateSelect
		) -> Weight {
			Executive::try_execute_block(block, state_root_check, signature_check, select).expect("execute-block failed")
		}
	}

	impl sp_genesis_builder::GenesisBuilder<Block> for Runtime {
		fn build_state(config: Vec<u8>) -> sp_genesis_builder::Result {
			build_state::<RuntimeGenesisConfig>(config)
		}

		fn get_preset(id: &Option<sp_genesis_builder::PresetId>) -> Option<Vec<u8>> {
			get_preset::<RuntimeGenesisConfig>(id, crate::genesis_config_presets::get_preset)
		}

		fn preset_names() -> Vec<sp_genesis_builder::PresetId> {
			crate::genesis_config_presets::preset_names()
		}
	}
}

// The legacy `build_cert_response` helper was removed — the
// `ZkPkiApi` impl above forwards every method to the pallet's
// `query_*` pub-fns, which own the cert-status / chain-validity
// synthesis now. `LookupTable` no longer exists (split into
// `CertLookupHot` + `CertLookupCold` by the optimization pass);
// `CertResponse` was replaced by `CertStatusResponse` +
// `CertSummary` at the RPC boundary.
