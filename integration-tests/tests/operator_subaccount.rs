// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Operator subaccounts: separating who may move vault funds from where those
//! funds go. Registering an address requires it to have already delegated its
//! ATA to the vault, so most tests start from `new_registered_subaccount`.
//!
//! Two things these cannot establish, because the program cannot see either:
//! that an address is custody the operator cannot sweep (a `Subaccount` here is
//! a plain keypair), and that admin is a different party than the operator.

use august_vault::errors::ErrorCode;
use integration_tests::harness::{
    assert_anchor_err, assert_anchor_framework_err, VaultCtx, DEPOSIT_DECIMALS,
};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);
const DEPLOYED: u64 = 1_000_000_000;

/// Anchor's `ConstraintTokenOwner`, which fires before the derived-address check
/// on a token account belonging to the wrong party. Pinned by number so a test
/// cannot pass on a merely uninitialized account.
const CONSTRAINT_TOKEN_OWNER: u32 = 2015;
/// Anchor's `AccountNotInitialized`, raised when an address has no ATA.
const ACCOUNT_NOT_INITIALIZED: u32 = 3012;

fn funded_vault() -> VaultCtx {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("initial deposit");
    ctx
}

// ---- the legacy default ----

/// What lets the upgrade ship to three live vaults without a migration.
#[test]
fn an_unconfigured_vault_pays_the_operators_own_ata() {
    let mut ctx = funded_vault();
    assert!(!ctx.vault_state_data().requires_subaccount());

    ctx.operator_withdraw(DEPLOYED).expect("legacy withdraw");
    assert_eq!(
        ctx.token_account_amount(&ctx.operator_deposit_ata),
        DEPLOYED
    );
    ctx.operator_deposit(DEPLOYED).expect("legacy return");
    assert_eq!(ctx.vault_state_data().deployed_principal, 0);
}

// ---- who may register ----

/// The central requirement: the operator must not be able to change where its
/// own payouts land.
#[test]
fn the_operator_cannot_register_a_subaccount() {
    let mut ctx = funded_vault();
    let operator = ctx.operator.insecure_clone();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);

    let err = ctx
        .register_subaccount_as(&operator, &sub)
        .expect_err("the operator must not name its own destination");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
    assert_eq!(ctx.vault_state_data().subaccount_count, 0);
}

#[test]
fn a_stranger_cannot_register_a_subaccount() {
    let mut ctx = funded_vault();
    let stranger = ctx.new_funded_keypair(1_000_000_000);
    let sub = ctx.new_delegated_subaccount(DEPLOYED);

    let err = ctx
        .register_subaccount_as(&stranger, &sub)
        .expect_err("only the admin may register");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
    assert_eq!(ctx.vault_state_data().subaccount_count, 0);
}

#[test]
fn the_admin_can_register_a_delegated_address() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);

    ctx.register_subaccount(&sub).expect("admin may register");
    assert_eq!(ctx.vault_state_data().subaccount_count, 1);
    assert!(ctx.vault_state_data().requires_subaccount());
    let entry = ctx.subaccount_data(&sub);
    assert_eq!(entry.address, sub.key());
    assert_eq!(entry.principal, 0);
}

// ---- the delegation is the proof ----

/// Without a delegation the address has not shown it can return funds, which is
/// what stops a vault reaching the "deploys but cannot recall" state at all.
#[test]
fn an_address_without_a_delegation_cannot_be_registered() {
    let mut ctx = funded_vault();
    let sub = ctx.new_subaccount(0);

    let err = ctx
        .register_subaccount(&sub)
        .expect_err("an undelegated address must be refused");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
    assert_eq!(ctx.vault_state_data().subaccount_count, 0);
}

/// A zero allowance is not a delegation — also the state a fully spent one
/// leaves behind, since SPL clears the delegate at zero.
#[test]
fn a_zero_allowance_does_not_prove_capability() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(0);

    let err = ctx
        .register_subaccount(&sub)
        .expect_err("a zero allowance must be refused");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
}

/// The delegation must name the vault. One granted elsewhere proves the address
/// can sign, but not that this vault can pull.
#[test]
fn a_delegation_to_anyone_but_the_vault_is_not_proof() {
    let mut ctx = funded_vault();
    let sub = ctx.new_subaccount(0);
    let operator = ctx.operator.pubkey();
    ctx.approve_delegate_as(&sub, &operator, DEPLOYED);

    let err = ctx
        .register_subaccount(&sub)
        .expect_err("only a delegation to the vault counts");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
}

