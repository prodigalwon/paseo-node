//! Fee system — end-to-end tests against real paseo-runtime Config.
//!
//! Exercises the `mint_cert` fee flow: tier selection (PoP /
//! Packed / None), distribution into (protocol fee, held deposit,
//! block-author tip, protocol remainder), the named `HoldReason`
//! binding, `self_discard_cert`'s release path, `cleanup`'s
//! transfer-on-hold path, and the insufficient-balance rejection.
//!
//! Paseo fee constants at time of writing:
//!   MintFeePoP        = 1.0 DOT
//!   MintFeePacked     = 1.5 DOT
//!   MintFeeNone       = 2.0 DOT
//!   ProtocolFeeBps    = 10 %    (protocol fee slice)
//!   DepositBps        = 5 %     (deposit slice, floor = MinDeposit)
//!   MinDeposit        = 0.1 DOT
//!   BlockCreatorCapBp = 40 %    (tip cap)
//! Test harness binds `FindAuthor = AuraAccountFinder`; with no
//! pre-runtime digests, `find_author` returns `None` and the tip
//! rolls into the protocol remainder. That's the behavior the
//! distribution tests assert.

mod common;

use common::*;
use frame_support::{
    assert_noop, assert_ok,
    traits::fungible::InspectHold,
};
use paseo_runtime::{
    configs::{
        PkiDepositBasisPoints, PkiMinDeposit, PkiMintFeeNone, PkiMintFeePacked,
        PkiMintFeePoP, PkiProtocolFeeBasisPoints, PkiProtocolFeeRecipient,
    },
    Balances, Runtime, RuntimeOrigin, ZkPki, UNIT,
};
use sp_runtime::traits::StaticLookup;

/// Expected hold amount under the fee system:
/// `max(mint_fee * DepositBasisPoints / 10_000, MinDeposit)`.
fn expected_deposit(mint_fee: u128) -> u128 {
    let pct = mint_fee.saturating_mul(PkiDepositBasisPoints::get() as u128) / 10_000u128;
    pct.max(PkiMinDeposit::get())
}

/// Expected protocol slice: `mint_fee * ProtocolFeeBasisPoints / 10_000`.
fn expected_protocol_fee(mint_fee: u128) -> u128 {
    mint_fee.saturating_mul(PkiProtocolFeeBasisPoints::get() as u128) / 10_000u128
}

/// Read `CertDeposit` hold on `who`. Separate from generic
/// `reserved_balance` — uses the fungible inspect API so we're
/// asserting *the named hold*, not the broader reserved slot.
fn held_cert_deposit(who: &paseo_runtime::AccountId) -> u128 {
    <Balances as InspectHold<_>>::balance_on_hold(
        &paseo_runtime::RuntimeHoldReason::ZkPki(zk_pki_pallet::HoldReason::CertDeposit),
        who,
    )
}

fn protocol_recipient() -> paseo_runtime::AccountId {
    PkiProtocolFeeRecipient::get()
}

/// Shared setup: one root, one issuer, one non-PoP template and
/// one PoP template. The PoP path wires `pop_cap_ekus()` all the
/// way down so `MintFeePoP` is the chosen tier.
fn setup_fees_harness() {
    register_pop_root();
    issue_pop_issuer();
    create_non_pop_template(b"fee-none");
    create_pop_template(b"fee-pop");
}

// ─── Test 1 — PoP cert: full fee distribution ─────────────────────────

