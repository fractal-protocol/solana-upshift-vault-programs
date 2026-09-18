// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! `request_withdrawal` and `update_request`: a holder's shares move into the
//! queue's escrow and a request account is born with the queue's current
//! cooldown and window stamped on it; the owner may then adjust the floor, the
//! recipient and the finalizer, and nothing else. Neither instruction touches
//! the vault, which is read for its gate only.

use august_withdrawal_queue::errors::{ErrorCode, ANCHOR_USER_ERROR_OFFSET};
use august_withdrawal_queue::events::{WithdrawalRequestUpdated, WithdrawalRequested};
use august_withdrawal_queue::state::{WithdrawalRequest, WITHDRAWAL_REQUEST_SEED};
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

/// A vault with a holder, and its queue initialized, attached and accepting.
fn open_vault_with_holder(cooldown: u64) -> (VaultCtx, u64) {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");
    ctx.open_queue(cooldown);
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    assert!(shares > 0);
    (ctx, shares)
}

fn escrow_shares_balance(ctx: &VaultCtx) -> u64 {
    ctx.token_account_amount(&ctx.queue_escrow(&ctx.share_mint))
}

// ---- request_withdrawal ----

#[test]
fn a_holder_escrows_shares_and_opens_a_request() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let half = shares / 2;
    let now = ctx.now();
    let user = ctx.user.pubkey();

    let meta = ctx
        .request_withdrawal(1, half, 1_000)
        .expect("request_withdrawal");
    eprintln!(
        "request_withdrawal consumed {} CU",
        meta.compute_units_consumed
    );
    assert!(
        meta.compute_units_consumed < 100_000,
        "the design budgets about 40k CU for a request"
    );

    assert_eq!(ctx.token_account_amount(&ctx.user_share_ata), shares - half);
    assert_eq!(
        escrow_shares_balance(&ctx),
        half,
        "exactly `shares` escrowed"
    );

    let r = ctx.request_state_data(&user, 1);
    let pda = ctx.request_pda(&user, 1);
    assert_eq!(r.queue, ctx.withdrawal_queue_pda());
    assert_eq!(r.owner, user);
    assert_eq!(r.recipient_token_account, ctx.user_deposit_ata);
    assert_eq!(r.allowed_finalizer(), None);
    assert_eq!(r.shares, half);
    assert_eq!(r.min_assets_out, 1_000);
    assert_eq!(r.request_id, 1);
    assert_eq!(r.sequence, 1, "the first stamp is 1");
    assert_eq!(r.requested_at, now);
    assert_eq!(r.scheduled_eligible_at, now + DAY as i64);
    assert_eq!(r.eligible_at, r.scheduled_eligible_at);
    assert_eq!(r.expires_at, 0, "no window configured, so never expires");
    let (_, bump) = Pubkey::find_program_address(
        &[
            WITHDRAWAL_REQUEST_SEED,
            ctx.withdrawal_queue_pda().as_ref(),
            user.as_ref(),
            &1u64.to_le_bytes(),
        ],
        &august_withdrawal_queue::ID,
    );
    assert_eq!(r.bump, bump);

    let q = ctx.queue_state_data();
    assert_eq!(
        (q.sequence, q.pending_requests, q.pending_shares),
        (1, 1, half)
    );

    let events = events_of::<WithdrawalRequested>(&meta);
    assert_eq!(events.len(), 1);
    let e = &events[0];
    assert_eq!(e.vault, ctx.vault_state);
    assert_eq!(e.queue, ctx.withdrawal_queue_pda());
    assert_eq!(e.request, pda);
    assert_eq!((e.request_id, e.owner, e.sequence), (1, user, 1));
    assert_eq!((e.shares, e.min_assets_out), (half, 1_000));
    assert_eq!(e.recipient_token_account, ctx.user_deposit_ata);
    assert_eq!(e.finalizer, Pubkey::default());
    assert_eq!((e.eligible_at, e.expires_at), (r.eligible_at, 0));

    let account = ctx.svm.get_account(&pda).expect("request account");
    assert_eq!(account.data.len(), WithdrawalRequest::LEN);
    assert_eq!(account.owner, august_withdrawal_queue::ID);
}

