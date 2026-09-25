// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! `expedite_request` moves one request's eligibility to now. It only ever
//! widens the owner's window: `scheduled_eligible_at` and `expires_at` are never
//! touched.

use august_withdrawal_queue::errors::{ErrorCode, ANCHOR_USER_ERROR_OFFSET};
use august_withdrawal_queue::events::WithdrawalExpedited;
use integration_tests::harness::{
    assert_anchor_framework_err, events_of, VaultCtx, DEPOSIT_DECIMALS,
};
use litesvm::types::FailedTransactionMetadata;
use solana_sdk::{pubkey::Pubkey, signer::Signer};

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);
const DAY: u64 = 24 * 60 * 60;

fn assert_queue_err(err: &FailedTransactionMetadata, code: ErrorCode) {
    assert_anchor_framework_err(err, code as u32 + ANCHOR_USER_ERROR_OFFSET);
}

/// A vault with a holder and its queue attached under a one-day cooldown.
fn attached_vault_with_holder() -> VaultCtx {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");
    ctx.open_queue(DAY);
    ctx
}

fn request_bytes(ctx: &VaultCtx, pda: &Pubkey) -> Vec<u8> {
    ctx.svm.get_account(pda).expect("request").data
}

/// Admin and operator both may; the request becomes finalizable at once, and
/// only `eligible_at` moved.
#[test]
fn admin_and_operator_expedite_and_the_request_finalizes_at_once() {
    let mut ctx = attached_vault_with_holder();
    let user = ctx.user.pubkey();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    ctx.set_fulfillment_window(7 * DAY).expect("window");
    ctx.request_withdrawal(1, shares / 4).expect("first");
    ctx.request_withdrawal(2, shares / 4).expect("second");
    let before = ctx.request_state_data(&user, 1);
    ctx.warp_forward_seconds(3600);
    let now = ctx.now();

    let meta = ctx.expedite_request(&user, 1, 1).expect("admin expedites");
    let after = ctx.request_state_data(&user, 1);
    assert_eq!(after.eligible_at, now);
    assert_eq!(
        (after.scheduled_eligible_at, after.expires_at),
        (before.scheduled_eligible_at, before.expires_at),
        "the deadline and the original schedule do not move"
    );
    let e = &events_of::<WithdrawalExpedited>(&meta)[0];
    assert_eq!(
        (e.request, e.request_id, e.sequence),
        (ctx.request_pda(&user, 1), 1, 1)
    );
    assert_eq!(e.by, ctx.admin.pubkey());
    assert_eq!(
        (e.previous_eligible_at, e.new_eligible_at),
        (before.eligible_at, now)
    );
    ctx.finalize_withdrawal(1, 1)
        .expect("finalizable a day early");

    let operator = ctx.operator.insecure_clone();
    let meta = ctx
        .expedite_request_as(&operator, &user, 2, 2)
        .expect("operator expedites");
    assert_eq!(
        events_of::<WithdrawalExpedited>(&meta)[0].by,
        operator.pubkey()
    );
    assert_eq!(ctx.request_state_data(&user, 2).eligible_at, now);
}

/// A later window change does not reach an expedited request's deadline.
#[test]
fn a_later_window_change_leaves_an_expedited_deadline_alone() {
    let mut ctx = attached_vault_with_holder();
    let user = ctx.user.pubkey();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    ctx.set_fulfillment_window(2 * DAY).expect("window");
    ctx.request_withdrawal(1, shares / 2).expect("request");
    let expires_at = ctx.request_state_data(&user, 1).expires_at;
    ctx.expedite_request(&user, 1, 1).expect("expedite");
    ctx.set_fulfillment_window(30 * DAY).expect("longer window");
    assert_eq!(ctx.request_state_data(&user, 1).expires_at, expires_at);
}

/// Every refusal, each with the request's bytes unchanged: a stranger, an
/// already-eligible request (which a repeat is), an expired one, a stale
/// stamp, a closed one, another vault, another queue's request.
#[test]
fn every_other_case_is_refused_and_changes_nothing() {
    let mut ctx = attached_vault_with_holder();
    let user = ctx.user.pubkey();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let quarter = shares / 4;
    ctx.set_fulfillment_window(DAY).expect("window");
    ctx.request_withdrawal(1, quarter).expect("fresh");
    ctx.request_withdrawal(2, quarter).expect("to expire");
    let fresh = ctx.request_pda(&user, 1);

    let stranger = ctx.new_funded_keypair(1_000_000_000);
    let bytes = request_bytes(&ctx, &fresh);
    let err = ctx
        .expedite_request_as(&stranger, &user, 1, 1)
        .expect_err("neither admin nor operator");
    assert_queue_err(&err, ErrorCode::NotVaultAdminOrOperator);
    assert_anchor_framework_err(&err, 6014);
    let err = ctx.expedite_request(&user, 1, 9).expect_err("stale stamp");
    assert_queue_err(&err, ErrorCode::StaleRequestSequence);
    assert_eq!(request_bytes(&ctx, &fresh), bytes);

    // Expedite, then again: the second finds it eligible already.
    ctx.expedite_request(&user, 1, 1).expect("first");
    let bytes = request_bytes(&ctx, &fresh);
    let err = ctx
        .expedite_request(&user, 1, 1)
        .expect_err("already eligible");
    assert_queue_err(&err, ErrorCode::RequestAlreadyEligible);
    assert_anchor_framework_err(&err, 6015);
    assert_eq!(
        request_bytes(&ctx, &fresh),
        bytes,
        "a repeat changes nothing"
    );

    // A closed request is gone.
    ctx.finalize_withdrawal(1, 1).expect("finalize the first");
    let err = ctx.expedite_request(&user, 1, 1).expect_err("closed");
    assert_anchor_framework_err(&err, 3012);

    // A request past its deadline is refused by name.
    ctx.warp_forward_seconds(3 * DAY as i64);
    let err = ctx.expedite_request(&user, 2, 2).expect_err("expired");
    assert_queue_err(&err, ErrorCode::RequestExpired);

    // A request of another vault's queue is not this queue's; another vault
    // in the vault slot is not this queue's vault.
    let other = ctx.new_vault_with_queue();
    let admin = ctx.admin.insecure_clone();
    let mut accounts = ctx.expedite_request_accounts(&admin.pubkey(), &user, 2);
    accounts.vault_state = other.vault_state;
    let err = ctx
        .send_expedite_request(&admin, accounts, 2)
        .expect_err("another vault");
    assert_queue_err(&err, ErrorCode::VaultMismatch);
    let mut accounts = ctx.expedite_request_accounts(&admin.pubkey(), &user, 2);
    accounts.queue = other.queue;
    accounts.vault_state = other.vault_state;
    let err = ctx
        .send_expedite_request(&admin, accounts, 2)
        .expect_err("another queue");
    assert_anchor_framework_err(&err, 2001);
}

