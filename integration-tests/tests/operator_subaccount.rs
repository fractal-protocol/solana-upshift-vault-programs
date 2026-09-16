// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! `operator_subaccount`: separating who may move vault funds from where those
//! funds go. Naming an address requires it to have already delegated its ATA to
//! the vault, so most tests start from `new_delegated_subaccount`.
//!
//! Two things these cannot establish, because the program cannot see either:
//! that the address is custody the operator cannot sweep (a `Subaccount` here is
//! a plain keypair), and that admin is a different party than the operator.

use august_vault::errors::ErrorCode;
use integration_tests::harness::{
    assert_anchor_err, assert_anchor_framework_err, VaultCtx, DEPOSIT_DECIMALS,
};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32); // 10 tokens
const DEPLOYED: u64 = 1_000_000_000; // 1 token pushed out to the destination

/// Anchor's `ConstraintTokenOwner`, which fires before the derived-address check
/// on a token account belonging to the wrong party. Pinned by number so the test
/// cannot pass on a merely uninitialized account.
const CONSTRAINT_TOKEN_OWNER: u32 = 2015;

/// Anchor's `AccountNotInitialized`, raised when a named address has no ATA.
const ACCOUNT_NOT_INITIALIZED: u32 = 3012;

fn funded_vault() -> VaultCtx {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("initial deposit");
    ctx
}

// ---- the legacy default ----

/// The property that lets the upgrade ship to three live vaults without a
/// migration. Asserted on the resolved destination, not just the raw zero.
#[test]
fn an_unconfigured_vault_pays_the_operators_own_ata() {
    let mut ctx = funded_vault();
    let state = ctx.vault_state_data();
    assert_eq!(state.operator_subaccount(), None);
    assert_eq!(state.operator_destination(), ctx.operator.pubkey());

    ctx.operator_withdraw(DEPLOYED)
        .expect("legacy withdraw to the operator's own ATA");
    assert_eq!(
        ctx.token_account_amount(&ctx.operator_deposit_ata),
        DEPLOYED
    );
    ctx.operator_deposit(DEPLOYED)
        .expect("legacy return from the operator's own ATA");
    assert_eq!(ctx.token_account_amount(&ctx.operator_deposit_ata), 0);
}

// ---- who may write the field ----

/// The central requirement: the operator must not be able to change where its
/// own payouts land.
#[test]
fn the_operator_cannot_set_the_subaccount() {
    let mut ctx = funded_vault();
    let operator = ctx.operator.insecure_clone();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);

    let err = ctx
        .set_operator_subaccount_as(&operator, sub.key())
        .expect_err("the operator must not name its own destination");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
    assert_eq!(ctx.vault_state_data().operator_subaccount(), None);
}

#[test]
fn a_stranger_cannot_set_the_subaccount() {
    let mut ctx = funded_vault();
    let stranger = ctx.new_funded_keypair(1_000_000_000);
    let sub = ctx.new_delegated_subaccount(DEPLOYED);

    let err = ctx
        .set_operator_subaccount_as(&stranger, sub.key())
        .expect_err("only the admin may set the subaccount");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
    assert_eq!(ctx.vault_state_data().operator_subaccount(), None);
}

#[test]
fn the_admin_can_name_a_delegated_address() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);

    ctx.set_operator_subaccount(sub.key())
        .expect("a delegated address may be named");
    let state = ctx.vault_state_data();
    assert_eq!(state.operator_subaccount(), Some(sub.key()));
    assert_eq!(state.operator_destination(), sub.key());
}

// ---- the delegation is the proof ----

/// The precondition. Without a delegation the address has not shown it can
/// return funds, so it cannot be named — which is what stops a vault reaching
/// the "deploys but cannot recall" state at all.
#[test]
fn an_address_without_a_delegation_cannot_be_named() {
    let mut ctx = funded_vault();
    let sub = ctx.new_subaccount(0); // ATA exists, no delegation

    let err = ctx
        .set_operator_subaccount(sub.key())
        .expect_err("an undelegated address must be refused");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
    assert_eq!(ctx.vault_state_data().operator_subaccount(), None);
}