/// The mis-paste an address check could never catch: an uncreated ATA address
/// reads as a System-owned wallet. It has no delegation, so it cannot register.
#[test]
fn an_address_with_no_ata_cannot_be_registered() {
    let mut ctx = funded_vault();
    let fresh = Pubkey::new_unique();
    let admin = ctx.admin.insecure_clone();
    let pda = ctx.subaccount_pda(&fresh);
    let ata = ctx.deposit_ata_for(&fresh);

    let err = ctx
        .register_address_as(&admin, fresh, pda, ata)
        .expect_err("an address with no ATA must be refused");
    assert_anchor_framework_err(&err, ACCOUNT_NOT_INITIALIZED);
}

/// Refused even with a real delegation — the one case the proof cannot see.
#[test]
fn the_subaccount_may_not_be_the_operator() {
    let mut ctx = funded_vault();
    let operator = ctx.operator.insecure_clone();
    let operator_ata = ctx.operator_deposit_ata;
    let vault_state = ctx.vault_state;
    ctx.approve_from(&operator, &operator_ata, &vault_state, DEPLOYED);

    let admin = ctx.admin.insecure_clone();
    let addr = operator.pubkey();
    let pda = ctx.subaccount_pda(&addr);
    let err = ctx
        .register_address_as(&admin, addr, pda, operator_ata)
        .expect_err("the operator must not be registered");
    assert_anchor_err(&err, ErrorCode::InvalidSubaccount);
}

/// The ATA is derived from the argument, so a caller cannot pass one address's
/// ATA while naming another.
#[test]
fn the_passed_ata_must_belong_to_the_named_address() {
    let mut ctx = funded_vault();
    let named = ctx.new_delegated_subaccount(DEPLOYED);
    let other = ctx.new_delegated_subaccount(DEPLOYED);
    let admin = ctx.admin.insecure_clone();

    let err = ctx
        .register_address_as(&admin, named.key(), named.pda, other.deposit_ata)
        .expect_err("the ATA must be the named address's own");
    assert_anchor_framework_err(&err, CONSTRAINT_TOKEN_OWNER);
}

// ---- the pairing rule ----

/// The bypass the pairing rule exists to stop: the operator omits the registry
/// entry *and* names its own ATA, so the destination constraint resolves back to
/// the signer and passes. Only the pairing check refuses it.
#[test]
fn omitting_the_registry_entry_cannot_pay_the_operator() {
    let mut ctx = funded_vault();
    let _sub = ctx.new_registered_subaccount(DEPLOYED);
    let operator = ctx.operator.insecure_clone();
    let own_ata = ctx.operator_deposit_ata;

    let err = ctx
        .operator_withdraw_with(&operator, own_ata, None, DEPLOYED)
        .expect_err("a configured vault must not pay the operator");
    assert_anchor_err(&err, ErrorCode::InvalidSubaccount);
    assert_eq!(ctx.vault_state_data().deployed_principal, 0);

    let err = ctx
        .operator_deposit_with(&operator, own_ata, None, DEPLOYED)
        .expect_err("nor accept a return from it");
    assert_anchor_err(&err, ErrorCode::InvalidSubaccount);
}

/// Omitting it while naming the destination's ATA fails earlier, on the
/// destination constraint, since that resolves to the signer without the entry.
#[test]
fn omitting_the_registry_entry_fails_the_destination_constraint() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);
    let operator = ctx.operator.insecure_clone();

    let err = ctx
        .operator_withdraw_with(&operator, sub.deposit_ata, None, DEPLOYED)
        .expect_err("the entry is required to resolve the destination");
    assert_anchor_framework_err(&err, CONSTRAINT_TOKEN_OWNER);
}

/// And the converse, so the account cannot be smuggled onto an unconfigured
/// vault to bypass the operator's own ATA.
#[test]
fn a_vault_without_registrations_must_not_name_one() {
    let mut ctx = funded_vault();
    // Register on a *different* vault, then try to use it here.
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    let operator = ctx.operator.insecure_clone();
    let ata = sub.deposit_ata;
    let pda = sub.pda;

    let err = ctx
        .operator_withdraw_with(&operator, ata, Some(pda), DEPLOYED)
        .expect_err("an unconfigured vault must not accept a registry entry");
    assert!(matches!(
        err.err,
        solana_sdk::transaction::TransactionError::InstructionError(..)
    ));
}

// ---- the withdraw direction ----

#[test]
fn withdraw_goes_to_the_registered_destination() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);

    ctx.operator_withdraw_to(&sub, DEPLOYED).expect("withdraw");
    assert_eq!(ctx.token_account_amount(&sub.deposit_ata), DEPLOYED);
    assert_eq!(ctx.token_account_amount(&ctx.operator_deposit_ata), 0);
    assert_eq!(ctx.subaccount_data(&sub).principal, DEPLOYED);
    assert_eq!(ctx.vault_state_data().deployed_principal, DEPLOYED);
}