/// The cooldown and window are stamped at request time; changing them later
/// leaves existing requests alone and applies to the next one.
#[test]
fn timestamps_are_stamped_from_the_settings_at_request_time() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    ctx.set_fulfillment_window(7 * DAY).expect("window");
    let user = ctx.user.pubkey();
    let now = ctx.now();

    ctx.request_withdrawal(1, shares / 4, 0).expect("first");
    let first = ctx.request_state_data(&user, 1);
    assert_eq!(first.scheduled_eligible_at, now + DAY as i64);
    assert_eq!(
        first.expires_at,
        first.scheduled_eligible_at + 7 * DAY as i64
    );

    ctx.set_cooldown(2 * DAY).expect("new cooldown");
    ctx.set_fulfillment_window(0).expect("no window");
    ctx.request_withdrawal(2, shares / 4, 0).expect("second");
    assert_eq!(
        ctx.request_state_data(&user, 1).scheduled_eligible_at,
        first.scheduled_eligible_at,
        "an existing request keeps its schedule"
    );
    let second = ctx.request_state_data(&user, 2);
    assert_eq!(second.scheduled_eligible_at, now + 2 * DAY as i64);
    assert_eq!(second.expires_at, 0);
}

#[test]
fn requests_are_owner_scoped_and_sequenced() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.pubkey();
    ctx.request_withdrawal(7, shares / 4, 0).expect("id 7");
    ctx.request_withdrawal(3, shares / 4, 0).expect("id 3");

    assert_eq!(ctx.request_state_data(&user, 7).sequence, 1);
    assert_eq!(
        ctx.request_state_data(&user, 3).sequence,
        2,
        "ids are the owner's; order is the queue's"
    );
    assert_ne!(ctx.request_pda(&user, 7), ctx.request_pda(&user, 3));
    let q = ctx.queue_state_data();
    assert_eq!((q.pending_requests, q.pending_shares), (2, shares / 2));

    // An id in use cannot be reused while its request exists: `init` refuses.
    ctx.request_withdrawal(7, 1, 0)
        .expect_err("request 7 already exists");
    let q = ctx.queue_state_data();
    assert_eq!(
        (q.pending_requests, q.pending_shares),
        (2, shares / 2),
        "nothing changed"
    );
}

#[test]
fn zero_shares_is_refused() {
    let (mut ctx, _) = open_vault_with_holder(DAY);
    let err = ctx.request_withdrawal(1, 0, 0).expect_err("zero");
    assert_queue_err(&err, ErrorCode::ZeroShares);
    assert_anchor_framework_err(&err, 6008);
}

/// The escrow transfer is the token program's; asking for more than the owner
/// holds fails there, and the request account is never created.
#[test]
fn more_shares_than_held_is_refused_by_the_token_program() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.pubkey();
    let err = ctx
        .request_withdrawal(1, shares + 1, 0)
        .expect_err("insufficient shares");
    // SPL Token `InsufficientFunds`.
    assert_anchor_framework_err(&err, 1);
    assert!(ctx.svm.get_account(&ctx.request_pda(&user, 1)).is_none());
    assert_eq!(ctx.queue_state_data().pending_requests, 0);
    assert_eq!(escrow_shares_balance(&ctx), 0);
}

#[test]
fn drain_mode_refuses_new_requests() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    ctx.set_accepting_requests(false).expect("drain");
    let err = ctx.request_withdrawal(1, shares, 0).expect_err("draining");
    assert_queue_err(&err, ErrorCode::NotAcceptingRequests);
    assert_anchor_framework_err(&err, 6007);
}

