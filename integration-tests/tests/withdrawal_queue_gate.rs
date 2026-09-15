// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Behaviour of the `withdrawal_queue_authority` gate on `redeem` /
//! `redeem_checked`.
//!
//! Three states matter: **zero** (no queue — what every live vault reads, so
//! "unchanged" is the property that protects funds), **set with someone else
//! signing** (refused, and refused having changed nothing), and **set with the
//! authority signing** (allowed — the path the queue itself takes by CPI).
//!
//! Layout compatibility against real mainnet bytes lives in
//! `mainnet_fork_compat.rs`; this file is behaviour only. The authority is
//! written straight into the account because the admin instruction that sets it
//! does not exist yet.

use august_vault::errors::ErrorCode;
use integration_tests::harness::{
    assert_anchor_err, assert_anchor_framework_err, VaultCtx, DEPOSIT_DECIMALS,
};
use solana_sdk::{pubkey::Pubkey, signer::Signer};

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);

/// A funded vault whose user holds redeemable shares.
fn vault_with_a_holder() -> VaultCtx {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("initial deposit");
    ctx
}

/// Point the vault at `authority`, bypassing the not-yet-written admin
/// instruction.
fn attach_queue(ctx: &mut VaultCtx, authority: Pubkey) {
    let mut state = ctx.vault_state_data();
    state.withdrawal_queue_authority = authority;
    ctx.force_overwrite_vault_state(state);
}

// ---- zero: nothing changes ----

/// The state every live vault is in. A holder redeems exactly as before.
#[test]
fn an_ungated_vault_redeems_as_before() {
    let mut ctx = vault_with_a_holder();
    assert_eq!(
        ctx.vault_state_data().withdrawal_queue_authority,
        Pubkey::default(),
        "a fresh vault must open ungated"
    );
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let before = ctx.token_account_amount(&ctx.user_deposit_ata);

    ctx.redeem(shares).expect("ungated redeem must succeed");

    assert_eq!(
        ctx.token_account_amount(&ctx.user_share_ata),
        0,
        "shares should have been burned"
    );
    assert!(
        ctx.token_account_amount(&ctx.user_deposit_ata) > before,
        "the holder should have been paid"
    );
}

// ---- set, wrong signer: refused ----

#[test]
fn a_gated_vault_refuses_a_direct_holder_redeem() {
    let mut ctx = vault_with_a_holder();
    attach_queue(&mut ctx, Pubkey::new_unique());

    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let err = ctx
        .redeem(shares)
        .expect_err("a holder must not redeem directly once a queue is attached");
    assert_anchor_err(&err, ErrorCode::WithdrawalQueueRequired);

    // The literal number: `assert_anchor_err` derives its expectation from the
    // same enum the program was built from, so a reorder moves both sides
    // together. Consumers match on the number, so pin the number.
    assert_anchor_framework_err(&err, 6021);
}

/// Both entry points share `handler_checked`, so the gate must cover
/// `redeem_checked` too — including with a slippage bound that would otherwise
/// pass.
#[test]
fn a_gated_vault_refuses_redeem_checked_as_well() {
    let mut ctx = vault_with_a_holder();
    let depositor = ctx.new_depositor(DEPOSIT_AMOUNT);
    ctx.deposit_as(&depositor, DEPOSIT_AMOUNT)
        .expect("second depositor");
    attach_queue(&mut ctx, Pubkey::new_unique());

    let shares = ctx.token_account_amount(&depositor.share_ata);
    let err = ctx
        .redeem_checked_as(&depositor, shares, 0)
        .expect_err("redeem_checked must be gated too");
    assert_anchor_err(&err, ErrorCode::WithdrawalQueueRequired);
}

