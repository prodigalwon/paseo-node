//! PNS advanced regression — the multi-account coordination flows:
//! top-level gifting via marketplace buy-for-recipient, accept/reject
//! paths, offer-window expiry semantics, and the subdomain authority
//! / depth split. Complements pns_red_team_regression +
//! pns_core_regression.

mod common;

use common::*;
use frame_support::{
    assert_noop, assert_ok,
    traits::{fungible::InspectHold, Currency},
    BoundedVec,
};
use pallet_timestamp::Now as TimestampNow;
use paseo_runtime::{
    Balances, PnsMarketplace, PnsRegistrar, Runtime, RuntimeHoldReason, RuntimeOrigin, System,
    UNIT,
};
use pns_types::{parse_name_to_node, NATIVE_BASENODE};
use sp_runtime::BuildStorage;

// ──────────────────────────────────────────────────────────────────────
// Harness (same shape as other PNS regression files)
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

const DAY_MS: u64 = 86_400_000;

/// Sets up Carol with a canonical name and an active listing priced
/// at 1 DOT expiring in 10 days. Returns the listed name's
/// namehash.
fn setup_listed_name(seller: [u8; 32], name: &[u8], price: u128) -> [u8; 32] {
    register(seller, name);
    let expires_at = TimestampNow::<Runtime>::get() + 10 * DAY_MS;
    assert_ok!(PnsMarketplace::create_listing(
        RuntimeOrigin::signed(account(seller)),
        price,
        expires_at,
    ));
    nh(name)
}

// ──────────────────────────────────────────────────────────────────────
// Gift flow
// ──────────────────────────────────────────────────────────────────────

#[test]
fn marketplace_buy_for_recipient_creates_pending_offer() {
    run_pns(|| {
        let node = setup_listed_name(CAROL_USER, b"giftable", 2 * UNIT);

        let eve_balance_before = Balances::free_balance(&account(EVE_USER));

        // Dave buys on behalf of Eve.
        assert_ok!(PnsMarketplace::buy_name(
            RuntimeOrigin::signed(account(DAVE_USER)),
            b"giftable".to_vec(),
            Some(account(EVE_USER)),
        ));

        // Eve does NOT become owner yet — the name enters OfferedNames.
        let offer = pns_registrar::registrar::OfferedNames::<Runtime>::get(sp_core::H256(node))
            .expect("gift purchase creates a pending OfferedNames row");
        assert_eq!(offer.recipient, account(EVE_USER));
        assert_eq!(offer.buyer, account(DAVE_USER));

        assert!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(EVE_USER))
                .is_none(),
            "recipient is not owner until they accept",
        );

        // Eve didn't pay anything.
        assert_eq!(Balances::free_balance(&account(EVE_USER)), eve_balance_before);

        // Listing was cleaned up.
        assert!(
            pns_marketplace::Listings::<Runtime>::get(sp_core::H256(node)).is_none(),
            "listing must be removed after successful buy",
        );
        // Seller's listing deposit was refunded.
        assert_eq!(held(listing_hold_reason(), CAROL_USER), 0);
    });
}

#[test]
fn accept_offered_name_completes_gift() {
    run_pns(|| {
        let node = setup_listed_name(CAROL_USER, b"giftaccept", 2 * UNIT);

        assert_ok!(PnsMarketplace::buy_name(
            RuntimeOrigin::signed(account(DAVE_USER)),
            b"giftaccept".to_vec(),
            Some(account(EVE_USER)),
        ));

        // Eve accepts.
        assert_ok!(PnsRegistrar::accept_offered_name(
            RuntimeOrigin::signed(account(EVE_USER)),
            b"giftaccept".to_vec(),
        ));

        // Offered entry consumed.
        assert!(
            pns_registrar::registrar::OfferedNames::<Runtime>::get(sp_core::H256(node)).is_none(),
        );
        // Eve is now owner.
        assert_eq!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(EVE_USER)),
            Some(sp_core::H256(node)),
        );
        // Carol is no longer owner.
        assert!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(CAROL_USER))
                .is_none(),
        );
    });
}

#[test]
fn reject_offered_name_via_register_burns_gift() {
    run_pns(|| {
        let node = setup_listed_name(CAROL_USER, b"giftreject", 2 * UNIT);

        assert_ok!(PnsMarketplace::buy_name(
            RuntimeOrigin::signed(account(DAVE_USER)),
            b"giftreject".to_vec(),
            Some(account(EVE_USER)),
        ));

        // Eve declines by submitting a fresh register with
        // `reject_offer` pointing at the gift name. This burns the
        // pending NFT and frees the registration slot; Eve then
        // registers her own separate name in the same extrinsic.
        assert_ok!(PnsRegistrar::register(
            RuntimeOrigin::signed(account(EVE_USER)),
            b"eveownname".to_vec(),
            Some(b"giftreject".to_vec()),
        ));

        // Gift offer consumed; gift NFT burned.
        assert!(
            pns_registrar::registrar::OfferedNames::<Runtime>::get(sp_core::H256(node)).is_none(),
        );
        let class_id: u32 = 0;
        assert!(
            pns_registrar::nft::Tokens::<Runtime>::get(class_id, sp_core::H256(node)).is_none(),
            "rejected gift NFT must be burned",
        );

        // Eve owns her own name, NOT the gift.
        let eve_own = nh(b"eveownname");
        assert_eq!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(EVE_USER)),
            Some(sp_core::H256(eve_own)),
        );
    });
}

