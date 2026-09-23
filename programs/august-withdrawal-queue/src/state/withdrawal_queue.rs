// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::errors::ErrorCode;
use anchor_lang::prelude::*;

/// First seed of the per-vault queue PDA; the second is the vault state address.
/// The vault derives the same address to validate the queue passed to
/// `attach_withdrawal_queue`, so this seed is part of the cross-program contract,
/// not just this crate's.
pub const WITHDRAWAL_QUEUE_SEED: &[u8] = b"withdrawal_queue";

/// Upper bound on `cooldown_seconds`: 30 days.
pub const MAX_COOLDOWN_SECONDS: u64 = 30 * 24 * 60 * 60;

/// Lower bound on a non-zero `fulfillment_window_seconds`: one day. A window
/// is set in seconds, so this refuses `7` meant as a week, which would make
/// every new request expire seconds after it matures.
pub const MIN_FULFILLMENT_WINDOW_SECONDS: u64 = 24 * 60 * 60;

/// Upper bound on `fulfillment_window_seconds`: 90 days.
pub const MAX_FULFILLMENT_WINDOW_SECONDS: u64 = 90 * 24 * 60 * 60;

/// How long a vault must stay released before `admin_cancel_withdrawal` may
/// touch a request: one day. Without it the admin could release, cancel and
/// re-attach in one transaction, and no holder would ever see the instant exit
/// the release is meant to offer.
pub const ADMIN_CANCEL_DELAY_SECONDS: i64 = 24 * 60 * 60;

// The design doc states these bounds as literals; pin the arithmetic to them.
const _: () = assert!(MAX_COOLDOWN_SECONDS == 2_592_000);
const _: () = assert!(MIN_FULFILLMENT_WINDOW_SECONDS == 86_400);
const _: () = assert!(MAX_FULFILLMENT_WINDOW_SECONDS == 7_776_000);
const _: () = assert!(ADMIN_CANCEL_DELAY_SECONDS == 86_400);

/// One queue per vault, at `["withdrawal_queue", vault_state]`. It is the escrow
/// authority for pending shares and the key that signs the vault's
/// `redeem_checked` by CPI at finalization, so its address is the value the vault
/// stores in `withdrawal_queue_authority`. Stores no admin key; see `crate::auth`.
///
/// The counters and bounds move only through the methods below, so the
/// instructions cannot update one counter and forget its partner.
#[account]
#[derive(Default, InitSpace)]
pub struct WithdrawalQueue {
    /// The vault this queue serves. Every instruction binds the passed vault
    /// account to this key.
    pub vault_state: Pubkey,
    /// Copied from the vault at init; binds the CPI's mint accounts.
    pub deposit_mint: Pubkey,
    /// Copied from the vault at init; binds the CPI's mint accounts.
    pub share_mint: Pubkey,
    /// ATA of this PDA for `share_mint`. Holds every pending request's shares.
    pub escrow_shares: Pubkey,
    /// ATA of this PDA for `deposit_mint`. Transit only: the payout is the balance
    /// delta across the CPI, forwarded to the recipient in the same instruction.
    pub escrow_assets: Pubkey,
    /// Wait between request and eligibility, `0 ..= MAX_COOLDOWN_SECONDS`. Stamped
    /// per request at creation, so a change applies to new requests only.
    pub cooldown_seconds: u64,
    /// How long after scheduled eligibility a request may still be finalized,
    /// `0` (never expires) or `MIN_FULFILLMENT_WINDOW_SECONDS ..=
    /// MAX_FULFILLMENT_WINDOW_SECONDS`. Stamped per request at creation.
    pub fulfillment_window_seconds: u64,
    /// Queue-wide counter, incremented before it is stamped on a request, so the
    /// first stamp is 1 and a zero stamp marks a request that was never opened.
    /// Not a seed: request identity is owner-scoped. It orders events and pins
    /// stale instructions (`expected_sequence`).
    pub sequence: u64,
    /// Live requests. Nothing gates on it: once the vault is released no new
    /// request can open, so the set closes by itself.
    pub pending_requests: u64,
    /// Sum of `shares` over live requests: escrowed exposure, not a demand
    /// forecast, since a mature request is a standing exit authorization.
    pub pending_shares: u64,
    /// Canonical bump of this PDA. The vault accepts the canonical address only,
    /// so this must come from Anchor's `bump` at init, never a caller.
    pub bump: u8,
    /// Unix time of the latest `release_vault`, `0` if never released. Starts
    /// the `ADMIN_CANCEL_DELAY_SECONDS` clock; a re-attach leaves it, since
    /// admin cancel also requires the vault to be released.
    pub released_at: i64,
    /// Reserved. Carve new fields **out of** this array so `LEN` stays 369. A
    /// field carved later reads zero on every queue that already exists, so zero
    /// must mean "legacy behaviour" for it, as it does for every field above.
    pub padding: [u64; 19],
}

