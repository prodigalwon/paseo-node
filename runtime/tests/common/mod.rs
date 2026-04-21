//! Shared helpers for the paseo-runtime ZK-PKI integration tests.
//!
//! Exercised by `zkpki_pop_flow`, `zkpki_lifecycle`, `zkpki_dedup`,
//! and `zkpki_template` test crates. Every helper runs against the
//! real `paseo_runtime::Runtime` — production Config values, real
//! `PalletProxyValidator`, real deposits. Used to prove chain-side
//! behavior without dotwave-in-the-loop.

#![allow(dead_code)]

use codec::Encode;
use frame_support::{assert_ok, traits::ConstU32, BoundedVec};
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use paseo_runtime::{
    configs::ProxyType, AccountId, Proxy, Runtime, RuntimeOrigin, System, ZkPki, UNIT,
};
use sp_runtime::{traits::StaticLookup, BuildStorage};
use zk_pki_primitives::cert::Thumbprint;
use zk_pki_primitives::crypto::DevicePublicKey;
use zk_pki_primitives::eku::Eku;
use zk_pki_primitives::hip::{CanonicalHipProof, HipPlatform, PcrValue};
use zk_pki_primitives::template::PopRequirement;
use zk_pki_tpm::test_mock_verifier::MockVerdict;
use zk_pki_tpm::AttestationPayloadV3;

// ──────────────────────────────────────────────────────────────────────
// Standard test accounts
// ──────────────────────────────────────────────────────────────────────

pub const ALICE_ROOT: [u8; 32] = [0xA1; 32];
pub const ALICE_PROXY: [u8; 32] = [0xA2; 32];
pub const BOB_ISSUER: [u8; 32] = [0xB1; 32];
pub const BOB_PROXY: [u8; 32] = [0xB2; 32];
pub const CAROL_USER: [u8; 32] = [0xC1; 32];
pub const DAVE_USER: [u8; 32] = [0xD1; 32];
pub const EVE_USER: [u8; 32] = [0xE1; 32];

// Second trust hierarchy — for root-scoped EK dedup tests that
// need two independent roots certifying the same device.
pub const FRANK_ROOT: [u8; 32] = [0xF1; 32];
pub const FRANK_PROXY: [u8; 32] = [0xF2; 32];
pub const GRACE_ISSUER: [u8; 32] = [0x61; 32];
pub const GRACE_PROXY: [u8; 32] = [0x62; 32];

pub fn account(seed: [u8; 32]) -> AccountId {
    AccountId::from(seed)
}

pub fn lookup(
    a: AccountId,
) -> <<Runtime as frame_system::Config>::Lookup as StaticLookup>::Source {
    <<Runtime as frame_system::Config>::Lookup as StaticLookup>::unlookup(a)
}

// ──────────────────────────────────────────────────────────────────────
// Deterministic P-256 keys for synth proofs
// ──────────────────────────────────────────────────────────────────────

pub const CERT_EC_SCALAR: [u8; 32] = [0x07; 32];
pub const AIK_SCALAR: [u8; 32] = [0x22; 32];
pub const EK_SCALAR: [u8; 32] = [0x11; 32];
pub const PCR7_GENESIS: [u8; 32] = [0x77; 32];

pub fn cert_ec_sk() -> SigningKey {
    SigningKey::from_slice(&CERT_EC_SCALAR).unwrap()
}

pub fn cert_ec_pubkey_bytes() -> Vec<u8> {
    cert_ec_sk()
        .verifying_key()
        .to_encoded_point(false)
        .as_bytes()
        .to_vec()
}

// ──────────────────────────────────────────────────────────────────────
// TestExternalities with funded dev accounts
// ──────────────────────────────────────────────────────────────────────

pub fn new_ext() -> sp_io::TestExternalities {
    let mut storage = frame_system::GenesisConfig::<Runtime>::default()
        .build_storage()
        .unwrap();
    let initial: u128 = 1_000_000 * UNIT;
    pallet_balances::GenesisConfig::<Runtime> {
        balances: vec![
            (account(ALICE_ROOT), initial),
            (account(ALICE_PROXY), initial),
            (account(BOB_ISSUER), initial),
            (account(BOB_PROXY), initial),
            (account(CAROL_USER), initial),
            (account(DAVE_USER), initial),
            (account(EVE_USER), initial),
            (account(FRANK_ROOT), initial),
            (account(FRANK_PROXY), initial),
            (account(GRACE_ISSUER), initial),
            (account(GRACE_PROXY), initial),
        ],
        dev_accounts: None,
    }
    .assimilate_storage(&mut storage)
    .unwrap();
    storage.into()
}