/// A zero allowance is not a delegation. SPL clears the delegate at zero, so
/// this is also the state a fully-spent allowance leaves behind.
#[test]
fn a_zero_allowance_does_not_prove_capability() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(0);

    let err = ctx
        .set_operator_subaccount(sub.key())
        .expect_err("a zero allowance must be refused");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
}

/// The delegation must name the vault. One granted to anyone else proves the
/// address can sign, but not that this vault can pull.
#[test]
fn a_delegation_to_anyone_but_the_vault_is_not_proof() {
    let mut ctx = funded_vault();
    let sub = ctx.new_subaccount(0);
    let operator = ctx.operator.pubkey();
    ctx.approve_delegate_as(&sub, &operator, DEPLOYED);

    let err = ctx
        .set_operator_subaccount(sub.key())
        .expect_err("only a delegation to the vault counts");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
}

/// An address with no ATA at all — the mis-paste the address-shape checks could
/// never catch, since an uncreated ATA address reads as a System-owned wallet.
/// It has no delegation, so it cannot be named.
#[test]
fn an_address_with_no_ata_cannot_be_named() {
    let mut ctx = funded_vault();
    let fresh = Pubkey::new_unique();

    let err = ctx
        .set_operator_subaccount(fresh)
        .expect_err("an address with no ATA must be refused");
    assert_anchor_framework_err(&err, ACCOUNT_NOT_INITIALIZED);
}

/// Refused by the proof, not by an enumeration. Each ATA is created first —
/// which any third party can do — or these fail at `AccountNotInitialized`
/// instead and the test stands behind nothing. The vault-PDA row matters most:
/// naming it would send the reserve somewhere no instruction can spend from.
#[test]
fn addresses_that_cannot_delegate_are_refused() {
    let mut ctx = funded_vault();
    let state = ctx.vault_state_data();

    for (label, addr) in [
        ("vault PDA", ctx.vault_state),
        ("share mint", state.share_mint),
        ("deposit mint", state.deposit_mint),
    ] {
        ctx.create_deposit_ata_for(&addr);
        let err = ctx
            .set_operator_subaccount(addr)
            .expect_err(&format!("{label} must be refused"));
        assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
        assert_eq!(
            ctx.vault_state_data().operator_subaccount(),
            None,
            "{label} was refused but the field was written"
        );
    }
}

/// The ATA is derived from the argument, so a caller cannot pass one address's
/// ATA while naming another. This is what makes the proof apply to the address
/// actually being stored — previously a separate equality check, now structural.
#[test]
fn the_passed_ata_must_belong_to_the_named_address() {
    let mut ctx = funded_vault();
    let named = ctx.new_delegated_subaccount(DEPLOYED);
    let other = ctx.new_delegated_subaccount(DEPLOYED);
    let admin = ctx.admin.insecure_clone();

    let err = ctx
        .set_operator_subaccount_with(&admin, named.key(), Some(other.deposit_ata))
        .expect_err("the ATA must be the named address's own");
    assert_anchor_framework_err(&err, CONSTRAINT_TOKEN_OWNER);
    assert_eq!(ctx.vault_state_data().operator_subaccount(), None);
}

/// Refused even though the operator could legitimately hold a delegated ATA —
/// the one case the proof cannot see, so it stays an explicit check.
#[test]
fn the_subaccount_may_not_be_the_operator() {
    let mut ctx = funded_vault();
    let operator = ctx.operator.insecure_clone();
    let operator_ata = ctx.operator_deposit_ata;
    // Give the operator a real delegation, so only the role check can refuse it.
    let vault_state = ctx.vault_state;
    ctx.approve_from(&operator, &operator_ata, &vault_state, DEPLOYED);

    let err = ctx
        .set_operator_subaccount(operator.pubkey())
        .expect_err("subaccount == operator must be refused");
    assert_anchor_err(&err, ErrorCode::InvalidOperatorSubaccount);
    assert_eq!(ctx.vault_state_data().operator_subaccount(), None);
}

