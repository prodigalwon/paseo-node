//! PNS lifecycle regression — permissionless cleanup incentive paths
//! and resolver record-type invariants. Closes the remaining gaps
//! left after pns_{red_team,core,advanced}_regression.

mod common;

use common::*;
use frame_support::{
    assert_noop, assert_ok,
    traits::{fungible::InspectHold, Currency},
    BoundedVec,
};
use pallet_timestamp::Now as TimestampNow;
use paseo_runtime::{
    Balances, PnsMarketplace, PnsRegistrar, PnsResolvers, Runtime, RuntimeHoldReason,
    RuntimeOrigin, System, UNIT,
};
use pns_types::{parse_name_to_node, NATIVE_BASENODE};
use sp_runtime::BuildStorage;

// ──────────────────────────────────────────────────────────────────────
// Harness
// ──────────────────────────────────────────────────────────────────────

const PNS_OFFICIAL: [u8; 32] = ALICE_ROOT;

fn pns_ext() -> sp_io::TestExternalities {
    let mut storage = frame_system::GenesisConfig::<Runtime>::default()
        .build_storage()
        .unwrap();
    let initial: u128 = 1_000_000 * UNIT;
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

    pns_registrar::price_oracle::GenesisConfig::<Runtime> {
        base_prices: [
            1000 * UNIT, 100 * UNIT, 45 * UNIT, 25 * UNIT, 10 * UNIT,
            UNIT / 2, UNIT / 2, UNIT / 2, UNIT / 2, UNIT / 2, UNIT / 2,
        ],
        rent_prices: [0; 11],
        init_rate: 1,
    }
    .assimilate_storage(&mut storage)
    .unwrap();

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
        TimestampNow::<Runtime>::put(1_000_000u64);
        f()
    })
}

fn nh(label: &[u8]) -> [u8; 32] {
    parse_name_to_node(label, &NATIVE_BASENODE).expect("valid label").0
}

fn cleanup_hold_reason() -> RuntimeHoldReason {
    RuntimeHoldReason::PnsRegistrar(pns_registrar::registrar::HoldReason::CleanupDeposit)
}

fn listing_hold_reason() -> RuntimeHoldReason {
    RuntimeHoldReason::PnsMarketplace(pns_marketplace::HoldReason::ListingDeposit)
}

fn held(reason: RuntimeHoldReason, who: [u8; 32]) -> u128 {
    <Balances as InspectHold<_>>::balance_on_hold(&reason, &account(who))
}

fn register(signer: [u8; 32], name: &[u8]) {
    assert_ok!(PnsRegistrar::register(
        RuntimeOrigin::signed(account(signer)),
        name.to_vec(),
        None,
    ));
}

fn content(bytes: &[u8]) -> pns_resolvers::resolvers::pallet::Content<Runtime> {
    BoundedVec::try_from(bytes.to_vec()).expect("content fits MaxContentLen")
}

const DAY_MS: u64 = 86_400_000;

// ──────────────────────────────────────────────────────────────────────
// Marketplace cleanup incentive path
// ──────────────────────────────────────────────────────────────────────

#[test]
fn cleanup_listing_after_grace_pays_caller() {
    run_pns(|| {
        register(CAROL_USER, b"staleplan");
        let expires_at = TimestampNow::<Runtime>::get() + 5 * DAY_MS;
        assert_ok!(PnsMarketplace::create_listing(
            RuntimeOrigin::signed(account(CAROL_USER)),
            1 * UNIT,
            expires_at,
        ));
        assert!(held(listing_hold_reason(), CAROL_USER) > 0);

        // Past expires_at + ListingGracePeriod (7 days).
        TimestampNow::<Runtime>::put(expires_at + 10 * DAY_MS);

        let dave_free_before = Balances::free_balance(&account(DAVE_USER));
        assert_ok!(PnsMarketplace::cleanup_listing(
            RuntimeOrigin::signed(account(DAVE_USER)),
            b"staleplan".to_vec(),
        ));

        // Carol's listing deposit released.
        assert_eq!(held(listing_hold_reason(), CAROL_USER), 0);
        // Dave received the bounty.
        let dave_gain = Balances::free_balance(&account(DAVE_USER)) - dave_free_before;
        assert!(dave_gain > 0, "cleanup caller paid from seller's listing deposit");
        // Listing entry gone.
        let node = nh(b"staleplan");
        assert!(pns_marketplace::Listings::<Runtime>::get(sp_core::H256(node)).is_none());
    });
}

