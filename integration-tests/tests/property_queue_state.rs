//! Property-based tests for the withdrawal queue's state methods: the bounded
//! setters and paired counters on `WithdrawalQueue`, and the schedule and
//! permission helpers on `WithdrawalRequest`.
//!
//! The unit suite in `programs/august-withdrawal-queue/src/state/tests.rs` pins
//! these at hand-picked values; this suite samples the input space, weighted
//! towards the boundaries where a checked operation flips from `Ok` to `Err`,
//! and holds every result against an independent model computed in `u128` /
//! `i128`.
//!
//! Every refusal must leave the account exactly as it was. The instructions
//! would roll back a refused call anyway, but these methods are written to
//! compute first and write last, and a later edit that writes one counter
//! before checking its partner would pass every instruction-level test.

use anchor_lang::prelude::*;
use august_withdrawal_queue::errors::{ErrorCode, ANCHOR_USER_ERROR_OFFSET};
use august_withdrawal_queue::state::{
    WithdrawalQueue, WithdrawalRequest, MAX_COOLDOWN_SECONDS, MAX_FULFILLMENT_WINDOW_SECONDS,
};
use proptest::prelude::*;

fn code_of(err: Error) -> u32 {
    match err {
        Error::AnchorError(e) => e.error_code_number,
        other => panic!("expected an AnchorError, got {other:?}"),
    }
}

fn expected(code: ErrorCode) -> u32 {
    code as u32 + ANCHOR_USER_ERROR_OFFSET
}

/// A `u64` drawn a third of the time from each end of the range and a third
/// from anywhere, so the overflow edges are hit rather than stumbled on.
fn edgy_u64() -> impl Strategy<Value = u64> {
    prop_oneof![
        0u64..1_000,
        (0u64..1_000).prop_map(|d| u64::MAX - d),
        any::<u64>(),
    ]
}

/// A duration around `bound`: at it, just either side of it, or anywhere.
fn around(bound: u64) -> impl Strategy<Value = u64> {
    prop_oneof![
        (0u64..=2).prop_map(move |d| bound - 1 + d),
        0..=bound,
        any::<u64>(),
    ]
}

/// A timestamp near either end of `i64`, near zero, or anywhere.
fn edgy_i64() -> impl Strategy<Value = i64> {
    prop_oneof![
        (0i64..1_000).prop_map(|d| i64::MAX - d),
        (0i64..1_000).prop_map(|d| i64::MIN + d),
        -1_000i64..1_000,
        any::<i64>(),
    ]
}

/// A key drawn from four, so owner, finalizer and caller often coincide.
fn small_key() -> impl Strategy<Value = Pubkey> {
    (0u8..4).prop_map(|b| Pubkey::new_from_array([b; 32]))
}

fn queue_with(sequence: u64, pending_requests: u64, pending_shares: u64) -> WithdrawalQueue {
    WithdrawalQueue {
        sequence,
        pending_requests,
        pending_shares,
        cooldown_seconds: 7,
        fulfillment_window_seconds: 11,
        ..Default::default()
    }
}

/// The fields a counter method may touch, for comparing whole states.
fn counters(queue: &WithdrawalQueue) -> (u64, u64, u64, u64, u64) {
    (
        queue.sequence,
        queue.pending_requests,
        queue.pending_shares,
        queue.cooldown_seconds,
        queue.fulfillment_window_seconds,
    )
}

fn timestamps(request: &WithdrawalRequest) -> (i64, i64, i64, i64) {
    (
        request.requested_at,
        request.scheduled_eligible_at,
        request.eligible_at,
        request.expires_at,
    )
}

