//! ZK-PKI end-to-end dry run against the actual paseo runtime.
//!
//! Exercises every pallet interaction the dotwave flow will hit:
//!
//!   Alice adds proxy → Alice register_root
//!     → Bob adds proxy → Alice issue_issuer_cert (to Bob)
//!       → Bob create_cert_template (PopRequirement::NotRequired)
//!         → Bob offer_contract (to Carol)
//!           → Carol mint_cert (MockVerdict::Packed — skips EK dedup)
//!             → assert cert landed in Hot/Cold + secondary indexes
//!
//! Non-PoP path (no HIP proof needed) — matches what an early
//! dotwave dry run looks like where the user just needs a cert
//! present on-chain without the full hardware ceremony.
//!
//! Runs in-process via `sp_io::TestExternalities` against
//! `paseo_runtime::Runtime` itself, not the zk-pki-runtime test
//! harness. Every Config type (real deposits, real TTLs, real
//! `PalletProxyValidator` reading `pallet_proxy::Proxies`) is
//! exercised exactly as a live paseo-node --dev chain would.

use codec::Encode;
use frame_support::{assert_ok, traits::Currency, BoundedVec};
use paseo_runtime::{
    configs::ProxyType, AccountId, Balances, BlockNumber, Runtime, RuntimeOrigin, System,
    ZkPki, Proxy, UNIT,
};
use sp_runtime::{traits::StaticLookup, BuildStorage};
use zk_pki_primitives::crypto::DevicePublicKey;
use zk_pki_primitives::eku::Eku;
use zk_pki_primitives::template::PopRequirement;
use zk_pki_tpm::test_mock_verifier::MockVerdict;
use zk_pki_tpm::AttestationPayloadV3;

// ──────────────────────────────────────────────────────────────────────
// Fixtures
// ──────────────────────────────────────────────────────────────────────

const ALICE_ROOT: [u8; 32] = [0xA1; 32];
const ALICE_PROXY: [u8; 32] = [0xA2; 32];
const BOB_ISSUER: [u8; 32] = [0xB1; 32];
const BOB_PROXY: [u8; 32] = [0xB2; 32];
const CAROL_USER: [u8; 32] = [0xC1; 32];

fn account(seed: [u8; 32]) -> AccountId {
    AccountId::from(seed)
}

fn lookup(a: AccountId) -> <<Runtime as frame_system::Config>::Lookup as StaticLookup>::Source {
    <<Runtime as frame_system::Config>::Lookup as StaticLookup>::unlookup(a)
}

fn test_p256_pubkey() -> Vec<u8> {
    use p256::ecdsa::{SigningKey, VerifyingKey};
    let sk = SigningKey::from_slice(&[7u8; 32]).expect("valid P-256 scalar");
    let vk: VerifyingKey = *sk.verifying_key();
    vk.to_encoded_point(false).as_bytes().to_vec()
}

fn new_ext() -> sp_io::TestExternalities {
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
        ],
        dev_accounts: None,
    }
    .assimilate_storage(&mut storage)
    .unwrap();
    storage.into()
}

// ──────────────────────────────────────────────────────────────────────
// The dry run
// ──────────────────────────────────────────────────────────────────────

