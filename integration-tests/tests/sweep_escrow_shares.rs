// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! `sweep_escrow_shares` moves shares that reached the escrow without a request
//! to an account the admin names, so shares sent there by mistake can be
//! returned and a stray share cannot hold `close_vault` off forever (audit L-2).
//! Pending requests' shares are never touched.

use august_vault::errors::ErrorCode as VaultError;
use august_withdrawal_queue::errors::{ErrorCode, ANCHOR_USER_ERROR_OFFSET};
use august_withdrawal_queue::events::EscrowSwept;
use integration_tests::harness::{
    assert_anchor_err, assert_anchor_framework_err, events_of, VaultCtx, DEPOSIT_DECIMALS,
};
use litesvm::types::FailedTransactionMetadata;
use solana_sdk::{pubkey::Pubkey, signer::Signer};

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);
const DAY: u64 = 24 * 60 * 60;

fn assert_queue_err(err: &FailedTransactionMetadata, code: ErrorCode) {
    assert_anchor_framework_err(err, code as u32 + ANCHOR_USER_ERROR_OFFSET);
}

/// A vault with a holder and its queue attached.
fn vault_with_queue() -> VaultCtx {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");
    ctx.open_queue(DAY);
    ctx
}

/// The holder sends `amount` shares straight to the escrow, with no request.
fn donate_to_escrow(ctx: &mut VaultCtx, amount: u64) {
    let user = ctx.user.insecure_clone();
    let (from, mint) = (ctx.user_share_ata, ctx.share_mint);
    let escrow = ctx.queue_escrow(&mint);
    ctx.transfer_mint_tokens_as(&user, &mint, &from, &escrow, amount);
}

fn escrow_balance(ctx: &VaultCtx) -> u64 {
    ctx.token_account_amount(&ctx.queue_escrow(&ctx.share_mint))
}

/// A share account for a fresh wallet, standing in for whoever sent the shares.
fn someones_share_account(ctx: &mut VaultCtx) -> Pubkey {
    let owner = Pubkey::new_unique();
    let mint = ctx.share_mint;
    ctx.create_token_account_for(&owner, &mint)
}

/// With requests pending, only the stray part moves: the escrow ends holding
/// exactly the pending shares, the destination gets the rest, the supply is
/// unchanged, and the request still finalizes.
#[test]
fn only_the_shares_no_request_owns_are_moved() {
    let mut ctx = vault_with_queue();
    let quarter = ctx.token_account_amount(&ctx.user_share_ata) / 4;
    ctx.request_withdrawal(1, quarter).expect("request");
    donate_to_escrow(&mut ctx, 7);
    let destination = someones_share_account(&mut ctx);
    let supply_before = ctx.share_mint_supply();

    let meta = ctx.sweep_escrow_shares(destination).expect("sweep");

    assert_eq!(escrow_balance(&ctx), quarter);
    assert_eq!(ctx.token_account_amount(&destination), 7);
    assert_eq!(ctx.share_mint_supply(), supply_before, "moved, not burned");
    let q = ctx.queue_state_data();
    assert_eq!((q.pending_requests, q.pending_shares), (1, quarter));
    let e = &events_of::<EscrowSwept>(&meta)[0];
    assert_eq!(
        (e.vault, e.queue),
        (ctx.vault_state, ctx.withdrawal_queue_pda())
    );
    assert_eq!(
        (e.shares, e.destination, e.by),
        (7, destination, ctx.admin.pubkey())
    );

    ctx.warp_forward_seconds(DAY as i64);
    ctx.finalize_withdrawal(1, 1)
        .expect("the pending request still finalizes");
}

/// The audit's scenario end to end: shares sent to the escrow by mistake keep
/// the supply above zero after every holder has left, so `close_vault` is
/// refused. Swept back to the sender, they redeem like any others and the vault
/// closes.
#[test]
fn stray_shares_returned_to_their_sender_let_the_vault_close() {
    let mut ctx = vault_with_queue();
    let stray = ctx.token_account_amount(&ctx.user_share_ata) / 10;
    donate_to_escrow(&mut ctx, stray);
    ctx.release_vault().expect("release");
    let rest = ctx.token_account_amount(&ctx.user_share_ata);
    ctx.redeem(rest).expect("the holder redeems what they kept");
    assert_eq!(ctx.share_mint_supply(), stray, "only the stray shares left");
    let err = ctx.close_vault().expect_err("supply is not zero");
    assert_anchor_err(&err, VaultError::VaultNotEmpty);

    let sender = ctx.user_share_ata;
    ctx.sweep_escrow_shares(sender)
        .expect("returned to the sender");
    assert_eq!(ctx.token_account_amount(&sender), stray);
    ctx.redeem(stray).expect("the sender redeems them");
    let dust = ctx.token_account_amount(&ctx.vault_token_pda);
    if dust > 0 {
        ctx.operator_withdraw(dust).expect("empty the reserve");
    }
    ctx.close_vault().expect("the vault closes");
}

#[test]
fn nothing_to_sweep_is_refused() {
    let mut ctx = vault_with_queue();
    let quarter = ctx.token_account_amount(&ctx.user_share_ata) / 4;
    let destination = someones_share_account(&mut ctx);
    let err = ctx
        .sweep_escrow_shares(destination)
        .expect_err("an empty escrow");
    assert_queue_err(&err, ErrorCode::NothingToSweep);
    assert_anchor_framework_err(&err, 6017);

    ctx.request_withdrawal(1, quarter).expect("request");
    let err = ctx
        .sweep_escrow_shares(destination)
        .expect_err("an escrow holding only pending shares");
    assert_queue_err(&err, ErrorCode::NothingToSweep);
    assert_eq!(escrow_balance(&ctx), quarter);
}

