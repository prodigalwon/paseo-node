//! PNS core regression — happy-path and hold-rotation invariants
//! across the non-red-team extrinsic surface. Run alongside
//! `pns_red_team_regression.rs`; no overlap.
//!
//! Coverage targets:
//!   - One-canonical-name invariant
//!   - Case-insensitive name collision
//!   - Reserved-name rejection
//!   - `renew` hold rotation (release old → hold new)
//!   - `transfer` hold rotation (sender → recipient)
//!   - `release_name` burns NFT + refunds hold
//!   - Marketplace: list → cancel refunds seller hold
//!   - Marketplace: list → buy rotates ownership + deposits
//!   - Subdomain revoke clears active subname records
//!   - Subdomain reject preserves offerer visibility

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
// PNS-aware TestExternalities + helpers
// Duplicates pns_red_team_regression.rs so the two files stay
// self-contained; the two sets of tests don't share state.
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
            1000 * UNIT,
            100 * UNIT,
            45 * UNIT,
            25 * UNIT,
            10 * UNIT,
            UNIT / 2,
            UNIT / 2,
            UNIT / 2,
            UNIT / 2,
            UNIT / 2,
            UNIT / 2,
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

    // Reserved name seeding — required by the reserved-name regression.
    pns_registrar::registrar::GenesisConfig::<Runtime> {
        infos: Default::default(),
        reserved_list: Default::default(),
        reserved_names: vec![b"polkadot".to_vec()],
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
// Invariants
// ──────────────────────────────────────────────────────────────────────

#[test]
fn register_enforces_one_canonical_name_per_account() {
    run_pns(|| {
        register(CAROL_USER, b"firstname");
        assert_noop!(
            PnsRegistrar::register(
                RuntimeOrigin::signed(account(CAROL_USER)),
                b"secondname".to_vec(),
                None,
            ),
            pns_registrar::registrar::Error::<Runtime>::AlreadyHasCanonicalName,
        );
    });
}

#[test]
fn register_is_case_insensitive() {
    run_pns(|| {
        register(CAROL_USER, b"alice");
        // "ALICE" normalizes to the same namehash as "alice"; Dave's
        // attempt should fail because the NFT already exists.
        assert_noop!(
            PnsRegistrar::register(
                RuntimeOrigin::signed(account(DAVE_USER)),
                b"ALICE".to_vec(),
                None,
            ),
            pns_registrar::registrar::Error::<Runtime>::Occupied,
        );
    });
}

#[test]
fn register_rejects_reserved_names() {
    run_pns(|| {
        // "polkadot" is seeded in pns_ext's reserved list.
        assert_noop!(
            PnsRegistrar::register(
                RuntimeOrigin::signed(account(CAROL_USER)),
                b"polkadot".to_vec(),
                None,
            ),
            pns_registrar::registrar::Error::<Runtime>::Frozen,
        );
        // Non-reserved name still works.
        register(CAROL_USER, b"polkadotish");
    });
}

// ──────────────────────────────────────────────────────────────────────
// Hold rotation
// ──────────────────────────────────────────────────────────────────────

#[test]
fn renew_releases_old_hold_and_places_new() {
    run_pns(|| {
        register(CAROL_USER, b"renewable");
        let node = nh(b"renewable");
        let hold_before = held(cleanup_hold_reason(), CAROL_USER);
        assert!(hold_before > 0);

        // Advance 100 days — still inside renewal window.
        TimestampNow::<Runtime>::put(TimestampNow::<Runtime>::get() + 100 * DAY_MS);

        assert_ok!(PnsRegistrar::renew(RuntimeOrigin::signed(account(CAROL_USER))));

        // Net hold on Carol is the same (old released + new hold),
        // and the CleanupDeposit storage row still points at Carol
        // with the new amount.
        let hold_after = held(cleanup_hold_reason(), CAROL_USER);
        assert_eq!(hold_after, hold_before, "renew rotates hold on same account");

        let (depositor, amount) =
            pns_registrar::registrar::CleanupDeposit::<Runtime>::get(sp_core::H256(node))
                .expect("CleanupDeposit row must survive renew");
        assert_eq!(depositor, account(CAROL_USER));
        assert_eq!(amount, hold_before);
    });
}

#[test]
fn transfer_rotates_cleanup_deposit_to_recipient() {
    run_pns(|| {
        register(CAROL_USER, b"moveable");
        let node = nh(b"moveable");
        let carol_hold_before = held(cleanup_hold_reason(), CAROL_USER);
        assert!(carol_hold_before > 0);
        assert_eq!(held(cleanup_hold_reason(), EVE_USER), 0);

        assert_ok!(PnsRegistrar::transfer(
            RuntimeOrigin::signed(account(CAROL_USER)),
            lookup(account(EVE_USER)),
        ));

        // Hold moved from Carol to Eve; storage row updated.
        assert_eq!(
            held(cleanup_hold_reason(), CAROL_USER),
            0,
            "sender's hold must be released on transfer",
        );
        assert_eq!(
            held(cleanup_hold_reason(), EVE_USER),
            carol_hold_before,
            "recipient must hold the same amount",
        );
        let (depositor, _) =
            pns_registrar::registrar::CleanupDeposit::<Runtime>::get(sp_core::H256(node))
                .expect("CleanupDeposit row must point at new owner");
        assert_eq!(depositor, account(EVE_USER));
    });
}

#[test]
fn release_name_burns_nft_and_refunds_hold() {
    run_pns(|| {
        register(CAROL_USER, b"goingaway");
        let node = nh(b"goingaway");
        let hold_before = held(cleanup_hold_reason(), CAROL_USER);
        assert!(hold_before > 0);

        assert_ok!(PnsRegistrar::release_name(
            RuntimeOrigin::signed(account(CAROL_USER)),
        ));

        // Hold refunded.
        assert_eq!(held(cleanup_hold_reason(), CAROL_USER), 0);
        // NFT burned.
        let class_id: u32 = 0;
        assert!(
            pns_registrar::nft::Tokens::<Runtime>::get(class_id, sp_core::H256(node)).is_none(),
            "NFT must be burned on release",
        );
        // Owner record cleared.
        assert!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(CAROL_USER))
                .is_none(),
        );
        // CleanupDeposit row gone.
        assert!(
            pns_registrar::registrar::CleanupDeposit::<Runtime>::get(sp_core::H256(node))
                .is_none(),
        );
    });
}

