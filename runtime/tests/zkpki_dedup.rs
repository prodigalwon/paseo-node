//! EK deduplication behavior under real paseo-runtime Config.
//!
//! Tpm verdicts are PoP-eligible and must not reuse an EK hash
//! across active certs within a single root's trust hierarchy
//! (one physical device → one PoP cert per root trust domain).
//! Different roots are independent trust domains — the same device
//! may hold active PoP certs under multiple roots concurrently.
//! Packed verdicts are NOT PoP-eligible (software / virtual TPM)
//! and skip the dedup gate entirely.

mod common;

use common::*;
use frame_support::{assert_noop, assert_ok};
use paseo_runtime::{Runtime, RuntimeOrigin, ZkPki};

fn setup_roots_and_issuer_with_templates() {
    register_pop_root();
    issue_pop_issuer();
    // One PoP-required template (for Tpm dedup tests) and one
    // non-PoP template (for Packed-skip-dedup test).
    create_pop_template(b"tpm-tmpl");
    create_non_pop_template(b"packed-tmpl");
}

// ─── Test 8 — two Tpm mints sharing an EK hash → second rejected ──────

#[test]
fn tpm_mints_with_same_ek_hash_second_rejected() {
    run(|| {
        setup_roots_and_issuer_with_templates();

        // First user mints successfully with ek_hash = [0x42; 32].
        let ek_shared = [0x42u8; 32];
        let t1 = mint_pop_cert_with_ek(CAROL_USER, b"tpm-tmpl", ek_shared);
        assert!(zk_pki_pallet::CertLookupHot::<Runtime>::get(t1).is_some());
        // Root-scoped index: cert is anchored under ALICE_ROOT.
        assert_eq!(
            zk_pki_pallet::EkRegistry::<Runtime>::get(
                &account(ALICE_ROOT),
                ek_shared,
            ),
            Some(t1),
            "EkRegistry must index the first Tpm cert under (root, EK hash)",
        );

        // Second user tries to mint with the same EK hash.
        let (nonce2, created_at2) = offer_and_read(DAVE_USER, b"tpm-tmpl");
        assert_noop!(
            ZkPki::mint_cert(
                RuntimeOrigin::signed(account(DAVE_USER)),
                nonce2,
                payload_tpm_with_ek(ek_shared),
                created_at2,
                Some(synth_hip_proof([0x01u8; 32])),
            ),
            zk_pki_pallet::Error::<Runtime>::EkAlreadyRegistered,
        );
    });
}

// ─── Test 9 — two Packed mints with same pubkey → both succeed ────────

#[test]
fn packed_mints_skip_ek_dedup() {
    run(|| {
        setup_roots_and_issuer_with_templates();

        // Two users mint under the non-PoP template with Packed
        // verdicts carrying the same pubkey bytes. Packed is not
        // PoP-eligible, so the EK-dedup gate is never consulted —
        // both mints land.
        let t1 = mint_non_pop_cert(CAROL_USER, b"packed-tmpl");
        let t2 = mint_non_pop_cert(DAVE_USER, b"packed-tmpl");
        assert_ne!(t1, t2, "distinct thumbprints for distinct users");
        assert!(zk_pki_pallet::CertLookupHot::<Runtime>::get(t1).is_some());
        assert!(zk_pki_pallet::CertLookupHot::<Runtime>::get(t2).is_some());
    });
}

// ─── Test 10 — root-scoped dedup: two roots, same EK, both mints land ─

#[test]
fn different_roots_same_ek_both_succeed() {
    run(|| {
        // Hierarchy A: Alice → Bob → user Carol.
        register_pop_root();
        issue_pop_issuer();
        create_pop_template(b"tpm-tmpl");

        // Hierarchy B: Frank → Grace → user Dave. Independent root
        // trust domain; shares no state with Alice's registry.
        register_pop_root_frank();
        issue_pop_issuer_grace();
        create_pop_template_grace(b"tpm-tmpl-b");

        // Shared physical device — same EK hash passed to both mints.
        let ek_shared = [0xAAu8; 32];

        let t_alice = mint_pop_cert_with_ek(CAROL_USER, b"tpm-tmpl", ek_shared);
        let t_frank =
            mint_pop_cert_with_ek_under_grace(DAVE_USER, b"tpm-tmpl-b", ek_shared);

        // Two distinct certs — thumbprints differ because the
        // canonical bytes differ in root + issuer + user.
        assert_ne!(
            t_alice, t_frank,
            "independent roots must produce distinct thumbprints",
        );

        // EkRegistry rows under BOTH roots — root scoping preserved.
        assert_eq!(
            zk_pki_pallet::EkRegistry::<Runtime>::get(
                &account(ALICE_ROOT),
                ek_shared,
            ),
            Some(t_alice),
        );
        assert_eq!(
            zk_pki_pallet::EkRegistry::<Runtime>::get(
                &account(FRANK_ROOT),
                ek_shared,
            ),
            Some(t_frank),
        );

        // Both certs active in storage.
        assert!(zk_pki_pallet::CertLookupHot::<Runtime>::get(t_alice).is_some());
        assert!(zk_pki_pallet::CertLookupHot::<Runtime>::get(t_frank).is_some());
    });
}