#[derive(Debug, Clone)]
enum CounterOp {
    Open(u64),
    Close(u64),
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    /// The cooldown is accepted exactly up to its bound, inclusive, and a
    /// refusal leaves every field as it was.
    #[test]
    fn set_cooldown_accepts_exactly_the_bounded_range(seconds in around(MAX_COOLDOWN_SECONDS)) {
        let mut queue = queue_with(3, 2, 100);
        let before = counters(&queue);
        match queue.set_cooldown(seconds) {
            Ok(()) => {
                prop_assert!(seconds <= MAX_COOLDOWN_SECONDS);
                prop_assert_eq!(queue.cooldown_seconds, seconds);
                prop_assert_eq!(queue.fulfillment_window_seconds, before.4);
            }
            Err(e) => {
                prop_assert!(seconds > MAX_COOLDOWN_SECONDS);
                prop_assert_eq!(code_of(e), expected(ErrorCode::CooldownOutOfBounds));
                prop_assert_eq!(counters(&queue), before);
            }
        }
    }

    /// Same for the fulfillment window, where zero (no window) is in range.
    #[test]
    fn set_fulfillment_window_accepts_exactly_the_bounded_range(
        seconds in around(MAX_FULFILLMENT_WINDOW_SECONDS)
    ) {
        let mut queue = queue_with(3, 2, 100);
        let before = counters(&queue);
        match queue.set_fulfillment_window(seconds) {
            Ok(()) => {
                prop_assert!(seconds <= MAX_FULFILLMENT_WINDOW_SECONDS);
                prop_assert_eq!(queue.fulfillment_window_seconds, seconds);
                prop_assert_eq!(queue.cooldown_seconds, before.3);
            }
            Err(e) => {
                prop_assert!(seconds > MAX_FULFILLMENT_WINDOW_SECONDS);
                prop_assert_eq!(code_of(e), expected(ErrorCode::FulfillmentWindowOutOfBounds));
                prop_assert_eq!(counters(&queue), before);
            }
        }
    }

    /// Any sequence of opens and closes, from any starting counters, matches a
    /// wide-integer model: an open succeeds exactly when none of its three
    /// counters would pass `u64::MAX` and stamps the next sequence; a close
    /// succeeds exactly when neither of its two would pass zero and never
    /// moves the sequence; a refusal moves nothing.
    #[test]
    fn counters_track_a_wide_model(
        start in (edgy_u64(), edgy_u64(), edgy_u64()),
        ops in proptest::collection::vec(
            prop_oneof![
                edgy_u64().prop_map(CounterOp::Open),
                edgy_u64().prop_map(CounterOp::Close),
            ],
            1..40,
        ),
    ) {
        let mut queue = queue_with(start.0, start.1, start.2);
        let mut model = (start.0 as u128, start.1 as u128, start.2 as u128);
        let max = u64::MAX as u128;
        for op in &ops {
            let before = counters(&queue);
            match *op {
                CounterOp::Open(shares) => {
                    let next = (model.0 + 1, model.1 + 1, model.2 + shares as u128);
                    let fits = next.0 <= max && next.1 <= max && next.2 <= max;
                    match queue.open_request(shares) {
                        Ok(stamp) => {
                            prop_assert!(fits, "open of {} past u64::MAX from {:?}", shares, before);
                            model = next;
                            prop_assert_eq!(stamp as u128, model.0, "open did not stamp the next sequence");
                        }
                        Err(e) => {
                            prop_assert!(!fits, "open of {} refused from {:?}", shares, before);
                            prop_assert_eq!(code_of(e), expected(ErrorCode::MathError));
                            prop_assert_eq!(counters(&queue), before, "refused open moved a counter");
                        }
                    }
                }
                CounterOp::Close(shares) => {
                    let fits = model.1 >= 1 && model.2 >= shares as u128;
                    match queue.close_request(shares) {
                        Ok(()) => {
                            prop_assert!(fits, "close of {} past zero from {:?}", shares, before);
                            model = (model.0, model.1 - 1, model.2 - shares as u128);
                        }
                        Err(e) => {
                            prop_assert!(!fits, "close of {} refused from {:?}", shares, before);
                            prop_assert_eq!(code_of(e), expected(ErrorCode::MathError));
                            prop_assert_eq!(counters(&queue), before, "refused close moved a counter");
                        }
                    }
                }
            }
            prop_assert_eq!(
                (queue.sequence as u128, queue.pending_requests as u128, queue.pending_shares as u128),
                model
            );
            prop_assert_eq!(queue.cooldown_seconds, before.3, "a counter op moved the cooldown");
            prop_assert_eq!(queue.fulfillment_window_seconds, before.4, "a counter op moved the window");
        }
    }

