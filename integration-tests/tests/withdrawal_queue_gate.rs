// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Behaviour of the `withdrawal_queue_authority` gate on `redeem` /
//! `redeem_checked` (WQ-02).
//!
//! The field is authorization state, so these tests are about who the program
//! lets through, not about pricing. Three states matter:
//!
//!   * **zero** — no queue. Every vault on mainnet today reads this, and every
//!     vault created before the field existed reads it without a migration, so
//!     "unchanged" is the property that protects live funds. Layout compatibility
//!     against real dumped mainnet accounts is proved separately in
//!     `mainnet_fork_compat.rs`; here we prove the *behaviour* is untouched.
//!   * **set, and the signer is someone else** — refused with
//!     `WithdrawalQueueRequired`, and refused having changed nothing (CEI).
//!   * **set, and the signer IS the authority** — allowed, because this is the
//!     path the queue program itself takes when it finalizes a request by CPI,
//!     signing as its PDA.
//!
//! The authority is written straight into the account here rather than through
//! an instruction: `set_withdrawal_queue_authority` is WQ-03 and does not exist
//! yet. That keeps this suite about the gate alone — WQ-03 brings the tests for
//! how the field may legally be set, and WQ-08 covers the real queue PDA signing
//! through a CPI.

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

/// Point the vault at `authority`, bypassing the (not yet written) admin
/// instruction. Returns nothing: read it back through `vault_state_data` if a
/// test needs to assert on it.
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

    // The literal number, not the enum. `assert_anchor_err` derives its expected
    // value from the same `ErrorCode` the program was built from, so a reordered
    // enum moves both sides together and every other assertion in this file still
    // passes. The SDK and the admin UI match on the number, so pin the number.
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

/// The path the queue program takes when finalizing: the signer equals the
/// stored authority, so the redemption proceeds normally.
///
/// The authority here is the holder's own key, so `signer`, the share account's
/// owner and the stored authority are all one pubkey. That means this test alone
/// cannot separate "signer equals the stored key" from "signer owns the share
/// account" — the `token::authority = signer` constraints force them equal on
/// this path anyway. The cross-key case, where the queue PDA signs for shares it
/// holds itself, is WQ-08's to pin, and it is what will exercise the coupling
/// this gate creates: a gated vault requires the queue PDA to hold the share
/// tokens AND own a deposit token account, since both are bound to the signer.
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

/// Detaching restores direct redemption, which is what `release_vault` does at
/// the end of a drain (WQ-09).
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

/// Deposits are untouched by the gate.
///
/// The gate lives in the redeem handler, but a reader has to take that on trust
/// unless something proves the other user-facing path still works on a gated
/// vault — which matters because a vault that takes deposits it cannot return is
/// the worst state this feature could produce by accident.
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
/// `VaultPaused`.
///
/// Pinned because the queue program depends on this exact ordering: decision 7
/// of the design has finalize inherit `VaultPaused` through the CPI, which only
/// holds while pause wins over the gate.
#[test]
fn pause_is_reported_ahead_of_the_gate() {
    let mut ctx = vault_with_a_holder();
    attach_queue(&mut ctx, Pubkey::new_unique());
    ctx.pause().expect("admin pause");

    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let err = ctx.redeem(shares).expect_err("paused and gated");
    assert_anchor_err(&err, ErrorCode::VaultPaused);
}
