// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! `expedite_request` moves one request's eligibility to now, and its batch
//! form does the same for a set. Both only ever widen the owner's window:
//! `scheduled_eligible_at` and `expires_at` are never touched.

use august_withdrawal_queue::batch::MAX_EXPEDITE_BATCH;
use august_withdrawal_queue::errors::{ErrorCode, ANCHOR_USER_ERROR_OFFSET};
use august_withdrawal_queue::events::WithdrawalExpedited;
use integration_tests::harness::{
    assert_anchor_framework_err, events_of, with_batch_budget, VaultCtx, DEPOSIT_DECIMALS,
};
use litesvm::types::FailedTransactionMetadata;
use solana_sdk::{
    hash::Hash, pubkey::Pubkey, signature::Keypair, signer::Signer, transaction::Transaction,
};

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);
const DAY: u64 = 24 * 60 * 60;
/// A legacy transaction's wire limit.
const PACKET_DATA_SIZE: usize = 1232;
/// How many requests a legacy transaction has room for, measured. The
/// program's bound is lower, so size never binds an expedite batch.
const MEASURED_LEGACY_ROOM: usize = 22;

fn assert_queue_err(err: &FailedTransactionMetadata, code: ErrorCode) {
    assert_anchor_framework_err(err, code as u32 + ANCHOR_USER_ERROR_OFFSET);
}

/// The last request announced before the failure is the offending one.
fn assert_blamed(err: &FailedTransactionMetadata, request: &Pubkey) {
    let logs = &err.meta.logs;
    let last = logs
        .iter()
        .rposition(|l| l == "Program log: request")
        .expect("a request was announced");
    assert_eq!(
        logs.get(last + 1).map(String::as_str),
        Some(format!("Program log: {request}").as_str()),
        "logs:
{}",
        logs.join(
            "
"
        )
    );
}

/// A vault with a holder and its queue attached under a one-day cooldown.
fn attached_vault_with_holder() -> VaultCtx {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");
    ctx.open_queue(DAY);
    ctx
}

/// Opens `n` requests of `each` shares for the user, ids 1..=n, and returns
/// their PDAs in id order.
fn open_requests(ctx: &mut VaultCtx, n: u64, each: u64) -> Vec<Pubkey> {
    let user = ctx.user.pubkey();
    (1..=n)
        .map(|id| {
            ctx.request_withdrawal(id, each, 0).expect("request");
            ctx.request_pda(&user, id)
        })
        .collect()
}

fn request_bytes(ctx: &VaultCtx, pda: &Pubkey) -> Vec<u8> {
    ctx.svm.get_account(pda).expect("request").data
}

/// The sequences of the user's requests `1..=n`, in the order of `requests`.
fn sequences_of(ctx: &VaultCtx, n: u64, requests: &[Pubkey]) -> Vec<u64> {
    let user = ctx.user.pubkey();
    requests
        .iter()
        .map(|r| {
            let id = (1..=n)
                .find(|id| ctx.request_pda(&user, *id) == *r)
                .expect("id");
            ctx.request_state_data(&user, id).sequence
        })
        .collect()
}

// ---- single ----

