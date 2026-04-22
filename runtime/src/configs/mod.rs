pub mod xcm_config;

// Substrate and Polkadot dependencies
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::{
	derive_impl,
	dispatch::DispatchClass,
	parameter_types,
	traits::{ConstBool, ConstU128, ConstU32, ConstU64, ConstU8, InstanceFilter, VariantCountOf},
	weights::{
		constants::{RocksDbWeight, WEIGHT_REF_TIME_PER_SECOND},
		IdentityFee, Weight,
	},
};
use frame_system::limits::{BlockLength, BlockWeights};
use pallet_transaction_payment::{ConstFeeMultiplier, FungibleAdapter, Multiplier};
use scale_info::TypeInfo;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_runtime::{traits::{BlakeTwo256, One}, Perbill};
use sp_version::RuntimeVersion;

use cumulus_pallet_parachain_system::RelayNumberMonotonicallyIncreases;
use cumulus_primitives_core::{AggregateMessageOrigin, ParaId};
use polkadot_runtime_common::xcm_sender::ExponentialPrice;
use xcm::latest::prelude::{AssetId as XcmAssetId, Location as XcmLocation};

/// Maps a `ParaId` → `AggregateMessageOrigin::Sibling(ParaId)` so
/// `MessageQueue` can route XCMP messages back through the shared queue.
pub struct ParaIdToSibling;
impl sp_runtime::traits::Convert<ParaId, AggregateMessageOrigin> for ParaIdToSibling {
	fn convert(para_id: ParaId) -> AggregateMessageOrigin {
		AggregateMessageOrigin::Sibling(para_id)
	}
}

/// Narrow the scope of a `QueueChangeHandler`/`QueuePausedQuery` from
/// `AggregateMessageOrigin` (what `MessageQueue` hands us) to `ParaId`
/// (what `cumulus_pallet_xcmp_queue` expects). Non-`Sibling` origins
/// are ignored. Inlined from `parachains_common::message_queue` —
/// tiny, self-contained, avoids pulling the whole crate.
pub struct NarrowOriginToSibling<Inner>(core::marker::PhantomData<Inner>);
impl<Inner: frame_support::traits::QueuePausedQuery<ParaId>>
	frame_support::traits::QueuePausedQuery<AggregateMessageOrigin>
	for NarrowOriginToSibling<Inner>
{
	fn is_paused(origin: &AggregateMessageOrigin) -> bool {
		match origin {
			AggregateMessageOrigin::Sibling(id) => Inner::is_paused(id),
			_ => false,
		}
	}
}
impl<Inner: pallet_message_queue::OnQueueChanged<ParaId>>
	pallet_message_queue::OnQueueChanged<AggregateMessageOrigin>
	for NarrowOriginToSibling<Inner>
{
	fn on_queue_changed(origin: AggregateMessageOrigin, fp: frame_support::traits::QueueFootprint) {
		if let AggregateMessageOrigin::Sibling(id) = origin {
			Inner::on_queue_changed(id, fp)
		}
	}
}

// Local module imports
use super::{
	AccountId, Aura, Balance, Balances, Block, BlockNumber, Hash, MessageQueue, Nonce,
	PalletInfo, ParachainSystem, Runtime, RuntimeCall, RuntimeEvent, RuntimeFreezeReason,
	RuntimeHoldReason, RuntimeOrigin, RuntimeTask, System, Timestamp, XcmpQueue,
	DAYS, EXISTENTIAL_DEPOSIT, MILLI_SECS_PER_BLOCK, SLOT_DURATION, UNIT, MILLI_UNIT, VERSION,
};

const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);

parameter_types! {
	pub const BlockHashCount: BlockNumber = 2400;
	pub const Version: RuntimeVersion = VERSION;

	/// We allow for 2 seconds of compute with a 6 second average block time.
	pub RuntimeBlockWeights: BlockWeights = BlockWeights::with_sensible_defaults(
		Weight::from_parts(2u64 * WEIGHT_REF_TIME_PER_SECOND, u64::MAX),
		NORMAL_DISPATCH_RATIO,
	);
	pub RuntimeBlockLength: BlockLength = BlockLength::builder()
		.max_length(5 * 1024 * 1024)
		.modify_max_length_for_class(DispatchClass::Normal, |m| *m = NORMAL_DISPATCH_RATIO * *m)
		.build();
	pub const SS58Prefix: u8 = 42;
}

#[allow(unused_parens)]
type SingleBlockMigrations = ();