/// The same collision from the other side: without this guard the
/// `subaccount != operator` rule is one `set_operator` away from being undone.
#[test]
fn set_operator_cannot_move_the_operator_onto_the_subaccount() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");

    let admin = ctx.admin.insecure_clone();
    let err = ctx
        .set_operator_as(&admin, sub.key())
        .expect_err("the operator must not be moved onto the subaccount");
    assert_anchor_err(&err, ErrorCode::InvalidOperatorSubaccount);
    assert_eq!(ctx.vault_state_data().operator, ctx.operator.pubkey());
}

#[test]
fn set_operator_still_rotates_onto_an_unrelated_key() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");

    let admin = ctx.admin.insecure_clone();
    let new_operator = ctx.new_funded_keypair(1_000_000_000);
    ctx.set_operator_as(&admin, new_operator.pubkey())
        .expect("rotating onto an unrelated key must still work");
    assert_eq!(ctx.vault_state_data().operator, new_operator.pubkey());
}

/// Omitting the ATA is Anchor's `None`, which may only mean the zero rollback.
///
/// The harness always derives the ATA, so this is the one path that can desync
/// the two — a client that dropped the optional account while naming a real
/// address. Previously only reachable via the program-id sentinel; now pinned
/// directly.
#[test]
fn omitting_the_ata_is_only_valid_for_the_rollback() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    let admin = ctx.admin.insecure_clone();

    let err = ctx
        .set_operator_subaccount_with(&admin, sub.key(), None)
        .expect_err("a non-zero address needs its ATA passed");
    assert_anchor_err(&err, ErrorCode::InvalidOperatorSubaccount);
    assert_eq!(ctx.vault_state_data().operator_subaccount(), None);

    // And the rollback is exactly the case it is for.
    ctx.set_operator_subaccount(sub.key()).expect("set");
    ctx.set_operator_subaccount_with(&admin, Pubkey::default(), None)
        .expect("omitting the ATA is how the rollback is expressed");
    assert_eq!(ctx.vault_state_data().operator_subaccount(), None);
}

// ---- the withdraw direction ----

#[test]
fn withdraw_goes_to_the_subaccount_once_set() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");

    let operator = ctx.operator.insecure_clone();
    ctx.operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("withdraw to the subaccount's ATA");
    assert_eq!(ctx.token_account_amount(&sub.deposit_ata), DEPLOYED);
    assert_eq!(ctx.token_account_amount(&ctx.operator_deposit_ata), 0);
    assert_eq!(ctx.vault_state_data().deployed_aum, DEPLOYED);
}

/// "The operator's own ATA must be rejected" — which falls out of deriving the
/// constraint from the destination: the address stops matching.
#[test]
fn withdraw_rejects_the_operators_own_ata_once_set() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");

    let err = ctx
        .operator_withdraw(DEPLOYED)
        .expect_err("the operator's own ATA is no longer a valid destination");
    assert_anchor_framework_err(&err, CONSTRAINT_TOKEN_OWNER);
    assert_eq!(ctx.vault_state_data().deployed_aum, 0);
}

/// The gate names one address, not "anything but the operator".
#[test]
fn withdraw_rejects_a_different_custody_address() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    let other = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");

    let operator = ctx.operator.insecure_clone();
    let err = ctx
        .operator_withdraw_as(&operator, other.deposit_ata, DEPLOYED)
        .expect_err("only the named subaccount may receive");
    assert_anchor_framework_err(&err, CONSTRAINT_TOKEN_OWNER);
    assert_eq!(ctx.token_account_amount(&other.deposit_ata), 0);
}

/// Holding the destination confers no power to move funds into it.
#[test]
fn withdraw_still_requires_the_operator_to_sign() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");

    let signer = sub.keypair.insecure_clone();
    let err = ctx
        .operator_withdraw_as(&signer, sub.deposit_ata, DEPLOYED)
        .expect_err("the subaccount must not pull funds to itself");
    assert_anchor_err(&err, ErrorCode::NotOperator);
}

// ---- the return direction ----

