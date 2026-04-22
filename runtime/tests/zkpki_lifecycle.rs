//! ZK-PKI lifecycle tests — self-discard recovery, reissue,
//! auto-purge via on_initialize, permissionless cleanup.
//!
//! Runs against real paseo-runtime Config values — in particular
//! `CertDeposit = 1 DOT` and `InactivePurgePeriod = 432_000 blocks`
//! (30 days at 6s blocks — the grace period). Deposit math and the
//! schedule-driven purge path are what this file exercises end-to-end.

mod common;

use common::*;
use frame_support::{assert_ok, traits::{Currency, Hooks}, BoundedVec};
use paseo_runtime::{Balances, Runtime, RuntimeOrigin, System, ZkPki, UNIT};
use zk_pki_primitives::crypto::DevicePublicKey;

// Helper — pre-setup for a minted non-PoP cert held by Carol.
fn setup_carol_cert() -> [u8; 32] {
    register_standard_root();
    issue_standard_issuer();
    create_non_pop_template(b"life-tmpl");
    mint_non_pop_cert(CAROL_USER, b"life-tmpl")
}

// ─── Test 5 — recovery-path self-discard returns deposit to holder ────

#[test]
fn self_discard_recovery_path_returns_deposit() {
    run(|| {
        let thumbprint = setup_carol_cert();
        // Fee system: a non-PoP mint's held deposit is
        // `max(MintFeeNone * DepositBasisPoints / 10_000, MinDeposit)` =
        // `max(2 DOT * 5%, 0.1 DOT)` = 0.1 DOT. Anything else in
        // Carol's reserved balance is unrelated (there isn't any).
        let expected_deposit = UNIT / 10;
        let reserved_before = Balances::reserved_balance(&account(CAROL_USER));
        assert!(
            reserved_before >= expected_deposit,
            "CertDeposit must be held post-mint (got {reserved_before})",
        );

        assert_ok!(ZkPki::self_discard_cert(
            RuntimeOrigin::signed(account(CAROL_USER)),
            thumbprint,
            None, // recovery path — no PopAssertion
        ));

        let reserved_after = Balances::reserved_balance(&account(CAROL_USER));
        assert_eq!(
            reserved_after,
            reserved_before - expected_deposit,
            "exactly the held CertDeposit must be released back to Carol",
        );
        assert!(zk_pki_pallet::CertLookupHot::<Runtime>::get(thumbprint).is_none());
    });
}

// ─── Test 10 — reissue_cert: atomic old-remove + new-write ────────────

#[test]
fn reissue_cert_replaces_old_and_nets_deposit() {
    run(|| {
        let old_thumbprint = setup_carol_cert();
        let issuer_reserved_before =
            Balances::reserved_balance(&account(BOB_ISSUER));

        // Reissue — issuer replaces Carol's cert with new material.
        // Same cert_ec pubkey (simpler — re-using the fixed scalar).
        let new_pubkey = DevicePublicKey::new_p256(&cert_ec_pubkey_bytes()).unwrap();
        let new_meta: BoundedVec<_, _> =
            BoundedVec::try_from(b"reissued".to_vec()).unwrap();
        assert_ok!(ZkPki::reissue_cert(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            old_thumbprint,
            new_pubkey,
            empty_att(),
            20_000u32,
            new_meta,
        ));

        // Old cert is gone.
        assert!(zk_pki_pallet::CertLookupHot::<Runtime>::get(old_thumbprint).is_none());
        assert!(zk_pki_pallet::CertLookupCold::<Runtime>::get(old_thumbprint).is_none());

        // New cert is present, resolvable via UserIssuerIndex.
        let new_thumbprint = resolve_thumbprint(CAROL_USER);
        assert_ne!(new_thumbprint, old_thumbprint);
        assert!(zk_pki_pallet::CertLookupHot::<Runtime>::get(new_thumbprint).is_some());

        // Issuer's reserve went up by exactly 1 DOT — they paid the
        // new cert's deposit. Carol got her old deposit back (which
        // her balance reflects).
        let issuer_reserved_after = Balances::reserved_balance(&account(BOB_ISSUER));
        assert_eq!(
            issuer_reserved_after,
            issuer_reserved_before + UNIT,
            "reissue reserves a fresh CertDeposit from the issuer",
        );
    });
}