#[derive_impl(frame_system::config_preludes::SolochainDefaultConfig)]
impl frame_system::Config for Runtime {
	type Block = Block;
	type BlockWeights = RuntimeBlockWeights;
	type BlockLength = RuntimeBlockLength;
	type AccountId = AccountId;
	type Nonce = Nonce;
	type Hash = Hash;
	type BlockHashCount = BlockHashCount;
	type DbWeight = RocksDbWeight;
	type Version = Version;
	type AccountData = pallet_balances::AccountData<Balance>;
	type SS58Prefix = SS58Prefix;
	type MaxConsumers = frame_support::traits::ConstU32<16>;
	type SingleBlockMigrations = SingleBlockMigrations;
	// Under cumulus the runtime doesn't self-update code; the relay
	// drives code upgrades via `set_validation_data`.
	type OnSetCode = cumulus_pallet_parachain_system::ParachainSetCode<Runtime>;
}

// ════════════════════════════════════════════════════════════
// Consensus: Aura (cumulus-managed; finality inherited from relay)
// ════════════════════════════════════════════════════════════

impl pallet_aura::Config for Runtime {
	type AuthorityId = AuraId;
	type DisabledValidators = ();
	type MaxAuthorities = ConstU32<32>;
	// Leave single-slot for now; flip to `ConstBool<true>` when we opt
	// into async backing (which also requires a runtime upgrade + node
	// code bump; don't toggle casually).
	type AllowMultipleBlocksPerSlot = ConstBool<false>;
	type SlotDuration = ConstU64<SLOT_DURATION>;
}

impl cumulus_pallet_aura_ext::Config for Runtime {}

impl pallet_timestamp::Config for Runtime {
	type Moment = u64;
	type OnTimestampSet = Aura;
	type MinimumPeriod = ConstU64<{ SLOT_DURATION / 2 }>;
	type WeightInfo = ();
}

// ════════════════════════════════════════════════════════════
// Parachain System / Info / Message Queue
// ════════════════════════════════════════════════════════════

parameter_types! {
	// Reserved weight for the `set_validation_data` inherent + DMP
	// processing. 5% of max block weight is the standard budget.
	pub const ReservedDmpWeight: Weight = Weight::from_parts(
		WEIGHT_REF_TIME_PER_SECOND.saturating_mul(2).saturating_div(20),
		u64::MAX,
	);
	pub const ReservedXcmpWeight: Weight = Weight::from_parts(
		WEIGHT_REF_TIME_PER_SECOND.saturating_mul(2).saturating_div(20),
		u64::MAX,
	);
	/// One inbound DMP/XCMP message chunk per block budget. Tune later
	/// once we have telemetry from Paseo.
	pub const RelayOrigin: AggregateMessageOrigin = AggregateMessageOrigin::Parent;
}

/// Slot duration on the Polkadot/Paseo relay chain. Do not change.
pub const RELAY_CHAIN_SLOT_DURATION_MILLIS: u32 = 6000;
/// How many parachain blocks we produce per relay block. 1 = same rate.
pub const BLOCK_PROCESSING_VELOCITY: u32 = 1;
/// Unincluded segment capacity. 1 = no async backing. Bump to 3 when
/// enabling async backing (also requires node-side collator swap).
pub const UNINCLUDED_SEGMENT_CAPACITY: u32 = 1;

/// Named alias for the consensus hook so runtime APIs can call its
/// `can_build_upon` method (see `AuraUnincludedSegmentApi`).
pub type ConsensusHook = cumulus_pallet_aura_ext::FixedVelocityConsensusHook<
	Runtime,
	RELAY_CHAIN_SLOT_DURATION_MILLIS,
	BLOCK_PROCESSING_VELOCITY,
	UNINCLUDED_SEGMENT_CAPACITY,
>;

impl cumulus_pallet_parachain_system::Config for Runtime {
	type WeightInfo = ();
	type RuntimeEvent = RuntimeEvent;
	type OnSystemEvent = ();
	type SelfParaId = parachain_info::Pallet<Runtime>;
	type OutboundXcmpMessageSource = XcmpQueue;
	type DmpQueue = frame_support::traits::EnqueueWithOrigin<MessageQueue, RelayOrigin>;
	type ReservedDmpWeight = ReservedDmpWeight;
	type XcmpMessageHandler = XcmpQueue;
	type ReservedXcmpWeight = ReservedXcmpWeight;
	type CheckAssociatedRelayNumber = RelayNumberMonotonicallyIncreases;
	type ConsensusHook = ConsensusHook;
	type RelayParentOffset = ConstU32<0>;
}

