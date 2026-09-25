// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Create a vault's withdrawal queue: the queue PDA and its two escrow token
//! accounts. Attaching it to the vault is a separate, vault-side step, and is
//! what makes the queue live.

use crate::errors::ErrorCode;
use crate::events::QueueInitialized;
use crate::mint_policy::{require_supported_deposit_mint, require_supported_share_mint};
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};
use august_vault::state::vault::VaultState;

/// Both mints must pass their policies, and the cooldown its bound. Everything
/// else about a new queue is fixed: escrows are this PDA's ATAs, and it starts
/// with no expiry window.
pub fn handler(ctx: Context<InitializeQueue>, cooldown_seconds: u64) -> Result<()> {
    require_supported_deposit_mint(&ctx.accounts.deposit_mint)?;
    require_supported_share_mint(&ctx.accounts.share_mint)?;

    let queue_key = ctx.accounts.queue.key();
    let queue = &mut ctx.accounts.queue;
    queue.init(
        ctx.accounts.vault_state.key(),
        ctx.accounts.deposit_mint.key(),
        ctx.accounts.share_mint.key(),
        ctx.accounts.escrow_shares.key(),
        ctx.accounts.escrow_assets.key(),
        ctx.bumps.queue,
        cooldown_seconds,
    )?;

    emit_cpi!(QueueInitialized {
        vault: queue.vault_state,
        queue: queue_key,
        deposit_mint: queue.deposit_mint,
        share_mint: queue.share_mint,
        escrow_shares: queue.escrow_shares,
        escrow_assets: queue.escrow_assets,
        cooldown_seconds: queue.cooldown_seconds,
    });
    Ok(())
}

/// The admin check is a constraint here rather than `require_vault_admin`,
/// because the queue is not yet written when accounts are checked. The binding
/// that helper enforces later is structural at creation: the PDA's seeds are
/// this vault's address. As in the vault's `initialize`, Anchor runs the `init`
/// CPIs before non-init constraints, so an unauthorized caller who cannot fund
/// the rent sees a System Program error rather than `NotVaultAdmin`; the check
/// is enforced either way.
///
/// Every deserialized account is boxed. Three `init` fields plus two 400-byte
/// state accounts put Anchor's generated `try_accounts` over the 4 KB SBF stack
/// frame, and an overflow there does not fail loudly: it silently corrupts the
/// neighbouring `AccountInfo`s, which surfaced as the deposit mint's owner
/// reading as the System Program inside the handler.
#[event_cpi]
#[derive(Accounts)]
pub struct InitializeQueue<'info> {
    /// `Account` pins the vault program as owner, so this is a genuine vault.
    pub vault_state: Box<Account<'info, VaultState>>,

    /// The vault's admin. Authorizes creation but need not fund it.
    #[account(constraint = vault_state.admin == admin.key() @ ErrorCode::NotVaultAdmin)]
    pub admin: Signer<'info>,

    /// Funds the three accounts below. May be the same key as `admin`.
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(address = vault_state.deposit_mint)]
    pub deposit_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(address = vault_state.share_mint)]
    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        init,
        payer = payer,
        space = WithdrawalQueue::LEN,
        seeds = [WITHDRAWAL_QUEUE_SEED, vault_state.key().as_ref()],
        bump,
    )]
    pub queue: Box<Account<'info, WithdrawalQueue>>,

    /// `init_if_needed`, not `init`: an ATA's address is a pure function of
    /// (owner, mint, token program) and anyone may create it, so a stranger who
    /// created these two first would otherwise make every `initialize_queue`
    /// for this vault fail in the create CPI. When the account exists, Anchor
    /// checks its mint, authority and token program against these constraints,
    /// which is exactly what a fresh create would have produced. A balance
    /// someone already sent there is a donation; payouts are balance deltas.
    #[account(
        init_if_needed,
        payer = payer,
        associated_token::mint = share_mint,
        associated_token::authority = queue,
        associated_token::token_program = token_program,
    )]
    pub escrow_shares: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        init_if_needed,
        payer = payer,
        associated_token::mint = deposit_mint,
        associated_token::authority = queue,
        associated_token::token_program = token_program,
    )]
    pub escrow_assets: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The program owning both mints; the vault creates its share mint under the
    /// deposit mint's program. The ATA program refuses a mismatch.
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}
