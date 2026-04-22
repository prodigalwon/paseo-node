//! PNS red-team regression — pins the four fixes landed 2026-04-20:
//!
//!   C1  — `register(name, reject_offer)` forces `owner = caller`;
//!         the previous `owner: Lookup::Source` parameter (forced-
//!         registration grief) is gone.
//!   C2  — ORIGIN record is preserved across `transfer`.
//!         Initial-registration block hash survives ownership
//!         changes.
//!   DF1 — `register` re-registration path releases the prior
//!         holder's `CleanupDeposit` hold before placing the new
//!         one.
//!   M1  — `accept_subname_offer` mints the canonical NFT + writes
//!         `RuntimeOrigin[subname] = RuntimeOrigin(parent)`. The
//!         resolver ownership gate now passes for subname holders,
//!         unblocking `set_record` for them.
//!
//! Tests run against the real `paseo_runtime::Runtime` — production
//! Config values, real `HoldReason::CleanupDeposit`, real namehash.

mod common;

use common::*;
use frame_support::{assert_ok, traits::fungible::InspectHold, BoundedVec};
// Direct `Now` storage access avoids Aura's slot/timestamp consistency
// check in `set_timestamp`, which panics when the two aren't advanced
// together. Tests don't care about Aura; they just need Moment math
// to work inside `PnsRegistrar`.
use pallet_timestamp::Now as TimestampNow;
use paseo_runtime::{
    Balances, PnsRegistrar, PnsResolvers, Runtime, RuntimeHoldReason, RuntimeOrigin, System,
};
use pns_types::{parse_name_to_node, NATIVE_BASENODE};
use sp_runtime::BuildStorage;

// ──────────────────────────────────────────────────────────────────────
// PNS-aware TestExternalities — common/mod.rs::new_ext() doesn't seed
// the PNS pallets' genesis (basenode NFT, official account, price
// oracle), so registrar extrinsics fail with `OfficialNotInitiated`.
// This harness wires the PNS genesis alongside balances.
// ──────────────────────────────────────────────────────────────────────

/// The account that owns the basenode NFT and acts as `official` for
/// the registry. Mirrors paseo-runtime's dev genesis (Alice as sudo/
/// official). Using Alice means any test account can register under
/// her TLD.
const PNS_OFFICIAL: [u8; 32] = ALICE_ROOT;

fn pns_ext() -> sp_io::TestExternalities {
    let mut storage = frame_system::GenesisConfig::<Runtime>::default()
        .build_storage()
        .unwrap();
    let initial: u128 = 1_000_000 * paseo_runtime::UNIT;
    pallet_balances::GenesisConfig::<Runtime> {
        balances: vec![
            (account(ALICE_ROOT), initial),
            (account(BOB_ISSUER), initial),
            (account(CAROL_USER), initial),
            (account(DAVE_USER), initial),
            (account(EVE_USER), initial),
        ],
        dev_accounts: None,
    }
    .assimilate_storage(&mut storage)
    .unwrap();

    // PnsNft: seed the basenode NFT for the official account.
    pns_registrar::nft::GenesisConfig::<Runtime> {
        tokens: vec![(
            account(PNS_OFFICIAL),
            vec![],
            (),
            vec![(
                account(PNS_OFFICIAL),
                vec![],
                pns_types::Record::default(),
                pns_types::NATIVE_BASENODE,
            )],
        )],
    }
    .assimilate_storage(&mut storage)
    .unwrap();

    // PnsPriceOracle: tiered pricing by label length (10 slots).
    let unit = paseo_runtime::UNIT;
    pns_registrar::price_oracle::GenesisConfig::<Runtime> {
        base_prices: [
            1000 * unit, // 1 char
            100 * unit,  // 2 chars
            45 * unit,   // 3 chars
            25 * unit,   // 4 chars
            10 * unit,   // 5 chars
            unit / 2,    // 6
            unit / 2,    // 7
            unit / 2,    // 8
            unit / 2,    // 9
            unit / 2,    // 10
            unit / 2,    // 11+
        ],
        rent_prices: [0; 11],
        init_rate: 1,
    }
    .assimilate_storage(&mut storage)
    .unwrap();

    // PnsRegistry: official account owns the basenode and gates issuance.
    pns_registrar::registry::GenesisConfig::<Runtime> {
        official: Some(account(PNS_OFFICIAL)),
        origin: vec![],
    }
    .assimilate_storage(&mut storage)
    .unwrap();

    storage.into()
}