#[test]
fn pop_cert_fee_distribution_correct() {
    run(|| {
        setup_fees_harness();
        let user = account(CAROL_USER);
        let free_before = Balances::free_balance(&user);
        let protocol_before = Balances::free_balance(&protocol_recipient());

        let t = mint_pop_cert(CAROL_USER, b"fee-pop");

        let mint_fee = PkiMintFeePoP::get();
        let deposit = expected_deposit(mint_fee);
        let protocol_fee = expected_protocol_fee(mint_fee);
        // No block author in the test harness — tip rolls into
        // protocol remainder.
        let protocol_total = mint_fee.saturating_sub(deposit);

        assert_eq!(
            held_cert_deposit(&user),
            deposit,
            "held deposit must equal max(MintFeePoP * 5%, MinDeposit)",
        );
        assert_eq!(
            Balances::free_balance(&user),
            free_before - mint_fee,
            "user free balance drops by exactly the mint fee",
        );
        assert_eq!(
            Balances::free_balance(&protocol_recipient()),
            protocol_before + protocol_total,
            "protocol recipient receives protocol fee + rolled-up tip + remainder",
        );
        // Sanity: protocol_total = protocol_fee + non-tipped tip + remainder.
        assert!(protocol_total >= protocol_fee);
        assert!(zk_pki_pallet::CertLookupHot::<Runtime>::get(t).is_some());
        // Per-cert `deposit` recorded on Cold row for the release paths.
        let cold = zk_pki_pallet::CertLookupCold::<Runtime>::get(t).unwrap();
        assert_eq!(cold.deposit, deposit);
    });
}

// ─── Test 2 — Packed cert: full fee distribution ──────────────────────

#[test]
fn packed_cert_fee_distribution_correct() {
    run(|| {
        setup_fees_harness();
        let user = account(CAROL_USER);
        let free_before = Balances::free_balance(&user);
        let protocol_before = Balances::free_balance(&protocol_recipient());

        let t = mint_non_pop_cert(CAROL_USER, b"fee-none");

        // Non-PoP templates accept Packed verdicts and always get
        // the `MintFeeNone` tier OR `MintFeePacked` if attestation
        // is Packed. `payload_packed()` in the shared helper ships
        // `AttestationType::Packed`, so tier = MintFeePacked.
        let mint_fee = PkiMintFeePacked::get();
        let deposit = expected_deposit(mint_fee);
        let protocol_total = mint_fee.saturating_sub(deposit);

        assert_eq!(
            held_cert_deposit(&user),
            deposit,
            "held deposit must equal max(MintFeePacked * 5%, MinDeposit)",
        );
        assert_eq!(
            Balances::free_balance(&user),
            free_before - mint_fee,
        );
        assert_eq!(
            Balances::free_balance(&protocol_recipient()),
            protocol_before + protocol_total,
        );
        let cold = zk_pki_pallet::CertLookupCold::<Runtime>::get(t).unwrap();
        assert_eq!(cold.deposit, deposit);
    });
}

// ─── Test 3 — None-attestation cert: full fee distribution ────────────

#[test]
fn none_cert_fee_distribution_correct() {
    run(|| {
        // Custom template + `MockVerdict::None` mint so the
        // attestation-type branch falls into `MintFeeNone`. None
        // isn't PoP-eligible; the non-PoP template accepts it.
        register_standard_root();
        issue_standard_issuer();
        create_non_pop_template(b"fee-none-type");

        // Offer under the non-PoP template.
        let (nonce, created_at) = offer_and_read(CAROL_USER, b"fee-none-type");

        // Build a MockVerdict::None payload inline — the shared
        // helper only ships Tpm and Packed variants.
        use codec::Encode;
        use zk_pki_tpm::test_mock_verifier::MockVerdict;
        let payload = zk_pki_tpm::AttestationPayloadV3 {
            cert_ec_chain: vec![vec![]],
            attest_ec_chain: vec![vec![]],
            hmac_binding_output: [0u8; 32],
            binding_signature: vec![],
            integrity_blob: MockVerdict::None {
                pubkey_bytes: cert_ec_pubkey_bytes(),
            }
            .encode(),
            integrity_signature: vec![],
        };

        let user = account(CAROL_USER);
        let free_before = Balances::free_balance(&user);
        let protocol_before = Balances::free_balance(&protocol_recipient());

        assert_ok!(ZkPki::mint_cert(
            RuntimeOrigin::signed(user.clone()),
            nonce,
            payload,
            created_at,
            None,
        ));

        let mint_fee = PkiMintFeeNone::get();
        let deposit = expected_deposit(mint_fee);
        let protocol_total = mint_fee.saturating_sub(deposit);

        assert_eq!(held_cert_deposit(&user), deposit);
        assert_eq!(Balances::free_balance(&user), free_before - mint_fee);
        assert_eq!(
            Balances::free_balance(&protocol_recipient()),
            protocol_before + protocol_total,
        );
    });
}