/// The gate is a Check, so a refused redeem must leave every observable slot
/// untouched — same invariant the C-01 CEI suite pins for the other revert
/// paths.
#[test]
fn a_refused_gated_redeem_has_no_side_effects() {
    let mut ctx = vault_with_a_holder();
    attach_queue(&mut ctx, Pubkey::new_unique());

    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let before = ctx.snapshot();

    let err = ctx.redeem(shares).expect_err("should be refused");
    assert_anchor_err(&err, ErrorCode::WithdrawalQueueRequired);

    assert_eq!(
        ctx.snapshot(),
        before,
        "CEI violated on the withdrawal-queue gate"
    );
}

/// The gate runs before pricing, so it fires even for a redeem that would have
/// failed anyway. Guards against a future reordering that lets a gated vault
/// report an unrelated error and mask the real reason.
#[test]
fn the_gate_precedes_the_liquidity_check() {
    let mut ctx = vault_with_a_holder();
    // Drain the reserve so an ungated redeem would hit NotEnoughLiquidity.
    ctx.operator_withdraw(DEPOSIT_AMOUNT * 9 / 10)
        .expect("operator withdraw");
    attach_queue(&mut ctx, Pubkey::new_unique());

    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let err = ctx.redeem(shares).expect_err("should be refused");
    assert_anchor_err(&err, ErrorCode::WithdrawalQueueRequired);
}

// ---- set, and the signer is the authority: allowed ----

/// The path the queue takes when finalizing: signer equals the stored authority.
///
/// The authority here is the holder's own key, so this cannot separate "signer
/// equals the stored key" from "signer owns the share account" — the
/// `token::authority = signer` constraints force them equal anyway. The cross-key
/// case belongs to the queue's own CPI tests, which will also exercise what this
/// gate implies: the queue PDA must hold the shares and own a payout account.
#[test]
fn the_queue_authority_may_redeem() {
    let mut ctx = vault_with_a_holder();
    let holder = ctx.user.pubkey();
    attach_queue(&mut ctx, holder);

    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let before = ctx.token_account_amount(&ctx.user_deposit_ata);

    ctx.redeem(shares)
        .expect("the stored authority must be allowed through the gate");

    assert_eq!(ctx.token_account_amount(&ctx.user_share_ata), 0);
    assert!(ctx.token_account_amount(&ctx.user_deposit_ata) > before);
}

/// Detaching restores direct redemption, which is what the queue's release path
/// does at the end of a drain.
#[test]
fn clearing_the_authority_restores_direct_redemption() {
    let mut ctx = vault_with_a_holder();
    attach_queue(&mut ctx, Pubkey::new_unique());

    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    assert_anchor_err(
        &ctx.redeem(shares).expect_err("gated"),
        ErrorCode::WithdrawalQueueRequired,
    );

    attach_queue(&mut ctx, Pubkey::default());
    ctx.redeem(shares)
        .expect("clearing the gate must reopen direct redemption");
}

// ---- the ABI number itself ----

/// Deposits are untouched by the gate — a vault that takes deposits it cannot
/// return is the worst state this feature could produce by accident.
#[test]
fn a_gated_vault_still_accepts_deposits() {
    let mut ctx = vault_with_a_holder();
    attach_queue(&mut ctx, Pubkey::new_unique());

    let before = ctx.token_account_amount(&ctx.user_share_ata);
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT)
        .expect("a gated vault must still accept deposits");
    assert!(
        ctx.token_account_amount(&ctx.user_share_ata) > before,
        "the depositor should have been minted shares"
    );
}

/// Pause is checked before the gate, so a paused AND gated vault reports
/// `VaultPaused`. The queue depends on this: its finalize inherits `VaultPaused`
/// through the CPI only while pause wins.
#[test]
fn pause_is_reported_ahead_of_the_gate() {
    let mut ctx = vault_with_a_holder();
    attach_queue(&mut ctx, Pubkey::new_unique());
    ctx.pause().expect("admin pause");

    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let err = ctx.redeem(shares).expect_err("paused and gated");
    assert_anchor_err(&err, ErrorCode::VaultPaused);
}