/// The destination must hold the share mint and be owned by neither PDA:
/// nothing moves shares out of an account the queue or the vault owns, so
/// shares moved there would be stuck again.
#[test]
fn the_destination_must_be_a_share_account_neither_program_owns() {
    let mut ctx = vault_with_queue();
    donate_to_escrow(&mut ctx, 1);
    let escrow = ctx.queue_escrow(&ctx.share_mint);
    let err = ctx
        .sweep_escrow_shares(escrow)
        .expect_err("the escrow itself");
    assert_queue_err(&err, ErrorCode::InvalidSweepDestination);
    assert_anchor_framework_err(&err, 6018);

    let (queue, share_mint) = (ctx.withdrawal_queue_pda(), ctx.share_mint);
    let queue_owned = ctx.create_token_account_for(&queue, &share_mint);
    let err = ctx
        .sweep_escrow_shares(queue_owned)
        .expect_err("another account the queue owns");
    assert_queue_err(&err, ErrorCode::InvalidSweepDestination);

    let vault_state = ctx.vault_state;
    let vault_owned = ctx.create_token_account_for(&vault_state, &share_mint);
    let err = ctx
        .sweep_escrow_shares(vault_owned)
        .expect_err("an account the vault owns");
    assert_queue_err(&err, ErrorCode::InvalidSweepDestination);

    let deposit_account = ctx.user_deposit_ata;
    let err = ctx
        .sweep_escrow_shares(deposit_account)
        .expect_err("a deposit-mint account");
    assert_anchor_framework_err(&err, 2014);
    assert_eq!(escrow_balance(&ctx), 1);
}

#[test]
fn only_the_vaults_admin_sweeps() {
    let mut ctx = vault_with_queue();
    donate_to_escrow(&mut ctx, 1);
    let destination = someones_share_account(&mut ctx);
    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let err = ctx
        .sweep_escrow_shares_as(&impostor, destination)
        .expect_err("not the admin");
    assert_queue_err(&err, ErrorCode::NotVaultAdmin);

    let operator = ctx.operator.insecure_clone();
    let err = ctx
        .sweep_escrow_shares_as(&operator, destination)
        .expect_err("the operator is not the admin");
    assert_queue_err(&err, ErrorCode::NotVaultAdmin);
    assert_eq!(escrow_balance(&ctx), 1);
}

/// Every slot is bound: the queue to its PDA, and the vault, escrow and mint to
/// the queue, so no other vault's accounts can be used to drain this escrow.
#[test]
fn substituted_accounts_are_refused() {
    let mut ctx = vault_with_queue();
    donate_to_escrow(&mut ctx, 1);
    let admin = ctx.admin.insecure_clone();
    let destination = someones_share_account(&mut ctx);
    let other = ctx.new_vault_with_queue();

    let mut accounts = ctx.sweep_escrow_shares_accounts(&admin.pubkey(), destination);
    accounts.vault_state = other.vault_state;
    let err = ctx
        .send_sweep_escrow_shares(&admin, accounts)
        .expect_err("another vault");
    assert_queue_err(&err, ErrorCode::VaultMismatch);

    // A byte-for-byte copy of the real queue at an address that is not its PDA.
    let copy = Pubkey::new_unique();
    let real = ctx
        .svm
        .get_account(&ctx.withdrawal_queue_pda())
        .expect("queue");
    ctx.svm.set_account(copy, real).expect("plant the copy");
    let mut accounts = ctx.sweep_escrow_shares_accounts(&admin.pubkey(), destination);
    accounts.queue = copy;
    let err = ctx
        .send_sweep_escrow_shares(&admin, accounts)
        .expect_err("a queue away from its PDA");
    assert_anchor_framework_err(&err, 2006);

    let mut accounts = ctx.sweep_escrow_shares_accounts(&admin.pubkey(), destination);
    accounts.escrow_shares = other.escrow_shares;
    let err = ctx
        .send_sweep_escrow_shares(&admin, accounts)
        .expect_err("another queue's escrow");
    assert_anchor_framework_err(&err, 2001);

    let mut accounts = ctx.sweep_escrow_shares_accounts(&admin.pubkey(), destination);
    accounts.share_mint = other.share_mint;
    let err = ctx
        .send_sweep_escrow_shares(&admin, accounts)
        .expect_err("another share mint");
    assert_anchor_framework_err(&err, 2001);
    assert_eq!(escrow_balance(&ctx), 1);
}

/// Works whether or not the queue is attached, and under Token-2022.
#[test]
fn a_token_2022_escrow_is_swept_after_a_release() {
    let mut ctx = VaultCtx::fresh_token_2022();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");
    ctx.open_queue(DAY);
    donate_to_escrow(&mut ctx, 3);
    ctx.release_vault().expect("release");

    let sender = ctx.user_share_ata;
    let before = ctx.token_account_amount(&sender);
    ctx.sweep_escrow_shares(sender)
        .expect("sweep under Token-2022");
    assert_eq!(escrow_balance(&ctx), 0);
    assert_eq!(ctx.token_account_amount(&sender) - before, 3);
}