fn run_pns<R>(f: impl FnOnce() -> R) -> R {
    pns_ext().execute_with(|| {
        System::set_block_number(1);
        // Timestamp must be initialized for `T::NowProvider::now()` to
        // return something usable inside register()'s duration math.
        TimestampNow::<Runtime>::put(1_000_000u64);
        f()
    })
}

// ──────────────────────────────────────────────────────────────────────
// Local helpers
// ──────────────────────────────────────────────────────────────────────

fn nh(label: &[u8]) -> [u8; 32] {
    parse_name_to_node(label, &NATIVE_BASENODE)
        .expect("valid label")
        .0
}

fn cleanup_held(who: [u8; 32]) -> u128 {
    <Balances as InspectHold<_>>::balance_on_hold(
        &RuntimeHoldReason::PnsRegistrar(
            pns_registrar::registrar::HoldReason::CleanupDeposit,
        ),
        &account(who),
    )
}

fn register(signer: [u8; 32], name: &[u8]) {
    assert_ok!(PnsRegistrar::register(
        RuntimeOrigin::signed(account(signer)),
        name.to_vec(),
        None,
    ));
}

/// Read ORIGIN record bytes or None. `Records` uses `ValueQuery`, so
/// the raw `get` returns an empty BoundedVec for absent entries —
/// distinguish via `contains_key`.
fn origin_record(node: [u8; 32]) -> Option<Vec<u8>> {
    use pns_resolvers::resolvers::pallet::Records;
    use pns_types::ddns::codec_type::RecordType;
    let key = sp_core::H256(node);
    if Records::<Runtime>::contains_key(key, RecordType::ORIGIN) {
        Some(Records::<Runtime>::get(key, RecordType::ORIGIN).into_inner())
    } else {
        None
    }
}

fn content(bytes: &[u8]) -> pns_resolvers::resolvers::pallet::Content<Runtime> {
    BoundedVec::try_from(bytes.to_vec()).expect("content fits MaxContentLen")
}

// ──────────────────────────────────────────────────────────────────────
// C1 — register forces owner = caller
// ──────────────────────────────────────────────────────────────────────

#[test]
fn register_forces_owner_to_caller() {
    run_pns(|| {
        register(CAROL_USER, b"carolname");

        let node = nh(b"carolname");
        assert_eq!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(CAROL_USER)),
            Some(sp_core::H256(node)),
            "caller must be recorded as the name's canonical owner",
        );

        assert!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(DAVE_USER))
                .is_none(),
            "no third-party account should be canonical-name-set by Carol's register",
        );
        register(DAVE_USER, b"davename");
        assert_eq!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(DAVE_USER)),
            Some(sp_core::H256(nh(b"davename"))),
        );
    });
}

// ──────────────────────────────────────────────────────────────────────
// C2 — ORIGIN record preserved across transfer
// ──────────────────────────────────────────────────────────────────────

#[test]
fn origin_record_preserved_across_transfer() {
    run_pns(|| {
        register(CAROL_USER, b"origintest");
        let node = nh(b"origintest");
        let v1 = origin_record(node).expect("ORIGIN written on register");
        assert_eq!(v1.len(), 32, "ORIGIN is a 32-byte block hash");

        assert!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(EVE_USER))
                .is_none(),
            "Eve must be free to receive the transfer",
        );

        assert_ok!(PnsRegistrar::transfer(
            RuntimeOrigin::signed(account(CAROL_USER)),
            lookup(account(EVE_USER)),
        ));

        assert_eq!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(EVE_USER)),
            Some(sp_core::H256(node)),
        );
        assert!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(CAROL_USER))
                .is_none(),
        );

        let v2 = origin_record(node).expect("ORIGIN must still exist after transfer");
        assert_eq!(
            v1, v2,
            "ORIGIN must be byte-identical post-transfer; otherwise the proof-of-registration block is forgeable via round-trip transfer",
        );
    });
}