pub fn run<R>(f: impl FnOnce() -> R) -> R {
    new_ext().execute_with(|| {
        System::set_block_number(1);
        f()
    })
}

// ──────────────────────────────────────────────────────────────────────
// Bounded-vec conveniences
// ──────────────────────────────────────────────────────────────────────

pub fn empty_att() -> BoundedVec<
    u8,
    ConstU32<{ zk_pki_primitives::bounds::MAX_ATTESTATION_LEN }>,
> {
    BoundedVec::try_from(vec![]).unwrap()
}

pub fn empty_cap_ekus() -> BoundedVec<Eku, ConstU32<8>> {
    BoundedVec::try_from(vec![]).unwrap()
}

pub fn pop_cap_ekus() -> BoundedVec<Eku, ConstU32<8>> {
    BoundedVec::try_from(vec![Eku::ProofOfPersonhood]).unwrap()
}

pub fn template_ekus_with(v: Vec<Eku>) -> BoundedVec<Eku, ConstU32<16>> {
    BoundedVec::try_from(v).unwrap()
}

pub fn empty_meta() -> BoundedVec<
    u8,
    ConstU32<{ zk_pki_primitives::bounds::MAX_METADATA_LEN }>,
> {
    BoundedVec::try_from(vec![]).unwrap()
}

pub fn template_name(label: &[u8]) -> BoundedVec<u8, ConstU32<64>> {
    BoundedVec::try_from(label.to_vec()).unwrap()
}

// ──────────────────────────────────────────────────────────────────────
// Setup chains — reusable building blocks
// ──────────────────────────────────────────────────────────────────────

pub fn setup_proxy(delegator: [u8; 32], delegatee: [u8; 32]) {
    assert_ok!(Proxy::add_proxy(
        RuntimeOrigin::signed(account(delegator)),
        lookup(account(delegatee)),
        ProxyType::Any,
        0u32, // delay blocks — polkadot-sdk stable2603 added this param
    ));
}

pub fn register_standard_root() {
    setup_proxy(ALICE_ROOT, ALICE_PROXY);
    let pubkey = DevicePublicKey::new_p256(&cert_ec_pubkey_bytes()).unwrap();
    assert_ok!(ZkPki::register_root(
        RuntimeOrigin::signed(account(ALICE_ROOT)),
        account(ALICE_PROXY),
        pubkey,
        empty_att(),
        1_000_000u32,
        empty_cap_ekus(),
    ));
}

pub fn register_pop_root() {
    setup_proxy(ALICE_ROOT, ALICE_PROXY);
    let pubkey = DevicePublicKey::new_p256(&cert_ec_pubkey_bytes()).unwrap();
    assert_ok!(ZkPki::register_root(
        RuntimeOrigin::signed(account(ALICE_ROOT)),
        account(ALICE_PROXY),
        pubkey,
        empty_att(),
        1_000_000u32,
        pop_cap_ekus(),
    ));
}

pub fn issue_standard_issuer() {
    setup_proxy(BOB_ISSUER, BOB_PROXY);
    let pubkey = DevicePublicKey::new_p256(&cert_ec_pubkey_bytes()).unwrap();
    assert_ok!(ZkPki::issue_issuer_cert(
        RuntimeOrigin::signed(account(ALICE_ROOT)),
        account(BOB_ISSUER),
        account(BOB_PROXY),
        pubkey,
        empty_att(),
        500_000u32,
        empty_cap_ekus(),
    ));
}

pub fn issue_pop_issuer() {
    setup_proxy(BOB_ISSUER, BOB_PROXY);
    let pubkey = DevicePublicKey::new_p256(&cert_ec_pubkey_bytes()).unwrap();
    assert_ok!(ZkPki::issue_issuer_cert(
        RuntimeOrigin::signed(account(ALICE_ROOT)),
        account(BOB_ISSUER),
        account(BOB_PROXY),
        pubkey,
        empty_att(),
        500_000u32,
        pop_cap_ekus(),
    ));
}

