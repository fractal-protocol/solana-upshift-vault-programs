// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::errors::ErrorCode;
use anchor_lang::prelude::*;

/// First seed of a request PDA. The rest are the queue, the owner and the
/// owner-chosen `request_id` as little-endian bytes.
pub const WITHDRAWAL_REQUEST_SEED: &[u8] = b"withdrawal_request";

/// One pending withdrawal, at `["withdrawal_request", queue, owner, request_id]`.
/// Rent is paid by the owner and returned to them when the account closes on
/// finalize or cancel. One account per request, so an owner can hold many at once
/// and nothing is ever reallocated.
///
/// Owner-scoped ids remove cross-user contention on a shared counter. Because an
/// id can be reused once its account closes, every instruction that targets an
/// existing request also quotes `sequence`, and a signed-but-delayed instruction
/// for the old request fails against the new one.
///
/// A zero-filled request, which is what Anchor's `init` hands the handler, reads
/// as eligible immediately and never expiring. [`Self::schedule`] is therefore
/// the only way the timestamps are meant to be written.
#[account]
#[derive(Default, InitSpace)]
pub struct WithdrawalRequest {
    /// The queue holding this request's shares.
    pub queue: Pubkey,
    /// Who requested, pays rent, may cancel, and may always finalize.
    pub owner: Pubkey,
    /// Deposit-mint token account paid at finalization. Never an escrow or the
    /// vault reserve, checked at request and again at finalize.
    pub recipient_token_account: Pubkey,
    /// Who may finalize besides the owner. Zero means anyone. Read it through
    /// [`Self::may_finalize`].
    pub finalizer: Pubkey,
    /// Shares held in the queue's escrow for this request.
    pub shares: u64,
    /// The owner's floor on net payout, same semantics as `redeem_checked`.
    /// Fixed at request time.
    pub min_assets_out: u64,
    /// Owner-chosen id, unique per owner while this account exists.
    pub request_id: u64,
    /// Queue-wide ordering stamp at creation, from `WithdrawalQueue::open_request`.
    /// Quoted as `expected_sequence` by every instruction that targets this
    /// request.
    pub sequence: u64,
    /// Unix time the request was created.
    pub requested_at: i64,
    /// `requested_at + cooldown` at creation. Never changes: `expires_at` is
    /// measured from it, so expediting cannot move a deadline.
    pub scheduled_eligible_at: i64,
    /// Unix time finalization may begin. Equals `scheduled_eligible_at` unless
    /// `expedite_request` moved it earlier; it only ever moves earlier.
    pub eligible_at: i64,
    /// `scheduled_eligible_at + fulfillment_window` at creation, or `0` when the
    /// window is disabled. Read it through [`Self::is_expired`]. An expired
    /// request stays pending, and counted, until cancelled.
    pub expires_at: i64,
    /// Bump of this PDA, for `bump = request.bump` constraints. This account
    /// never signs.
    pub bump: u8,
    /// Reserved. Carve new fields **out of** this array so `LEN` stays 265.
    pub padding: [u64; 8],
}

const _: () = assert!(
    WithdrawalRequest::LEN == 265,
    "WithdrawalRequest::LEN must stay 265; carve new fields out of `padding`"
);

impl WithdrawalRequest {
    pub const LEN: usize = 8 + Self::INIT_SPACE;

    /// Fills a freshly created request and stamps its schedule. The one writer of
    /// a request's identity, so a handler cannot forget a field.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        &mut self,
        queue: Pubkey,
        owner: Pubkey,
        recipient_token_account: Pubkey,
        finalizer: Pubkey,
        shares: u64,
        min_assets_out: u64,
        request_id: u64,
        sequence: u64,
        bump: u8,
        now: i64,
        cooldown_seconds: u64,
        window_seconds: u64,
    ) -> Result<()> {
        self.queue = queue;
        self.owner = owner;
        self.recipient_token_account = recipient_token_account;
        self.finalizer = finalizer;
        self.shares = shares;
        self.min_assets_out = min_assets_out;
        self.request_id = request_id;
        self.sequence = sequence;
        self.bump = bump;
        self.padding = [0; 8];
        self.schedule(now, cooldown_seconds, window_seconds)
    }

    /// Writes every timestamp from one `now` and the queue's current settings.
    /// This is the only place the `expires_at` zero sentinel is decided: a
    /// disabled window stores `0`, never `scheduled_eligible_at + 0`.
    pub fn schedule(&mut self, now: i64, cooldown_seconds: u64, window_seconds: u64) -> Result<()> {
        let secs = |s: u64| i64::try_from(s).map_err(|_| error!(ErrorCode::MathError));
        let scheduled = now
            .checked_add(secs(cooldown_seconds)?)
            .ok_or(ErrorCode::MathError)?;
        let expires = match window_seconds {
            0 => 0,
            w => scheduled
                .checked_add(secs(w)?)
                .ok_or(ErrorCode::MathError)?,
        };
        self.requested_at = now;
        self.scheduled_eligible_at = scheduled;
        self.eligible_at = scheduled;
        self.expires_at = expires;
        Ok(())
    }

    /// The finalizer restriction: `None` is no restriction. Named apart from the
    /// field so the raw key cannot be compared by mistake, which would read
    /// "anyone" as "nobody".
    pub fn allowed_finalizer(&self) -> Option<Pubkey> {
        (self.finalizer != Pubkey::default()).then_some(self.finalizer)
    }

    /// Design decision 14. The owner may always finalize; anyone else must be
    /// the key the request names, if it names one. A restriction, never a grant.
    pub fn may_finalize(&self, caller: &Pubkey) -> bool {
        if *caller == self.owner {
            return true;
        }
        match self.allowed_finalizer() {
            None => true,
            Some(finalizer) => finalizer == *caller,
        }
    }

    /// Whether finalization may begin at `now`. Reads the movable `eligible_at`,
    /// which `expedite_request` may have brought forward.
    pub fn is_eligible(&self, now: i64) -> bool {
        now >= self.eligible_at
    }

    /// Whether the fulfillment window has closed at `now`. A zero `expires_at`
    /// never expires; the deadline instant counts as expired.
    pub fn is_expired(&self, now: i64) -> bool {
        self.expires_at != 0 && now >= self.expires_at
    }
}