/// On a gated vault the ATA constraint resolves to the subaccount for *any*
/// signer, so the operator check is the only thing gating who may spend
/// custody's allowance into the vault.
#[test]
fn deposit_still_requires_the_operator_to_sign() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");
    let operator = ctx.operator.insecure_clone();
    ctx.operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("withdraw");

    let stranger = ctx.new_funded_keypair(1_000_000_000);
    let err = ctx
        .operator_deposit_as(&stranger, sub.deposit_ata, DEPLOYED)
        .expect_err("a stranger must not pull the allowance into the vault");
    assert_anchor_err(&err, ErrorCode::NotOperator);
    assert_eq!(
        ctx.token_account_amount(&sub.deposit_ata),
        DEPLOYED,
        "the refused call must leave custody's funds alone"
    );
}

/// The delegation can lapse after the vault was configured — custody revokes,
/// or the allowance runs out. The return path must then refuse clearly.
#[test]
fn deposit_requires_a_live_delegation() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");
    let operator = ctx.operator.insecure_clone();
    ctx.operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("withdraw");

    ctx.revoke_delegate(&sub);
    let err = ctx
        .operator_deposit_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect_err("a revoked delegation must stop returns");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
    assert_eq!(ctx.token_account_amount(&sub.deposit_ata), DEPLOYED);
}

/// An allowance below the transfer is refused up front.
#[test]
fn deposit_refuses_an_allowance_below_the_amount() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");
    let operator = ctx.operator.insecure_clone();
    ctx.operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("withdraw");

    // Narrow the standing allowance below the amount.
    ctx.approve_vault_as_delegate(&sub, DEPLOYED - 1);
    let err = ctx
        .operator_deposit_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect_err("the allowance must cover the amount");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);

    // One unit less fits: the boundary is the allowance, not the delegation.
    ctx.operator_deposit_as(&operator, sub.deposit_ata, DEPLOYED - 1)
        .expect("a return within the allowance must succeed");
}

#[test]
fn deposit_rejects_the_operators_own_ata_once_set() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");

    // The ATA exists and is well-formed, so what fails is its owner.
    let err = ctx
        .operator_deposit(DEPLOYED)
        .expect_err("the operator's own ATA is no longer a valid source");
    assert_anchor_framework_err(&err, CONSTRAINT_TOKEN_OWNER);
}

/// The round trip, and the accounting criterion: `deployed_aum` stays the net
/// deployed amount for the single destination.
#[test]
fn a_delegated_subaccount_round_trips_through_deployed_aum() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");
    let operator = ctx.operator.insecure_clone();
    let local_before = ctx.vault_state_data().local_aum;

    ctx.operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("withdraw");
    let state = ctx.vault_state_data();
    assert_eq!(state.deployed_aum, DEPLOYED);
    assert_eq!(state.local_aum, local_before - DEPLOYED);

    ctx.operator_deposit_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("return the whole amount");
    let state = ctx.vault_state_data();
    assert_eq!(state.deployed_aum, 0, "net deployed back to zero");
    assert_eq!(state.local_aum, local_before);
    assert_eq!(ctx.token_account_amount(&sub.deposit_ata), 0);
}

/// SPL clears the delegate once the allowance reaches zero, so a spent
/// allowance stops both directions until custody re-grants — the state every
/// operating cycle ends in.
#[test]
fn an_exhausted_allowance_stops_both_directions() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");
    let operator = ctx.operator.insecure_clone();

    ctx.operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("withdraw");
    ctx.operator_deposit_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("return drains the allowance");
    assert_eq!(ctx.token_account_delegate(&sub.deposit_ata), None);

    // Deploying again is refused too, which is what keeps funds at custody
    // always covered — before the coverage rule this succeeded, leaving the
    // whole balance unrecallable.
    let err = ctx
        .operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect_err("a spent allowance must not authorize a further deploy");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);

    ctx.approve_vault_as_delegate(&sub, DEPLOYED);
    ctx.operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("re-approved");
    ctx.operator_deposit_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("return");
    assert_eq!(ctx.vault_state_data().deployed_aum, 0);
}