#[test]
fn withdraw_rejects_the_operators_own_ata_once_registered() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);
    let operator = ctx.operator.insecure_clone();
    let own_ata = ctx.operator_deposit_ata;

    let err = ctx
        .operator_withdraw_with(&operator, own_ata, Some(sub.pda), DEPLOYED)
        .expect_err("the operator's own ATA is no longer a valid destination");
    assert_anchor_framework_err(&err, CONSTRAINT_TOKEN_OWNER);
}

/// An unregistered custody address is refused even with a live delegation.
#[test]
fn withdraw_rejects_an_unregistered_custody_address() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);
    let other = ctx.new_delegated_subaccount(DEPLOYED);
    let operator = ctx.operator.insecure_clone();

    let err = ctx
        .operator_withdraw_with(&operator, other.deposit_ata, Some(sub.pda), DEPLOYED)
        .expect_err("only a registered destination may receive");
    assert_anchor_framework_err(&err, CONSTRAINT_TOKEN_OWNER);
    assert_eq!(ctx.token_account_amount(&other.deposit_ata), 0);
}

/// Holding the destination confers no power to move funds into it.
#[test]
fn withdraw_still_requires_the_operator_to_sign() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);
    let signer = sub.keypair.insecure_clone();
    let (ata, pda) = (sub.deposit_ata, sub.pda);

    let err = ctx
        .operator_withdraw_with(&signer, ata, Some(pda), DEPLOYED)
        .expect_err("the subaccount must not pull funds to itself");
    assert_anchor_err(&err, ErrorCode::NotOperator);
}

// ---- the return direction ----

/// On a configured vault the ATA constraint resolves to the destination for
/// *any* signer, so the operator check is the only thing gating who may spend
/// custody's allowance into the vault.
#[test]
fn deposit_still_requires_the_operator_to_sign() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);
    ctx.operator_withdraw_to(&sub, DEPLOYED).expect("withdraw");

    let stranger = ctx.new_funded_keypair(1_000_000_000);
    let (ata, pda) = (sub.deposit_ata, sub.pda);
    let err = ctx
        .operator_deposit_with(&stranger, ata, Some(pda), DEPLOYED)
        .expect_err("a stranger must not pull the allowance into the vault");
    assert_anchor_err(&err, ErrorCode::NotOperator);
    assert_eq!(ctx.token_account_amount(&sub.deposit_ata), DEPLOYED);
}

/// The delegation can lapse after registration — custody revokes, or the
/// allowance runs out.
#[test]
fn deposit_requires_a_live_delegation() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);
    ctx.operator_withdraw_to(&sub, DEPLOYED).expect("withdraw");

    ctx.revoke_delegate(&sub);
    let err = ctx
        .operator_deposit_from(&sub, DEPLOYED)
        .expect_err("a revoked delegation must stop returns");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
    assert_eq!(ctx.token_account_amount(&sub.deposit_ata), DEPLOYED);
}

/// `Approve` overwrites the delegate but leaves the allowance standing, so only
/// the identity half of the check can refuse this.
#[test]
fn a_delegation_re_pointed_elsewhere_stops_returns() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);
    ctx.operator_withdraw_to(&sub, DEPLOYED).expect("withdraw");

    let stranger = ctx.new_funded_keypair(1_000_000_000);
    ctx.approve_delegate_as(&sub, &stranger.pubkey(), DEPLOYED);

    let err = ctx
        .operator_deposit_from(&sub, DEPLOYED)
        .expect_err("a delegation to someone else must not authorize a return");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
}

#[test]
fn deposit_refuses_an_allowance_below_the_amount() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);
    ctx.operator_withdraw_to(&sub, DEPLOYED).expect("withdraw");

    ctx.approve_vault_as_delegate(&sub, DEPLOYED - 1);
    let err = ctx
        .operator_deposit_from(&sub, DEPLOYED)
        .expect_err("the allowance must cover the amount");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);

    ctx.operator_deposit_from(&sub, DEPLOYED - 1)
        .expect("a return within the allowance must succeed");
}

/// The round trip, tracked on both the destination and the vault total.
#[test]
fn a_registered_subaccount_round_trips_through_principal() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);
    let local_before = ctx.vault_state_data().local_aum;

    ctx.operator_withdraw_to(&sub, DEPLOYED).expect("withdraw");
    assert_eq!(ctx.vault_state_data().local_aum, local_before - DEPLOYED);

    ctx.operator_deposit_from(&sub, DEPLOYED).expect("return");
    assert_eq!(ctx.subaccount_data(&sub).principal, 0);
    assert_eq!(ctx.vault_state_data().deployed_principal, 0);
    assert_eq!(ctx.vault_state_data().local_aum, local_before);
}

