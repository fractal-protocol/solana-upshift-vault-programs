// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Delayed redemption for `august_vault`.
//!
//! The vault half is live: a vault whose `withdrawal_queue_authority` is set
//! accepts redemptions from that key alone, and one left unset redeems instantly
//! as before. Holders request a withdrawal here and wait out a cooldown; then
//! anyone the request permits finalizes it, and this program redeems the
//! escrowed shares by CPI into the vault's `redeem_checked`, signing as the
//! per-vault PDA the vault was pointed at.
//!
//! This crate carries the program identity, the error ABI pin, the build wiring,
//! the two state accounts (`state`), the admin check (`auth`), the queue's
//! admin instructions, the owner's request and cancel instructions,
//! finalization, and `release_vault`, which returns the vault to instant
//! redemption and is the one place the co-signature the vault's detach demands
//! is produced. With it, attaching a queue is reversible.

pub mod auth;
pub mod batch;
pub mod errors;
pub mod events;
pub mod instructions;
pub mod mint_policy;
pub mod recipient;
pub mod state;

use instructions::cancel_withdrawal::*;
use instructions::expedite_request::*;
use instructions::finalize_withdrawal::*;
use instructions::initialize_queue::*;
use instructions::queue_admin::*;
use instructions::release_vault::*;
use instructions::request_withdrawal::*;

use anchor_lang::prelude::*;

declare_id!("NmJ9CGaiPJSfAGSdhNeVDkMPGi7ZQMABYYwWUC4GHyf");

// On-chain security contact + source provenance, queryable from the deployed
// program. Gated out of CPI/library builds via `no-entrypoint`, so it ships only
// in the deployable program — the vault does the same.
#[cfg(not(feature = "no-entrypoint"))]
use solana_security_txt::security_txt;

#[cfg(not(feature = "no-entrypoint"))]
security_txt! {
    name: "Upshift Withdrawal Queue (august_withdrawal_queue)",
    project_url: "https://github.com/fractal-protocol/solana-upshift-vault-programs",
    contacts: "email:alex@augustdigital.io,link:https://github.com/fractal-protocol/solana-upshift-vault-programs/security/advisories/new",
    policy: "https://github.com/fractal-protocol/solana-upshift-vault-programs/security/policy",
    source_code: "https://github.com/fractal-protocol/solana-upshift-vault-programs",
    preferred_languages: "en"
}

#[program]
pub mod august_withdrawal_queue {
    use super::*;

    /// Admin creates the vault's withdrawal queue: the queue PDA and its two
    /// escrow token accounts. The deposit mint must be classic
    /// SPL, or Token-2022 carrying at most the two metadata extensions.
    /// Attaching the queue to the vault is a separate, vault-side step, and is
    /// what makes it live.
    ///
    /// ### Parameters
    /// - `cooldown_seconds` - Wait between request and eligibility, at most 30 days
    pub fn initialize_queue(ctx: Context<InitializeQueue>, cooldown_seconds: u64) -> Result<()> {
        return instructions::initialize_queue::handler(ctx, cooldown_seconds);
    }

    /// Admin sets the cooldown for new requests, at most 30 days.
    pub fn set_cooldown(ctx: Context<QueueAdmin>, seconds: u64) -> Result<()> {
        return instructions::set_cooldown::handler(ctx, seconds);
    }

    /// Admin sets how long after scheduled eligibility a new request may still
    /// be finalized: zero disables expiry, otherwise at most 90 days.
    pub fn set_fulfillment_window(ctx: Context<QueueAdmin>, seconds: u64) -> Result<()> {
        return instructions::set_fulfillment_window::handler(ctx, seconds);
    }

    /// A holder escrows `shares` and opens a request that becomes finalizable
    /// after the queue's cooldown, paying `recipient_token_account` the shares'
    /// value at the moment of finalization. Requires the vault's gate to point
    /// at this queue.
    ///
    /// ### Parameters
    /// - `request_id` - Owner-chosen id, unique per owner while the request exists
    /// - `shares` - Shares to escrow, nonzero
    /// - `finalizer` - Who may finalize besides the owner; zero for anyone
    pub fn request_withdrawal(
        ctx: Context<RequestWithdrawal>,
        request_id: u64,
        shares: u64,
        finalizer: Pubkey,
    ) -> Result<()> {
        return instructions::request_withdrawal::handler(ctx, request_id, shares, finalizer);
    }