impl parachain_info::Config for Runtime {}

parameter_types! {
	pub MessageQueueServiceWeight: Weight = Perbill::from_percent(35) * RuntimeBlockWeights::get().max_block;
	pub const MessageQueueIdleServiceWeight: Weight = Weight::from_parts(10_000_000, 0);
}

impl pallet_message_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = ();
	#[cfg(feature = "runtime-benchmarks")]
	type MessageProcessor = pallet_message_queue::mock_helpers::NoopMessageProcessor<
		cumulus_primitives_core::AggregateMessageOrigin,
	>;
	#[cfg(not(feature = "runtime-benchmarks"))]
	type MessageProcessor = xcm_builder::ProcessXcmMessage<
		AggregateMessageOrigin,
		xcm_executor::XcmExecutor<xcm_config::XcmConfig>,
		RuntimeCall,
	>;
	type Size = u32;
	type QueueChangeHandler = NarrowOriginToSibling<cumulus_pallet_xcmp_queue::Pallet<Runtime>>;
	type QueuePausedQuery = NarrowOriginToSibling<cumulus_pallet_xcmp_queue::Pallet<Runtime>>;
	type HeapSize = ConstU32<{ 64 * 1024 }>;
	type MaxStale = ConstU32<8>;
	type ServiceWeight = MessageQueueServiceWeight;
	type IdleMaxServiceWeight = MessageQueueIdleServiceWeight;
}

impl cumulus_pallet_xcmp_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type ChannelInfo = ParachainSystem;
	type VersionWrapper = ();
	type XcmpQueue = frame_support::traits::TransformOrigin<
		MessageQueue,
		AggregateMessageOrigin,
		ParaId,
		ParaIdToSibling,
	>;
	type MaxInboundSuspended = ConstU32<1_000>;
	type MaxActiveOutboundChannels = ConstU32<128>;
	// Max size the XCMP queue can hold inbound per channel in bytes.
	type MaxPageSize = ConstU32<{ 103 * 1024 }>;
	// `ControllerOrigin` gates XCMP admin extrinsics (suspend / resume
	// channels). With Root unreachable locally (no sudo), this is
	// dormant until governance lands. If the network needs emergency
	// channel management before then, Paseo relay-chain-forced
	// upgrade is the lever.
	type ControllerOrigin = frame_system::EnsureRoot<AccountId>;
	type ControllerOriginConverter = xcm_config::XcmOriginToTransactDispatchOrigin;
	type WeightInfo = ();
	// Exponential delivery pricing: base fee + per-byte fee, both
	// charged to the sibling sending us XCMP. Makes sibling spam cost
	// real money (paid in the sender's sovereign account), mitigating
	// the `MessageQueue` 35%-of-max-block weight DoS surface.
	type PriceForSiblingDelivery = ExponentialPrice<
		FeeAssetId,
		BaseDeliveryFee,
		TransactionByteFee,
		XcmpQueue,
	>;
}

parameter_types! {
	pub FeeAssetId: XcmAssetId = XcmAssetId(XcmLocation::here());
	/// 0.3 DOT base fee per inbound XCMP message.
	pub const BaseDeliveryFee: Balance = 300 * MILLI_UNIT;
	/// 0.001 DOT per byte delivered.
	pub const TransactionByteFee: Balance = MILLI_UNIT;
}

// ════════════════════════════════════════════════════════════
// Balances / Fees
// ════════════════════════════════════════════════════════════

impl pallet_balances::Config for Runtime {
	type MaxLocks = ConstU32<50>;
	type MaxReserves = ();
	type ReserveIdentifier = [u8; 8];
	type Balance = Balance;
	type RuntimeEvent = RuntimeEvent;
	type DustRemoval = ();
	type ExistentialDeposit = ConstU128<EXISTENTIAL_DEPOSIT>;
	type AccountStore = System;
	type WeightInfo = pallet_balances::weights::SubstrateWeight<Runtime>;
	type FreezeIdentifier = RuntimeFreezeReason;
	type MaxFreezes = VariantCountOf<RuntimeFreezeReason>;
	type RuntimeHoldReason = RuntimeHoldReason;
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type DoneSlashHandler = ();
}

parameter_types! {
	pub FeeMultiplier: Multiplier = Multiplier::one();
}