// ──────────────────────────────────────────────────────────────────────
// Marketplace
// ──────────────────────────────────────────────────────────────────────

#[test]
fn marketplace_list_cancel_refunds_seller_deposit() {
    run_pns(|| {
        register(CAROL_USER, b"forsale");
        assert_eq!(held(listing_hold_reason(), CAROL_USER), 0);

        let expires_at = TimestampNow::<Runtime>::get() + 10 * DAY_MS;
        assert_ok!(PnsMarketplace::create_listing(
            RuntimeOrigin::signed(account(CAROL_USER)),
            1 * UNIT,
            expires_at,
        ));
        let listing_hold = held(listing_hold_reason(), CAROL_USER);
        assert!(listing_hold > 0, "listing places a deposit");

        assert_ok!(PnsMarketplace::cancel_listing(
            RuntimeOrigin::signed(account(CAROL_USER)),
        ));

        assert_eq!(
            held(listing_hold_reason(), CAROL_USER),
            0,
            "cancel must refund the listing deposit",
        );
    });
}

#[test]
fn marketplace_buy_atomic_with_payment() {
    run_pns(|| {
        register(CAROL_USER, b"tradename");
        let node = nh(b"tradename");
        let price: u128 = 2 * UNIT;
        let expires_at = TimestampNow::<Runtime>::get() + 10 * DAY_MS;

        assert_ok!(PnsMarketplace::create_listing(
            RuntimeOrigin::signed(account(CAROL_USER)),
            price,
            expires_at,
        ));

        let carol_free_before = Balances::free_balance(&account(CAROL_USER));
        let dave_free_before = Balances::free_balance(&account(DAVE_USER));
        let carol_cleanup_before = held(cleanup_hold_reason(), CAROL_USER);
        assert_eq!(held(cleanup_hold_reason(), DAVE_USER), 0);

        assert_ok!(PnsMarketplace::buy_name(
            RuntimeOrigin::signed(account(DAVE_USER)),
            b"tradename".to_vec(),
            None,
        ));

        // Ownership rotated.
        assert_eq!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(DAVE_USER)),
            Some(sp_core::H256(node)),
        );
        assert!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(CAROL_USER))
                .is_none(),
        );

        // CleanupDeposit row now pointed at Dave.
        let (depositor, _) =
            pns_registrar::registrar::CleanupDeposit::<Runtime>::get(sp_core::H256(node))
                .expect("CleanupDeposit must be rotated to buyer");
        assert_eq!(depositor, account(DAVE_USER));

        // Carol's cleanup hold was released (no longer owner).
        assert_eq!(held(cleanup_hold_reason(), CAROL_USER), 0);
        assert!(held(cleanup_hold_reason(), DAVE_USER) > 0);

        // Carol received payment (net-positive free balance change
        // includes 98% of price + released cleanup hold + released
        // listing hold minus any residual tx accounting). Keep the
        // assertion coarse — the atomicity is the invariant, the
        // exact math is a function of protocol-fee + cleanup-hold.
        let carol_gain = Balances::free_balance(&account(CAROL_USER)) - carol_free_before;
        assert!(
            carol_gain >= (price * 98 / 100) + carol_cleanup_before / 2,
            "seller nets at least 98% of price plus cleanup-hold refund; got {carol_gain}",
        );
        // Dave paid at least the price.
        let dave_loss = dave_free_before - Balances::free_balance(&account(DAVE_USER));
        assert!(dave_loss >= price, "buyer paid at least the listed price; got {dave_loss}");
    });
}