/// One allowance spent across two partial returns.
#[test]
fn an_allowance_covers_several_partial_returns() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");
    let operator = ctx.operator.insecure_clone();
    ctx.operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("withdraw");

    let half = DEPLOYED / 2;
    ctx.operator_deposit_as(&operator, sub.deposit_ata, half)
        .expect("first half");
    ctx.operator_deposit_as(&operator, sub.deposit_ata, DEPLOYED - half)
        .expect("second half");
    assert_eq!(ctx.vault_state_data().deployed_aum, 0);
    assert_eq!(ctx.token_account_amount(&sub.deposit_ata), 0);
}

// ---- rollback and incident response ----

/// Zero is the documented rollback. Asserts the vault was really gated in
/// between, or this passes against a setter that writes nothing.
#[test]
fn setting_zero_reverts_to_the_operators_ata() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");

    let err = ctx
        .operator_withdraw(DEPLOYED)
        .expect_err("gated vault must refuse the operator's own ATA");
    assert_anchor_framework_err(&err, CONSTRAINT_TOKEN_OWNER);

    ctx.set_operator_subaccount(Pubkey::default())
        .expect("zero must be accepted as the rollback");
    let state = ctx.vault_state_data();
    assert_eq!(state.operator_subaccount(), None);
    assert_eq!(state.operator_destination(), ctx.operator.pubkey());

    ctx.operator_withdraw(DEPLOYED).expect("legacy withdraw");
    ctx.operator_deposit(DEPLOYED).expect("legacy return");
    assert_eq!(ctx.vault_state_data().deployed_aum, 0);
}

/// Rolling back stops this vault pulling from custody, but does *not* revoke
/// the delegation — asserted below — so an admin can re-name and resume. A lever
/// against a compromised operator, not a compromised admin.
#[test]
fn rolling_back_stops_the_vault_honouring_a_live_delegation() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED * 4);
    ctx.set_operator_subaccount(sub.key()).expect("set");
    let operator = ctx.operator.insecure_clone();
    ctx.operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("withdraw");

    ctx.set_operator_subaccount(Pubkey::default())
        .expect("rollback");

    assert_eq!(
        ctx.token_account_delegate(&sub.deposit_ata),
        Some(ctx.vault_state),
        "the delegation is still standing; the vault just stops using it"
    );
    let err = ctx
        .operator_deposit_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect_err("a rolled-back vault must not pull from the old subaccount");
    assert_anchor_framework_err(&err, CONSTRAINT_TOKEN_OWNER);
}

/// `set_operator` refuses the zero key, matching the three config-authority
/// setters. With a subaccount set, a zeroed operator makes both operator
/// handlers uncallable while funds sit at custody.
#[test]
fn the_operator_may_not_be_zeroed() {
    let mut ctx = funded_vault();
    let admin = ctx.admin.insecure_clone();

    let err = ctx
        .set_operator_as(&admin, Pubkey::default())
        .expect_err("the zero key must be refused");
    assert_anchor_err(&err, ErrorCode::InvalidAuthority);
    assert_eq!(ctx.vault_state_data().operator, ctx.operator.pubkey());
}

/// The rollback must still work on a vault that already carries a zero operator
/// — reachable on-chain, since nothing refused it before the guard above. The
/// setter therefore skips its `subaccount != operator` comparison on the
/// rollback path, or zero would collide with the zero operator and strand the
/// only lever the admin has.
#[test]
fn the_rollback_survives_an_already_zeroed_operator() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");

    // Simulate a vault zeroed before the guard existed.
    let mut state = ctx.vault_state_data();
    state.operator = Pubkey::default();
    ctx.force_overwrite_vault_state(state);

    ctx.set_operator_subaccount(Pubkey::default())
        .expect("the rollback must work with a zeroed operator");
    assert_eq!(ctx.vault_state_data().operator_subaccount(), None);
}