impl pallet_transaction_payment::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type OnChargeTransaction = FungibleAdapter<Balances, ()>;
	type OperationalFeeMultiplier = ConstU8<5>;
	type WeightToFee = IdentityFee<Balance>;
	type LengthToFee = IdentityFee<Balance>;
	type FeeMultiplierUpdate = ConstFeeMultiplier<FeeMultiplier>;
	type WeightInfo = pallet_transaction_payment::weights::SubstrateWeight<Runtime>;
}

// ════════════════════════════════════════════════════════════
// PNS Configuration (verbatim from pns-node)
// ════════════════════════════════════════════════════════════

use pns_types::DomainHash;
use pns_registrar::traits::Registrar as RegistrarTrait;

/// Gates PNS admin extrinsics (previously `EnsureRoot`-gated, when
/// `pallet-sudo` was wired). Accepts only a Signed origin from the
/// `PnsCustodian` multisig account. For Paseo launch, swap the
/// `PnsCustodianAccount` parameter below to the reserved multisig
/// AccountId before generating the runtime wasm.
///
/// Why this exists: sudo was removed for the parachain branch.
/// With no Root-origin path reachable locally, PNS admin operations
/// would become permanently unreachable unless we provide an
/// alternate origin. `EnsureCustodian` is that alternate — a single
/// account that controls PNS governance until a full collective /
/// democracy pallet lands.
pub struct EnsureCustodian;
impl frame_support::traits::EnsureOrigin<RuntimeOrigin> for EnsureCustodian {
    type Success = AccountId;