// ── Second trust hierarchy: Frank (root) → Grace (issuer) ──
// Used by root-scoped EK dedup tests that prove two independent
// roots may certify the same physical device.

pub fn register_pop_root_frank() {
    setup_proxy(FRANK_ROOT, FRANK_PROXY);
    let pubkey = DevicePublicKey::new_p256(&cert_ec_pubkey_bytes()).unwrap();
    assert_ok!(ZkPki::register_root(
        RuntimeOrigin::signed(account(FRANK_ROOT)),
        account(FRANK_PROXY),
        pubkey,
        empty_att(),
        1_000_000u32,
        pop_cap_ekus(),
    ));
}

pub fn issue_pop_issuer_grace() {
    setup_proxy(GRACE_ISSUER, GRACE_PROXY);
    let pubkey = DevicePublicKey::new_p256(&cert_ec_pubkey_bytes()).unwrap();
    assert_ok!(ZkPki::issue_issuer_cert(
        RuntimeOrigin::signed(account(FRANK_ROOT)),
        account(GRACE_ISSUER),
        account(GRACE_PROXY),
        pubkey,
        empty_att(),
        500_000u32,
        pop_cap_ekus(),
    ));
}

pub fn create_pop_template_grace(label: &[u8]) {
    assert_ok!(ZkPki::create_cert_template(
        RuntimeOrigin::signed(account(GRACE_ISSUER)),
        template_name(label),
        PopRequirement::Required,
        400_000u64,
        1_000u64,
        None,
        None,
        template_ekus_with(vec![Eku::ProofOfPersonhood]),
    ));
}

/// Same shape as `mint_pop_cert_with_ek` but anchored under Grace
/// (who is anchored under Frank). Exists so root-scoped dedup
/// tests can mint the same EK under a second trust hierarchy.
pub fn mint_pop_cert_with_ek_under_grace(
    user: [u8; 32],
    label: &[u8],
    ek: [u8; 32],
) -> Thumbprint {
    assert_ok!(ZkPki::offer_contract(
        RuntimeOrigin::signed(account(GRACE_ISSUER)),
        account(user),
        10_000u32,
        template_name(label),
        empty_meta(),
    ));
    let ui_key = zk_pki_primitives::keys::IssuerUserKey::new(
        account(GRACE_ISSUER),
        account(user),
    );
    let nonce = zk_pki_pallet::OfferIndex::<Runtime>::get(&ui_key).unwrap();
    let created_at = zk_pki_pallet::ContractOffers::<Runtime>::get(nonce)
        .unwrap()
        .created_at;
    let genesis_nonce = [0x02u8; 32];
    assert_ok!(ZkPki::mint_cert(
        RuntimeOrigin::signed(account(user)),
        nonce,
        payload_tpm_with_ek(ek),
        created_at,
        Some(synth_hip_proof_with_pcr7(genesis_nonce, PCR7_GENESIS)),
    ));
    let user_issuer_key = zk_pki_primitives::keys::UserIssuerKey::new(
        account(user),
        account(GRACE_ISSUER),
    );
    zk_pki_pallet::UserIssuerIndex::<Runtime>::get(&user_issuer_key)
        .expect("cert minted under Grace and indexed")
}

pub fn create_non_pop_template(label: &[u8]) {
    assert_ok!(ZkPki::create_cert_template(
        RuntimeOrigin::signed(account(BOB_ISSUER)),
        template_name(label),
        PopRequirement::NotRequired,
        400_000u64,
        1_000u64,
        None,
        None,
        template_ekus_with(vec![]),
    ));
}

pub fn create_pop_template(label: &[u8]) {
    assert_ok!(ZkPki::create_cert_template(
        RuntimeOrigin::signed(account(BOB_ISSUER)),
        template_name(label),
        PopRequirement::Required,
        400_000u64,
        1_000u64,
        None,
        None,
        template_ekus_with(vec![Eku::ProofOfPersonhood]),
    ));
}

/// Offer a cert to `user` under the named template, then read back the
/// nonce + created_at from storage.
pub fn offer_and_read(user: [u8; 32], label: &[u8]) -> ([u8; 32], u32) {
    assert_ok!(ZkPki::offer_contract(
        RuntimeOrigin::signed(account(BOB_ISSUER)),
        account(user),
        10_000u32,
        template_name(label),
        empty_meta(),
    ));
    let ui_key = zk_pki_primitives::keys::IssuerUserKey::new(
        account(BOB_ISSUER),
        account(user),
    );
    let nonce = zk_pki_pallet::OfferIndex::<Runtime>::get(&ui_key).unwrap();
    let created_at = zk_pki_pallet::ContractOffers::<Runtime>::get(nonce)
        .unwrap()
        .created_at;
    (nonce, created_at)
}