    /// Pays out a request whose cooldown has run and whose window is open: the
    /// queue redeems the escrowed shares by CPI into the vault as its PDA,
    /// forwards the net payout to the request's recipient, and closes the
    /// request with its rent to the owner. Anyone the request's `finalizer`
    /// permits may call it, the owner always.
    /// The vault's `VaultPaused` and `NotEnoughLiquidity` propagate unchanged
    /// and leave the request pending.
    ///
    /// ### Parameters
    /// - `expected_sequence` - The request's stamp, so a delayed call cannot land
    ///   on a recreated request with the same id
    pub fn finalize_withdrawal(
        ctx: Context<FinalizeWithdrawal>,
        expected_sequence: u64,
    ) -> Result<()> {
        return instructions::finalize_withdrawal::handler(ctx, expected_sequence);
    }

    /// The owner takes a pending request back: its shares return to a share
    /// account the owner controls and the request closes with its rent to the
    /// owner. Allowed at any time while the request exists, in either vault
    /// state, and while the vault is paused.
    ///
    /// ### Parameters
    /// - `expected_sequence` - The request's stamp, so a delayed call cannot land
    ///   on a recreated request with the same id
    pub fn cancel_withdrawal(ctx: Context<CancelWithdrawal>, expected_sequence: u64) -> Result<()> {
        return instructions::cancel_withdrawal::handler(ctx, expected_sequence);
    }

    /// Admin returns the vault to instant redemption: the queue co-signs the
    /// vault's `detach_withdrawal_queue` by CPI. It checks no liquidity: nothing
    /// could hold assets back for the pending set once the gate is off. Pending requests
    /// survive and finalize or cancel afterwards; the vault's gate stops new
    /// ones. The queue account persists, so the vault can be attached again.
    pub fn release_vault(ctx: Context<ReleaseVault>) -> Result<()> {
        return instructions::release_vault::handler(ctx);
    }

    /// Admin or operator makes one not-yet-eligible request finalizable now:
    /// `eligible_at` moves to the current time; `scheduled_eligible_at` and
    /// `expires_at` do not, so the window only ever widens. An eligible or
    /// expired request is refused.
    ///
    /// ### Parameters
    /// - `expected_sequence` - The request's stamp, so a delayed call cannot land
    ///   on a recreated request with the same id
    pub fn expedite_request(ctx: Context<ExpediteRequest>, expected_sequence: u64) -> Result<()> {
        return instructions::expedite_request::handler(ctx, expected_sequence);
    }

    /// `expedite_request` over the trailing request accounts, one expected
    /// sequence each, in strictly ascending key order. Any request failing a
    /// precondition aborts the whole batch.
    ///
    /// ### Parameters
    /// - `expected_sequences` - One stamp per trailing request account, in order
    pub fn expedite_requests<'info>(
        ctx: Context<'_, '_, 'info, 'info, ExpediteRequests<'info>>,
        expected_sequences: Vec<u64>,
    ) -> Result<()> {
        return instructions::expedite_request::handler_batch(ctx, expected_sequences);
    }

    /// `finalize_withdrawal` over the trailing accounts, three per request:
    /// the request, its owner and its recipient, in strictly ascending request
    /// order. The vault-side accounts are passed once. Any request failing a
    /// precondition, a liquidity shortfall included, aborts the whole batch.
    ///
    /// ### Parameters
    /// - `expected_sequences` - One stamp per request, in order
    pub fn finalize_withdrawals<'info>(
        ctx: Context<'_, '_, 'info, 'info, FinalizeWithdrawals<'info>>,
        expected_sequences: Vec<u64>,
    ) -> Result<()> {
        return instructions::finalize_withdrawal::handler_batch(ctx, expected_sequences);
    }
}
