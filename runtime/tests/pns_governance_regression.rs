//! PNS governance regression — extrinsics gated by `ManagerOrigin`
//! (EnsureRoot on paseo-runtime): price-oracle admin, reserved-name
//! admin, and `set_official` basenode handover. Covers both the
//! authorization gate (signed origin rejected) and the behavioral
//! effect (state change persists and affects downstream flows).

mod common;

use common::*;
use frame_support::{
    assert_noop, assert_ok,
    traits::{fungible::InspectHold, Currency},
    BoundedVec,
};
use pallet_timestamp::Now as TimestampNow;
use paseo_runtime::{
    Balances, PnsRegistrar, Runtime, RuntimeHoldReason, RuntimeOrigin, System, UNIT,
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
    let initial: u128 = 2_000_000 * UNIT;
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

fn held(reason: RuntimeHoldReason, who: [u8; 32]) -> u128 {
    <Balances as InspectHold<_>>::balance_on_hold(&reason, &account(who))
}

// ──────────────────────────────────────────────────────────────────────
// Price oracle admin
// ──────────────────────────────────────────────────────────────────────

#[test]
fn governance_set_base_price_applied_to_next_mint() {
    run_pns(|| {
        // Baseline: 3-char tier is 45 DOT at genesis.
        // Re-price it to 500 DOT via Root.
        let mut new_prices = [
            1000 * UNIT, 100 * UNIT, 45 * UNIT, 25 * UNIT, 10 * UNIT,
            UNIT / 2, UNIT / 2, UNIT / 2, UNIT / 2, UNIT / 2, UNIT / 2,
        ];
        new_prices[2] = 500 * UNIT; // override 3-char tier

        assert_ok!(pns_registrar::price_oracle::Pallet::<Runtime>::set_base_price(
            RuntimeOrigin::root(),
            new_prices,
        ));

        // Register a 3-char name — should hold 5% of NEW price (25 DOT).
        assert_ok!(PnsRegistrar::register(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"xyz".to_vec(),
            None,
        ));
        assert_eq!(
            held(cleanup_hold_reason(), CAROL_USER),
            500 * UNIT / 20,
            "3-char tier re-priced to 500 DOT; 5% hold = 25 DOT",
        );

        // Storage reflects the new prices.
        let stored = pns_registrar::price_oracle::BasePrice::<Runtime>::get();
        assert_eq!(stored[2], 500 * UNIT);
    });
}

#[test]
fn governance_set_exchange_rate_round_trips() {
    run_pns(|| {
        assert_ok!(pns_registrar::price_oracle::Pallet::<Runtime>::set_exchange_rate(
            RuntimeOrigin::root(),
            42_000_000,
        ));
        assert_eq!(
            pns_registrar::price_oracle::ExchangeRate::<Runtime>::get(),
            42_000_000,
        );
    });
}

#[test]
fn governance_set_rent_price_round_trips() {
    run_pns(|| {
        let new_rent = [
            UNIT, UNIT, UNIT, UNIT, UNIT, UNIT, UNIT, UNIT, UNIT, UNIT, UNIT,
        ];
        assert_ok!(pns_registrar::price_oracle::Pallet::<Runtime>::set_rent_price(
            RuntimeOrigin::root(),
            new_rent,
        ));
        assert_eq!(pns_registrar::price_oracle::RentPrice::<Runtime>::get(), new_rent);
    });
}

// ──────────────────────────────────────────────────────────────────────
// Reserved-name admin
// ──────────────────────────────────────────────────────────────────────

#[test]
fn governance_add_reserved_blocks_future_registration() {
    run_pns(|| {
        // "newresv" is free to register at baseline.
        // Admin reserves it.
        assert_ok!(PnsRegistrar::add_reserved(
            RuntimeOrigin::root(),
            b"newresv".to_vec(),
        ));
        // Now any registration attempt fails with Frozen.
        assert_noop!(
            PnsRegistrar::register(
                RuntimeOrigin::signed(account(CAROL_USER)),
                b"newresv".to_vec(),
                None,
            ),
            pns_registrar::registrar::Error::<Runtime>::Frozen,
        );
        // ReservedList storage has the entry.
        assert!(pns_registrar::registrar::ReservedList::<Runtime>::contains_key(
            sp_core::H256(nh(b"newresv")),
        ));
    });
}

#[test]
fn governance_remove_reserved_unblocks_name() {
    run_pns(|| {
        // Reserve, confirm blocked, unreserve, confirm unblocked.
        assert_ok!(PnsRegistrar::add_reserved(
            RuntimeOrigin::root(),
            b"tempresv".to_vec(),
        ));
        assert_noop!(
            PnsRegistrar::register(
                RuntimeOrigin::signed(account(CAROL_USER)),
                b"tempresv".to_vec(),
                None,
            ),
            pns_registrar::registrar::Error::<Runtime>::Frozen,
        );

        assert_ok!(PnsRegistrar::remove_reserved(
            RuntimeOrigin::root(),
            b"tempresv".to_vec(),
        ));
        // ReservedList entry gone.
        assert!(!pns_registrar::registrar::ReservedList::<Runtime>::contains_key(
            sp_core::H256(nh(b"tempresv")),
        ));

        // Registration now succeeds.
        assert_ok!(PnsRegistrar::register(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"tempresv".to_vec(),
            None,
        ));
        assert_eq!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(CAROL_USER)),
            Some(sp_core::H256(nh(b"tempresv"))),
        );
    });
}