/// Admin and operator both may; the request becomes finalizable at once, and
/// only `eligible_at` moved.
#[test]
fn admin_and_operator_expedite_and_the_request_finalizes_at_once() {
    let mut ctx = attached_vault_with_holder();
    let user = ctx.user.pubkey();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    ctx.set_fulfillment_window(7 * DAY).expect("window");
    ctx.request_withdrawal(1, shares / 4, 0).expect("first");
    ctx.request_withdrawal(2, shares / 4, 0).expect("second");
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
    ctx.request_withdrawal(1, shares / 2, 0).expect("request");
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
    ctx.request_withdrawal(1, quarter, 0).expect("fresh");
    ctx.request_withdrawal(2, quarter, 0).expect("to expire");
    let fresh = ctx.request_pda(&user, 1);

    let stranger = ctx.new_funded_keypair(1_000_000_000);
    let bytes = request_bytes(&ctx, &fresh);
    let err = ctx
        .expedite_request_as(&stranger, &user, 1, 1)
        .expect_err("neither admin nor operator");
    assert_queue_err(&err, ErrorCode::NotVaultAdminOrOperator);
    assert_anchor_framework_err(&err, 6018);
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
    assert_anchor_framework_err(&err, 6019);
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

// ---- batch ----

#[test]
fn a_batch_expedites_every_request_and_emits_one_event_each() {
    let mut ctx = attached_vault_with_holder();
    let user = ctx.user.pubkey();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let mut requests = open_requests(&mut ctx, 5, shares / 10);
    requests.sort();
    let sequences: Vec<u64> = requests
        .iter()
        .map(|r| {
            let id = (1..=5)
                .find(|id| ctx.request_pda(&user, *id) == *r)
                .expect("id");
            ctx.request_state_data(&user, id).sequence
        })
        .collect();
    ctx.warp_forward_seconds(60);
    let now = ctx.now();
    let admin = ctx.admin.insecure_clone();

    let meta = ctx
        .expedite_requests_as(&admin, &requests, &sequences)
        .expect("batch");
    eprintln!(
        "expedite_requests({}) consumed {} CU",
        requests.len(),
        meta.compute_units_consumed
    );
    let events = events_of::<WithdrawalExpedited>(&meta);
    assert_eq!(events.len(), 5);
    for (e, r) in events.iter().zip(&requests) {
        assert_eq!(e.request, *r);
        assert_eq!(e.new_eligible_at, now);
    }
    for id in 1..=5 {
        assert_eq!(ctx.request_state_data(&user, id).eligible_at, now);
    }
}

/// Decision 16: all or nothing, with the offending request named.
#[test]
fn a_batch_with_one_ineligible_request_changes_nothing() {
    let mut ctx = attached_vault_with_holder();
    let user = ctx.user.pubkey();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let mut requests = open_requests(&mut ctx, 3, shares / 6);
    requests.sort();
    // Make the middle one eligible already.
    let middle_id = (1..=3)
        .find(|id| ctx.request_pda(&user, *id) == requests[1])
        .expect("id");
    ctx.expedite_request(&user, middle_id, middle_id)
        .expect("pre-expedite");
    let bytes: Vec<Vec<u8>> = requests.iter().map(|r| request_bytes(&ctx, r)).collect();
    let sequences: Vec<u64> = requests
        .iter()
        .map(|r| {
            let id = (1..=3)
                .find(|id| ctx.request_pda(&user, *id) == *r)
                .unwrap();
            ctx.request_state_data(&user, id).sequence
        })
        .collect();
    ctx.warp_forward_seconds(60);
    let admin = ctx.admin.insecure_clone();

    let err = ctx
        .expedite_requests_as(&admin, &requests, &sequences)
        .expect_err("one request already eligible");
    assert_queue_err(&err, ErrorCode::RequestAlreadyEligible);
    assert_blamed(&err, &requests[1]);
    for (r, b) in requests.iter().zip(&bytes) {
        assert_eq!(request_bytes(&ctx, r), *b, "untouched: {r}");
    }
}

#[test]
fn malformed_batches_are_refused() {
    let mut ctx = attached_vault_with_holder();
    let user = ctx.user.pubkey();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let mut requests = open_requests(&mut ctx, 2, shares / 4);
    requests.sort();
    let seqs = |ctx: &VaultCtx, rs: &[Pubkey]| -> Vec<u64> {
        rs.iter()
            .map(|r| {
                let id = (1..=2)
                    .find(|id| ctx.request_pda(&user, *id) == *r)
                    .unwrap();
                ctx.request_state_data(&user, id).sequence
            })
            .collect()
    };
    let admin = ctx.admin.insecure_clone();
    let sequences = seqs(&ctx, &requests);

    let err = ctx
        .expedite_requests_as(&admin, &[], &[])
        .expect_err("empty");
    assert_queue_err(&err, ErrorCode::EmptyBatch);
    assert_anchor_framework_err(&err, 6020);

    let err = ctx
        .expedite_requests_as(&admin, &requests, &sequences[..1])
        .expect_err("more accounts than sequences");
    assert_queue_err(&err, ErrorCode::BatchLengthMismatch);
    assert_anchor_framework_err(&err, 6021);

    let reversed: Vec<Pubkey> = requests.iter().rev().copied().collect();
    let err = ctx
        .expedite_requests_as(&admin, &reversed, &seqs(&ctx, &reversed))
        .expect_err("descending");
    assert_queue_err(&err, ErrorCode::RequestsNotSorted);
    assert_anchor_framework_err(&err, 6022);

    let doubled = [requests[0], requests[0]];
    let err = ctx
        .expedite_requests_as(&admin, &doubled, &[sequences[0], sequences[0]])
        .expect_err("duplicated");
    assert_queue_err(&err, ErrorCode::RequestsNotSorted);

    // Not a request at all, and a request the batch may not write.
    let not_a_request = [ctx.withdrawal_queue_pda()];
    let err = ctx
        .expedite_requests_as(&admin, &not_a_request, &[1])
        .expect_err("the queue account in a request slot");
    assert_anchor_framework_err(&err, 3002);
    let mut ix = ctx.expedite_requests_ix(&admin.pubkey(), &requests[..1], &sequences[..1]);
    ix.accounts.last_mut().unwrap().is_writable = false;
    let err = {
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&admin.pubkey()),
            &[&admin],
            ctx.svm.latest_blockhash(),
        );
        ctx.svm.send_transaction(tx).expect_err("read-only request")
    };
    assert_anchor_framework_err(&err, 2000);

    // Another queue's request, genuine in every respect but its queue.
    let other = ctx.new_vault_with_queue();
    let user_ata = ctx.user_deposit_ata;
    let foreign = ctx.install_foreign_request(&other, &user, 9, 1, user_ata);
    let err = ctx
        .expedite_requests_as(&admin, &[foreign], &[1])
        .expect_err("another queue's request");
    assert_anchor_framework_err(&err, 2001);

    let stranger = ctx.new_funded_keypair(1_000_000_000);
    let err = ctx
        .expedite_requests_as(&stranger, &requests, &sequences)
        .expect_err("not admin or operator");
    assert_queue_err(&err, ErrorCode::NotVaultAdminOrOperator);
    assert_eq!(
        ctx.request_state_data(&user, 1).eligible_at,
        ctx.request_state_data(&user, 1).scheduled_eligible_at
    );
}