// ──────────────────────────────────────────────────────────────────────
// DF1 — CleanupDeposit released on re-registration
// ──────────────────────────────────────────────────────────────────────

#[test]
fn cleanup_deposit_released_on_re_registration() {
    run_pns(|| {
        register(CAROL_USER, b"popular");
        let node = nh(b"popular");
        let carol_hold_before =
            pns_registrar::registrar::CleanupDeposit::<Runtime>::get(sp_core::H256(node))
                .expect("CleanupDeposit row written at register")
                .1;
        assert!(carol_hold_before > 0, "registration places a non-zero hold");
        assert_eq!(cleanup_held(CAROL_USER), carol_hold_before);

        // Advance past expire + grace. Registration is 365 days, grace
        // is 30; advancing 400 days is plenty of margin.
        let one_day_ms: u64 = 86_400_000;
        let advance = 400 * one_day_ms;
        let now = TimestampNow::<Runtime>::get();
        TimestampNow::<Runtime>::put(now + advance);

        // Dave registers the now-expired name. Pre-DF1-fix, the
        // re-register path overwrote CleanupDeposit[node] without
        // releasing Carol's hold — her funds would be stranded
        // forever because HoldReason::CleanupDeposit is releasable
        // only by this pallet, and the only storage row pointing at
        // her funds would be gone.
        register(DAVE_USER, b"popular");

        let (depositor, dave_hold) =
            pns_registrar::registrar::CleanupDeposit::<Runtime>::get(sp_core::H256(node))
                .expect("re-register writes a fresh CleanupDeposit row");
        assert_eq!(depositor, account(DAVE_USER));
        assert!(dave_hold > 0);

        assert_eq!(
            cleanup_held(CAROL_USER),
            0,
            "DF1: prior registrant's hold must be released on re-registration",
        );
        assert_eq!(cleanup_held(DAVE_USER), dave_hold);
    });
}

// ──────────────────────────────────────────────────────────────────────
// M1 — subname holder can set_record
// ──────────────────────────────────────────────────────────────────────

#[test]
fn subname_holder_can_set_record() {
    run_pns(|| {
        register(CAROL_USER, b"parentname");

        assert_ok!(PnsRegistrar::offer_subdomain(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"sub".to_vec(),
            lookup(account(BOB_ISSUER)),
        ));

        assert_ok!(PnsRegistrar::accept_subdomain(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            b"parentname".to_vec(),
            b"sub".to_vec(),
        ));

        let sub_node = parse_name_to_node(b"sub.parentname", &NATIVE_BASENODE)
            .expect("valid subname")
            .0;

        // M1 mint: the canonical NFT is written to Bob.
        let class_id: u32 = 0;
        let nft_token =
            pns_registrar::nft::Tokens::<Runtime>::get(class_id, sp_core::H256(sub_node))
                .expect("M1: accept_subdomain must mint the canonical NFT for the acceptor");
        assert_eq!(nft_token.owner, account(BOB_ISSUER));

        // M1 RuntimeOrigin: subname's origin points at the parent.
        let parent_node = nh(b"parentname");
        let origin = pns_registrar::registry::pallet::RuntimeOrigin::<Runtime>::get(
            sp_core::H256(sub_node),
        );
        assert!(
            matches!(
                origin,
                Some(pns_types::DomainTracing::RuntimeOrigin(h))
                    if h == sp_core::H256(parent_node),
            ),
            "subname RuntimeOrigin must be RuntimeOrigin(parent_node)",
        );

        // M1 effect: Bob can now call set_record. Pre-fix this failed
        // with InvalidPermission because the resolver gate
        // (`registry::Pallet::verify` → `nft::Tokens::get`) found no
        // NFT for the subname.
        use pns_types::ddns::codec_type::RecordType;
        assert_ok!(PnsResolvers::set_record(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            b"sub.parentname".to_vec(),
            RecordType::TXT,
            content(b"bob-owns-this"),
        ));

        let rec = pns_resolvers::resolvers::pallet::Records::<Runtime>::get(
            sp_core::H256(sub_node),
            RecordType::TXT,
        );
        assert_eq!(rec.into_inner(), b"bob-owns-this".to_vec());
    });
}
