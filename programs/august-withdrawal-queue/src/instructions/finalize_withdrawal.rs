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

use crate::batch;
use crate::errors::ErrorCode;
use crate::events::WithdrawalFinalized;
use crate::recipient::require_valid_recipient;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_lang::AccountsClose;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};
use august_vault::cpi::accounts::Redeem;
use august_vault::program::AugustVault;
use august_vault::state::vault::VaultState;

/// Checks, then the counters, then the two CPIs. A vault refusal (`VaultPaused`,
/// `NotEnoughLiquidity`, `SlippageExceeded`) propagates as the vault's own code
/// and rolls the transaction back, so the request stays pending and untouched.
///
/// The payout is the `escrow_assets` balance delta across the redeem rather than
/// the amount the vault computed, so a donation sitting in the escrow is never
/// paid out. The floor is then re-checked on the recipient's own increase. With
/// a decision-12 deposit mint the two amounts cannot differ, so that check is
/// defence in depth behind the vault's `SlippageExceeded`, not the check.
pub fn handler(ctx: Context<FinalizeWithdrawal>, expected_sequence: u64) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let side = VaultSide::from_single(ctx.accounts);
    let event = finalize_one(
        &side,
        &mut ctx.accounts.queue,
        &mut ctx.accounts.escrow_assets,
        &ctx.accounts.request,
        &mut ctx.accounts.recipient_token_account,
        ctx.accounts.finalizer.key(),
        now,
        expected_sequence,
    )?;
    emit_cpi!(event);
    Ok(())
}

/// The batch form (decision 16): three trailing accounts per request, the
/// request, its owner and its recipient, in ascending request order, all or
/// nothing. The vault-side accounts are passed once. Each request is bound by
/// hand to the same things the single form binds by constraint, and closed by
/// hand with its rent to its owner.
pub fn handler_batch<'info>(
    ctx: Context<'_, '_, 'info, 'info, FinalizeWithdrawals<'info>>,
    expected_sequences: Vec<u64>,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let side = VaultSide::from_batch(ctx.accounts);
    let queue_key = ctx.accounts.queue.key();
    let finalizer = ctx.accounts.finalizer.key();
    let groups = batch::groups(
        ctx.remaining_accounts,
        &expected_sequences,
        3,
        batch::MAX_FINALIZE_BATCH,
    )?;
    for (group, expected_sequence) in groups.into_iter().zip(expected_sequences) {
        let (request_info, owner_info, recipient_info) = (&group[0], &group[1], &group[2]);
        batch::announce(request_info.key);
        let request = batch::load_request(request_info, &queue_key)?;
        require!(
            owner_info.is_writable && recipient_info.is_writable,
            anchor_lang::error::ErrorCode::ConstraintMut
        );
        require_keys_eq!(
            request.owner,
            owner_info.key(),
            anchor_lang::error::ErrorCode::ConstraintHasOne
        );
        require_keys_eq!(
            request.recipient_token_account,
            recipient_info.key(),
            anchor_lang::error::ErrorCode::ConstraintHasOne
        );
        let mut recipient = InterfaceAccount::<TokenAccount>::try_from(recipient_info)?;
        let event = finalize_one(
            &side,
            &mut ctx.accounts.queue,
            &mut ctx.accounts.escrow_assets,
            &request,
            &mut recipient,
            finalizer,
            now,
            expected_sequence,
        )?;
        request.close(owner_info.clone())?;
        emit_cpi!(event);
    }
    Ok(())
}

/// The accounts the vault's `redeem_checked` needs, shared by every request in
/// a transaction. `AccountInfo`s are cheap to clone; the escrow the payout
/// transits and the queue stay borrowed mutably by the caller, since both are
/// reloaded or written per request.
pub(crate) struct VaultSide<'info> {
    vault_program: AccountInfo<'info>,
    vault_state: AccountInfo<'info>,
    vault_deposit_ata: AccountInfo<'info>,
    fee_recipient_account: AccountInfo<'info>,
    escrow_shares: AccountInfo<'info>,
    share_mint: AccountInfo<'info>,
    deposit_mint: AccountInfo<'info>,
    deposit_decimals: u8,
    token_program: AccountInfo<'info>,
}

impl<'info> VaultSide<'info> {
    fn from_single(a: &FinalizeWithdrawal<'info>) -> Self {
        Self {
            vault_program: a.vault_program.to_account_info(),
            vault_state: a.vault_state.to_account_info(),
            vault_deposit_ata: a.vault_deposit_ata.to_account_info(),
            fee_recipient_account: a.fee_recipient_account.to_account_info(),
            escrow_shares: a.escrow_shares.to_account_info(),
            share_mint: a.share_mint.to_account_info(),
            deposit_mint: a.deposit_mint.to_account_info(),
            deposit_decimals: a.deposit_mint.decimals,
            token_program: a.token_program.to_account_info(),
        }
    }