/// The bound is the program's, not the packet's: one over it is refused
/// before anything else about the batch is looked at, and a full batch goes
/// through under the batch budget.
#[test]
fn a_full_batch_goes_through_and_one_over_the_bound_is_refused() {
    let mut ctx = attached_vault_with_holder();
    let user = ctx.user.pubkey();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let n = MAX_EXPEDITE_BATCH as u64 + 1;
    let mut requests = open_requests(&mut ctx, n, shares / (2 * n));
    requests.sort();
    let sequences = sequences_of(&ctx, n, &requests);
    ctx.warp_forward_seconds(60);
    let now = ctx.now();
    let admin = ctx.admin.insecure_clone();

    let err = ctx
        .expedite_requests_as(&admin, &requests, &sequences)
        .expect_err("one over the bound");
    assert_queue_err(&err, ErrorCode::BatchTooLarge);
    assert_anchor_framework_err(&err, 6023);

    let meta = ctx
        .expedite_requests_as(
            &admin,
            &requests[..MAX_EXPEDITE_BATCH],
            &sequences[..MAX_EXPEDITE_BATCH],
        )
        .expect("a full batch");
    eprintln!(
        "expedite_requests({MAX_EXPEDITE_BATCH}) consumed {} CU",
        meta.compute_units_consumed
    );
    assert_eq!(
        events_of::<WithdrawalExpedited>(&meta).len(),
        MAX_EXPEDITE_BATCH
    );
    let expedited = (1..=n)
        .filter(|id| ctx.request_state_data(&user, *id).eligible_at == now)
        .count();
    assert_eq!(expedited, MAX_EXPEDITE_BATCH, "exactly the batch moved");
}

/// A legacy transaction has room for more than the bound, so the bound is
/// what limits an expedite batch; the room is measured for the SDK.
#[test]
fn the_bound_fits_a_legacy_transaction() {
    let ctx = attached_vault_with_holder();
    let admin = Keypair::new();
    let size_for = |n: usize| {
        let requests: Vec<Pubkey> = (0..n).map(|_| Pubkey::new_unique()).collect();
        let sequences: Vec<u64> = (1..=n as u64).collect();
        let ix = ctx.expedite_requests_ix(&admin.pubkey(), &requests, &sequences);
        let tx = Transaction::new_signed_with_payer(
            &with_batch_budget(ix),
            Some(&admin.pubkey()),
            &[&admin],
            Hash::default(),
        );
        // One byte of signature count, 64 per signature, then the message.
        1 + 64 * tx.signatures.len() + tx.message_data().len()
    };
    let max = (1..=64)
        .take_while(|n| size_for(*n) <= PACKET_DATA_SIZE)
        .last()
        .unwrap();
    eprintln!(
        "expedite_requests: {max} requests fit a legacy transaction with its compute-budget \
         instruction ({} bytes at {max})",
        size_for(max)
    );
    assert_eq!(
        max, MEASURED_LEGACY_ROOM,
        "the recorded room must be the measured one"
    );
    assert!(
        max >= MAX_EXPEDITE_BATCH,
        "the bound must fit a legacy transaction"
    );
}