const _: () = assert!(
    WithdrawalQueue::LEN == 369,
    "WithdrawalQueue::LEN must stay 369; carve new fields out of `padding`"
);

impl WithdrawalQueue {
    pub const LEN: usize = 8 + Self::INIT_SPACE;

    /// Fills a freshly created account. Everything not passed starts at its
    /// legacy-zero meaning: no expiry window, empty counters.
    #[allow(clippy::too_many_arguments)]
    pub fn init(
        &mut self,
        vault_state: Pubkey,
        deposit_mint: Pubkey,
        share_mint: Pubkey,
        escrow_shares: Pubkey,
        escrow_assets: Pubkey,
        bump: u8,
        cooldown_seconds: u64,
    ) -> Result<()> {
        self.vault_state = vault_state;
        self.deposit_mint = deposit_mint;
        self.share_mint = share_mint;
        self.escrow_shares = escrow_shares;
        self.escrow_assets = escrow_assets;
        self.fulfillment_window_seconds = 0;
        self.sequence = 0;
        self.pending_requests = 0;
        self.pending_shares = 0;
        self.bump = bump;
        self.released_at = 0;
        self.padding = [0; 19];
        self.set_cooldown(cooldown_seconds)
    }

    /// Sets the cooldown, refusing anything over [`MAX_COOLDOWN_SECONDS`].
    pub fn set_cooldown(&mut self, seconds: u64) -> Result<()> {
        require!(
            seconds <= MAX_COOLDOWN_SECONDS,
            ErrorCode::CooldownOutOfBounds
        );
        self.cooldown_seconds = seconds;
        Ok(())
    }

    /// Sets the fulfillment window. Zero disables expiry; anything else must lie
    /// in [`MIN_FULFILLMENT_WINDOW_SECONDS`] ..= [`MAX_FULFILLMENT_WINDOW_SECONDS`].
    pub fn set_fulfillment_window(&mut self, seconds: u64) -> Result<()> {
        require!(
            seconds <= MAX_FULFILLMENT_WINDOW_SECONDS,
            ErrorCode::FulfillmentWindowOutOfBounds
        );
        if seconds != 0 {
            require!(
                seconds >= MIN_FULFILLMENT_WINDOW_SECONDS,
                ErrorCode::FulfillmentWindowOutOfBounds
            );
        }
        self.fulfillment_window_seconds = seconds;
        Ok(())
    }

    /// Counts a new request in and returns the sequence stamp for it. The three
    /// counters move together or not at all: every new value is computed before
    /// any is written.
    pub fn open_request(&mut self, shares: u64) -> Result<u64> {
        let sequence = self.sequence.checked_add(1).ok_or(ErrorCode::MathError)?;
        let pending_requests = self
            .pending_requests
            .checked_add(1)
            .ok_or(ErrorCode::MathError)?;
        let pending_shares = self
            .pending_shares
            .checked_add(shares)
            .ok_or(ErrorCode::MathError)?;
        self.sequence = sequence;
        self.pending_requests = pending_requests;
        self.pending_shares = pending_shares;
        Ok(sequence)
    }

    /// Counts a finalized or cancelled request out. Underflow means a request was
    /// closed twice or never opened, and is refused rather than wrapped, with
    /// neither counter touched.
    pub fn close_request(&mut self, shares: u64) -> Result<()> {
        let pending_requests = self
            .pending_requests
            .checked_sub(1)
            .ok_or(ErrorCode::MathError)?;
        let pending_shares = self
            .pending_shares
            .checked_sub(shares)
            .ok_or(ErrorCode::MathError)?;
        self.pending_requests = pending_requests;
        self.pending_shares = pending_shares;
        Ok(())
    }

    /// Whether `ADMIN_CANCEL_DELAY_SECONDS` have passed since the latest release.
    /// A queue never released has not waited at all.
    pub fn admin_cancel_delay_elapsed(&self, now: i64) -> Result<bool> {
        if self.released_at == 0 {
            return Ok(false);
        }
        let opens_at = self
            .released_at
            .checked_add(ADMIN_CANCEL_DELAY_SECONDS)
            .ok_or(ErrorCode::MathError)?;
        Ok(now >= opens_at)
    }

    /// Seeds this PDA signs with, for `invoke_signed` into the vault and the token
    /// program. Includes the stored bump; see `bump` for why it must be canonical.
    pub fn signer_seeds(&self) -> [&[u8]; 3] {
        [
            WITHDRAWAL_QUEUE_SEED,
            self.vault_state.as_ref(),
            core::slice::from_ref(&self.bump),
        ]
    }
}
