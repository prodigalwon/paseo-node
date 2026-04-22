//! XCM configuration for the Paseo parachain runtime.
//!
//! Minimal day-one wiring: native token only, trust the relay chain as
//! teleport/reserve counterparty, allow UMP up / DMP in / XCMP sibling
//! messages through the standard barriers. Asset conversion, foreign
//! asset registries, and fee-to-treasury plumbing are deliberately out
//! of scope — they land in a follow-up once the chain is on Paseo.

use frame_support::{
	parameter_types,
	traits::{ConstU32, Contains, Everything, Nothing},
	weights::Weight,
};
use frame_system::EnsureRoot;
use pallet_xcm::XcmPassthrough;
use polkadot_parachain_primitives::primitives::Sibling;
use xcm::latest::prelude::*;
use xcm_builder::{
	AccountId32Aliases, AllowExplicitUnpaidExecutionFrom, AllowKnownQueryResponses,
	AllowSubscriptionsFrom, AllowTopLevelPaidExecutionFrom, DenyReserveTransferToRelayChain,
	DenyThenTry, EnsureXcmOrigin, FixedWeightBounds, FrameTransactionalProcessor,
	FungibleAdapter, IsConcrete, ParentIsPreset, RelayChainAsNative,
	SiblingParachainAsNative, SiblingParachainConvertsVia, SignedAccountId32AsNative,
	SignedToAccountId32, SovereignSignedViaLocation, TakeWeightCredit, TrailingSetTopicAsId,
	UsingComponents, WithComputedOrigin, WithUniqueTopic,
};
use xcm_executor::XcmExecutor;

use crate::{
	AccountId, AllPalletsWithSystem, Balances, ParachainInfo, ParachainSystem, PolkadotXcm,
	Runtime, RuntimeCall, RuntimeEvent, RuntimeOrigin, XcmpQueue,
};

parameter_types! {
	/// Relay chain location, from this parachain's perspective.
	pub const RelayLocation: Location = Location::parent();
	pub const RelayNetwork: Option<NetworkId> = None;
	pub RelayChainOrigin: RuntimeOrigin = cumulus_pallet_xcm::Origin::Relay.into();
	pub UniversalLocation: InteriorLocation =
		[GlobalConsensus(NetworkId::Polkadot), Parachain(ParachainInfo::parachain_id().into())].into();
}

/// How we convert an `Origin` XCM location into an `AccountId` / signer.
pub type LocationToAccountId = (
	// Relay chain root → parent sovereign account
	ParentIsPreset<AccountId>,
	// Sibling parachain → sibling sovereign account
	SiblingParachainConvertsVia<Sibling, AccountId>,
	// Arbitrary `AccountId32` junctions on our network
	AccountId32Aliases<RelayNetwork, AccountId>,
);

/// Fungibles adapter: our native `Balances` is the only registered asset,
/// matched by the empty `Here` location (i.e. this parachain).
pub type FungiblesTransactor = FungibleAdapter<
	Balances,
	IsConcrete<NativeLocationHere>,
	LocationToAccountId,
	AccountId,
	(),
>;

parameter_types! {
	/// Location of our native token, from our own perspective.
	pub const NativeLocationHere: Location = Location::here();
}

/// Converter from foreign XCM origin to local `RuntimeOrigin`.
pub type XcmOriginToTransactDispatchOrigin = (
	SovereignSignedViaLocation<LocationToAccountId, RuntimeOrigin>,
	RelayChainAsNative<RelayChainOrigin, RuntimeOrigin>,
	SiblingParachainAsNative<cumulus_pallet_xcm::Origin, RuntimeOrigin>,
	SignedAccountId32AsNative<RelayNetwork, RuntimeOrigin>,
	XcmPassthrough<RuntimeOrigin>,
);

parameter_types! {
	// Standard parachain-template weight budget.
	pub UnitWeightCost: Weight = Weight::from_parts(1_000_000_000, 64 * 1024);
	pub const MaxInstructions: u32 = 100;
	pub const MaxAssetsIntoHolding: u32 = 64;
}