    fn try_origin(o: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
        let custodian = PnsCustodianAccount::get();
        match o.clone().into() {
            Ok(frame_system::RawOrigin::Signed(who)) if who == custodian => Ok(who),
            _ => Err(o),
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
        Ok(frame_system::RawOrigin::Signed(PnsCustodianAccount::get()).into())
    }
}

parameter_types! {
    pub const GracePeriod: u64 = 30 * DAYS as u64 * MILLI_SECS_PER_BLOCK;
    pub const OfferWindow: u64 = 90 * DAYS as u64 * MILLI_SECS_PER_BLOCK;
    pub const DefaultCapacity: u32 = 10;
    pub const MinRegistrationDuration: u64 = 28 * DAYS as u64 * MILLI_SECS_PER_BLOCK;
    pub const MaxRegistrationDuration: u64 = 365 * DAYS as u64 * MILLI_SECS_PER_BLOCK;
    pub const BaseNode: DomainHash = pns_types::NATIVE_BASENODE;
    pub const OffchainPrefix: &'static [u8] = b"pns/";
    pub const MaxContentLen: u32 = 1024;
}

impl pns_registrar::nft::Config for Runtime {
    type ClassId = u32;
    type TotalId = u128;
    type TokenId = DomainHash;
    type ClassData = ();
    type TokenData = pns_types::Record;
    type MaxClassMetadata = ConstU32<0>;
    type MaxTokenMetadata = ConstU32<0>;
}

impl pns_registrar::price_oracle::Config for Runtime {
    type Currency = Balances;
    type Moment = u64;
    type ExchangeRate = pns_registrar::price_oracle::Pallet<Runtime>;
    type WeightInfo = pns_registrar::price_oracle_weights::SubstrateWeight<Runtime>;
    type ManagerOrigin = EnsureCustodian;
}

impl pns_registrar::registry::Config for Runtime {
    type WeightInfo = ();
    type Registrar = pns_registrar::registrar::Pallet<Runtime>;
    type ManagerOrigin = EnsureCustodian;
    type Ss58Updater = PnsSs58Updater;
    type RecordCleaner = PnsRecordCleaner;
    type OriginRecorder = PnsOriginRecorder;
}

pub struct PnsIsOpen;
impl pns_registrar::traits::IsRegistrarOpen for PnsIsOpen {
    fn is_open() -> bool { true }
}

pub struct PnsSs58Updater;
impl pns_registrar::traits::Ss58Updater for PnsSs58Updater {
    type AccountId = AccountId;
    fn update_ss58(node: DomainHash, owner: &AccountId) -> sp_runtime::DispatchResult {
        pns_resolvers::resolvers::Pallet::<Runtime>::set_ss58_record(node, owner)
    }
}

pub struct PnsOriginRecorder;
impl pns_registrar::traits::OriginRecorder for PnsOriginRecorder {
    fn record_origin(node: DomainHash, block_hash: [u8; 32]) -> sp_runtime::DispatchResult {
        pns_resolvers::resolvers::Pallet::<Runtime>::set_origin_record(node, block_hash)
    }
}

pub struct PnsRecordCleaner;
impl pns_registrar::traits::RecordCleaner for PnsRecordCleaner {
    fn clear_records_except_ss58(node: DomainHash) {
        pns_resolvers::resolvers::Pallet::<Runtime>::clear_records_except_ss58(node)
    }
    fn clear_all_records(node: DomainHash) {
        pns_resolvers::resolvers::Pallet::<Runtime>::clear_all_records(node)
    }
}

// PNS Custodian — the DAO/multisig responsible for maintaining PNS infrastructure.
// Replace with the real multisig AccountId32 before mainnet.
parameter_types! {
    pub PnsCustodianAccount: AccountId = AccountId::from([0xFE; 32]);
}

pub struct PnsCustodian;
impl frame_support::traits::Get<AccountId> for PnsCustodian {
    fn get() -> AccountId { PnsCustodianAccount::get() }
}

pub struct PnsBlockAuthor;
impl pns_registrar::traits::BlockAuthor for PnsBlockAuthor {
    type AccountId = AccountId;
    fn author() -> Option<AccountId> {
        pallet_authorship::Pallet::<Runtime>::author()
    }
}

pub struct PnsRegistryChecker;
impl pns_resolvers::resolvers::RegistryChecker for PnsRegistryChecker {
    type AccountId = AccountId;
    fn check_node_useable(node: DomainHash, owner: &AccountId) -> bool {
        if pns_registrar::registry::Pallet::<Runtime>::verify(owner, node).is_err() {
            return false;
        }
        pns_registrar::registrar::Pallet::<Runtime>::get_info(node)
            .map(|_| pns_registrar::registrar::Pallet::<Runtime>::check_expires_useable(node).is_ok())
            .unwrap_or(true)
    }
    fn base_node() -> DomainHash {
        BaseNode::get()
    }
}

impl pns_registrar::registrar::Config for Runtime {
    type Registry = pns_registrar::registry::Pallet<Runtime>;
    type Currency = Balances;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Fungible = Balances;
    type NowProvider = Timestamp;
    type Moment = u64;
    type GracePeriod = GracePeriod;
    type DefaultCapacity = DefaultCapacity;
    type BaseNode = BaseNode;
    type MinRegistrationDuration = MinRegistrationDuration;
    type MaxRegistrationDuration = MaxRegistrationDuration;
    type OfferWindow = OfferWindow;
    type WeightInfo = pns_registrar::registrar_weights::SubstrateWeight<Runtime>;
    type PriceOracle = pns_registrar::price_oracle::Pallet<Runtime>;
    type ManagerOrigin = EnsureCustodian;
    type IsOpen = PnsIsOpen;
    type Official = pns_registrar::registry::Pallet<Runtime>;
    type Ss58Updater = PnsSs58Updater;
    type OriginRecorder = PnsOriginRecorder;
    type RecordCleaner = PnsRecordCleaner;
    type PnsCustodian = PnsCustodian;
    type BlockAuthor = PnsBlockAuthor;
}

impl pns_resolvers::resolvers::Config for Runtime {
    const OFFCHAIN_PREFIX: &'static [u8] = b"pns/";
    type WeightInfo = pns_resolvers::resolvers_weights::SubstrateWeight<Runtime>;
    type MaxContentLen = MaxContentLen;
    type RegistryChecker = PnsRegistryChecker;
}

parameter_types! {
    pub const MarketplaceListingDeposit: Balance = 10 * MILLI_UNIT;
    pub const MarketplaceListingGracePeriod: u64 = 7 * DAYS as u64 * MILLI_SECS_PER_BLOCK;
}

impl pns_marketplace::Config for Runtime {
    type Currency = Balances;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Fungible = Balances;
    type ListingDeposit = MarketplaceListingDeposit;
    type ListingGracePeriod = MarketplaceListingGracePeriod;
    type Moment = u64;
    type NowProvider = Timestamp;
    type NameRegistry = pns_registrar::registrar::Pallet<Runtime>;
    type Ss58Updater = PnsSs58Updater;
    type RecordCleaner = PnsRecordCleaner;
    type OriginRecorder = PnsOriginRecorder;
    type BaseNode = BaseNode;
    type WeightInfo = pns_marketplace::marketplace_weights::SubstrateWeight<Runtime>;
}

// ════════════════════════════════════════════════════════════
// Scheduler (required by SecretSquirrel)
// ════════════════════════════════════════════════════════════

parameter_types! {
    pub MaxScheduledPerBlock: u32 = 50;
    pub MaximumSchedulerWeight: Weight = Weight::from_parts(
        WEIGHT_REF_TIME_PER_SECOND, u64::MAX
    );
}

impl pallet_scheduler::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeOrigin = RuntimeOrigin;
    type PalletsOrigin = super::OriginCaller;
    type RuntimeCall = RuntimeCall;
    type MaximumWeight = MaximumSchedulerWeight;
    // `EnsureNever` since sudo is removed and Root is unreachable. No
    // extrinsic caller can invoke `pallet_scheduler::schedule`.
    // SecretSquirrel uses the `Scheduler` trait API directly (which
    // bypasses this gate) and is unaffected.
    type ScheduleOrigin = frame_system::EnsureNever<AccountId>;
    type MaxScheduledPerBlock = MaxScheduledPerBlock;
    type WeightInfo = pallet_scheduler::weights::SubstrateWeight<Runtime>;
    type OriginPrivilegeCmp = frame_support::traits::EqualPrivilegeOnly;
    type Preimages = ();
    type BlockNumberProvider = frame_system::Pallet<Runtime>;
}

