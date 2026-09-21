// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! `release_vault` returns the vault to instant redemption through the real
//! `invoke_signed` path: the queue PDA co-signs the vault's
//! `detach_withdrawal_queue`. Its one precondition is that the reserve covers
//! every pending request; those requests then finalize or cancel in the
//! released state, and the vault can be attached again.

use august_vault::errors::ErrorCode as VaultError;
use august_vault::instructions::detach_withdrawal_queue::WithdrawalQueueDetached;
use august_vault::state::vault::VAULT_STATE_SEED;
use august_withdrawal_queue::errors::{ErrorCode, ANCHOR_USER_ERROR_OFFSET};
use august_withdrawal_queue::events::VaultReleased;
use integration_tests::harness::{
    assert_anchor_err, assert_anchor_framework_err, events_of, VaultCtx, DEPOSIT_DECIMALS,
    VAULT_VERSION,
};
use litesvm::types::FailedTransactionMetadata;
use solana_sdk::{pubkey::Pubkey, signer::Signer};

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);
const DAY: u64 = 24 * 60 * 60;

fn assert_queue_err(err: &FailedTransactionMetadata, code: ErrorCode) {
    assert_anchor_framework_err(err, code as u32 + ANCHOR_USER_ERROR_OFFSET);
}

/// A vault with a holder and its queue attached.
fn attached_vault_with_holder() -> VaultCtx {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");
    ctx.open_queue(DAY);
    ctx
}

fn assert_released(
    meta: &litesvm::types::TransactionMetadata,
    ctx: &VaultCtx,
    pending: (u64, u64),
) {
    let queue = ctx.withdrawal_queue_pda();
    assert_eq!(
        ctx.vault_state_data().withdrawal_queue(),
        None,
        "the vault's authority is cleared"
    );
    let detached = events_of::<WithdrawalQueueDetached>(meta);
    assert_eq!(detached.len(), 1, "the vault's own event, through the CPI");
    assert_eq!(
        (detached[0].vault, detached[0].queue),
        (ctx.vault_state, queue)
    );
    let released = events_of::<VaultReleased>(meta);
    assert_eq!(released.len(), 1);
    let e = &released[0];
    assert_eq!((e.vault, e.queue), (ctx.vault_state, queue));
    assert_eq!((e.pending_requests, e.pending_shares), pending);
    assert_eq!(e.assets_owed, ctx.quote_redeem(pending.1).0);
}

// ---- the transition ----

#[test]
fn the_admin_releases_an_empty_queue_and_instant_redemption_returns() {
    let mut ctx = attached_vault_with_holder();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    assert!(ctx.redeem(1).is_err(), "gated");

    let meta = ctx.release_vault().expect("release");
    eprintln!("release_vault consumed {} CU", meta.compute_units_consumed);
    assert_released(&meta, &ctx, (0, 0));

    ctx.redeem(shares / 4).expect("instant redemption is back");
    let err = ctx
        .request_withdrawal(1, shares / 4, 0)
        .expect_err("no new requests once released");
    assert_queue_err(&err, ErrorCode::QueueNotActiveOnVault);
    let q = ctx.queue_state_data();
    assert_eq!(q.vault_state, ctx.vault_state, "the queue account persists");
}

/// Pending requests survive a release: the reserve covers them, they stay
/// payable through the queue with the gate off, the owner can still cancel,
/// and a re-attach picks up where the queue left off.
#[test]
fn requests_pending_across_a_release_finalize_or_cancel_and_the_vault_reattaches() {
    let mut ctx = attached_vault_with_holder();
    let user = ctx.user.pubkey();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let quarter = shares / 4;
    ctx.request_withdrawal(1, quarter, 0).expect("first");
    ctx.request_withdrawal(2, quarter, 0).expect("second");

    let meta = ctx.release_vault().expect("release with two pending");
    assert_released(&meta, &ctx, (2, 2 * quarter));

    // A third holder exits instantly meanwhile, as state A allows.
    let bob = ctx.new_depositor(DEPOSIT_AMOUNT);
    ctx.deposit_as(&bob, DEPOSIT_AMOUNT).expect("bob deposits");
    let bob_shares = ctx.token_account_amount(&bob.share_ata);
    ctx.redeem_as(&bob, bob_shares).expect("instant");

    ctx.warp_forward_seconds(DAY as i64);
    let paid_before = ctx.token_account_amount(&ctx.user_deposit_ata);
    ctx.finalize_withdrawal(1, 1)
        .expect("finalize with the gate off");
    assert!(ctx.token_account_amount(&ctx.user_deposit_ata) > paid_before);
    ctx.cancel_withdrawal(2, 2).expect("cancel the other");
    assert_eq!(ctx.queue_state_data().pending_requests, 0);

    let pda = ctx.withdrawal_queue_pda();
    ctx.attach_withdrawal_queue(pda).expect("re-attach");
    assert!(ctx.redeem(1).is_err(), "gated again");
    ctx.request_withdrawal(3, quarter, 0)
        .expect("requests resume");
    assert_eq!(
        ctx.request_state_data(&user, 3).sequence,
        3,
        "the sequence continues across the release"
    );
}