// ──────────────────────────────────────────────────────────────────────
// set_official — basenode NFT handover
// ──────────────────────────────────────────────────────────────────────

#[test]
fn governance_set_official_transfers_basenode_nft() {
    run_pns(|| {
        let class_id: u32 = 0;
        let basenode = pns_types::NATIVE_BASENODE;

        // Pre-state: Alice is official, owns the basenode NFT.
        assert_eq!(
            pns_registrar::registry::Official::<Runtime>::get(),
            Some(account(ALICE_ROOT)),
        );
        let basenode_token_before =
            pns_registrar::nft::Tokens::<Runtime>::get(class_id, basenode)
                .expect("basenode NFT exists at genesis");
        assert_eq!(basenode_token_before.owner, account(ALICE_ROOT));

        // Root hands off to Bob.
        assert_ok!(pns_registrar::registry::Pallet::<Runtime>::set_official(
            RuntimeOrigin::root(),
            account(BOB_ISSUER),
        ));

        // Official updated, NFT transferred, class owner updated.
        assert_eq!(
            pns_registrar::registry::Official::<Runtime>::get(),
            Some(account(BOB_ISSUER)),
        );
        let basenode_token_after =
            pns_registrar::nft::Tokens::<Runtime>::get(class_id, basenode)
                .expect("basenode NFT still exists after set_official");
        assert_eq!(basenode_token_after.owner, account(BOB_ISSUER));

        // Subsequent registrations still work under Bob as official.
        assert_ok!(PnsRegistrar::register(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"afterhand".to_vec(),
            None,
        ));
    });
}

// ──────────────────────────────────────────────────────────────────────
// Authorization — non-root callers cannot invoke any governance extrinsic
// ──────────────────────────────────────────────────────────────────────

#[test]
fn governance_extrinsics_reject_signed_origin() {
    run_pns(|| {
        let arbitrary = [UNIT; 11];

        // Price oracle admin — three extrinsics, all gated by ManagerOrigin.
        assert_noop!(
            pns_registrar::price_oracle::Pallet::<Runtime>::set_base_price(
                RuntimeOrigin::signed(account(CAROL_USER)),
                arbitrary,
            ),
            sp_runtime::DispatchError::BadOrigin,
        );
        assert_noop!(
            pns_registrar::price_oracle::Pallet::<Runtime>::set_rent_price(
                RuntimeOrigin::signed(account(CAROL_USER)),
                arbitrary,
            ),
            sp_runtime::DispatchError::BadOrigin,
        );
        assert_noop!(
            pns_registrar::price_oracle::Pallet::<Runtime>::set_exchange_rate(
                RuntimeOrigin::signed(account(CAROL_USER)),
                1_000_000,
            ),
            sp_runtime::DispatchError::BadOrigin,
        );

        // Reserved-name admin — both extrinsics.
        assert_noop!(
            PnsRegistrar::add_reserved(
                RuntimeOrigin::signed(account(CAROL_USER)),
                b"cantreserve".to_vec(),
            ),
            sp_runtime::DispatchError::BadOrigin,
        );
        assert_noop!(
            PnsRegistrar::remove_reserved(
                RuntimeOrigin::signed(account(CAROL_USER)),
                b"polkadot".to_vec(),
            ),
            sp_runtime::DispatchError::BadOrigin,
        );

        // Basenode handover.
        assert_noop!(
            pns_registrar::registry::Pallet::<Runtime>::set_official(
                RuntimeOrigin::signed(account(CAROL_USER)),
                account(DAVE_USER),
            ),
            sp_runtime::DispatchError::BadOrigin,
        );
    });
}