// ════════════════════════════════════════════════════════════
// Authorship (block author tracking)
// ════════════════════════════════════════════════════════════

impl pallet_authorship::Config for Runtime {
    type FindAuthor = AuraAccountFinder;
    type EventHandler = ();
}

/// Reads the Aura slot from pre-runtime digests, indexes into the
/// authority list, and converts the AuraId (sr25519::Public) to
/// AccountId32. SecretSquirrel + ZK-PKI use this to pay the block
/// author on slot cleanup / cert-mint events.
///
/// Note: `cumulus_pallet_aura_ext::Authorities` is `pub(crate)` at
/// this SDK snapshot. `pallet_aura::Authorities` is the public
/// accessor and holds the same list — aura-ext just mirrors it into
/// the PoV on `on_finalize`. For the `FindAuthor` impl (which runs
/// mid-block) either source yields identical results in a static-
/// authority setup.
pub struct AuraAccountFinder;
impl frame_support::traits::FindAuthor<AccountId> for AuraAccountFinder {
    fn find_author<'a, I>(digests: I) -> Option<AccountId>
    where
        I: 'a + IntoIterator<Item = (sp_runtime::ConsensusEngineId, &'a [u8])>,
    {
        use codec::Decode;
        for (engine, data) in digests {
            if engine == sp_consensus_aura::AURA_ENGINE_ID {
                let slot = sp_consensus_aura::Slot::decode(&mut &data[..]).ok()?;
                let authorities = pallet_aura::Authorities::<Runtime>::get();
                if authorities.is_empty() {
                    return None;
                }
                let idx = *slot % authorities.len() as u64;
                let authority: &AuraId = authorities.get(idx as usize)?;
                return Some(AccountId::from(
                    sp_core::sr25519::Public::from(authority.clone())
                ));
            }
        }
        None
    }
}

// ════════════════════════════════════════════════════════════
// Relay-chain Randomness
// ════════════════════════════════════════════════════════════
//
// Sourced from the relay parent storage root committed each block via
// `set_validation_data`. A collator cannot manipulate this — the hash
// is validated against the PVF parent state. Mixing the caller's
// `subject` bytes in via BLAKE2 means distinct subjects get distinct
// random outputs in the same block, and an attacker can't predict
// output for a subject they don't control until their tx lands in a
// block whose relay parent they don't yet know.
//
// This is STRICTLY BETTER than `pallet_insecure_randomness_collective_flip`
// (which is attacker-manipulable by any collator) but is NOT epoch
// randomness — it rotates every relay block, not every 10 min epoch.
// For use cases requiring uniform-distribution cryptographic randomness
// (e.g. a lottery drawing a uniform-random winner across thousands of
// entries with adversary observation), wire a VRF consumer on top.

use frame_support::traits::Randomness as RandomnessT;

pub struct RelayChainRandomness;
impl RandomnessT<Hash, BlockNumber> for RelayChainRandomness {
	fn random(subject: &[u8]) -> (Hash, BlockNumber) {
		use sp_runtime::traits::Hash as HashT;
		let validation_data = cumulus_pallet_parachain_system::ValidationData::<Runtime>::get();
		let (relay_number, relay_root) = validation_data
			.map(|v| (v.relay_parent_number, v.relay_parent_storage_root))
			.unwrap_or_default();
		let mixed = BlakeTwo256::hash_of(&(relay_root, subject));
		// Cast relay block number (u32 in RelayChainBlockNumber) into
		// our local BlockNumber (also u32) — widths match.
		(mixed, relay_number as BlockNumber)
	}
}