#[test]
fn release_works_while_paused() {
    let mut ctx = attached_vault_with_holder();
    ctx.pause().expect("pause");
    let meta = ctx.release_vault().expect("release while paused");
    assert_released(&meta, &ctx, (0, 0));
}

// ---- the liquidity precondition ----

/// The reserve must cover the pending set at today's price, exactly: covered
/// to the token passes, one token short is refused with nothing changed, and an
/// operator deposit makes it pass again.
#[test]
fn release_is_refused_while_the_reserve_cannot_cover_the_pending_set() {
    let mut ctx = attached_vault_with_holder();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    ctx.request_withdrawal(1, shares / 2, 0).expect("request");
    let (owed, _) = ctx.quote_redeem(shares / 2);
    let reserve = ctx.token_account_amount(&ctx.vault_token_pda);
    assert!(owed < reserve);

    ctx.operator_withdraw(reserve - owed + 1).expect("deploy");
    assert_eq!(ctx.vault_state_data().local_aum, owed - 1);
    let err = ctx.release_vault().expect_err("one token short");
    assert_queue_err(&err, ErrorCode::ReleaseUnderfunded);
    assert_anchor_framework_err(&err, 6016);
    assert_eq!(
        ctx.vault_state_data().withdrawal_queue(),
        Some(ctx.withdrawal_queue_pda()),
        "still attached"
    );
    assert!(ctx.redeem(1).is_err(), "still gated");

    ctx.operator_deposit(1).expect("return one token");
    assert_eq!(ctx.vault_state_data().local_aum, owed);
    let meta = ctx.release_vault().expect("covered to the token");
    assert_released(&meta, &ctx, (1, shares / 2));
}

// ---- who, and against what ----

#[test]
fn only_the_vaults_admin_releases() {
    let mut ctx = attached_vault_with_holder();
    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let err = ctx.release_vault_as(&impostor).expect_err("not the admin");
    assert_queue_err(&err, ErrorCode::NotVaultAdmin);
    assert_anchor_framework_err(&err, 6000);
    assert_eq!(
        ctx.vault_state_data().withdrawal_queue(),
        Some(ctx.withdrawal_queue_pda())
    );
}

/// The vault's own checks come back through the CPI: a queue that is not
/// attached cannot release, and releasing twice reports the same.
#[test]
fn releasing_a_vault_the_queue_is_not_attached_to_is_refused_by_the_vault() {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");
    ctx.initialize_queue(DAY).expect("initialize only");
    let err = ctx.release_vault().expect_err("never attached");
    assert_anchor_err(&err, VaultError::WithdrawalQueueNotAttached);

    let pda = ctx.withdrawal_queue_pda();
    ctx.attach_withdrawal_queue(pda).expect("attach");
    ctx.release_vault().expect("release");
    let err = ctx.release_vault().expect_err("released twice");
    assert_anchor_err(&err, VaultError::WithdrawalQueueNotAttached);
}

/// Every slot is bound to the queue's stored keys; the admin of another vault
/// holding that vault's account cannot release this one.
#[test]
fn substituted_accounts_are_refused() {
    let mut ctx = attached_vault_with_holder();
    let admin = ctx.admin.insecure_clone();
    let mint_b = ctx.create_extra_deposit_mint();
    let authority = ctx.protocol_authority.insecure_clone();
    ctx.try_initialize_vault(&authority, mint_b, VAULT_VERSION)
        .expect("vault B");
    let vault_b = Pubkey::find_program_address(
        &[VAULT_STATE_SEED, mint_b.as_ref(), &[VAULT_VERSION]],
        &august_vault::ID,
    )
    .0;
    let (deposit_mint, share_mint) = (ctx.deposit_mint, ctx.share_mint);

    let mut accounts = ctx.release_vault_accounts(&admin.pubkey());
    accounts.vault_state = vault_b;
    let err = ctx
        .send_release_vault(&admin, accounts)
        .expect_err("another vault");
    assert_queue_err(&err, ErrorCode::VaultMismatch);

    let mut accounts = ctx.release_vault_accounts(&admin.pubkey());
    accounts.share_mint = deposit_mint;
    let err = ctx
        .send_release_vault(&admin, accounts)
        .expect_err("share mint substituted");
    assert_anchor_framework_err(&err, 2001);

    let mut accounts = ctx.release_vault_accounts(&admin.pubkey());
    accounts.deposit_mint = share_mint;
    let err = ctx
        .send_release_vault(&admin, accounts)
        .expect_err("deposit mint substituted");
    assert_anchor_framework_err(&err, 2001);

    assert_eq!(
        ctx.vault_state_data().withdrawal_queue(),
        Some(ctx.withdrawal_queue_pda()),
        "still attached"
    );
}