/// The sequence the off-par mainnet vault would go through: deploy to the
/// operator's own ATA, then switch. Those funds can no longer be returned, so
/// the rollback is the recovery path. The runbook, executable.
#[test]
fn funds_left_in_the_old_operator_ata_are_recovered_by_rolling_back() {
    let mut ctx = funded_vault();
    let operator = ctx.operator.insecure_clone();
    ctx.operator_withdraw(DEPLOYED).expect("deploy pre-switch");
    assert_eq!(
        ctx.token_account_amount(&ctx.operator_deposit_ata),
        DEPLOYED
    );

    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key())
        .expect("switching over is permitted with funds still out");

    let err = ctx
        .operator_deposit_as(&operator, ctx.operator_deposit_ata, DEPLOYED)
        .expect_err("the old ATA is no longer an accepted source");
    assert_anchor_framework_err(&err, CONSTRAINT_TOKEN_OWNER);
    assert_eq!(ctx.vault_state_data().deployed_aum, DEPLOYED);

    ctx.set_operator_subaccount(Pubkey::default())
        .expect("rollback");
    ctx.operator_deposit(DEPLOYED)
        .expect("return after rollback");
    assert_eq!(ctx.vault_state_data().deployed_aum, 0);
}

// ---- Token-2022 ----

/// Both the ATA derivation and the delegation go through the token program, so
/// the whole flow is re-run on Token-2022 — including the config-time proof,
/// whose ATA constraint is token-program-specific.
#[test]
fn the_subaccount_flow_works_on_token_2022() {
    let mut ctx = VaultCtx::fresh_token_2022();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");

    let undelegated = ctx.new_subaccount(0);
    let err = ctx
        .set_operator_subaccount(undelegated.key())
        .expect_err("the proof applies on Token-2022 too");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);

    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");
    let operator = ctx.operator.insecure_clone();

    ctx.operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("withdraw");
    assert_eq!(ctx.token_account_amount(&sub.deposit_ata), DEPLOYED);
    ctx.operator_deposit_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("return");
    assert_eq!(ctx.vault_state_data().deployed_aum, 0);
}

/// `Approve` overwrites the delegate but leaves the allowance standing, so only
/// the identity half of the check can refuse this — the one clause no other
/// test reaches.
#[test]
fn a_delegation_re_pointed_elsewhere_stops_returns() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");
    let operator = ctx.operator.insecure_clone();
    ctx.operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("withdraw");

    let stranger = ctx.new_funded_keypair(1_000_000_000);
    ctx.approve_delegate_as(&sub, &stranger.pubkey(), DEPLOYED);

    let err = ctx
        .operator_deposit_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect_err("a delegation to someone else must not authorize a return");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
    assert_eq!(ctx.token_account_amount(&sub.deposit_ata), DEPLOYED);
}

/// A minimal allowance no longer authorizes an unbounded deployment: before the
/// coverage rule a one-unit grant let the operator push out the whole reserve.
#[test]
fn the_allowance_bounds_deployments_too() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(1);
    ctx.set_operator_subaccount(sub.key())
        .expect("a one-unit allowance still proves capability");
    let operator = ctx.operator.insecure_clone();

    let err = ctx
        .operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect_err("a one-unit allowance must not cover a full deployment");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
    assert_eq!(ctx.vault_state_data().deployed_aum, 0);

    // Exactly covered succeeds; one over does not.
    ctx.approve_vault_as_delegate(&sub, DEPLOYED);
    ctx.operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("an exactly-covering allowance must work");
    let err = ctx
        .operator_withdraw_as(&operator, sub.deposit_ata, 1)
        .expect_err("cumulative outflow must not exceed the allowance");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);
}

