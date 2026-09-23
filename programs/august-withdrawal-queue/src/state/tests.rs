// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Layout pins for both accounts, in the vault's style: one table per account
//! covering every byte from 8 to `LEN`, so a field added, resized or reordered
//! without a row fails with its neighbour named. Then the state helpers.

use super::*;
use crate::errors::{ErrorCode, ANCHOR_USER_ERROR_OFFSET};
use anchor_lang::prelude::*;
use anchor_lang::AccountSerialize;

fn pk(b: u8) -> Pubkey {
    Pubkey::new_from_array([b; 32])
}

fn code_of(err: Error) -> u32 {
    match err {
        Error::AnchorError(e) => e.error_code_number,
        other => panic!("expected an AnchorError, got {other:?}"),
    }
}

fn expected(code: ErrorCode) -> u32 {
    code as u32 + ANCHOR_USER_ERROR_OFFSET
}

/// Walks a `(field, first byte, serialized form)` table and asserts each row lands
/// where it claims and that the rows tile the account contiguously up to `len`.
/// The discriminator is pinned as a literal: renaming the struct changes it and
/// strands every live account, while every other test here stays green.
fn assert_layout(
    bytes: &[u8],
    discriminator: [u8; 8],
    layout: &[(&str, usize, Vec<u8>)],
    len: usize,
) {
    assert_eq!(bytes.len(), len, "serialized size must equal LEN");
    assert_eq!(
        &bytes[..8],
        &discriminator,
        "discriminator changed; was the struct renamed?"
    );
    let mut cursor = 8;
    for (name, at, want) in layout {
        assert_eq!(
            *at, cursor,
            "`{name}` is recorded at byte {at} but the fields before it end at \
             {cursor}: a field was added, resized or reordered without updating \
             this table"
        );
        assert_eq!(
            &bytes[*at..*at + want.len()],
            want.as_slice(),
            "`{name}` did not serialize at byte {at}"
        );
        cursor = at + want.len();
    }
    assert_eq!(cursor, len, "the table must cover the account up to LEN");
}

#[test]
fn withdrawal_queue_every_field_stays_at_its_byte_offset() {
    const COOLDOWN: u64 = 0x1111_1111_1111_1111;
    const WINDOW: u64 = 0x2222_2222_2222_2222;
    const SEQ: u64 = 0x3333_3333_3333_3333;
    const PENDING: u64 = 0x4444_4444_4444_4444;
    const SHARES: u64 = 0x5555_5555_5555_5555;
    const RELEASED: i64 = 0x0666_6666_6666_6666;
    const PAD: u64 = 0x9999_9999_9999_9999;

    let queue = WithdrawalQueue {
        vault_state: pk(1),
        deposit_mint: pk(2),
        share_mint: pk(3),
        escrow_shares: pk(4),
        escrow_assets: pk(5),
        cooldown_seconds: COOLDOWN,
        fulfillment_window_seconds: WINDOW,
        sequence: SEQ,
        pending_requests: PENDING,
        pending_shares: SHARES,
        bump: 0x77,
        released_at: RELEASED,
        padding: [PAD; 19],
    };
    let mut bytes = Vec::new();
    queue.try_serialize(&mut bytes).expect("serialize");

    let layout: Vec<(&str, usize, Vec<u8>)> = vec![
        ("vault_state", 8, pk(1).to_bytes().to_vec()),
        ("deposit_mint", 40, pk(2).to_bytes().to_vec()),
        ("share_mint", 72, pk(3).to_bytes().to_vec()),
        ("escrow_shares", 104, pk(4).to_bytes().to_vec()),
        ("escrow_assets", 136, pk(5).to_bytes().to_vec()),
        ("cooldown_seconds", 168, COOLDOWN.to_le_bytes().to_vec()),
        (
            "fulfillment_window_seconds",
            176,
            WINDOW.to_le_bytes().to_vec(),
        ),
        ("sequence", 184, SEQ.to_le_bytes().to_vec()),
        ("pending_requests", 192, PENDING.to_le_bytes().to_vec()),
        ("pending_shares", 200, SHARES.to_le_bytes().to_vec()),
        ("bump", 208, vec![0x77]),
        ("released_at", 209, RELEASED.to_le_bytes().to_vec()),
        (
            "padding",
            217,
            [PAD; 19].iter().flat_map(|w| w.to_le_bytes()).collect(),
        ),
    ];
    // sha256("account:WithdrawalQueue")[..8]
    let disc = [54, 56, 158, 88, 232, 203, 241, 163];
    assert_layout(&bytes, disc, &layout, WithdrawalQueue::LEN);
}