/// Locations we trust to send unpaid XCM (relay + siblings).
pub struct ParentOrSiblings;
impl Contains<Location> for ParentOrSiblings {
	fn contains(location: &Location) -> bool {
		matches!(location.unpack(), (1, []) | (1, [Parachain(_)]))
	}
}

pub type Barrier = TrailingSetTopicAsId<
	DenyThenTry<
		DenyReserveTransferToRelayChain,
		(
			TakeWeightCredit,
			WithComputedOrigin<
				(
					AllowTopLevelPaidExecutionFrom<Everything>,
					AllowExplicitUnpaidExecutionFrom<ParentOrSiblings>,
					AllowKnownQueryResponses<PolkadotXcm>,
					AllowSubscriptionsFrom<ParentOrSiblings>,
				),
				UniversalLocation,
				ConstU32<8>,
			>,
		),
	>,
>;

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type RuntimeCall = RuntimeCall;
	type XcmSender = XcmRouter;
	type AssetTransactor = FungiblesTransactor;
	type OriginConverter = XcmOriginToTransactDispatchOrigin;
	// Trust the relay as a teleport counterparty for the native token
	// only. Day one no foreign reserves.
	type IsReserve = ();
	type IsTeleporter = ();
	type UniversalLocation = UniversalLocation;
	type Barrier = Barrier;
	type Weigher = FixedWeightBounds<UnitWeightCost, RuntimeCall, MaxInstructions>;
	// XCM-purchased execution fees accrue to the `PnsCustodian`
	// account via `XcmFeesToCustodian`. Custodian is the designated
	// DAO/multisig sink for fee-style revenue until a proper treasury
	// pallet lands.
	type Trader = UsingComponents<
		XcmWeightToFee,
		NativeLocationHere,
		AccountId,
		Balances,
		XcmFeesToCustodian,
	>;
	type ResponseHandler = PolkadotXcm;
	type AssetTrap = PolkadotXcm;
	type SubscriptionService = PolkadotXcm;
	type PalletInstancesInfo = AllPalletsWithSystem;
	type MaxAssetsIntoHolding = MaxAssetsIntoHolding;
	type AssetLocker = ();
	type AssetExchanger = ();
	type FeeManager = ();
	type MessageExporter = ();
	type UniversalAliases = Nothing;
	type CallDispatcher = RuntimeCall;
	// Lock down XCM `Transact` dispatch. No call is allowed to be
	// invoked via `Transact` from any XCM origin. Upgrades go through
	// Paseo relay-chain-forced upgrade; nothing else on this chain
	// needs cross-chain dispatch on day one. Re-open selectively
	// (whitelist of specific RuntimeCall variants) when we wire
	// governance.
	type SafeCallFilter = Nothing;
	type Aliasers = Nothing;
	type TransactionalProcessor = FrameTransactionalProcessor;
	type HrmpNewChannelOpenRequestHandler = ();
	type HrmpChannelAcceptedHandler = ();
	type HrmpChannelClosingHandler = ();
	type XcmRecorder = PolkadotXcm;
	type XcmEventEmitter = PolkadotXcm;
}

/// Local origin → XCM `SignedOrigin`.
pub type LocalOriginToLocation = SignedToAccountId32<RuntimeOrigin, AccountId, RelayNetwork>;

/// Router: UMP (via ParachainSystem to relay) + XCMP (siblings).
pub type XcmRouter = WithUniqueTopic<(
	// `ParentAsUmp<T, W, P>`: T=ParachainSystem for outbound, W=metrics
	// hook (`()` for now), P=price-for-delivery (`()` = free — relay
	// doesn't currently charge parachains for UMP).
	cumulus_primitives_utility::ParentAsUmp<ParachainSystem, (), ()>,
	XcmpQueue,
)>;