// ---- an expedite changes when, never who ----

/// The finalizer rule is the same before and after an expedite. On an open
/// request that rule admits anyone, so a keeper settles it the moment the
/// operator expedites, well before the original eligibility.
#[test]
fn an_expedited_open_request_is_finalizable_by_anyone_at_once() {
    let mut ctx = attached_vault_with_holder();
    let user = ctx.user.pubkey();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    ctx.request_withdrawal(1, shares / 4).expect("request");
    let operator = ctx.operator.insecure_clone();
    ctx.expedite_request_as(&operator, &user, 1, 1)
        .expect("operator expedites");
    let scheduled = ctx.request_state_data(&user, 1).scheduled_eligible_at;
    assert!(
        ctx.now() < scheduled,
        "still before the original eligibility"
    );

    let keeper = ctx.new_funded_keypair(1_000_000_000);
    ctx.finalize_withdrawal_as(&keeper, &user, 1, 1)
        .expect("a keeper settles it before the original eligibility");
}

/// On a request naming a finalizer the rule is that key or the owner, and an
/// expedite does not widen it: a stranger is refused after the expedite with
/// the same error as before it, and the named finalizer succeeds.
#[test]
fn an_expedite_does_not_change_who_may_finalize_a_named_request() {
    let mut ctx = attached_vault_with_holder();
    let user = ctx.user.insecure_clone();
    let owner = user.pubkey();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let (share_ata, deposit_ata) = (ctx.user_share_ata, ctx.user_deposit_ata);
    let ops = ctx.new_funded_keypair(1_000_000_000);
    ctx.request_withdrawal_as(&user, share_ata, deposit_ata, 1, shares / 4, ops.pubkey())
        .expect("request naming a finalizer");
    let stranger = ctx.new_funded_keypair(1_000_000_000);
    let err = ctx
        .finalize_withdrawal_as(&stranger, &owner, 1, 1)
        .expect_err("a stranger before the expedite");
    assert_queue_err(&err, ErrorCode::FinalizerNotAllowed);

    ctx.expedite_request(&owner, 1, 1).expect("expedite");
    let before = request_bytes(&ctx, &ctx.request_pda(&owner, 1));
    let err = ctx
        .finalize_withdrawal_as(&stranger, &owner, 1, 1)
        .expect_err("a stranger after the expedite");
    assert_queue_err(&err, ErrorCode::FinalizerNotAllowed);
    assert_eq!(request_bytes(&ctx, &ctx.request_pda(&owner, 1)), before);

    ctx.finalize_withdrawal_as(&ops, &owner, 1, 1)
        .expect("the named finalizer settles early");
}

/// Expedite and finalize in one operator transaction. On an open request the
/// finalizer rule admits the operator like anyone else, so it succeeds: the
/// price paid is the vault's, bounded by its cap on `operator_update_aum`.
/// On a request naming a finalizer the finalize is refused and the expedite
/// rolls back with it, exactly as it would without the expedite.
#[test]
fn expedite_and_finalize_in_one_transaction_follow_the_finalizer_rule() {
    let mut ctx = attached_vault_with_holder();
    let user = ctx.user.insecure_clone();
    let owner = user.pubkey();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let operator = ctx.operator.insecure_clone();

    ctx.request_withdrawal(1, shares / 4).expect("open request");
    let ixs = [
        ctx.expedite_request_ix(&operator.pubkey(), &owner, 1, 1),
        ctx.finalize_withdrawal_ix(&operator.pubkey(), &owner, 1, 1),
    ];
    ctx.send_instructions(&operator, &ixs)
        .expect("expedite then settle an open request, atomically");

    let (share_ata, deposit_ata) = (ctx.user_share_ata, ctx.user_deposit_ata);
    let ops = ctx.new_funded_keypair(1_000_000_000);
    ctx.request_withdrawal_as(&user, share_ata, deposit_ata, 2, shares / 4, ops.pubkey())
        .expect("request naming a finalizer");
    let before = request_bytes(&ctx, &ctx.request_pda(&owner, 2));
    let ixs = [
        ctx.expedite_request_ix(&operator.pubkey(), &owner, 2, 2),
        ctx.finalize_withdrawal_ix(&operator.pubkey(), &owner, 2, 2),
    ];
    let err = ctx
        .send_instructions(&operator, &ixs)
        .expect_err("the operator is not the named finalizer");
    assert_queue_err(&err, ErrorCode::FinalizerNotAllowed);
    assert_eq!(request_bytes(&ctx, &ctx.request_pda(&owner, 2)), before);
}