#[test]
fn withdrawal_request_every_field_stays_at_its_byte_offset() {
    const SHARES: u64 = 0x1111_1111_1111_1111;
    const ID: u64 = 0x3333_3333_3333_3333;
    const SEQ: u64 = 0x4444_4444_4444_4444;
    const REQUESTED: i64 = 0x5555_5555_5555_5555;
    const SCHEDULED: i64 = 0x6666_6666_6666_6666;
    const ELIGIBLE: i64 = 0x7777_7777_7777_7777;
    const EXPIRES: i64 = 0x0888_8888_8888_8888;
    const PAD: u64 = 0x9999_9999_9999_9999;

    let request = WithdrawalRequest {
        queue: pk(1),
        owner: pk(2),
        recipient_token_account: pk(3),
        finalizer: pk(4),
        shares: SHARES,
        request_id: ID,
        sequence: SEQ,
        requested_at: REQUESTED,
        scheduled_eligible_at: SCHEDULED,
        eligible_at: ELIGIBLE,
        expires_at: EXPIRES,
        bump: 0xAA,
        padding: [PAD; 9],
    };
    let mut bytes = Vec::new();
    request.try_serialize(&mut bytes).expect("serialize");

    let layout: Vec<(&str, usize, Vec<u8>)> = vec![
        ("queue", 8, pk(1).to_bytes().to_vec()),
        ("owner", 40, pk(2).to_bytes().to_vec()),
        ("recipient_token_account", 72, pk(3).to_bytes().to_vec()),
        ("finalizer", 104, pk(4).to_bytes().to_vec()),
        ("shares", 136, SHARES.to_le_bytes().to_vec()),
        ("request_id", 144, ID.to_le_bytes().to_vec()),
        ("sequence", 152, SEQ.to_le_bytes().to_vec()),
        ("requested_at", 160, REQUESTED.to_le_bytes().to_vec()),
        (
            "scheduled_eligible_at",
            168,
            SCHEDULED.to_le_bytes().to_vec(),
        ),
        ("eligible_at", 176, ELIGIBLE.to_le_bytes().to_vec()),
        ("expires_at", 184, EXPIRES.to_le_bytes().to_vec()),
        ("bump", 192, vec![0xAA]),
        (
            "padding",
            193,
            [PAD; 9].iter().flat_map(|w| w.to_le_bytes()).collect(),
        ),
    ];
    // sha256("account:WithdrawalRequest")[..8]
    let disc = [242, 88, 147, 173, 182, 62, 229, 193];
    assert_layout(&bytes, disc, &layout, WithdrawalRequest::LEN);
}

// ---- queue helpers ----

/// Pins seed order in `signer_seeds()`: the stored bump must re-derive the
/// address `find_program_address` produced. The vault attaches only that
/// address, and `invoke_signed` signs only for the address the seeds actually
/// derive, so a wrong stored bump breaks one or the other.
#[test]
fn signer_seeds_reproduce_the_canonical_queue_pda() {
    let vault_state = Pubkey::new_unique();
    let (expected, bump) =
        Pubkey::find_program_address(&[WITHDRAWAL_QUEUE_SEED, vault_state.as_ref()], &crate::ID);
    let queue = WithdrawalQueue {
        vault_state,
        bump,
        ..Default::default()
    };
    let derived = Pubkey::create_program_address(&queue.signer_seeds(), &crate::ID)
        .expect("stored bump must be valid");
    assert_eq!(derived, expected);
}

#[test]
fn init_starts_with_empty_counters() {
    let mut queue = WithdrawalQueue {
        // Dirty every field init must reset, to prove it does.
        fulfillment_window_seconds: 5,
        sequence: 5,
        pending_requests: 5,
        pending_shares: 5,
        released_at: 5,
        padding: [7; 19],
        ..Default::default()
    };
    queue
        .init(pk(1), pk(2), pk(3), pk(4), pk(5), 0x42, 3_600)
        .expect("init");

    assert_eq!(queue.vault_state, pk(1));
    assert_eq!(queue.deposit_mint, pk(2));
    assert_eq!(queue.share_mint, pk(3));
    assert_eq!(queue.escrow_shares, pk(4));
    assert_eq!(queue.escrow_assets, pk(5));
    assert_eq!(queue.bump, 0x42);
    assert_eq!(queue.cooldown_seconds, 3_600);
    assert_eq!(queue.fulfillment_window_seconds, 0);
    assert_eq!(queue.sequence, 0);
    assert_eq!(queue.pending_requests, 0);
    assert_eq!(queue.pending_shares, 0);
    assert_eq!(queue.released_at, 0);
    assert_eq!(queue.padding, [0; 19]);

    let err = WithdrawalQueue::default()
        .init(
            pk(1),
            pk(2),
            pk(3),
            pk(4),
            pk(5),
            0,
            MAX_COOLDOWN_SECONDS + 1,
        )
        .expect_err("init validates the cooldown");
    assert_eq!(code_of(err), expected(ErrorCode::CooldownOutOfBounds));
}

