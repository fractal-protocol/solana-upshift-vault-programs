// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Pay out a mature request. The queue redeems the escrowed shares by CPI into
//! the vault, signing as the PDA the vault was pointed at, and forwards what the
//! vault paid to the request's recipient. The caller chooses the moment, never
//! the amount or the destination: both come from the request.

use crate::errors::ErrorCode;
use crate::events::WithdrawalFinalized;
use crate::recipient::require_valid_recipient;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};
use august_vault::cpi::accounts::Redeem;
use august_vault::program::AugustVault;
use august_vault::state::vault::VaultState;

/// Checks, then the counters, then the two CPIs. A vault refusal (`VaultPaused`,
/// `NotEnoughLiquidity`) propagates as the vault's own code and rolls the
/// transaction back, so the request stays pending and untouched.
///
/// The payout is the shares' value at this moment, with no floor: there is no
/// trade, so nothing can slip. It is the `escrow_assets` balance delta across
/// the redeem rather than the amount the vault computed, so a donation sitting
/// in the escrow is never paid out.
pub fn handler(ctx: Context<FinalizeWithdrawal>, expected_sequence: u64) -> Result<()> {
    let request = &ctx.accounts.request;
    require!(
        request.sequence == expected_sequence,
        ErrorCode::StaleRequestSequence
    );
    require!(
        request.may_finalize(&ctx.accounts.finalizer.key()),
        ErrorCode::FinalizerNotAllowed
    );
    let now = Clock::get()?.unix_timestamp;
    require!(request.is_eligible(now), ErrorCode::CooldownNotElapsed);
    require!(!request.is_expired(now), ErrorCode::RequestExpired);
    require_valid_recipient(&ctx.accounts.recipient_token_account, &ctx.accounts.queue)?;
    // The vault binds the fee account only by `fee_recipient`'s authority. Were
    // that ever set to this PDA, the escrow would pass as the fee account, and
    // the fee would land in the balance delta and go to the recipient.
    require_keys_neq!(
        ctx.accounts.fee_recipient_account.key(),
        ctx.accounts.escrow_assets.key(),
        ErrorCode::FeeAccountIsEscrow
    );

    let shares = request.shares;
    let escrow_before = ctx.accounts.escrow_assets.amount;

    // Effects before interactions: the counters drop before any CPI.
    ctx.accounts.queue.close_request(shares)?;

    let seeds = ctx.accounts.queue.signer_seeds();
    let signer: &[&[&[u8]]] = &[&seeds];
    august_vault::cpi::redeem(
        CpiContext::new_with_signer(
            ctx.accounts.vault_program.to_account_info(),
            Redeem {
                vault_state: ctx.accounts.vault_state.to_account_info(),
                vault_deposit_ata: ctx.accounts.vault_deposit_ata.to_account_info(),
                sender_token_account: ctx.accounts.escrow_assets.to_account_info(),
                sender_share_account: ctx.accounts.escrow_shares.to_account_info(),
                fee_recipient_account: ctx.accounts.fee_recipient_account.to_account_info(),
                share_mint: ctx.accounts.share_mint.to_account_info(),
                deposit_mint: ctx.accounts.deposit_mint.to_account_info(),
                signer: ctx.accounts.queue.to_account_info(),
                token_program: ctx.accounts.token_program.to_account_info(),
            },
            signer,
        ),
        shares,
    )?;

    ctx.accounts.escrow_assets.reload()?;
    let assets = ctx
        .accounts
        .escrow_assets
        .amount
        .checked_sub(escrow_before)
        .ok_or(ErrorCode::MathError)?;
    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.escrow_assets.to_account_info(),
                to: ctx.accounts.recipient_token_account.to_account_info(),
                authority: ctx.accounts.queue.to_account_info(),
                mint: ctx.accounts.deposit_mint.to_account_info(),
            },
            signer,
        ),
        assets,
        ctx.accounts.deposit_mint.decimals,
    )?;

    emit_cpi!(WithdrawalFinalized::snapshot(
        &ctx.accounts.request,
        ctx.accounts.queue.vault_state,
        ctx.accounts.request.key(),
        ctx.accounts.finalizer.key(),
        assets,
    ));
    Ok(())
}

/// Every deserialized account is boxed; see `InitializeQueue` for why.
///
/// The vault-side accounts the queue stores are bound to the stored keys here.
/// The two it does not store, the vault's reserve and the fee account, are bound
/// by the vault's own constraints inside the CPI: the reserve by its seeds, the
/// fee account by `fee_recipient`'s authority. Nothing is left to the caller.
/// Everything the vault's `Redeem` declares writable, both mints included, must
/// arrive writable here, since a CPI cannot widen an account's privileges.
#[event_cpi]
#[derive(Accounts)]
pub struct FinalizeWithdrawal<'info> {
    #[account(
        mut,
        seeds = [WITHDRAWAL_QUEUE_SEED, queue.vault_state.as_ref()],
        bump = queue.bump,
        has_one = vault_state @ ErrorCode::VaultMismatch,
        has_one = deposit_mint,
        has_one = share_mint,
        has_one = escrow_shares,
        has_one = escrow_assets,
    )]
    pub queue: Box<Account<'info, WithdrawalQueue>>,

    /// Written by the vault inside the CPI, never by this program.
    #[account(mut)]
    pub vault_state: Box<Account<'info, VaultState>>,

    #[account(mut)]
    pub vault_deposit_ata: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub fee_recipient_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub escrow_shares: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub escrow_assets: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut)]
    pub deposit_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Whoever chooses the moment; `WithdrawalRequest::may_finalize` decides.
    pub finalizer: Signer<'info>,

    /// Closed on success with its rent to `owner`. Seeds come from its own
    /// stored fields, so no argument can point it elsewhere.
    #[account(
        mut,
        close = owner,
        seeds = [
            WITHDRAWAL_REQUEST_SEED,
            request.queue.as_ref(),
            request.owner.as_ref(),
            &request.request_id.to_le_bytes(),
        ],
        bump = request.bump,
        has_one = queue,
        has_one = owner,
        has_one = recipient_token_account,
    )]
    pub request: Box<Account<'info, WithdrawalRequest>>,

    /// CHECK: the request's owner, bound by `has_one`. Receives the rent and
    /// need not sign.
    #[account(mut)]
    pub owner: UncheckedAccount<'info>,

    /// The request's stored recipient, re-validated in the handler.
    #[account(mut)]
    pub recipient_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub vault_program: Program<'info, AugustVault>,
    pub token_program: Interface<'info, TokenInterface>,
}
