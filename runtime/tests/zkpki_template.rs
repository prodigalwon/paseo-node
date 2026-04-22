//! Template lifecycle + issuer-side governance under real paseo
//! Config. Exercises: symmetric proxy gate on issue_issuer_cert,
//! template deactivate blocking new offers, discard-blocked-while-
//! active-certs invariant, and the non-cascading behavior of
//! invalidate_issuer on existing user certs.

mod common;

use common::*;
use frame_support::{assert_noop, assert_ok};
use paseo_runtime::{Runtime, RuntimeOrigin, ZkPki};
use zk_pki_primitives::crypto::DevicePublicKey;

// ─── Test 3 — issue_issuer_cert rejects when issuer lacks proxy ───────

#[test]
fn issue_issuer_cert_rejects_when_issuer_candidate_lacks_proxy() {
    run(|| {
        // Alice is a fully registered root.
        register_standard_root();

        // Bob is NOT in pallet_proxy::Proxies. Attempting to issue
        // an issuer cert to him must hit the proxy-validation gate
        // on `issue_issuer_cert` — same `has_proxy(issuer_candidate,
        // named_proxy)` check as on register_root, but rooted at
        // Bob's side of the delegation rather than Alice's.
        let pubkey = DevicePublicKey::new_p256(&cert_ec_pubkey_bytes()).unwrap();
        assert_noop!(
            ZkPki::issue_issuer_cert(
                RuntimeOrigin::signed(account(ALICE_ROOT)),
                account(BOB_ISSUER),
                account(BOB_PROXY),
                pubkey,
                empty_att(),
                500_000u32,
                empty_cap_ekus(),
            ),
            zk_pki_pallet::Error::<Runtime>::ProxyNotFound,
        );
    });
}

// ─── Test 13 — deactivated template blocks new offers ─────────────────

#[test]
fn deactivated_template_blocks_new_offers() {
    run(|| {
        register_standard_root();
        issue_standard_issuer();
        create_non_pop_template(b"d-tmpl");

        // Deactivate — no new offers, but existing behavior otherwise
        // preserved.
        assert_ok!(ZkPki::deactivate_cert_template(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            template_name(b"d-tmpl"),
        ));

        // Attempt to offer under a deactivated template — must fail.
        assert_noop!(
            ZkPki::offer_contract(
                RuntimeOrigin::signed(account(BOB_ISSUER)),
                account(CAROL_USER),
                10_000u32,
                template_name(b"d-tmpl"),
                empty_meta(),
            ),
            zk_pki_pallet::Error::<Runtime>::TemplateInactive,
        );
    });
}

// ─── Test 14 — discard_cert_template blocked while certs are active ───

#[test]
fn discard_cert_template_blocked_while_active_certs_exist() {
    run(|| {
        register_standard_root();
        issue_standard_issuer();
        create_non_pop_template(b"live-tmpl");
        // Mint one cert under the template — active_cert_count = 1.
        let _thumbprint = mint_non_pop_cert(CAROL_USER, b"live-tmpl");

        // Deactivate is fine (doesn't care about active count).
        assert_ok!(ZkPki::deactivate_cert_template(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            template_name(b"live-tmpl"),
        ));

        // Discard blocked — TemplateActiveCertCount > 0 gate.
        assert_noop!(
            ZkPki::discard_cert_template(
                RuntimeOrigin::signed(account(BOB_ISSUER)),
                template_name(b"live-tmpl"),
            ),
            zk_pki_pallet::Error::<Runtime>::TemplateHasActiveCerts,
        );
    });
}

// ─── Test 15 — invalidate_issuer doesn't cascade to user certs ────────

#[test]
fn invalidate_issuer_preserves_existing_user_certs() {
    run(|| {
        register_standard_root();
        issue_standard_issuer();
        create_non_pop_template(b"cascade-tmpl");
        let thumbprint = mint_non_pop_cert(CAROL_USER, b"cascade-tmpl");

        // Alice (root) invalidates Bob (issuer).
        assert_ok!(ZkPki::invalidate_issuer(
            RuntimeOrigin::signed(account(ALICE_ROOT)),
            account(BOB_ISSUER),
        ));

        // Bob's IssuerRecord transitions to Compromised (still in
        // storage — the hybrid-cascade invariant says end-user certs
        // stay put, relying parties synthesize chain status at read
        // time via RPC).
        let bob_rec = zk_pki_pallet::Issuers::<Runtime>::get(&account(BOB_ISSUER))
            .expect("issuer record still present after invalidate_issuer");
        assert!(
            bob_rec.state.is_compromised(),
            "invalidate_issuer marks issuer Compromised, not deleted",
        );

        // Carol's cert is still in storage, still active — the pallet
        // does NOT cascade-flip it. Relying parties see the issuer
        // compromise via `query_entity_status(BOB_ISSUER)` and decide
        // whether to trust certs minted under it.
        let hot = zk_pki_pallet::CertLookupHot::<Runtime>::get(thumbprint)
            .expect("user cert must survive invalidate_issuer");
        assert!(hot.is_active());
    });
}