/// Design decision 13. If the vault stopped pointing at this queue while it
/// still accepts, a request would escrow shares into a cooldown that direct
/// redeemers bypass. The state is produced here by writing the vault directly,
/// since no instruction can reach it once `release_vault` exists.
#[test]
fn a_queue_the_vault_no_longer_points_at_refuses_requests() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let mut vault = ctx.vault_state_data();
    vault.withdrawal_queue_authority = Pubkey::default();
    ctx.force_overwrite_vault_state(vault);
    assert!(
        ctx.queue_state_data().accepting_requests,
        "the queue still believes it is open"
    );

    let err = ctx
        .request_withdrawal(1, shares, 0)
        .expect_err("gate is gone");
    assert_queue_err(&err, ErrorCode::QueueNotActiveOnVault);
}

/// Design decision 5: a deposit-mint account, and neither program's.
#[test]
fn the_recipient_must_hold_the_deposit_mint_and_belong_to_neither_program() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.insecure_clone();
    let share_account = ctx.user_share_ata;
    let quarter = shares / 4;

    let bad = [
        ("a share-mint account", ctx.user_share_ata),
        (
            "the queue's asset escrow",
            ctx.queue_escrow(&ctx.deposit_mint),
        ),
        ("the vault's reserve", ctx.vault_token_pda),
    ];
    for (i, (what, recipient)) in bad.into_iter().enumerate() {
        let err = ctx
            .request_withdrawal_as(
                &user,
                share_account,
                recipient,
                i as u64,
                quarter,
                0,
                Pubkey::default(),
            )
            .expect_err(what);
        assert_queue_err(&err, ErrorCode::InvalidRecipient);
        assert_anchor_framework_err(&err, 6009);
    }
    assert_eq!(ctx.queue_state_data().pending_requests, 0);

    // Any other deposit-mint account is fine, whoever owns it.
    let elsewhere = ctx.fee_recipient_deposit_ata;
    ctx.request_withdrawal_as(
        &user,
        share_account,
        elsewhere,
        9,
        quarter,
        0,
        Pubkey::default(),
    )
    .expect("a third party's deposit-mint account is a valid recipient");
    assert_eq!(
        ctx.request_state_data(&user.pubkey(), 9)
            .recipient_token_account,
        elsewhere
    );
}

/// The invariant the counters exist for: `pending_shares` equals the escrow's
/// balance and `pending_requests` the number of live requests, after every
/// request.
#[test]
fn counters_track_the_escrow_across_many_requests() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.pubkey();
    let mut total = 0u64;
    let amounts = [7u64, 1, 300, 42, 9_999, shares / 10];
    for (i, amount) in amounts.iter().enumerate() {
        ctx.request_withdrawal(i as u64, *amount, 0)
            .expect("request");
        total += amount;
        let q = ctx.queue_state_data();
        assert_eq!(q.pending_requests, i as u64 + 1);
        assert_eq!(q.pending_shares, total);
        assert_eq!(
            escrow_shares_balance(&ctx),
            total,
            "counters equal the escrow balance"
        );
        assert_eq!(
            ctx.request_state_data(&user, i as u64).sequence,
            i as u64 + 1
        );
    }
}

// ---- update_request ----