#[test]
fn expired_offer_allows_fresh_register() {
    run_pns(|| {
        let _node = setup_listed_name(CAROL_USER, b"windowed", 2 * UNIT);

        assert_ok!(PnsMarketplace::buy_name(
            RuntimeOrigin::signed(account(DAVE_USER)),
            b"windowed".to_vec(),
            Some(account(EVE_USER)),
        ));

        // Eve never accepts. Advance past the 90-day offer window
        // plus the 365-day registration + 30-day grace period — by
        // that point the stale OfferedNames entry AND the stale
        // RegistrarInfos entry should be treated as cleanable.
        TimestampNow::<Runtime>::put(TimestampNow::<Runtime>::get() + 500 * DAY_MS);

        // After the window + the 365-day registration window expires,
        // someone else can register the name fresh. The register()
        // flow detects the expired offer on lookup and cleans it up
        // (see registrar.rs:464-470).
        register(BOB_ISSUER, b"windowed");

        let node = nh(b"windowed");
        assert_eq!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(BOB_ISSUER)),
            Some(sp_core::H256(node)),
        );
        assert!(
            pns_registrar::registrar::OfferedNames::<Runtime>::get(sp_core::H256(node)).is_none(),
            "stale OfferedNames row must be cleaned up on re-registration",
        );
    });
}

// ──────────────────────────────────────────────────────────────────────
// Subdomain security
// ──────────────────────────────────────────────────────────────────────

#[test]
fn subdomain_holder_cannot_offer_further_subdomains() {
    run_pns(|| {
        // Carol owns the top-level "top".
        register(CAROL_USER, b"top");
        // Bob accepts a subname under it.
        assert_ok!(PnsRegistrar::offer_subdomain(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"mid".to_vec(),
            lookup(account(BOB_ISSUER)),
        ));
        assert_ok!(PnsRegistrar::accept_subdomain(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            b"top".to_vec(),
            b"mid".to_vec(),
        ));

        // Bob is a subname holder, not a canonical-name owner.
        // `offer_subdomain` derives parent from OwnerToPrimaryName —
        // Bob doesn't have one. Depth enforcement is structural:
        // only canonical owners can offer, and subname holders
        // never become canonical owners.
        assert_noop!(
            PnsRegistrar::offer_subdomain(
                RuntimeOrigin::signed(account(BOB_ISSUER)),
                b"deep".to_vec(),
                lookup(account(DAVE_USER)),
            ),
            pns_registrar::registrar::Error::<Runtime>::NoCanonicalName,
        );
    });
}

#[test]
fn cannot_own_subname_under_own_domain() {
    run_pns(|| {
        register(CAROL_USER, b"mine");
        // Carol tries to offer a subname to herself. The
        // registry's `offer_subname` rejects — she's the parent
        // NFT owner and the check at registry.rs:692 fires.
        assert_noop!(
            PnsRegistrar::offer_subdomain(
                RuntimeOrigin::signed(account(CAROL_USER)),
                b"self".to_vec(),
                lookup(account(CAROL_USER)),
            ),
            pns_registrar::registry::pallet::Error::<Runtime>::CannotOwnSubnameUnderOwnDomain,
        );
    });
}

#[test]
fn release_subdomain_active_holder_only() {
    run_pns(|| {
        register(CAROL_USER, b"authtest");
        assert_ok!(PnsRegistrar::offer_subdomain(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"bobsub".to_vec(),
            lookup(account(BOB_ISSUER)),
        ));
        assert_ok!(PnsRegistrar::accept_subdomain(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            b"authtest".to_vec(),
            b"bobsub".to_vec(),
        ));

        // Dave (not the holder) attempts release — must fail.
        assert_noop!(
            PnsRegistrar::release_subdomain(
                RuntimeOrigin::signed(account(DAVE_USER)),
                b"authtest".to_vec(),
                b"bobsub".to_vec(),
            ),
            pns_registrar::registry::pallet::Error::<Runtime>::NotSubnameTarget,
        );

        // Bob (the actual holder) can release cleanly.
        assert_ok!(PnsRegistrar::release_subdomain(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            b"authtest".to_vec(),
            b"bobsub".to_vec(),
        ));
    });
}

#[test]
fn revoke_subdomain_parent_authority_only() {
    run_pns(|| {
        register(CAROL_USER, b"revauth");
        assert_ok!(PnsRegistrar::offer_subdomain(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"rsub".to_vec(),
            lookup(account(BOB_ISSUER)),
        ));
        assert_ok!(PnsRegistrar::accept_subdomain(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            b"revauth".to_vec(),
            b"rsub".to_vec(),
        ));

        // Dave has no canonical name — revoke_subdomain derives
        // parent from OwnerToPrimaryName, which is None for Dave,
        // so he can't even look up a subname to revoke.
        assert_noop!(
            PnsRegistrar::revoke_subdomain(
                RuntimeOrigin::signed(account(DAVE_USER)),
                b"rsub".to_vec(),
            ),
            pns_registrar::registrar::Error::<Runtime>::NoCanonicalName,
        );

        // Give Dave his own top-level. He STILL can't revoke a
        // subname issued under Carol's tree — his revoke would
        // target `rsub.davetop`, not `rsub.revauth`.
        register(DAVE_USER, b"davetop");
        assert_noop!(
            PnsRegistrar::revoke_subdomain(
                RuntimeOrigin::signed(account(DAVE_USER)),
                b"rsub".to_vec(),
            ),
            pns_registrar::registry::pallet::Error::<Runtime>::SubnameNotFound,
        );

        // Carol (the legitimate parent) can revoke cleanly.
        assert_ok!(PnsRegistrar::revoke_subdomain(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"rsub".to_vec(),
        ));
    });
}