// ──────────────────────────────────────────────────────────────────────
// Subdomain lifecycle
// ──────────────────────────────────────────────────────────────────────

#[test]
fn subdomain_revoke_clears_active_subname_records() {
    run_pns(|| {
        register(CAROL_USER, b"parentsub");

        assert_ok!(PnsRegistrar::offer_subdomain(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"kid".to_vec(),
            lookup(account(BOB_ISSUER)),
        ));
        assert_ok!(PnsRegistrar::accept_subdomain(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            b"parentsub".to_vec(),
            b"kid".to_vec(),
        ));

        let sub_node = parse_name_to_node(b"kid.parentsub", &NATIVE_BASENODE)
            .expect("valid subname")
            .0;

        // Bob sets a TXT record on the live subname.
        use pns_types::ddns::codec_type::RecordType;
        assert_ok!(PnsResolvers::set_record(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            b"kid.parentsub".to_vec(),
            RecordType::TXT,
            content(b"bob-here"),
        ));
        assert!(pns_resolvers::resolvers::pallet::Records::<Runtime>::contains_key(
            sp_core::H256(sub_node),
            RecordType::TXT,
        ));

        // Carol revokes.
        assert_ok!(PnsRegistrar::revoke_subdomain(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"kid".to_vec(),
        ));

        // NFT, record, subname all gone.
        let class_id: u32 = 0;
        assert!(
            pns_registrar::nft::Tokens::<Runtime>::get(class_id, sp_core::H256(sub_node))
                .is_none(),
        );
        assert!(
            !pns_resolvers::resolvers::pallet::Records::<Runtime>::contains_key(
                sp_core::H256(sub_node),
                RecordType::TXT,
            ),
        );
        assert!(
            pns_registrar::registry::pallet::SubnameRecords::<Runtime>::get(sp_core::H256(
                sub_node
            ))
            .is_none(),
        );
    });
}

#[test]
fn subdomain_reject_preserves_offerer_visibility() {
    run_pns(|| {
        register(CAROL_USER, b"parentrej");

        assert_ok!(PnsRegistrar::offer_subdomain(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"sub".to_vec(),
            lookup(account(BOB_ISSUER)),
        ));
        let sub_node = parse_name_to_node(b"sub.parentrej", &NATIVE_BASENODE)
            .expect("valid subname")
            .0;

        // Bob rejects. The record flips to Rejected and stays
        // visible to the offerer (Carol) for audit.
        assert_ok!(PnsRegistrar::reject_subdomain(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            b"parentrej".to_vec(),
            b"sub".to_vec(),
        ));

        let record =
            pns_registrar::registry::pallet::SubnameRecords::<Runtime>::get(sp_core::H256(sub_node))
                .expect("rejected subname record must persist");
        assert!(matches!(record.state, pns_types::SubnameState::Rejected));

        // Pending offer cleared from Bob's inbox.
        assert!(
            !pns_registrar::registry::pallet::OfferedToAccount::<Runtime>::contains_key(
                &account(BOB_ISSUER),
                sp_core::H256(sub_node),
            ),
        );

        // Carol can now revoke to fully clean up.
        assert_ok!(PnsRegistrar::revoke_subdomain(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"sub".to_vec(),
        ));
        assert!(
            pns_registrar::registry::pallet::SubnameRecords::<Runtime>::get(sp_core::H256(
                sub_node
            ))
            .is_none(),
        );
    });
}
