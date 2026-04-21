//! PNS edge-case regression — closes the remaining gap list:
//! release_subdomain happy path, Texts vs Records storage isolation,
//! price-oracle tier boundaries, marketplace input validation, and
//! the full register → release → re-register lifecycle.

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
// Harness (same shape as other PNS regression files)
// ──────────────────────────────────────────────────────────────────────

const PNS_OFFICIAL: [u8; 32] = ALICE_ROOT;

fn pns_ext() -> sp_io::TestExternalities {
    let mut storage = frame_system::GenesisConfig::<Runtime>::default()
        .build_storage()
        .unwrap();
    // Give accounts ~2M DOT so 1-char tier (1000 DOT) tests don't
    // bump against existential deposit edge cases.
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
// Subdomain release — happy path
// ──────────────────────────────────────────────────────────────────────

#[test]
fn release_subdomain_happy_path_clears_subname_state() {
    run_pns(|| {
        register(CAROL_USER, b"parenthappy");
        let parent_node = nh(b"parenthappy");

        assert_ok!(PnsRegistrar::offer_subdomain(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"kid".to_vec(),
            lookup(account(BOB_ISSUER)),
        ));
        assert_ok!(PnsRegistrar::accept_subdomain(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            b"parenthappy".to_vec(),
            b"kid".to_vec(),
        ));

        let sub_node = parse_name_to_node(b"kid.parenthappy", &NATIVE_BASENODE)
            .expect("valid subname")
            .0;
        let class_id: u32 = 0;

        // Pre-release state: NFT exists, subname record Active, parent
        // children counter at 1.
        assert!(pns_registrar::nft::Tokens::<Runtime>::get(class_id, sp_core::H256(sub_node))
            .is_some());
        let parent_token = pns_registrar::nft::Tokens::<Runtime>::get(
            class_id,
            sp_core::H256(parent_node),
        )
        .expect("parent token");
        assert_eq!(parent_token.data.children, 1);

        // Bob sets a TXT record then voluntarily releases.
        use pns_types::ddns::codec_type::RecordType;
        assert_ok!(PnsResolvers::set_record(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            b"kid.parenthappy".to_vec(),
            RecordType::TXT,
            content(b"bobtext"),
        ));
        assert_ok!(PnsRegistrar::release_subdomain(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            b"parenthappy".to_vec(),
            b"kid".to_vec(),
        ));

        // Subname torn down fully.
        assert!(
            pns_registrar::registry::pallet::SubnameRecords::<Runtime>::get(sp_core::H256(sub_node))
                .is_none(),
        );
        assert!(
            pns_registrar::nft::Tokens::<Runtime>::get(class_id, sp_core::H256(sub_node))
                .is_none(),
            "subname NFT must be burned on release",
        );
        assert!(
            pns_registrar::registry::pallet::RuntimeOrigin::<Runtime>::get(sp_core::H256(sub_node))
                .is_none(),
            "subname RuntimeOrigin must be removed on release",
        );
        assert!(!pns_resolvers::resolvers::pallet::Records::<Runtime>::contains_key(
            sp_core::H256(sub_node), RecordType::TXT,
        ));

        // Parent's children counter decremented back to 0.
        let parent_after = pns_registrar::nft::Tokens::<Runtime>::get(
            class_id,
            sp_core::H256(parent_node),
        )
        .expect("parent token survives");
        assert_eq!(parent_after.data.children, 0, "parent children counter returns to zero");
    });
}

// ──────────────────────────────────────────────────────────────────────
// Texts storage isolation
// ──────────────────────────────────────────────────────────────────────

#[test]
fn set_text_stores_in_separate_map_from_records() {
    run_pns(|| {
        register(CAROL_USER, b"texttest");
        let node = nh(b"texttest");
        use pns_resolvers::resolvers::pallet::TextKind;
        use pns_types::ddns::codec_type::RecordType;

        // Set a Twitter handle via set_text. Writes to `Texts`, not
        // `Records`.
        assert_ok!(PnsResolvers::set_text(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"texttest".to_vec(),
            TextKind::Twitter,
            content(b"@carolhandle"),
        ));

        // Visible in Texts.
        let got = pns_resolvers::resolvers::pallet::Texts::<Runtime>::get(
            sp_core::H256(node),
            TextKind::Twitter,
        );
        assert_eq!(got.into_inner(), b"@carolhandle".to_vec());

        // Not visible in Records under any record type — the two
        // stores are separate. Also confirms `set_record(TXT, ...)`
        // wouldn't conflict.
        assert!(
            !pns_resolvers::resolvers::pallet::Records::<Runtime>::contains_key(
                sp_core::H256(node), RecordType::TXT,
            ),
            "Texts and Records are separate stores; Twitter TextKind is not a TXT Record",
        );

        // A Records-side TXT is independent of the Texts entry.
        assert_ok!(PnsResolvers::set_record(
            RuntimeOrigin::signed(account(CAROL_USER)),
            b"texttest".to_vec(),
            RecordType::TXT,
            content(b"dns-txt-value"),
        ));
        // Texts entry still there, unchanged.
        let txt_text = pns_resolvers::resolvers::pallet::Texts::<Runtime>::get(
            sp_core::H256(node),
            TextKind::Twitter,
        );
        assert_eq!(txt_text.into_inner(), b"@carolhandle".to_vec());
        // Records entry is the new one.
        let dns_txt = pns_resolvers::resolvers::pallet::Records::<Runtime>::get(
            sp_core::H256(node),
            RecordType::TXT,
        );
        assert_eq!(dns_txt.into_inner(), b"dns-txt-value".to_vec());
    });
}

// ──────────────────────────────────────────────────────────────────────
// Price-oracle tier boundaries
// ──────────────────────────────────────────────────────────────────────

#[test]
fn registration_fee_scales_by_label_length_tier() {
    run_pns(|| {
        // 1-char tier = 1000 DOT. 5% held as cleanup deposit.
        register(CAROL_USER, b"x");
        let one_char_hold = held(cleanup_hold_reason(), CAROL_USER);
        // 5% of 1000 DOT = 50 DOT held.
        assert_eq!(
            one_char_hold,
            50 * UNIT,
            "1-char tier: 5% cleanup hold = 50 DOT, got {one_char_hold}",
        );

        // 6-char tier = 0.5 DOT. 5% = 0.025 DOT = UNIT / 40.
        register(DAVE_USER, b"sixchr");
        let six_char_hold = held(cleanup_hold_reason(), DAVE_USER);
        assert_eq!(
            six_char_hold,
            UNIT / 40,
            "6-char tier: 5% of 0.5 DOT = 0.025 DOT = UNIT/40, got {six_char_hold}",
        );

        // 3-char tier = 45 DOT. 5% = 2.25 DOT = 45*UNIT/20 = 9*UNIT/4.
        register(EVE_USER, b"abc");
        let three_char_hold = held(cleanup_hold_reason(), EVE_USER);
        assert_eq!(
            three_char_hold,
            (45 * UNIT) / 20,
            "3-char tier: 5% of 45 DOT = 2.25 DOT, got {three_char_hold}",
        );
    });
}

// ──────────────────────────────────────────────────────────────────────
// Marketplace input validation
// ──────────────────────────────────────────────────────────────────────

#[test]
fn marketplace_rejects_past_or_current_expires_at() {
    run_pns(|| {
        register(CAROL_USER, b"listtest");
        let now = TimestampNow::<Runtime>::get();

        // expires_at == now — must reject (must be strictly future).
        assert_noop!(
            PnsMarketplace::create_listing(
                RuntimeOrigin::signed(account(CAROL_USER)),
                1 * UNIT,
                now,
            ),
            pns_marketplace::Error::<Runtime>::ExpiryNotInFuture,
        );

        // expires_at in the past — must reject.
        assert_noop!(
            PnsMarketplace::create_listing(
                RuntimeOrigin::signed(account(CAROL_USER)),
                1 * UNIT,
                now.saturating_sub(1),
            ),
            pns_marketplace::Error::<Runtime>::ExpiryNotInFuture,
        );

        // A future expires_at is accepted.
        assert_ok!(PnsMarketplace::create_listing(
            RuntimeOrigin::signed(account(CAROL_USER)),
            1 * UNIT,
            now + DAY_MS,
        ));
    });
}

#[test]
fn marketplace_rejects_buyer_equals_seller() {
    run_pns(|| {
        register(CAROL_USER, b"selfbuy");
        let expires_at = TimestampNow::<Runtime>::get() + 10 * DAY_MS;
        assert_ok!(PnsMarketplace::create_listing(
            RuntimeOrigin::signed(account(CAROL_USER)),
            1 * UNIT,
            expires_at,
        ));

        // Carol cannot buy her own listing.
        assert_noop!(
            PnsMarketplace::buy_name(
                RuntimeOrigin::signed(account(CAROL_USER)),
                b"selfbuy".to_vec(),
                None,
            ),
            pns_marketplace::Error::<Runtime>::BuyerIsSeller,
        );
    });
}

#[test]
fn marketplace_rejects_buyer_recipient_is_self_or_seller() {
    run_pns(|| {
        register(CAROL_USER, b"giftchk");
        let expires_at = TimestampNow::<Runtime>::get() + 10 * DAY_MS;
        assert_ok!(PnsMarketplace::create_listing(
            RuntimeOrigin::signed(account(CAROL_USER)),
            1 * UNIT,
            expires_at,
        ));

        // Buyer cannot gift to themselves.
        assert_noop!(
            PnsMarketplace::buy_name(
                RuntimeOrigin::signed(account(DAVE_USER)),
                b"giftchk".to_vec(),
                Some(account(DAVE_USER)),
            ),
            pns_marketplace::Error::<Runtime>::BuyerIsRecipient,
        );

        // Buyer cannot gift back to the seller.
        assert_noop!(
            PnsMarketplace::buy_name(
                RuntimeOrigin::signed(account(DAVE_USER)),
                b"giftchk".to_vec(),
                Some(account(CAROL_USER)),
            ),
            pns_marketplace::Error::<Runtime>::SellerIsRecipient,
        );
    });
}

// ──────────────────────────────────────────────────────────────────────
// Full lifecycle
// ──────────────────────────────────────────────────────────────────────

#[test]
fn full_lifecycle_register_release_reregister() {
    run_pns(|| {
        // Phase 1: Carol registers "cyclic".
        register(CAROL_USER, b"cyclic");
        let node = nh(b"cyclic");
        let carol_hold_after_register = held(cleanup_hold_reason(), CAROL_USER);
        assert!(carol_hold_after_register > 0);

        // Phase 2: Carol releases voluntarily. NFT burned, hold
        // refunded, OwnerToPrimaryName cleared.
        assert_ok!(PnsRegistrar::release_name(
            RuntimeOrigin::signed(account(CAROL_USER)),
        ));
        assert_eq!(held(cleanup_hold_reason(), CAROL_USER), 0);
        let class_id: u32 = 0;
        assert!(pns_registrar::nft::Tokens::<Runtime>::get(class_id, sp_core::H256(node))
            .is_none());
        assert!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(CAROL_USER))
                .is_none(),
        );

        // Phase 3: Dave registers the same name — fresh registration,
        // no stranded state from Carol's lifecycle.
        register(DAVE_USER, b"cyclic");
        assert_eq!(
            pns_registrar::registrar::OwnerToPrimaryName::<Runtime>::get(&account(DAVE_USER)),
            Some(sp_core::H256(node)),
        );
        let dave_hold = held(cleanup_hold_reason(), DAVE_USER);
        assert_eq!(
            dave_hold, carol_hold_after_register,
            "Dave's hold matches Carol's original — same label, same tier",
        );
        // Cleanup row points at Dave now.
        let (depositor, _) =
            pns_registrar::registrar::CleanupDeposit::<Runtime>::get(sp_core::H256(node))
                .expect("re-registered name has fresh CleanupDeposit row");
        assert_eq!(depositor, account(DAVE_USER));
    });
}