#[test]
fn cooldown_and_window_are_bounded_inclusively() {
    let mut queue = WithdrawalQueue::default();
    queue
        .set_cooldown(0)
        .expect("zero cooldown is instant eligibility");
    queue
        .set_cooldown(MAX_COOLDOWN_SECONDS)
        .expect("the maximum itself is allowed");
    assert_eq!(queue.cooldown_seconds, MAX_COOLDOWN_SECONDS);
    let err = queue
        .set_cooldown(MAX_COOLDOWN_SECONDS + 1)
        .expect_err("one over is refused");
    assert_eq!(code_of(err), expected(ErrorCode::CooldownOutOfBounds));
    assert_eq!(
        queue.cooldown_seconds, MAX_COOLDOWN_SECONDS,
        "a refused value leaves the old one"
    );

    queue
        .set_fulfillment_window(0)
        .expect("zero disables expiry");
    queue
        .set_fulfillment_window(MAX_FULFILLMENT_WINDOW_SECONDS)
        .expect("the maximum itself is allowed");
    let err = queue
        .set_fulfillment_window(MAX_FULFILLMENT_WINDOW_SECONDS + 1)
        .expect_err("one over is refused");
    assert_eq!(
        code_of(err),
        expected(ErrorCode::FulfillmentWindowOutOfBounds)
    );
    assert_eq!(
        queue.fulfillment_window_seconds,
        MAX_FULFILLMENT_WINDOW_SECONDS
    );

    queue
        .set_fulfillment_window(MIN_FULFILLMENT_WINDOW_SECONDS)
        .expect("the minimum itself is allowed");
    for seconds in [1, 7, MIN_FULFILLMENT_WINDOW_SECONDS - 1] {
        let err = queue
            .set_fulfillment_window(seconds)
            .expect_err("a non-zero window under a day is refused");
        assert_eq!(
            code_of(err),
            expected(ErrorCode::FulfillmentWindowOutOfBounds)
        );
    }
    assert_eq!(
        queue.fulfillment_window_seconds,
        MIN_FULFILLMENT_WINDOW_SECONDS
    );
}

#[test]
fn admin_cancel_opens_a_full_delay_after_the_latest_release() {
    let mut queue = WithdrawalQueue::default();
    assert!(
        !queue.admin_cancel_delay_elapsed(i64::MAX).expect("never"),
        "a queue never released has not waited"
    );

    queue.released_at = 1_000;
    let opens_at = 1_000 + ADMIN_CANCEL_DELAY_SECONDS;
    assert!(!queue
        .admin_cancel_delay_elapsed(opens_at - 1)
        .expect("early"));
    assert!(queue.admin_cancel_delay_elapsed(opens_at).expect("on time"));

    queue.released_at = i64::MAX;
    let err = queue
        .admin_cancel_delay_elapsed(i64::MAX)
        .expect_err("overflow is refused, not wrapped");
    assert_eq!(code_of(err), expected(ErrorCode::MathError));
}

#[test]
fn counters_move_together_and_refuse_to_wrap() {
    let mut queue = WithdrawalQueue::default();
    assert_eq!(
        queue.open_request(100).expect("first"),
        1,
        "stamps start at 1"
    );
    assert_eq!(queue.open_request(50).expect("second"), 2);
    assert_eq!(queue.pending_requests, 2);
    assert_eq!(queue.pending_shares, 150);

    queue.close_request(100).expect("close the first");
    assert_eq!(queue.pending_requests, 1);
    assert_eq!(queue.pending_shares, 50);
    assert_eq!(queue.sequence, 2, "closing never rewinds the stamp");

    // Closing more shares than are pending is a request that was never counted.
    // The refusal leaves both counters as they were, so the real close still works.
    let err = queue.close_request(51).expect_err("underflow");
    assert_eq!(code_of(err), expected(ErrorCode::MathError));
    assert_eq!((queue.pending_requests, queue.pending_shares), (1, 50));
    queue.close_request(50).expect("close the second");
    let err = queue.close_request(0).expect_err("nothing left to close");
    assert_eq!(code_of(err), expected(ErrorCode::MathError));

    let mut full = WithdrawalQueue {
        pending_shares: u64::MAX,
        ..Default::default()
    };
    let err = full.open_request(1).expect_err("overflow");
    assert_eq!(code_of(err), expected(ErrorCode::MathError));
}