#[test]
fn zkpki_end_to_end_non_pop() {
    new_ext().execute_with(|| {
        System::set_block_number(1);

        let pubkey = DevicePublicKey::new_p256(&test_p256_pubkey()).unwrap();
        let empty_att: BoundedVec<_, _> = BoundedVec::try_from(vec![]).unwrap();
        let empty_cap_ekus: BoundedVec<Eku, frame_support::traits::ConstU32<8>> =
            BoundedVec::try_from(vec![]).unwrap();
        let empty_template_ekus: BoundedVec<Eku, frame_support::traits::ConstU32<16>> =
            BoundedVec::try_from(vec![]).unwrap();
        let empty_meta: BoundedVec<_, _> = BoundedVec::try_from(vec![]).unwrap();
        let template_name: BoundedVec<_, _> =
            BoundedVec::try_from(b"dryrun-template".to_vec()).unwrap();

        // 1. Alice registers Alice_proxy as her proxy delegate.
        assert_ok!(Proxy::add_proxy(
            RuntimeOrigin::signed(account(ALICE_ROOT)),
            lookup(account(ALICE_PROXY)),
            ProxyType::Any,
            0u32,
        ));

        // 2. Alice registers as a root CA. Pallet reads
        //    `pallet_proxy::Proxies::<T>::get(Alice)` — finds
        //    Alice_proxy — validation passes.
        assert_ok!(ZkPki::register_root(
            RuntimeOrigin::signed(account(ALICE_ROOT)),
            account(ALICE_PROXY),
            pubkey.clone(),
            empty_att.clone(),
            1_000_000u32, // 5-year TTL-bounded value (< MaxRootTtlBlocks)
            empty_cap_ekus.clone(),
        ));
        assert!(zk_pki_pallet::Roots::<Runtime>::contains_key(
            &account(ALICE_ROOT)
        ));

        // 3. Bob registers Bob_proxy.
        assert_ok!(Proxy::add_proxy(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            lookup(account(BOB_PROXY)),
            ProxyType::Any,
            0u32,
        ));

        // 4. Alice (as root) issues an issuer cert to Bob.
        //    Pallet reads Bob's proxy record — passes.
        assert_ok!(ZkPki::issue_issuer_cert(
            RuntimeOrigin::signed(account(ALICE_ROOT)),
            account(BOB_ISSUER),
            account(BOB_PROXY),
            pubkey.clone(),
            empty_att.clone(),
            500_000u32,
            empty_cap_ekus,
        ));
        assert!(zk_pki_pallet::Issuers::<Runtime>::contains_key(
            &account(BOB_ISSUER)
        ));

        // 5. Bob creates a permissive template.
        assert_ok!(ZkPki::create_cert_template(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            template_name.clone(),
            PopRequirement::NotRequired,
            400_000u64, // max_ttl (u64 regardless of runtime BlockNumber)
            1_000u64,   // min_ttl
            None,       // max_certs
            None,       // metadata_schema
            empty_template_ekus,
        ));
        assert!(zk_pki_pallet::CertTemplates::<Runtime>::contains_key(
            &account(BOB_ISSUER),
            &template_name,
        ));

        // 6. Bob offers a cert to Carol.
        assert_ok!(ZkPki::offer_contract(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            account(CAROL_USER),
            10_000u32, // cert ttl
            template_name.clone(),
            empty_meta,
        ));
        let offer_key = zk_pki_primitives::keys::IssuerUserKey::new(
            account(BOB_ISSUER),
            account(CAROL_USER),
        );
        let nonce = zk_pki_pallet::OfferIndex::<Runtime>::get(&offer_key)
            .expect("offer indexed by (issuer, user)");
        let offer = zk_pki_pallet::ContractOffers::<Runtime>::get(nonce)
            .expect("offer record present");

        // 7. Carol accepts the offer — mints the cert. Non-PoP
        //    template, so no HIP proof required. The integrity_blob
        //    is a SCALE-encoded `MockVerdict::Packed` so the
        //    NoopBindingProofVerifier's `MockVerdict::decode` call
        //    succeeds and returns a Packed verdict (EK-dedup skipped).
        let payload = AttestationPayloadV3 {
            cert_ec_chain: vec![vec![]],
            attest_ec_chain: vec![vec![]],
            hmac_binding_output: [0u8; 32],
            binding_signature: vec![],
            integrity_blob: MockVerdict::Packed {
                pubkey_bytes: test_p256_pubkey(),
            }
            .encode(),
            integrity_signature: vec![],
        };
        assert_ok!(ZkPki::mint_cert(
            RuntimeOrigin::signed(account(CAROL_USER)),
            nonce,
            payload,
            offer.created_at,
            None, // no HIP proof — template is NotRequired
        ));

        // 8. Verify the cert landed in every expected storage map.
        let user_issuer_key = zk_pki_primitives::keys::UserIssuerKey::new(
            account(CAROL_USER),
            account(BOB_ISSUER),
        );
        let thumbprint = zk_pki_pallet::UserIssuerIndex::<Runtime>::get(&user_issuer_key)
            .expect("cert minted and indexed");
        assert!(zk_pki_pallet::CertLookupHot::<Runtime>::get(thumbprint).is_some());
        assert!(zk_pki_pallet::CertLookupCold::<Runtime>::get(thumbprint).is_some());
        assert!(zk_pki_pallet::CertsByIssuer::<Runtime>::contains_key(
            &account(BOB_ISSUER),
            thumbprint,
        ));
        assert!(zk_pki_pallet::CertsByUser::<Runtime>::contains_key(
            &account(CAROL_USER),
            thumbprint,
        ));
        assert!(zk_pki_pallet::CertsByRoot::<Runtime>::contains_key(
            &account(ALICE_ROOT),
            thumbprint,
        ));

        // 9. Read the cert via the runtime-api query fn — this is
        //    the exact path `zkpki_certStatus` RPC takes.
        let status = zk_pki_pallet::Pallet::<Runtime>::query_cert_status(thumbprint)
            .expect("cert_status returns the newly-minted cert");
        assert_eq!(status.issuer, account(BOB_ISSUER));
        assert_eq!(status.root, account(ALICE_ROOT));
    });
}

#[test]
fn zkpki_register_root_rejects_without_proxy() {
    // Without a pallet_proxy::Proxies entry for Alice, register_root
    // must fail with `ProxyNotFound`. Confirms the proxy validation
    // is actually wired in paseo-runtime (not silently bypassed).
    new_ext().execute_with(|| {
        System::set_block_number(1);
        let pubkey = DevicePublicKey::new_p256(&test_p256_pubkey()).unwrap();
        let empty_att: BoundedVec<_, _> = BoundedVec::try_from(vec![]).unwrap();
        let empty_cap_ekus: BoundedVec<Eku, frame_support::traits::ConstU32<8>> =
            BoundedVec::try_from(vec![]).unwrap();

        let result = ZkPki::register_root(
            RuntimeOrigin::signed(account(ALICE_ROOT)),
            account(ALICE_PROXY),
            pubkey,
            empty_att,
            1_000_000u32,
            empty_cap_ekus,
        );
        assert!(result.is_err(), "register_root must reject without a proxy record");
    });
}

// Silence unused-import warning for Currency (pulled in for genesis
// convenience, not used directly in the current assertions).
#[allow(dead_code)]
fn _unused() {
    let _ = <Balances as Currency<AccountId>>::total_balance(&account([0u8; 32]));
    let _: BlockNumber = 0;
}
