// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! `admin_cancel_withdrawal` returns a request's shares to its owner without
//! the owner's involvement, only once the vault is released: the tool for
//! clearing abandoned escrow, never for touching a live queue.

use august_withdrawal_queue::errors::{ErrorCode, ANCHOR_USER_ERROR_OFFSET};
use august_withdrawal_queue::events::WithdrawalCancelled;
use integration_tests::harness::{
    assert_anchor_framework_err, events_of, VaultCtx, DEPOSIT_DECIMALS,
};
use litesvm::types::FailedTransactionMetadata;
use solana_sdk::signer::Signer;

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);
const DAY: u64 = 24 * 60 * 60;

fn assert_queue_err(err: &FailedTransactionMetadata, code: ErrorCode) {
    assert_anchor_framework_err(err, code as u32 + ANCHOR_USER_ERROR_OFFSET);
}

/// A vault with a holder, its queue attached, and request 1 for half the
/// holder's shares. Returns the context and that half.
fn holder_with_request() -> (VaultCtx, u64) {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");
    ctx.open_queue(DAY);
    let half = ctx.token_account_amount(&ctx.user_share_ata) / 2;
    ctx.request_withdrawal(1, half, 0).expect("request");
    (ctx, half)
}

fn counters(ctx: &VaultCtx) -> (u64, u64) {
    let q = ctx.queue_state_data();
    (q.pending_requests, q.pending_shares)
}

#[test]
fn admin_cancel_is_refused_while_the_queue_is_attached() {
    let (mut ctx, half) = holder_with_request();
    let user = ctx.user.pubkey();
    let ata = ctx.user_share_ata;
    ctx.warp_forward_seconds(2 * DAY as i64);

    let err = ctx
        .admin_cancel_withdrawal(&user, 1, 1, ata)
        .expect_err("attached, even with the request mature");
    assert_queue_err(&err, ErrorCode::QueueStillAttached);
    assert_anchor_framework_err(&err, 6017);
    assert_eq!(counters(&ctx), (1, half));
    assert!(ctx.svm.get_account(&ctx.request_pda(&user, 1)).is_some());
}

/// Once released, any request goes, mature or not: the owner could redeem
/// instantly anyway. Rent returns to the owner, who signs nothing.
#[test]
fn after_a_release_the_admin_returns_an_immature_requests_shares_and_rent() {
    let (mut ctx, half) = holder_with_request();
    let user = ctx.user.pubkey();
    let ata = ctx.user_share_ata;
    ctx.release_vault().expect("release");
    let rent = ctx
        .svm
        .get_account(&ctx.request_pda(&user, 1))
        .expect("request")
        .lamports;
    let owner_lamports = ctx.svm.get_balance(&user).expect("owner");
    let shares_before = ctx.token_account_amount(&ata);
    assert!(
        !ctx.request_state_data(&user, 1).is_eligible(ctx.now()),
        "still in its cooldown"
    );

    let meta = ctx
        .admin_cancel_withdrawal(&user, 1, 1, ata)
        .expect("admin cancel");

    assert_eq!(ctx.token_account_amount(&ata) - shares_before, half);
    assert_eq!(
        ctx.svm.get_balance(&user).expect("owner"),
        owner_lamports + rent
    );
    assert!(ctx.svm.get_account(&ctx.request_pda(&user, 1)).is_none());
    assert_eq!(counters(&ctx), (0, 0));
    let e = &events_of::<WithdrawalCancelled>(&meta)[0];
    assert_eq!((e.owner, e.by), (user, ctx.admin.pubkey()));
    assert_eq!((e.shares, e.destination), (half, ata));
    assert_eq!((e.request_id, e.sequence), (1, 1));

    ctx.redeem(half).expect("and the owner redeems instantly");
}