// ---- allowance lifecycle ----

/// SPL clears the delegate at zero, so a spent allowance stops both directions
/// until custody re-grants — the state every cycle ends in.
#[test]
fn an_exhausted_allowance_stops_both_directions() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);

    ctx.operator_withdraw_to(&sub, DEPLOYED).expect("withdraw");
    ctx.operator_deposit_from(&sub, DEPLOYED)
        .expect("return drains the allowance");
    assert_eq!(ctx.token_account_delegate(&sub.deposit_ata), None);

    let err = ctx
        .operator_withdraw_to(&sub, DEPLOYED)
        .expect_err("a spent allowance must not authorize a further deploy");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);

    ctx.approve_vault_as_delegate(&sub, DEPLOYED);
    ctx.operator_withdraw_to(&sub, DEPLOYED)
        .expect("re-approved");
    ctx.operator_deposit_from(&sub, DEPLOYED).expect("return");
    assert_eq!(ctx.vault_state_data().deployed_principal, 0);
}

#[test]
fn an_allowance_covers_several_partial_returns() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);
    ctx.operator_withdraw_to(&sub, DEPLOYED).expect("withdraw");

    let half = DEPLOYED / 2;
    ctx.operator_deposit_from(&sub, half).expect("first half");
    ctx.operator_deposit_from(&sub, DEPLOYED - half)
        .expect("second half");
    assert_eq!(ctx.subaccount_data(&sub).principal, 0);
    assert_eq!(ctx.token_account_amount(&sub.deposit_ata), 0);
}

/// A minimal allowance no longer authorizes an unbounded deployment.
#[test]
fn the_allowance_bounds_deployments_too() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(1);

    let err = ctx
        .operator_withdraw_to(&sub, DEPLOYED)
        .expect_err("a one-unit allowance must not cover a full deployment");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);

    ctx.approve_vault_as_delegate(&sub, DEPLOYED);
    ctx.operator_withdraw_to(&sub, DEPLOYED).expect("covered");
    let err = ctx
        .operator_withdraw_to(&sub, 1)
        .expect_err("cumulative outflow must not exceed the allowance");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
}

/// The destination's balance is externally mutable, so measuring coverage
/// against it would let one donated unit block a planned deployment.
#[test]
fn a_donation_to_the_subaccount_cannot_block_deployments() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);

    let donor = ctx.new_subaccount(1);
    let (from, to) = (donor.deposit_ata, sub.deposit_ata);
    let donor_key = donor.keypair.insecure_clone();
    ctx.transfer_tokens_as(&donor_key, &from, &to, 1);

    ctx.operator_withdraw_to(&sub, DEPLOYED)
        .expect("a donation must not block the planned deployment");
    ctx.operator_deposit_from(&sub, DEPLOYED).expect("return");
    assert_eq!(ctx.subaccount_data(&sub).principal, 0);
}

/// `operator_update_aum` moves reported value with no tokens changing hands, so
/// coverage based on `deployed_aum` let a mark-down free allowance that a
/// further deployment spent.
#[test]
fn an_aum_report_cannot_reopen_deployment_capacity() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);
    ctx.operator_withdraw_to(&sub, DEPLOYED).expect("deploy");

    let marked_down = DEPLOYED - (DEPLOYED / 500);
    ctx.operator_update_aum(marked_down).expect("report a loss");
    assert_eq!(ctx.subaccount_data(&sub).principal, DEPLOYED);

    let err = ctx
        .operator_withdraw_to(&sub, DEPLOYED / 500)
        .expect_err("a mark-down must not reopen allowance capacity");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
}

// ---- several destinations ----

/// Each destination tracks its own principal, so one's exposure does not demand
/// coverage from another's custody.
#[test]
fn coverage_is_per_destination_not_vault_wide() {
    let mut ctx = funded_vault();
    let a = ctx.new_registered_subaccount(DEPLOYED);
    let b = ctx.new_registered_subaccount(DEPLOYED);
    assert_eq!(ctx.vault_state_data().subaccount_count, 2);

    ctx.operator_withdraw_to(&a, DEPLOYED).expect("deploy to A");
    // B's allowance covers only B's own exposure, which is still zero — a
    // vault-wide basis would demand it cover A's DEPLOYED as well.
    ctx.operator_withdraw_to(&b, DEPLOYED).expect("deploy to B");

    assert_eq!(ctx.subaccount_data(&a).principal, DEPLOYED);
    assert_eq!(ctx.subaccount_data(&b).principal, DEPLOYED);
    assert_eq!(ctx.vault_state_data().deployed_principal, DEPLOYED * 2);
}