/// Mint a non-PoP cert for `user` under `label`. Returns thumbprint.
pub fn mint_non_pop_cert(user: [u8; 32], label: &[u8]) -> Thumbprint {
    let (nonce, created_at) = offer_and_read(user, label);
    assert_ok!(ZkPki::mint_cert(
        RuntimeOrigin::signed(account(user)),
        nonce,
        payload_packed(),
        created_at,
        None,
    ));
    resolve_thumbprint(user)
}

/// Mint a PoP cert for `user` under `label`. Genesis nonce is hard-coded.
pub fn mint_pop_cert(user: [u8; 32], label: &[u8]) -> Thumbprint {
    let (nonce, created_at) = offer_and_read(user, label);
    let genesis_nonce = [0x01u8; 32];
    assert_ok!(ZkPki::mint_cert(
        RuntimeOrigin::signed(account(user)),
        nonce,
        payload_tpm(),
        created_at,
        Some(synth_hip_proof_with_pcr7(genesis_nonce, PCR7_GENESIS)),
    ));
    resolve_thumbprint(user)
}

/// Same as `mint_pop_cert` but uses a caller-supplied EK hash in the
/// MockVerdict::Tpm integrity blob. Used to exercise EK-dedup tests.
pub fn mint_pop_cert_with_ek(user: [u8; 32], label: &[u8], ek: [u8; 32]) -> Thumbprint {
    let (nonce, created_at) = offer_and_read(user, label);
    let genesis_nonce = [0x01u8; 32];
    assert_ok!(ZkPki::mint_cert(
        RuntimeOrigin::signed(account(user)),
        nonce,
        payload_tpm_with_ek(ek),
        created_at,
        Some(synth_hip_proof_with_pcr7(genesis_nonce, PCR7_GENESIS)),
    ));
    resolve_thumbprint(user)
}

pub fn resolve_thumbprint(user: [u8; 32]) -> Thumbprint {
    let user_issuer_key = zk_pki_primitives::keys::UserIssuerKey::new(
        account(user),
        account(BOB_ISSUER),
    );
    zk_pki_pallet::UserIssuerIndex::<Runtime>::get(&user_issuer_key)
        .expect("cert minted and indexed")
}

// ──────────────────────────────────────────────────────────────────────
// AttestationPayloadV3 helpers (MockVerdict-encoded integrity_blob)
// ──────────────────────────────────────────────────────────────────────

pub fn payload_packed() -> AttestationPayloadV3 {
    AttestationPayloadV3 {
        cert_ec_chain: vec![vec![]],
        attest_ec_chain: vec![vec![]],
        hmac_binding_output: [0u8; 32],
        binding_signature: vec![],
        integrity_blob: MockVerdict::Packed {
            pubkey_bytes: cert_ec_pubkey_bytes(),
        }
        .encode(),
        integrity_signature: vec![],
    }
}

pub fn payload_tpm() -> AttestationPayloadV3 {
    payload_tpm_with_ek([0x42u8; 32])
}

pub fn payload_tpm_with_ek(ek_hash: [u8; 32]) -> AttestationPayloadV3 {
    AttestationPayloadV3 {
        cert_ec_chain: vec![vec![]],
        attest_ec_chain: vec![vec![]],
        hmac_binding_output: [0u8; 32],
        binding_signature: vec![],
        integrity_blob: MockVerdict::Tpm {
            ek_hash,
            pubkey_bytes: cert_ec_pubkey_bytes(),
        }
        .encode(),
        integrity_signature: vec![],
    }
}

// ──────────────────────────────────────────────────────────────────────
// Synth HIP proof builders
// ──────────────────────────────────────────────────────────────────────