// ════════════════════════════════════════════════════════════
// pallet_proxy Configuration
// ════════════════════════════════════════════════════════════
//
// Required by ZK-PKI invariant #12 — `register_root` and
// `issue_issuer_cert` reject unless the caller has a proxy
// relationship in `pallet_proxy::Proxies`. The proxy pallet is
// wired with zero deposits and an `Any`-variant filter for paseo;
// real production would tighten the filter by call category and
// charge deposits.

/// Category filter for proxy calls. Only the `Any` variant exists;
/// paseo doesn't scope proxied calls by type.
#[derive(
    Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, DecodeWithMemTracking,
    MaxEncodedLen, TypeInfo, Debug, Default,
)]
pub enum ProxyType {
    #[default]
    Any,
}

impl InstanceFilter<RuntimeCall> for ProxyType {
    fn filter(&self, _call: &RuntimeCall) -> bool {
        match self {
            ProxyType::Any => true,
        }
    }
}

impl pallet_proxy::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type Currency = Balances;
    type ProxyType = ProxyType;
    type ProxyDepositBase = ConstU128<0>;
    type ProxyDepositFactor = ConstU128<0>;
    type MaxProxies = ConstU32<32>;
    type WeightInfo = ();
    type MaxPending = ConstU32<32>;
    type CallHasher = BlakeTwo256;
    type AnnouncementDepositBase = ConstU128<0>;
    type AnnouncementDepositFactor = ConstU128<0>;
    type BlockNumberProvider = frame_system::Pallet<Runtime>;
}

// ════════════════════════════════════════════════════════════
// ZK-PKI Configuration
// ════════════════════════════════════════════════════════════

// Block-count constants for 6-second parachain blocks.
// 10 blocks/min → 14,400 blocks/day.
parameter_types! {
    pub const PkiInactivePurgePeriod: BlockNumber    = 14_400 * 30;       // 30 days — grace period
    pub const PkiContractOfferTtlBlocks: BlockNumber = 14_400;            // 1 day
    pub const PkiMaxRootTtlBlocks: BlockNumber       = 14_400 * 365 * 5;  // 5 years
    pub const PkiMaxIssuersPerRoot: u32              = 5;
    pub const PkiChallengeWindowBlocks: BlockNumber  = 14_400 * 45;       // 45 days
    pub const PkiCertDeposit: Balance                = UNIT;              // 1 DOT, covers hot + cold
    pub const PkiOfferDeposit: Balance               = UNIT / 10;         // 0.1 DOT
    pub const PkiMinRootTtlBlocks: BlockNumber       = 14_400 * 90;       // 90 days
    pub const PkiMinIssuerTtlBlocks: BlockNumber     = 14_400 * 30;       // 30 days
    pub const PkiTtlCheckInterval: BlockNumber       = 14_400;            // 1 day — OCSP-style re-query cadence
    pub const PkiTemplateDeposit: Balance            = 10 * UNIT;         // 10 DOT per template
    pub const PkiMaxTemplatesPerIssuer: u32          = 256;
    // Fee system.
    pub const PkiProtocolFeeBasisPoints: u32         = 1_000;   // 10% of mint fee
    pub const PkiBlockCreatorCapBasisPoints: u32     = 4_000;   // 40% of mint fee cap
    pub const PkiDepositBasisPoints: u32             = 500;     // 5% of mint fee
    pub const PkiMinDeposit: Balance                 = UNIT / 10;   // 0.1 DOT floor
    pub const PkiMintFeePoP: Balance                 = UNIT;        // 1 DOT
    pub const PkiMintFeePacked: Balance              = UNIT + (UNIT / 2); // 1.5 DOT
    pub const PkiMintFeeNone: Balance                = 2 * UNIT;    // 2 DOT
    // TODO: pending governance wiring — swap this placeholder for
    // a treasury pallet account once the chain stands up a
    // sovereign sink for protocol fees. For now it's a burn hole
    // (fixed account, no keypair issuance).
    pub PkiProtocolFeeRecipient: AccountId = AccountId::new([0u8; 32]);
}