#[test]
fn cleanup_listing_before_grace_rejects() {
    run_pns(|| {
        register(CAROL_USER, b"livelist");
        let expires_at = TimestampNow::<Runtime>::get() + 5 * DAY_MS;
        assert_ok!(PnsMarketplace::create_listing(
            RuntimeOrigin::signed(account(CAROL_USER)),
            1 * UNIT,
            expires_at,
        ));

        // Still in the listing's active window — reject.
        assert_noop!(
            PnsMarketplace::cleanup_listing(
                RuntimeOrigin::signed(account(DAVE_USER)),
                b"livelist".to_vec(),
            ),
            pns_marketplace::Error::<Runtime>::ListingNotExpired,
        );

        // Past expires_at but within 7-day grace — still reject.
        TimestampNow::<Runtime>::put(expires_at + 1 * DAY_MS);
        assert_noop!(
            PnsMarketplace::cleanup_listing(
                RuntimeOrigin::signed(account(DAVE_USER)),
                b"livelist".to_vec(),
            ),
            pns_marketplace::Error::<Runtime>::ListingNotExpired,
        );
    });
}

// ──────────────────────────────────────────────────────────────────────
// Registrar cleanup incentive path
// ──────────────────────────────────────────────────────────────────────

#[test]
fn registrar_cleanup_expired_name_pays_caller() {
    run_pns(|| {
        register(CAROL_USER, b"expiring");
        let node = nh(b"expiring");
        assert!(held(cleanup_hold_reason(), CAROL_USER) > 0);

        // Deadline = expire + 30-day grace.
        let info = pns_registrar::registrar::RegistrarInfos::<Runtime>::get(sp_core::H256(node))
            .expect("RegistrarInfos row written at register");
        let deadline: u64 = info.expire + 30 * DAY_MS;

        // Advance one ms past the deadline.
        TimestampNow::<Runtime>::put(deadline + 1);

        let dave_free_before = Balances::free_balance(&account(DAVE_USER));
        assert_ok!(PnsRegistrar::cleanup(
            RuntimeOrigin::signed(account(DAVE_USER)),
            deadline,
        ));

        // Carol's cleanup deposit released + paid to Dave.
        assert_eq!(held(cleanup_hold_reason(), CAROL_USER), 0);
        let dave_gain = Balances::free_balance(&account(DAVE_USER)) - dave_free_before;
        assert!(dave_gain > 0, "cleanup caller paid from depositor's hold");

        // State torn down.
        let class_id: u32 = 0;
        assert!(
            pns_registrar::nft::Tokens::<Runtime>::get(class_id, sp_core::H256(node)).is_none(),
        );
        assert!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(CAROL_USER))
                .is_none(),
        );
        assert!(
            pns_registrar::registrar::CleanupDeposit::<Runtime>::get(sp_core::H256(node))
                .is_none(),
        );
        assert!(
            pns_registrar::registrar::RegistrarInfos::<Runtime>::get(sp_core::H256(node))
                .is_none(),
        );
    });
}

#[test]
fn registrar_cleanup_before_grace_rejects() {
    run_pns(|| {
        register(CAROL_USER, b"premature");
        let node = nh(b"premature");
        let info = pns_registrar::registrar::RegistrarInfos::<Runtime>::get(sp_core::H256(node))
            .expect("RegistrarInfos row");
        let deadline: u64 = info.expire + 30 * DAY_MS;

        // Deadline is in the future — cleanup must reject.
        assert_noop!(
            PnsRegistrar::cleanup(
                RuntimeOrigin::signed(account(DAVE_USER)),
                deadline,
            ),
            pns_registrar::registrar::Error::<Runtime>::NotExpired,
        );
    });
}

// ──────────────────────────────────────────────────────────────────────
// Resolver invariants
// ──────────────────────────────────────────────────────────────────────

#[test]
fn set_record_rejects_non_user_settable_types() {
    run_pns(|| {
        register(CAROL_USER, b"recordtest");
        use pns_types::ddns::codec_type::RecordType;

        // ORIGIN is chain-managed — caller cannot override it via
        // set_record. The USER_SETTABLE whitelist (resolvers.rs:211)
        // rejects attempts.
        assert_noop!(
            PnsResolvers::set_record(
                RuntimeOrigin::signed(account(CAROL_USER)),
                b"recordtest".to_vec(),
                RecordType::ORIGIN,
                content(b"\x00".repeat(32).as_slice()),
            ),
            pns_resolvers::resolvers::pallet::Error::<Runtime>::InvalidRecordType,
        );

        // SS58 is also chain-managed — written implicitly by
        // register/transfer/buy, not settable via extrinsic.
        assert_noop!(
            PnsResolvers::set_record(
                RuntimeOrigin::signed(account(CAROL_USER)),
                b"recordtest".to_vec(),
                RecordType::SS58,
                content(b"\x11".repeat(32).as_slice()),
            ),
            pns_resolvers::resolvers::pallet::Error::<Runtime>::InvalidRecordType,
        );

        // User-settable types (TXT) still work.
        assert_ok!(PnsResolvers::set_record(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"recordtest".to_vec(),
            RecordType::TXT,
            content(b"hello world"),
        ));
    });
}