pub fn synth_tpms_attest_quote(nonce: &[u8; 32], pcr_digest: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0xFF54_4347u32.to_be_bytes()); // magic
    out.extend_from_slice(&0x8018u16.to_be_bytes()); // TPM_ST_ATTEST_QUOTE
    out.extend_from_slice(&0u16.to_be_bytes()); // qualifiedSigner: empty
    out.extend_from_slice(&32u16.to_be_bytes()); // extraData size
    out.extend_from_slice(nonce);
    out.extend_from_slice(&[0u8; 17]); // clockInfo
    out.extend_from_slice(&0u64.to_be_bytes()); // firmwareVersion
    out.extend_from_slice(&0u32.to_be_bytes()); // pcrSelect count = 0
    out.extend_from_slice(&32u16.to_be_bytes()); // pcrDigest size
    out.extend_from_slice(pcr_digest);
    out
}

pub fn synth_hip_proof_with_pcr7(nonce: [u8; 32], pcr7: [u8; 32]) -> CanonicalHipProof {
    let ek = SigningKey::from_slice(&EK_SCALAR).unwrap();
    let aik = SigningKey::from_slice(&AIK_SCALAR).unwrap();
    let ek_pub = ek
        .verifying_key()
        .to_encoded_point(false)
        .as_bytes()
        .to_vec();
    let aik_pub = aik
        .verifying_key()
        .to_encoded_point(false)
        .as_bytes()
        .to_vec();

    let aik_certify_info = b"aik-certify-info".to_vec();
    let aik_certify_sig: Signature = ek.sign(&aik_certify_info);

    let pcr_digest = [0xAAu8; 32];
    let quote_attest = synth_tpms_attest_quote(&nonce, &pcr_digest);
    let quote_sig: Signature = aik.sign(&quote_attest);

    let pcr_values: BoundedVec<PcrValue, ConstU32<16>> = BoundedVec::try_from(vec![
        PcrValue { index: 7, value: pcr7 },
    ])
    .unwrap();

    let ek_hash = sp_io::hashing::blake2_256(&ek_pub);

    CanonicalHipProof {
        platform: HipPlatform::Tpm2Windows,
        ek_hash,
        ek_public: BoundedVec::try_from(ek_pub).unwrap(),
        aik_public: BoundedVec::try_from(aik_pub).unwrap(),
        aik_certify_info: BoundedVec::try_from(aik_certify_info).unwrap(),
        aik_certify_signature: BoundedVec::try_from(
            aik_certify_sig.to_der().as_bytes().to_vec(),
        )
        .unwrap(),
        pcr_values,
        pcr_digest,
        quote_attest: BoundedVec::try_from(quote_attest).unwrap(),
        quote_signature: BoundedVec::try_from(quote_sig.to_der().as_bytes().to_vec()).unwrap(),
        nonce,
    }
}

pub fn synth_hip_proof(nonce: [u8; 32]) -> CanonicalHipProof {
    synth_hip_proof_with_pcr7(nonce, PCR7_GENESIS)
}

// ──────────────────────────────────────────────────────────────────────
// PopAssertion construction for self_discard_cert's standard path
// ──────────────────────────────────────────────────────────────────────

pub fn build_self_discard_assertion_pcr7(
    thumbprint: Thumbprint,
    pcr7: [u8; 32],
) -> zk_pki_primitives::pop::PopAssertion {
    let call_data = thumbprint.encode();
    let parent_hash = frame_system::Pallet::<Runtime>::parent_hash();
    let parent_bytes: [u8; 32] = parent_hash.as_ref().try_into().unwrap_or([0u8; 32]);
    let nonce = zk_pki_primitives::pop::derive_pop_nonce(
        &parent_bytes,
        &thumbprint,
        &call_data,
    );

    let mut signed_input = Vec::with_capacity(call_data.len() + 32);
    signed_input.extend_from_slice(&call_data);
    signed_input.extend_from_slice(&nonce);
    let signed_payload = sp_io::hashing::blake2_256(&signed_input);
    let cert_ec_sig: Signature = cert_ec_sk().sign(&signed_payload);

    zk_pki_primitives::pop::PopAssertion {
        cert_thumbprint: thumbprint,
        cert_ec_signature: BoundedVec::try_from(cert_ec_sig.to_der().as_bytes().to_vec())
            .unwrap(),
        hip_proof: synth_hip_proof_with_pcr7(nonce, pcr7),
    }
}

pub fn build_self_discard_assertion(
    thumbprint: Thumbprint,
) -> zk_pki_primitives::pop::PopAssertion {
    build_self_discard_assertion_pcr7(thumbprint, PCR7_GENESIS)
}