/// Returns decrement only the destination they came from.
#[test]
fn returns_decrement_only_their_own_destination() {
    let mut ctx = funded_vault();
    let a = ctx.new_registered_subaccount(DEPLOYED);
    let b = ctx.new_registered_subaccount(DEPLOYED);
    ctx.operator_withdraw_to(&a, DEPLOYED / 2).expect("to A");
    ctx.operator_withdraw_to(&b, DEPLOYED / 2).expect("to B");

    ctx.operator_deposit_from(&a, DEPLOYED / 2).expect("from A");
    assert_eq!(ctx.subaccount_data(&a).principal, 0);
    assert_eq!(ctx.subaccount_data(&b).principal, DEPLOYED / 2);
    assert_eq!(ctx.vault_state_data().deployed_principal, DEPLOYED / 2);
}

// ---- deregistration ----

/// Removing a destination the vault is still owed funds from would lose the
/// record of what is outstanding.
#[test]
fn deregistering_requires_zero_principal() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);
    ctx.operator_withdraw_to(&sub, DEPLOYED).expect("deploy");

    let err = ctx
        .deregister_subaccount(&sub)
        .expect_err("a funded destination must not be deregistered");
    assert_anchor_err(&err, ErrorCode::SubaccountNotEmpty);
    assert_eq!(ctx.vault_state_data().subaccount_count, 1);

    ctx.operator_deposit_from(&sub, DEPLOYED).expect("drain");
    ctx.deregister_subaccount(&sub).expect("now removable");
    assert_eq!(ctx.vault_state_data().subaccount_count, 0);
}

/// Removing the last one returns the vault to paying the operator's own ATA —
/// the rollback path.
#[test]
fn deregistering_the_last_one_returns_to_the_operators_ata() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);
    let err = ctx
        .operator_withdraw(DEPLOYED)
        .expect_err("configured vault refuses the operator's own ATA");
    assert_anchor_err(&err, ErrorCode::InvalidSubaccount);

    ctx.deregister_subaccount(&sub).expect("deregister");
    assert!(!ctx.vault_state_data().requires_subaccount());
    ctx.operator_withdraw(DEPLOYED)
        .expect("legacy path restored");
    ctx.operator_deposit(DEPLOYED).expect("and returns");
}

#[test]
fn the_operator_cannot_deregister() {
    let mut ctx = funded_vault();
    let sub = ctx.new_registered_subaccount(DEPLOYED);
    let operator = ctx.operator.insecure_clone();

    let err = ctx
        .deregister_subaccount_as(&operator, &sub)
        .expect_err("only the admin may deregister");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
}

// ---- migration ----

/// The first registration adopts the vault's outstanding principal, so a vault
/// with funds already out stays covered rather than starting from a clean slate
/// that under-states what is owed.
#[test]
fn the_first_registration_adopts_existing_principal() {
    let mut ctx = funded_vault();
    ctx.operator_withdraw(DEPLOYED)
        .expect("deploy pre-registry");
    assert_eq!(ctx.vault_state_data().deployed_principal, DEPLOYED);

    let sub = ctx.new_registered_subaccount(DEPLOYED * 4);
    assert_eq!(
        ctx.subaccount_data(&sub).principal,
        DEPLOYED,
        "the first registration inherits what the vault is already owed"
    );

    // A second registration starts clean.
    let b = ctx.new_registered_subaccount(DEPLOYED);
    assert_eq!(ctx.subaccount_data(&b).principal, 0);
}

// ---- Token-2022 ----

/// The ATA derivation and the delegation both go through the token program.
#[test]
fn the_subaccount_flow_works_on_token_2022() {
    let mut ctx = VaultCtx::fresh_token_2022();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");

    let undelegated = ctx.new_subaccount(0);
    let err = ctx
        .register_subaccount(&undelegated)
        .expect_err("the proof applies on Token-2022 too");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);

    let sub = ctx.new_registered_subaccount(DEPLOYED);
    ctx.operator_withdraw_to(&sub, DEPLOYED).expect("withdraw");
    assert_eq!(ctx.token_account_amount(&sub.deposit_ata), DEPLOYED);
    ctx.operator_deposit_from(&sub, DEPLOYED).expect("return");
    assert_eq!(ctx.vault_state_data().deployed_principal, 0);
}