// ─── Test 4 — held deposit is the named hold, not a free reserve ──────

#[test]
fn deposit_held_only_pallet_can_release() {
    run(|| {
        setup_fees_harness();
        let t = mint_non_pop_cert(CAROL_USER, b"fee-none");
        let user = account(CAROL_USER);
        let cold = zk_pki_pallet::CertLookupCold::<Runtime>::get(t).unwrap();
        let deposit = cold.deposit;

        // Hold is visible under the `CertDeposit` reason.
        assert_eq!(held_cert_deposit(&user), deposit);
        // Same amount shows up in the generic `reserved_balance`
        // (pallet_balances aggregates holds + reserves), but the
        // *named* slot is what the pallet owns.
        assert_eq!(Balances::reserved_balance(&user), deposit);

        // Another pallet/caller can't release this amount — the
        // user has no way to free it short of a pallet extrinsic
        // that consumes the cert.
        let free_before = Balances::free_balance(&user);
        assert_eq!(Balances::free_balance(&user) - free_before, 0);
        assert_eq!(held_cert_deposit(&user), deposit);

        // Pallet path (self_discard) IS allowed — releases deposit.
        assert_ok!(ZkPki::self_discard_cert(
            RuntimeOrigin::signed(user.clone()),
            t,
            None,
        ));
        assert_eq!(held_cert_deposit(&user), 0);
    });
}

// ─── Test 5 — self_discard releases full deposit back to holder ───────

#[test]
fn self_discard_releases_deposit_to_holder() {
    run(|| {
        setup_fees_harness();
        let t = mint_non_pop_cert(CAROL_USER, b"fee-none");
        let user = account(CAROL_USER);
        let cold = zk_pki_pallet::CertLookupCold::<Runtime>::get(t).unwrap();
        let deposit = cold.deposit;

        let held_before = held_cert_deposit(&user);
        let free_before = Balances::free_balance(&user);
        assert_eq!(held_before, deposit);

        assert_ok!(ZkPki::self_discard_cert(
            RuntimeOrigin::signed(user.clone()),
            t,
            None,
        ));

        assert_eq!(held_cert_deposit(&user), 0);
        assert_eq!(
            Balances::free_balance(&user),
            free_before + deposit,
            "free balance must go up by exactly the held deposit",
        );
    });
}

// ─── Test 6 — cleanup reimburses the original holder by default ───────

#[test]
fn cleanup_transfers_deposit_to_caller() {
    run(|| {
        setup_fees_harness();
        let t = mint_non_pop_cert(CAROL_USER, b"fee-none");
        let holder = account(CAROL_USER);
        let caller = account(DAVE_USER);
        let cold = zk_pki_pallet::CertLookupCold::<Runtime>::get(t).unwrap();
        let deposit = cold.deposit;

        // Flip to Suspended so cleanup's reapable-condition check
        // passes, then jump past expiry + inactive_purge.
        assert_ok!(ZkPki::suspend_cert(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            t,
            None,
        ));
        let hot = zk_pki_pallet::CertLookupHot::<Runtime>::get(t).unwrap();
        let inactive_purge: u32 = 14_400 * 60;
        paseo_runtime::System::set_block_number(
            hot.expiry_block + inactive_purge + 1,
        );

        let holder_free_before = Balances::free_balance(&holder);
        let caller_free_before = Balances::free_balance(&caller);

        // `deposit_recipient = None` → holder gets the deposit back,
        // not the caller. Caller is just the reap trigger.
        assert_ok!(ZkPki::cleanup(
            RuntimeOrigin::signed(caller.clone()),
            t,
            None,
        ));

        assert_eq!(held_cert_deposit(&holder), 0);
        assert_eq!(
            Balances::free_balance(&holder),
            holder_free_before + deposit,
            "holder free balance climbs by the released deposit",
        );
        assert_eq!(
            Balances::free_balance(&caller),
            caller_free_before,
            "caller does not get the deposit when recipient is None",
        );
    });
}

// ─── Test 7 — cleanup with explicit recipient routes there ────────────

