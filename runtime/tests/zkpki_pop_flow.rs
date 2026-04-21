//! ZK-PKI PoP-path dry run against real paseo-runtime Config.
//!
//! Exercises the full PoP-required mint flow with a synth HIP
//! proof, then a standard-path `self_discard_cert` with a
//! `PopAssertion`, then a PCR7-drift rejection. All using
//! production deposits, production TTL bounds, and the real
//! `PalletProxyValidator` reading `pallet_proxy::Proxies`.

mod common;

use common::*;
use frame_support::{assert_noop, assert_ok};
use paseo_runtime::{RuntimeOrigin, ZkPki};

// ─── Test 4 — PoP-required mint records genesis fingerprint ────────────

#[test]
fn pop_required_mint_records_genesis_fingerprint() {
    run(|| {
        register_pop_root();
        issue_pop_issuer();
        create_pop_template(b"pop-tmpl");
        let thumbprint = mint_pop_cert(CAROL_USER, b"pop-tmpl");

        // Hot record carries PoP EKU.
        let hot =
            zk_pki_pallet::CertLookupHot::<paseo_runtime::Runtime>::get(thumbprint).unwrap();
        assert!(
            hot.ekus.contains(&zk_pki_primitives::eku::Eku::ProofOfPersonhood),
            "PoP template must propagate ProofOfPersonhood onto the hot record",
        );
        assert_eq!(
            hot.attestation_type,
            zk_pki_primitives::tpm::AttestationType::Tpm,
            "PoP-required + MockVerdict::Tpm must yield attestation_type = Tpm",
        );
        assert!(
            hot.manufacturer_verified,
            "MockVerdict::Tpm with a valid pubkey sets manufacturer_verified=true",
        );

        // Cold record carries a genesis fingerprint.
        let cold =
            zk_pki_pallet::CertLookupCold::<paseo_runtime::Runtime>::get(thumbprint).unwrap();
        let fp = cold
            .genesis_fingerprint
            .expect("PoP mint must record a genesis fingerprint");
        assert!(
            matches!(fp.platform, zk_pki_primitives::hip::HipPlatform::Tpm2Windows),
        );
        // PCR7 was pinned to PCR7_GENESIS at mint.
        let pcr7 = fp
            .pcr_values
            .iter()
            .find(|p| p.index == 7)
            .expect("genesis fingerprint has PCR 7");
        assert_eq!(pcr7.value, PCR7_GENESIS);
    });
}

// ─── Test 6 — self_discard_cert standard path with valid PopAssertion ──

#[test]
fn self_discard_standard_path_with_valid_pop_assertion() {
    run(|| {
        register_pop_root();
        issue_pop_issuer();
        create_pop_template(b"pop-tmpl");
        let thumbprint = mint_pop_cert(CAROL_USER, b"pop-tmpl");

        // Build a valid assertion — cert_ec_signature over
        // blake2_256(thumbprint.encode() || nonce), HIP proof bound
        // to the derived nonce, PCR7 = genesis.
        let assertion = build_self_discard_assertion(thumbprint);
        assert_ok!(ZkPki::self_discard_cert(
            RuntimeOrigin::signed(account(CAROL_USER)),
            thumbprint,
            Some(assertion),
        ));

        // Cert removed from every storage map.
        assert!(
            zk_pki_pallet::CertLookupHot::<paseo_runtime::Runtime>::get(thumbprint)
                .is_none()
        );
        assert!(
            zk_pki_pallet::CertLookupCold::<paseo_runtime::Runtime>::get(thumbprint)
                .is_none()
        );
    });
}

// ─── Test 7 — PCR7 drift rejects with HipProofInvalid ──────────────────

#[test]
fn self_discard_pop_assertion_rejected_on_pcr7_drift() {
    run(|| {
        register_pop_root();
        issue_pop_issuer();
        create_pop_template(b"pop-tmpl");
        let thumbprint = mint_pop_cert(CAROL_USER, b"pop-tmpl");

        // PopAssertion whose HIP proof carries a different PCR7 than
        // what was pinned at genesis. Everything else valid —
        // signatures match, AIK matches genesis, nonce matches
        // `derive_pop_nonce`. Only PCR7 drifted.
        const PCR7_COMPROMISED: [u8; 32] = [0x99; 32];
        let assertion = build_self_discard_assertion_pcr7(thumbprint, PCR7_COMPROMISED);

        assert_noop!(
            ZkPki::self_discard_cert(
                RuntimeOrigin::signed(account(CAROL_USER)),
                thumbprint,
                Some(assertion),
            ),
            zk_pki_pallet::Error::<paseo_runtime::Runtime>::HipProofInvalid,
        );

        // Cert still exists — PopAssertion rejection short-circuits
        // before `remove_cert_entry` runs.
        assert!(zk_pki_pallet::CertLookupHot::<paseo_runtime::Runtime>::get(thumbprint)
            .is_some());
    });
}