// ---- request helpers ----

#[test]
fn open_writes_every_field_and_the_schedule() {
    let mut request = WithdrawalRequest {
        padding: [7; 9],
        ..Default::default()
    };
    request
        .open(pk(1), pk(2), pk(3), pk(4), 500, 9, 3, 0x55, 1_000, 600, 60)
        .expect("open");
    assert_eq!((request.queue, request.owner), (pk(1), pk(2)));
    assert_eq!(
        (request.recipient_token_account, request.finalizer),
        (pk(3), pk(4))
    );
    assert_eq!(request.shares, 500);
    assert_eq!(
        (request.request_id, request.sequence, request.bump),
        (9, 3, 0x55)
    );
    assert_eq!(
        (request.requested_at, request.scheduled_eligible_at),
        (1_000, 1_600)
    );
    assert_eq!((request.eligible_at, request.expires_at), (1_600, 1_660));
    assert_eq!(request.padding, [0; 9]);
}

#[test]
fn schedule_derives_every_timestamp_from_now() {
    let mut request = WithdrawalRequest::default();
    request.schedule(1_000, 600, 0).expect("no window");
    assert_eq!(request.requested_at, 1_000);
    assert_eq!(request.scheduled_eligible_at, 1_600);
    assert_eq!(
        request.eligible_at, 1_600,
        "eligible_at starts at the schedule"
    );
    assert_eq!(
        request.expires_at, 0,
        "a disabled window stores the sentinel, not scheduled + 0"
    );

    request.schedule(1_000, 600, 1).expect("one-second window");
    assert_eq!(request.expires_at, 1_601);

    let err = WithdrawalRequest::default()
        .schedule(i64::MAX, 1, 0)
        .expect_err("overflow");
    assert_eq!(code_of(err), expected(ErrorCode::MathError));
}

/// Anchor's `init` hands the handler a zero-filled account, and zero reads as
/// eligible now and never expiring. This pins why `schedule` must always run.
#[test]
fn a_zero_filled_request_is_the_most_permissive_state() {
    let request = WithdrawalRequest::default();
    assert!(request.is_eligible(0));
    assert!(!request.is_expired(i64::MAX));
}

#[test]
fn eligibility_and_expiry_boundaries() {
    let request = WithdrawalRequest {
        scheduled_eligible_at: 1_500,
        eligible_at: 1_000,
        expires_at: 2_000,
        ..Default::default()
    };
    assert!(!request.is_eligible(999));
    assert!(request.is_eligible(1_000), "eligible at the instant");
    assert!(request.is_eligible(1_001));

    assert!(!request.is_expired(1_999));
    assert!(request.is_expired(2_000), "now == expires_at is expired");
    assert!(request.is_expired(2_001));

    let never = WithdrawalRequest {
        expires_at: 0,
        ..Default::default()
    };
    assert!(!never.is_expired(i64::MAX), "a zero deadline never expires");
}

#[test]
fn a_zero_finalizer_reads_as_no_restriction() {
    assert_eq!(WithdrawalRequest::default().allowed_finalizer(), None);
    assert_eq!(
        WithdrawalRequest {
            finalizer: pk(8),
            ..Default::default()
        }
        .allowed_finalizer(),
        Some(pk(8))
    );
}

/// The two rows of the design doc's finalization-permission table, each checked
/// for the owner, the named key `F` and a stranger. The restriction narrows who
/// may finalize; it never removes the owner.
#[test]
fn may_finalize_follows_the_permission_table() {
    let owner = pk(1);
    let f = pk(2);
    let stranger = pk(3);

    // (request.finalizer, [F allowed, stranger allowed])
    let rows = [
        (Pubkey::default(), [true, true]), // anyone
        (f, [true, false]),                // only F, and the owner
    ];
    for (i, (finalizer, [f_ok, stranger_ok])) in rows.into_iter().enumerate() {
        let request = WithdrawalRequest {
            owner,
            finalizer,
            ..Default::default()
        };
        let row = i + 1;
        assert!(
            request.may_finalize(&owner),
            "row {row}: the owner always may"
        );
        assert_eq!(request.may_finalize(&f), f_ok, "row {row}: F");
        assert_eq!(
            request.may_finalize(&stranger),
            stranger_ok,
            "row {row}: stranger"
        );
    }
}