    /// `schedule` succeeds exactly when both instants fit an `i64`, writes them
    /// from one `now`, stores the zero sentinel exactly when the window is off,
    /// and on refusal leaves every timestamp as it was.
    #[test]
    fn schedule_matches_a_wide_model(
        now in edgy_i64(),
        cooldown in prop_oneof![around(MAX_COOLDOWN_SECONDS), edgy_u64()],
        window in prop_oneof![Just(0u64), around(MAX_FULFILLMENT_WINDOW_SECONDS), edgy_u64()],
    ) {
        let mut request = WithdrawalRequest {
            requested_at: 1,
            scheduled_eligible_at: 2,
            eligible_at: 3,
            expires_at: 4,
            ..Default::default()
        };
        let before = timestamps(&request);
        let range = i64::MIN as i128..=i64::MAX as i128;
        let scheduled = now as i128 + cooldown as i128;
        let expires = if window == 0 { 0 } else { scheduled + window as i128 };
        let fits = cooldown <= i64::MAX as u64
            && window <= i64::MAX as u64
            && range.contains(&scheduled)
            && range.contains(&expires);

        match request.schedule(now, cooldown, window) {
            Ok(()) => {
                prop_assert!(fits, "scheduled past i64 from now={} cooldown={} window={}", now, cooldown, window);
                prop_assert_eq!(
                    timestamps(&request),
                    (now, scheduled as i64, scheduled as i64, expires as i64)
                );
            }
            Err(e) => {
                prop_assert!(!fits, "refused now={} cooldown={} window={}", now, cooldown, window);
                prop_assert_eq!(code_of(e), expected(ErrorCode::MathError));
                prop_assert_eq!(timestamps(&request), before, "refused schedule wrote a timestamp");
            }
        }
    }

    /// Once scheduled, eligibility opens at `scheduled_eligible_at` and the
    /// window closes at `expires_at`, both instants inclusive of the change;
    /// a request with a window is always finalizable at the moment it becomes
    /// eligible; a request without one never expires.
    #[test]
    fn eligibility_and_expiry_follow_the_schedule(
        now in -1_000_000i64..1_000_000,
        cooldown in 0..=MAX_COOLDOWN_SECONDS,
        window in prop_oneof![Just(0u64), 1..=MAX_FULFILLMENT_WINDOW_SECONDS],
        probe in -3_000_000i64..20_000_000,
    ) {
        let mut request = WithdrawalRequest::default();
        request.schedule(now, cooldown, window).expect("in range");
        let eligible_from = now + cooldown as i64;
        prop_assert_eq!(request.is_eligible(probe), probe >= eligible_from);
        if window == 0 {
            prop_assert!(!request.is_expired(probe), "a request without a window expired");
        } else {
            let expires_at = eligible_from + window as i64;
            prop_assert_eq!(request.is_expired(probe), probe >= expires_at);
            prop_assert!(request.is_eligible(eligible_from) && !request.is_expired(eligible_from));
        }
    }

    /// The owner may always finalize; anyone else only if the request names no
    /// finalizer or names them.
    #[test]
    fn may_finalize_follows_the_permission_table(
        owner in small_key(),
        finalizer in prop_oneof![Just(Pubkey::default()), small_key()],
        caller in prop_oneof![Just(Pubkey::default()), small_key()],
    ) {
        let request = WithdrawalRequest { owner, finalizer, ..Default::default() };
        let allowed = caller == owner || finalizer == Pubkey::default() || caller == finalizer;
        prop_assert_eq!(request.may_finalize(&caller), allowed);
        prop_assert_eq!(
            request.allowed_finalizer(),
            if finalizer == Pubkey::default() { None } else { Some(finalizer) }
        );
    }
}