#[test]
fn records_cleared_on_transfer_except_ss58_and_origin() {
    run_pns(|| {
        register(CAROL_USER, b"transrec");
        let node = nh(b"transrec");
        use pns_types::ddns::codec_type::RecordType;

        // Carol sets a few user-settable records before transferring.
        for (rt, c) in [
            (RecordType::TXT, b"carol-txt" as &[u8]),
            (RecordType::AVATAR, b"ipfs://carolavatar" as &[u8]),
            (RecordType::RPC, b"wss://carol.dot:9944" as &[u8]),
        ] {
            assert_ok!(PnsResolvers::set_record(
                RuntimeOrigin::signed(account(CAROL_USER)),
                b"transrec".to_vec(),
                rt,
                content(c),
            ));
        }

        // SS58 + ORIGIN are pre-written by register.
        assert!(pns_resolvers::resolvers::pallet::Records::<Runtime>::contains_key(
            sp_core::H256(node), RecordType::SS58,
        ));
        assert!(pns_resolvers::resolvers::pallet::Records::<Runtime>::contains_key(
            sp_core::H256(node), RecordType::ORIGIN,
        ));
        let origin_before = pns_resolvers::resolvers::pallet::Records::<Runtime>::get(
            sp_core::H256(node), RecordType::ORIGIN,
        ).into_inner();

        // Transfer.
        assert_ok!(PnsRegistrar::transfer(
            RuntimeOrigin::signed(account(CAROL_USER)),
            lookup(account(EVE_USER)),
        ));

        // User-set records cleared; SS58 + ORIGIN preserved.
        for rt in [RecordType::TXT, RecordType::AVATAR, RecordType::RPC] {
            assert!(
                !pns_resolvers::resolvers::pallet::Records::<Runtime>::contains_key(
                    sp_core::H256(node), rt,
                ),
                "{rt:?} must be cleared on transfer",
            );
        }
        assert!(pns_resolvers::resolvers::pallet::Records::<Runtime>::contains_key(
            sp_core::H256(node), RecordType::SS58,
        ));
        assert!(pns_resolvers::resolvers::pallet::Records::<Runtime>::contains_key(
            sp_core::H256(node), RecordType::ORIGIN,
        ));
        // ORIGIN bytes unchanged (C2 invariant — doubly pinned here).
        let origin_after = pns_resolvers::resolvers::pallet::Records::<Runtime>::get(
            sp_core::H256(node), RecordType::ORIGIN,
        ).into_inner();
        assert_eq!(origin_before, origin_after, "ORIGIN must survive transfer byte-identical");
    });
}

#[test]
fn set_multiple_record_types_all_queryable() {
    run_pns(|| {
        register(CAROL_USER, b"multirec");
        let node = nh(b"multirec");
        use pns_types::ddns::codec_type::RecordType;

        let cases: &[(RecordType, &[u8])] = &[
            (RecordType::A, &[192, 0, 2, 1]),
            (RecordType::AAAA, &[0x20, 0x01, 0xdb, 0x8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
            (RecordType::TXT, b"hello"),
            (RecordType::PUBKEY1, b"\x04pubkey1bytes"),
            (RecordType::AVATAR, b"bafybeigcarolcid"),
            (RecordType::CONTENT, b"bafybeigsite"),
        ];

        for (rt, val) in cases {
            assert_ok!(PnsResolvers::set_record(
                RuntimeOrigin::signed(account(CAROL_USER)),
                b"multirec".to_vec(),
                *rt,
                content(val),
            ));
        }

        // Every record readable back with the exact content.
        for (rt, expected) in cases {
            let got = pns_resolvers::resolvers::pallet::Records::<Runtime>::get(
                sp_core::H256(node), *rt,
            );
            assert_eq!(
                got.into_inner(),
                expected.to_vec(),
                "{rt:?} content round-trip mismatch",
            );
        }

        // Updating one record in place preserves the others.
        assert_ok!(PnsResolvers::set_record(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"multirec".to_vec(),
            RecordType::TXT,
            content(b"updated"),
        ));
        let txt_after = pns_resolvers::resolvers::pallet::Records::<Runtime>::get(
            sp_core::H256(node), RecordType::TXT,
        );
        assert_eq!(txt_after.into_inner(), b"updated".to_vec());
        // The A record is untouched.
        let a_after = pns_resolvers::resolvers::pallet::Records::<Runtime>::get(
            sp_core::H256(node), RecordType::A,
        );
        assert_eq!(a_after.into_inner(), vec![192, 0, 2, 1]);
    });
}