impl zk_pki_pallet::Config for Runtime {
    type InactivePurgePeriod = PkiInactivePurgePeriod;
    type ContractOfferTtlBlocks = PkiContractOfferTtlBlocks;
    type MaxRootTtlBlocks = PkiMaxRootTtlBlocks;
    type MaxIssuersPerRoot = PkiMaxIssuersPerRoot;
    type ChallengeWindowBlocks = PkiChallengeWindowBlocks;
    type CertDeposit = PkiCertDeposit;
    type OfferDeposit = PkiOfferDeposit;
    type MinRootTtlBlocks = PkiMinRootTtlBlocks;
    type MinIssuerTtlBlocks = PkiMinIssuerTtlBlocks;
    type TtlCheckInterval = PkiTtlCheckInterval;
    type TemplateDeposit = PkiTemplateDeposit;
    type MaxTemplatesPerIssuer = PkiMaxTemplatesPerIssuer;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type FindAuthor = AuraAccountFinder;
    type ProtocolFeeRecipient = PkiProtocolFeeRecipient;
    type ProtocolFeeBasisPoints = PkiProtocolFeeBasisPoints;
    type BlockCreatorCapBasisPoints = PkiBlockCreatorCapBasisPoints;
    type DepositBasisPoints = PkiDepositBasisPoints;
    type MinDeposit = PkiMinDeposit;
    type MintFeePoP = PkiMintFeePoP;
    type MintFeePacked = PkiMintFeePacked;
    type MintFeeNone = PkiMintFeeNone;
    /// TESTNET: Tpm-returning test verifier. Derives a per-call-unique
    /// EK hash from `(pubkey, challenge)` so PoP-capability happy
    /// paths on `register_root` / `issue_issuer_cert` can be
    /// exercised without real hardware, and multiple registrations
    /// in one session don't collide at the EK-dedup gate. Production
    /// must swap in the real `TpmAttestationVerifier`.
    type Attestation = zk_pki_primitives::traits::TpmTestAttestationVerifier;
    /// TESTNET: bypass-crypto binding verifier (decodes `MockVerdict`
    /// from `integrity_blob`). Production must swap in
    /// `zk_pki_tpm::ProductionBindingProofVerifier` once the real
    /// `DOTWAVE_SIGNING_CERT_HASH` constant lands.
    type BindingProofVerifier =
        zk_pki_tpm::test_mock_verifier::NoopBindingProofVerifier;
    /// Placeholder weights — replace with `--pallet zk-pki-pallet
    /// --extrinsic '*'` output before Kusama.
    type WeightInfo = zk_pki_pallet::weights::SubstrateWeight<Runtime>;
    /// Production proxy-relationship check: reads `pallet_proxy::Proxies`
    /// directly via the local `proxy_validator` module. Under
    /// `runtime-benchmarks` we swap to `NoopProxyValidator` so benchmark
    /// setup doesn't have to seed real proxy entries — the proxy check
    /// is a single StorageMap read whose cost folds into other reads the
    /// benchmark already measures.
    #[cfg(not(feature = "runtime-benchmarks"))]
    type ProxyValidator = super::proxy_validator::PalletProxyValidator<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type ProxyValidator = zk_pki_primitives::proxy::NoopProxyValidator;
}

// ════════════════════════════════════════════════════════════
// Secret Squirrel Configuration
// ════════════════════════════════════════════════════════════

parameter_types! {
    pub const SsDepositBase: Balance    = 100 * MILLI_UNIT;  // 0.1 DOT
    pub const SsDepositPerByte: Balance = MILLI_UNIT;        // 0.001 DOT/byte
    pub const SsDepositPerBlock: Balance = MILLI_UNIT;       // 0.001 DOT/block
    pub const SsMaxActiveSlots: u32     = 10_000;
}

impl pallet_secret_squirrel::Config for Runtime {
    type RuntimeCall = RuntimeCall;
    type Currency = Balances;
    // Relay-chain-sourced randomness. Per-block rotation (not epoch
    // randomness) — collator can't manipulate, sender can't predict.
    // See `RelayChainRandomness` docs above.
    type BabeRandomness = RelayChainRandomness;
    type Scheduler = super::Scheduler;
    type FindAuthor = AuraAccountFinder;
    type PalletsOrigin = super::OriginCaller;
    type DepositBase = SsDepositBase;
    type DepositPerByte = SsDepositPerByte;
    type DepositPerBlock = SsDepositPerBlock;
    type MaxActiveSlots = SsMaxActiveSlots;
}