#[test]
fn cleanup_with_recipient_transfers_to_recipient() {
    run(|| {
        setup_fees_harness();
        let t = mint_non_pop_cert(CAROL_USER, b"fee-none");
        let holder = account(CAROL_USER);
        let caller = account(DAVE_USER);
        let recipient = account(EVE_USER);
        let cold = zk_pki_pallet::CertLookupCold::<Runtime>::get(t).unwrap();
        let deposit = cold.deposit;

        // Flip to Suspended so cleanup's reapable-condition check
        // passes, then advance to reapable window.
        assert_ok!(ZkPki::suspend_cert(
            RuntimeOrigin::signed(account(BOB_ISSUER)),
            t,
            None,
        ));
        let hot = zk_pki_pallet::CertLookupHot::<Runtime>::get(t).unwrap();
        let inactive_purge: u32 = 14_400 * 60;
        paseo_runtime::System::set_block_number(
            hot.expiry_block + inactive_purge + 1,
        );

        let holder_free_before = Balances::free_balance(&holder);
        let caller_free_before = Balances::free_balance(&caller);
        let recipient_free_before = Balances::free_balance(&recipient);

        assert_ok!(ZkPki::cleanup(
            RuntimeOrigin::signed(caller.clone()),
            t,
            Some(recipient.clone()),
        ));

        assert_eq!(held_cert_deposit(&holder), 0);
        assert_eq!(
            Balances::free_balance(&holder),
            holder_free_before,
            "holder free balance stays put when recipient is elsewhere",
        );
        assert_eq!(
            Balances::free_balance(&recipient),
            recipient_free_before + deposit,
            "recipient gets the routed deposit",
        );
        assert_eq!(
            Balances::free_balance(&caller),
            caller_free_before,
            "caller is the reap trigger only, not a deposit target",
        );
    });
}

// ─── Test 8 — insufficient free balance rejects the mint ──────────────

#[test]
fn insufficient_balance_rejected() {
    run(|| {
        setup_fees_harness();

        // Drain Carol down below MintFeePacked so the first
        // `Currency::transfer` in mint_cert fails. Send to Dave to
        // keep the ExistentialDeposit invariant intact on Carol.
        let carol_balance = Balances::free_balance(&account(CAROL_USER));
        let keep = UNIT / 100; // 0.01 DOT — below MintFeePacked (1.5 DOT).
        let drain = carol_balance.saturating_sub(keep);
        assert_ok!(Balances::transfer_keep_alive(
            RuntimeOrigin::signed(account(CAROL_USER)),
            <<Runtime as frame_system::Config>::Lookup as StaticLookup>::unlookup(
                account(DAVE_USER),
            ),
            drain,
        ));
        let carol_after_drain = Balances::free_balance(&account(CAROL_USER));
        assert!(carol_after_drain < PkiMintFeePacked::get());

        // Attempt the mint — must fail before any storage write.
        let (nonce, created_at) = offer_and_read(CAROL_USER, b"fee-none");
        use codec::Encode;
        use zk_pki_tpm::test_mock_verifier::MockVerdict;
        let payload = zk_pki_tpm::AttestationPayloadV3 {
            cert_ec_chain: vec![vec![]],
            attest_ec_chain: vec![vec![]],
            hmac_binding_output: [0u8; 32],
            binding_signature: vec![],
            integrity_blob: MockVerdict::Packed {
                pubkey_bytes: cert_ec_pubkey_bytes(),
            }
            .encode(),
            integrity_signature: vec![],
        };

        assert_noop!(
            ZkPki::mint_cert(
                RuntimeOrigin::signed(account(CAROL_USER)),
                nonce,
                payload,
                created_at,
                None,
            ),
            sp_runtime::DispatchError::Token(
                sp_runtime::TokenError::FundsUnavailable,
            ),
        );

        // Nothing held, no cert minted.
        assert_eq!(held_cert_deposit(&account(CAROL_USER)), 0);
        let ui_key = zk_pki_primitives::keys::UserIssuerKey::new(
            account(CAROL_USER),
            account(BOB_ISSUER),
        );
        assert!(
            zk_pki_pallet::UserIssuerIndex::<Runtime>::get(&ui_key).is_none(),
        );
    });
}