/// Rotating with funds out strands them at the old subaccount, and rollback does
/// not recover those. Re-naming it does, and the coverage rule keeps that path
/// open: a funded subaccount cannot have a lapsed allowance.
#[test]
fn rotating_away_from_a_funded_subaccount_is_recovered_by_re_naming_it() {
    let mut ctx = funded_vault();
    let a = ctx.new_delegated_subaccount(DEPLOYED);
    let b = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(a.key()).expect("name A");
    let operator = ctx.operator.insecure_clone();
    ctx.operator_withdraw_as(&operator, a.deposit_ata, DEPLOYED)
        .expect("deploy to A");

    ctx.set_operator_subaccount(b.key())
        .expect("rotating with funds still at A is permitted");

    // A's funds are now unreachable, and the rollback lever does not help.
    let err = ctx
        .operator_deposit_as(&operator, a.deposit_ata, DEPLOYED)
        .expect_err("A is no longer an accepted source");
    assert_anchor_framework_err(&err, CONSTRAINT_TOKEN_OWNER);
    ctx.set_operator_subaccount(Pubkey::default())
        .expect("rollback");
    let err = ctx
        .operator_deposit_as(&operator, a.deposit_ata, DEPLOYED)
        .expect_err("rollback points at the operator's ATA, not at A");
    assert_anchor_framework_err(&err, CONSTRAINT_TOKEN_OWNER);

    // Re-naming A works, because its allowance still covers its balance.
    ctx.set_operator_subaccount(a.key())
        .expect("a funded subaccount can always be re-named");
    ctx.operator_deposit_as(&operator, a.deposit_ata, DEPLOYED)
        .expect("and drained");
    assert_eq!(ctx.vault_state_data().deployed_aum, 0);
}

/// The destination's balance is externally mutable, so measuring coverage
/// against it would let one donated unit block a planned deployment — and
/// returning the donation would not give the capacity back.
#[test]
fn a_donation_to_the_subaccount_cannot_block_deployments() {
    let mut ctx = funded_vault();
    // Allowance sized to exactly the planned deployment.
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");
    let operator = ctx.operator.insecure_clone();

    // An unrelated party drops a unit in.
    let donor = ctx.new_subaccount(1);
    ctx.transfer_tokens_as(&donor.keypair, &donor.deposit_ata, &sub.deposit_ata, 1);
    assert_eq!(ctx.token_account_amount(&sub.deposit_ata), 1);

    ctx.operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("a donation must not block the planned deployment");
    assert_eq!(ctx.vault_state_data().deployed_aum, DEPLOYED);

    // And the vault's own exposure is still fully recallable.
    ctx.operator_deposit_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("return");
    assert_eq!(ctx.vault_state_data().deployed_aum, 0);
}

/// An AUM report must not reopen deployment capacity.
///
/// Your finding, as a test: `operator_update_aum` moves reported value with no
/// tokens changing hands, so basing coverage on `deployed_aum` let a mark-down
/// free allowance that a further deployment then spent — leaving principal at
/// custody after the approved amount had been returned and the delegation
/// cleared. `deployed_principal` only moves on real transfers, so the report
/// changes nothing here.
#[test]
fn an_aum_report_cannot_reopen_deployment_capacity() {
    let mut ctx = funded_vault();
    let sub = ctx.new_delegated_subaccount(DEPLOYED);
    ctx.set_operator_subaccount(sub.key()).expect("set");
    let operator = ctx.operator.insecure_clone();

    ctx.operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("deploy the whole approved amount");

    // A 0.2% mark-down, within the default limit.
    let marked_down = DEPLOYED - (DEPLOYED / 500);
    ctx.operator_update_aum(marked_down)
        .expect("reporting a small loss is permitted");
    assert_eq!(ctx.vault_state_data().deployed_aum, marked_down);
    assert_eq!(
        ctx.vault_state_data().deployed_principal,
        DEPLOYED,
        "a report must not move principal"
    );

    // The freed `deployed_aum` headroom must not buy another deployment.
    let err = ctx
        .operator_withdraw_as(&operator, sub.deposit_ata, DEPLOYED / 500)
        .expect_err("a mark-down must not reopen allowance capacity");
    assert_anchor_err(&err, ErrorCode::SubaccountDelegationMissing);

    // The approved amount still returns in full, leaving nothing behind.
    ctx.operator_deposit_as(&operator, sub.deposit_ata, DEPLOYED)
        .expect("return");
    assert_eq!(ctx.token_account_amount(&sub.deposit_ata), 0);
    assert_eq!(ctx.vault_state_data().deployed_principal, 0);
}