/// Decision 6's fallback: an owner who gave their ATA away cannot block the
/// admin, who creates a plain token account with the owner as authority, which
/// anyone may do, and pays that instead.
#[test]
fn the_admin_pays_a_fresh_account_when_the_owner_gave_their_ata_away() {
    let (mut ctx, half) = holder_with_request();
    let user = ctx.user.insecure_clone();
    let (ata, share_mint) = (ctx.user_share_ata, ctx.share_mint);
    ctx.release_vault().expect("release");
    let stranger = ctx.new_funded_keypair(1_000_000_000).pubkey();
    ctx.set_token_account_authority_as(&user, &ata, &stranger);

    let err = ctx
        .admin_cancel_withdrawal(&user.pubkey(), 1, 1, ata)
        .expect_err("the ATA belongs to the stranger now");
    assert_anchor_framework_err(&err, 2015);

    let fresh = ctx.create_token_account_for(&user.pubkey(), &share_mint);
    ctx.admin_cancel_withdrawal(&user.pubkey(), 1, 1, fresh)
        .expect("a fresh owner-controlled account");
    assert_eq!(ctx.token_account_amount(&fresh), half);
}

#[test]
fn only_the_vaults_admin_cancels_and_only_into_the_owners_account() {
    let (mut ctx, half) = holder_with_request();
    let user = ctx.user.pubkey();
    let ata = ctx.user_share_ata;
    ctx.release_vault().expect("release");
    let other = ctx.new_depositor(0);

    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let err = ctx
        .admin_cancel_withdrawal_as(&impostor, &user, 1, 1, ata)
        .expect_err("not the admin");
    assert_queue_err(&err, ErrorCode::NotVaultAdmin);

    let operator = ctx.operator.insecure_clone();
    let err = ctx
        .admin_cancel_withdrawal_as(&operator, &user, 1, 1, ata)
        .expect_err("the operator is not the admin");
    assert_queue_err(&err, ErrorCode::NotVaultAdmin);

    let err = ctx
        .admin_cancel_withdrawal(&user, 1, 1, other.share_ata)
        .expect_err("someone else's share account");
    assert_anchor_framework_err(&err, 2015);

    let deposit_ata = ctx.user_deposit_ata;
    let err = ctx
        .admin_cancel_withdrawal(&user, 1, 1, deposit_ata)
        .expect_err("the owner's account, but for the deposit mint");
    assert_anchor_framework_err(&err, 2014);

    let err = ctx
        .admin_cancel_withdrawal(&user, 1, 2, ata)
        .expect_err("stale sequence");
    assert_queue_err(&err, ErrorCode::StaleRequestSequence);

    assert_eq!(counters(&ctx), (1, half));
    assert_eq!(ctx.token_account_amount(&other.share_ata), 0);
}

/// The owner slot is bound by `has_one`, so the rent cannot be pointed at the
/// admin; the vault slot is bound to the queue, so another vault's admin
/// cannot use their own released vault to cancel here; and the request is
/// bound to the queue, so another queue's request cannot draw on this escrow.
#[test]
fn substituted_accounts_are_refused() {
    let (mut ctx, _) = holder_with_request();
    let admin = ctx.admin.insecure_clone();
    let user = ctx.user.pubkey();
    let ata = ctx.user_share_ata;
    ctx.release_vault().expect("release");
    let other = ctx.new_vault_with_queue();

    let mut accounts = ctx.admin_cancel_withdrawal_accounts(&admin.pubkey(), &user, 1, ata);
    accounts.owner = admin.pubkey();
    let err = ctx
        .send_admin_cancel_withdrawal(&admin, accounts, 1)
        .expect_err("rent redirected");
    assert_anchor_framework_err(&err, 2001);

    let mut accounts = ctx.admin_cancel_withdrawal_accounts(&admin.pubkey(), &user, 1, ata);
    accounts.vault_state = other.vault_state;
    let err = ctx
        .send_admin_cancel_withdrawal(&admin, accounts, 1)
        .expect_err("another vault");
    assert_queue_err(&err, ErrorCode::VaultMismatch);

    let foreign = ctx.install_foreign_request(&other, &user, 1, 1, ata);
    let mut accounts = ctx.admin_cancel_withdrawal_accounts(&admin.pubkey(), &user, 1, ata);
    accounts.request = foreign;
    let err = ctx
        .send_admin_cancel_withdrawal(&admin, accounts, 1)
        .expect_err("another queue's request");
    assert_anchor_framework_err(&err, 2001);

    assert!(ctx.svm.get_account(&ctx.request_pda(&user, 1)).is_some());
    assert_eq!(counters(&ctx), (1, ctx.request_state_data(&user, 1).shares));
}