#[test]
fn the_owner_updates_only_what_they_ask_for() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.pubkey();
    ctx.request_withdrawal(1, shares / 2, 1_000)
        .expect("request");
    let before = ctx.request_state_data(&user, 1);
    let ops = Pubkey::new_unique();
    let elsewhere = ctx.fee_recipient_deposit_ata;

    let meta = ctx
        .update_request(1, 1, Some(2_000), None, None)
        .expect("floor only");
    let r = ctx.request_state_data(&user, 1);
    assert_eq!(r.min_assets_out, 2_000);
    assert_eq!(r.recipient_token_account, before.recipient_token_account);
    assert_eq!(r.finalizer, before.finalizer);
    assert_eq!(
        (r.shares, r.eligible_at, r.expires_at, r.sequence),
        (
            before.shares,
            before.eligible_at,
            before.expires_at,
            before.sequence
        )
    );
    let events = events_of::<WithdrawalRequestUpdated>(&meta);
    assert_eq!(events.len(), 1);
    assert_eq!(
        (
            events[0].request,
            events[0].sequence,
            events[0].min_assets_out
        ),
        (ctx.request_pda(&user, 1), 1, 2_000)
    );

    ctx.update_request(1, 1, None, Some(elsewhere), Some(ops))
        .expect("recipient and finalizer");
    let r = ctx.request_state_data(&user, 1);
    assert_eq!(r.recipient_token_account, elsewhere);
    assert_eq!(r.allowed_finalizer(), Some(ops));
    assert_eq!(r.min_assets_out, 2_000, "untouched by the second update");

    ctx.update_request(1, 1, None, None, Some(Pubkey::default()))
        .expect("zero lifts the finalizer restriction");
    assert_eq!(ctx.request_state_data(&user, 1).allowed_finalizer(), None);
}

/// Design decision 11: an id may be reused once its account closes, so every
/// instruction targeting a request quotes the sequence it was signed against.
#[test]
fn an_update_needs_the_requests_current_sequence() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    ctx.request_withdrawal(1, shares / 2, 0).expect("request");
    let err = ctx
        .update_request(1, 2, Some(5), None, None)
        .expect_err("wrong sequence");
    assert_queue_err(&err, ErrorCode::StaleRequestSequence);
    assert_anchor_framework_err(&err, 6012);
    assert_eq!(
        ctx.request_state_data(&ctx.user.pubkey(), 1).min_assets_out,
        0
    );
}

#[test]
fn only_the_owner_updates_a_request() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.pubkey();
    ctx.request_withdrawal(1, shares / 2, 0).expect("request");
    let impostor = ctx.new_funded_keypair(1_000_000_000);

    let err = ctx
        .update_request_as(&impostor, &user, 1, 1, Some(5), None, None)
        .expect_err("not the owner");
    assert_queue_err(&err, ErrorCode::NotRequestOwner);
    assert_anchor_framework_err(&err, 6010);
}

/// After the fulfillment window an update is refused: the request can only be
/// cancelled. The deadline instant itself counts as expired.
#[test]
fn an_expired_request_cannot_be_updated() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    ctx.set_fulfillment_window(DAY).expect("window");
    ctx.request_withdrawal(1, shares / 2, 0).expect("request");
    let r = ctx.request_state_data(&ctx.user.pubkey(), 1);

    ctx.warp_forward_seconds(r.expires_at - ctx.now() - 1);
    ctx.update_request(1, 1, Some(5), None, None)
        .expect("one second before the deadline");

    ctx.warp_forward_seconds(1);
    let err = ctx
        .update_request(1, 1, Some(6), None, None)
        .expect_err("at the deadline");
    assert_queue_err(&err, ErrorCode::RequestExpired);
    assert_anchor_framework_err(&err, 6011);
    assert_eq!(
        ctx.request_state_data(&ctx.user.pubkey(), 1).min_assets_out,
        5
    );
}

#[test]
fn a_new_recipient_follows_the_same_rule() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    ctx.request_withdrawal(1, shares / 2, 0).expect("request");
    for bad in [
        ctx.queue_escrow(&ctx.deposit_mint),
        ctx.vault_token_pda,
        ctx.user_share_ata,
    ] {
        let err = ctx
            .update_request(1, 1, None, Some(bad), None)
            .expect_err("bad recipient");
        assert_queue_err(&err, ErrorCode::InvalidRecipient);
    }
    assert_eq!(
        ctx.request_state_data(&ctx.user.pubkey(), 1)
            .recipient_token_account,
        ctx.user_deposit_ata
    );
}