    fn from_batch(a: &FinalizeWithdrawals<'info>) -> Self {
        Self {
            vault_program: a.vault_program.to_account_info(),
            vault_state: a.vault_state.to_account_info(),
            vault_deposit_ata: a.vault_deposit_ata.to_account_info(),
            fee_recipient_account: a.fee_recipient_account.to_account_info(),
            escrow_shares: a.escrow_shares.to_account_info(),
            share_mint: a.share_mint.to_account_info(),
            deposit_mint: a.deposit_mint.to_account_info(),
            deposit_decimals: a.deposit_mint.decimals,
            token_program: a.token_program.to_account_info(),
        }
    }
}

/// One request's payout, shared by both forms. Checks, then the counters, then
/// the two CPIs. Returns the event for the caller to emit, since `emit_cpi!`
/// needs the caller's `ctx`; the caller closes the request account too.
#[allow(clippy::too_many_arguments)]
pub(crate) fn finalize_one<'info>(
    side: &VaultSide<'info>,
    queue: &mut Account<'info, WithdrawalQueue>,
    escrow_assets: &mut InterfaceAccount<'info, TokenAccount>,
    request: &Account<'info, WithdrawalRequest>,
    recipient: &mut InterfaceAccount<'info, TokenAccount>,
    finalizer: Pubkey,
    now: i64,
    expected_sequence: u64,
) -> Result<WithdrawalFinalized> {
    require!(
        request.sequence == expected_sequence,
        ErrorCode::StaleRequestSequence
    );
    require!(
        request.may_finalize(&finalizer),
        ErrorCode::FinalizerNotAllowed
    );
    require!(request.is_eligible(now), ErrorCode::CooldownNotElapsed);
    require!(!request.is_expired(now), ErrorCode::RequestExpired);
    require_valid_recipient(recipient, queue)?;

    let (shares, min_assets_out) = (request.shares, request.min_assets_out);
    // Reload first: in a batch, the previous request's payout has left the
    // escrow since this struct last read it, and a stale figure here would
    // shortchange this one.
    escrow_assets.reload()?;
    let escrow_before = escrow_assets.amount;
    let recipient_before = recipient.amount;

    // Effects before interactions: the counters drop before any CPI.
    queue.close_request(shares)?;

    let seeds = queue.signer_seeds();
    let signer: &[&[&[u8]]] = &[&seeds];
    august_vault::cpi::redeem_checked(
        CpiContext::new_with_signer(
            side.vault_program.clone(),
            Redeem {
                vault_state: side.vault_state.clone(),
                vault_deposit_ata: side.vault_deposit_ata.clone(),
                sender_token_account: escrow_assets.to_account_info(),
                sender_share_account: side.escrow_shares.clone(),
                fee_recipient_account: side.fee_recipient_account.clone(),
                share_mint: side.share_mint.clone(),
                deposit_mint: side.deposit_mint.clone(),
                signer: queue.to_account_info(),
                token_program: side.token_program.clone(),
            },
            signer,
        ),
        shares,
        min_assets_out,
    )?;

    escrow_assets.reload()?;
    let assets = escrow_assets
        .amount
        .checked_sub(escrow_before)
        .ok_or(ErrorCode::MathError)?;
    transfer_checked(
        CpiContext::new_with_signer(
            side.token_program.clone(),
            TransferChecked {
                from: escrow_assets.to_account_info(),
                to: recipient.to_account_info(),
                authority: queue.to_account_info(),
                mint: side.deposit_mint.clone(),
            },
            signer,
        ),
        assets,
        side.deposit_decimals,
    )?;

    recipient.reload()?;
    let received = recipient
        .amount
        .checked_sub(recipient_before)
        .ok_or(ErrorCode::MathError)?;
    require!(received >= min_assets_out, ErrorCode::PayoutBelowFloor);

    Ok(WithdrawalFinalized::snapshot(
        request,
        queue.vault_state,
        request.key(),
        finalizer,
        assets,
    ))
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

/// The single form's vault-side accounts, with the requests, their owners and
/// their recipients as trailing accounts. Bound exactly as `FinalizeWithdrawal`
/// binds them.
#[event_cpi]
#[derive(Accounts)]
pub struct FinalizeWithdrawals<'info> {
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

    /// Whoever chooses the moment, for every request in the batch.
    pub finalizer: Signer<'info>,

    pub vault_program: Program<'info, AugustVault>,
    pub token_program: Interface<'info, TokenInterface>,
}