// ─── Test 11 — on_initialize auto-purges inactive certs past grace ────

#[test]
fn on_initialize_auto_purges_inactive_cert_past_grace() {
    run(|| {
        let thumbprint = setup_carol_cert();
        let hot = zk_pki_pallet::CertLookupHot::<Runtime>::get(thumbprint).unwrap();
        let expiry = hot.expiry_block;
        // InactivePurgePeriod = 14_400 * 30 (432_000 blocks). We
        // only need to fire on_initialize at the two specific
        // scheduled blocks — ExpiryIndex::take and PurgeIndex::take
        // are O(1) takes, so no per-block looping.
        let inactive_purge: u32 = 14_400 * 30;

        // 1. Advance to expiry — on_initialize flips Hot to Suspended.
        System::set_block_number(expiry);
        let _ = <zk_pki_pallet::Pallet<Runtime> as Hooks<u32>>::on_initialize(expiry);
        let hot = zk_pki_pallet::CertLookupHot::<Runtime>::get(thumbprint)
            .expect("cert still in Hot after expiry flip — just Suspended");
        assert!(!hot.is_active(), "expiry must flip state to Suspended");

        // 2. Advance to the scheduled purge block — on_initialize
        //    reaps and the cert disappears.
        let purge_block = expiry + inactive_purge;
        System::set_block_number(purge_block);
        let _ = <zk_pki_pallet::Pallet<Runtime> as Hooks<u32>>::on_initialize(purge_block);
        assert!(
            zk_pki_pallet::CertLookupHot::<Runtime>::get(thumbprint).is_none(),
            "cert must be purged at scheduled block",
        );
        assert!(zk_pki_pallet::CertLookupCold::<Runtime>::get(thumbprint).is_none());
    });
}

// ─── Test 12 — cleanup extrinsic reaps suspended cert past grace ──────

#[test]
fn cleanup_reaps_suspended_cert_past_grace() {
    run(|| {
        let thumbprint = setup_carol_cert();

        // Bob suspends Carol's cert at block 1.
        assert_ok!(ZkPki::suspend_cert(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            thumbprint,
            None,
        ));

        // Jump past the expiry + grace window without firing
        // on_initialize. Forces the cleanup-extrinsic path.
        let hot = zk_pki_pallet::CertLookupHot::<Runtime>::get(thumbprint).unwrap();
        let expiry = hot.expiry_block;
        let inactive_purge: u32 = 14_400 * 30;
        System::set_block_number(expiry + inactive_purge + 1);

        let carol_reserved_before = Balances::reserved_balance(&account(CAROL_USER));

        // Any account can call cleanup — Dave here, reimbursing Carol
        // (holder gets the deposit back, not the caller).
        assert_ok!(ZkPki::cleanup(
            RuntimeOrigin::signed(account(DAVE_USER)),
            thumbprint,
            None, // no deposit redirect — goes to holder (Carol)
        ));

        // Non-PoP cert: held deposit is `max(MintFeeNone*5%, MinDeposit)`
        // = 0.1 DOT.
        let expected_deposit = UNIT / 10;
        let carol_reserved_after = Balances::reserved_balance(&account(CAROL_USER));
        assert_eq!(
            carol_reserved_after,
            carol_reserved_before - expected_deposit,
            "cleanup releases Carol's CertDeposit to her free balance",
        );
        assert!(zk_pki_pallet::CertLookupHot::<Runtime>::get(thumbprint).is_none());
    });
}