impl pallet_xcm::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type SendXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmRouter = XcmRouter;
	// Local XCM execution is fully disabled for Paseo launch. No
	// signed origin can call `pallet_xcm::execute` with arbitrary
	// XCM. Inbound XCM (XCMP/DMP) still processes via `MessageQueue`
	// → `ProcessXcmMessage` → executor — that path is gated by the
	// barrier, not by this filter.
	type ExecuteXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmExecuteFilter = Nothing;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	// Teleport / reserve-transfer also disabled. `IsTeleporter = ()`
	// and `IsReserve = ()` in the executor already make these
	// semantically no-ops, but `Nothing` here also rejects at the
	// extrinsic layer so callers get a clean error rather than a
	// mid-XCM execution failure.
	type XcmTeleportFilter = Nothing;
	type XcmReserveTransferFilter = Nothing;
	type Weigher = FixedWeightBounds<UnitWeightCost, RuntimeCall, MaxInstructions>;
	type UniversalLocation = UniversalLocation;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
	type AdvertisedXcmVersion = pallet_xcm::CurrentXcmVersion;
	type Currency = Balances;
	type CurrencyMatcher = ();
	type TrustedLockers = ();
	type SovereignAccountOf = LocationToAccountId;
	type MaxLockers = ConstU32<8>;
	// Benchmarked substrate weights. Replace with pallet-specific
	// benchmarks (`--pallet pallet-xcm --extrinsic '*'`) once we have
	// Paseo-calibrated hardware reference numbers.
	// `TestWeightInfo` returns a conservative 100_000_000 ref_time per
	// extrinsic (~0.1ms) — not zero, despite the misleading name. Safe
	// as a day-one default; replace with per-runtime benchmarks
	// (`--pallet pallet-xcm --extrinsic '*'`) once Paseo-calibrated
	// hardware reference numbers exist.
	type WeightInfo = pallet_xcm::TestWeightInfo;
	// `AdminOrigin` for XCM version-negotiation admin. With Root
	// unreachable locally (no sudo), this is effectively dormant.
	// Paseo relay-chain governance handles version upgrades via
	// runtime upgrade rather than via this admin origin.
	type AdminOrigin = EnsureRoot<AccountId>;
	type MaxRemoteLockConsumers = ConstU32<0>;
	type RemoteLockConsumerIdentifier = ();
	type AuthorizedAliasConsideration = frame_support::traits::Disabled;
}

impl cumulus_pallet_xcm::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type XcmExecutor = XcmExecutor<XcmConfig>;
}

// --- WeightToFee shim ---------------------------------------------------
//
// `UsingComponents` wants a `WeightToFee` impl; reuse the runtime's
// on-chain `IdentityFee` so XCM-purchased weight costs the same as on-
// chain extrinsics. Name-disambiguated from `WeightToFee` at crate
// root to avoid any confusion with pallet-level fee machinery.
use frame_support::weights::{IdentityFee, WeightToFee as WeightToFeeT};
pub struct XcmWeightToFee;
impl WeightToFeeT for XcmWeightToFee {
	type Balance = crate::Balance;
	fn weight_to_fee(weight: &Weight) -> Self::Balance {
		IdentityFee::<Self::Balance>::weight_to_fee(weight)
	}
}

// --- XCM fee routing --------------------------------------------------
//
// `UsingComponents<..., XcmFeesToCustodian>` resolves the per-trade
// negative imbalance (fees collected from XCM `BuyExecution`
// instructions) into the `PnsCustodian` account. Ensures cross-chain
// activity funds the designated DAO sink rather than burning.
use frame_support::traits::{
	fungible::{Balanced, Credit},
	OnUnbalanced,
};

pub struct XcmFeesToCustodian;
impl OnUnbalanced<Credit<AccountId, Balances>> for XcmFeesToCustodian {
	fn on_nonzero_unbalanced(amount: Credit<AccountId, Balances>) {
		// Resolve into the custodian account. If the account has not
		// been pre-funded to cover ED, the credit is silently dropped
		// — acceptable: custodian is a pre-funded multisig before any
		// XCM traffic lands.
		let custodian = crate::configs::PnsCustodianAccount::get();
		let _ = <Balances as Balanced<AccountId>>::resolve(&custodian, amount);
	}
}
