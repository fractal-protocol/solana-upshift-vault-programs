// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

pub mod errors;
pub mod state;
use instructions::accept_admin_nomination::*;
use instructions::close_vault::*;
use instructions::create_metadata::*;
use instructions::deposit::*;
use instructions::initialize::*;
use instructions::initialize_config::*;
use instructions::nominate_admin::*;
use instructions::operator_deposit::*;
use instructions::operator_update_aum::*;
use instructions::operator_withdraw::*;
use instructions::override_config_authority::*;
use instructions::pause::*;
use instructions::redeem::*;
use instructions::set_aum_limits::*;
use instructions::set_config_authority::*;
use instructions::set_fee_recipient::*;
use instructions::set_operator::*;
use instructions::set_withdrawal_fee::*;
use instructions::unpause::*;
use instructions::update_metadata::*;
pub mod instructions;

use anchor_lang::prelude::*;

// Devnet: C8B1EpsSGVWK2vMrk3aDT3kL7RCE77otokUh4EC35kK7
// Mainnet: up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt
declare_id!("up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt");

// On-chain security contact + source provenance, queryable from the deployed
// program (e.g. `solana-verify` / explorers). Gated out of CPI/library builds
// via `no-entrypoint`, so it ships only in the deployable program.
#[cfg(not(feature = "no-entrypoint"))]
use solana_security_txt::security_txt;

#[cfg(not(feature = "no-entrypoint"))]
security_txt! {
    name: "Upshift Vault (august_vault)",
    project_url: "https://github.com/fractal-protocol/solana-upshift-vault-programs",
    contacts: "email:alex@augustdigital.io,link:https://github.com/fractal-protocol/solana-upshift-vault-programs/security/advisories/new",
    policy: "https://github.com/fractal-protocol/solana-upshift-vault-programs/security/policy",
    source_code: "https://github.com/fractal-protocol/solana-upshift-vault-programs",
    preferred_languages: "en"
}

#[program]
pub mod august_vault {
    use super::*;

    /// Create the singleton program config, naming the key permitted to create
    /// vaults. Callable only by the program's current upgrade authority, and
    /// only once.
    ///
    /// ### Parameters
    /// - `authority` - The key that may call `initialize` from now on
    pub fn initialize_config(ctx: Context<InitializeConfig>, authority: Pubkey) -> Result<()> {
        return instructions::initialize_config::handler(ctx, authority);
    }

    /// Rotate the key permitted to create vaults. Signed by the current
    /// authority; see `override_config_authority` for the recovery path.
    ///
    /// ### Parameters
    /// - `new_authority` - The replacement authority (must not be the zero key)
    pub fn set_config_authority(
        ctx: Context<SetConfigAuthority>,
        new_authority: Pubkey,
    ) -> Result<()> {
        return instructions::set_config_authority::handler(ctx, new_authority);
    }

    /// Reset the vault-creation authority using the program's **upgrade
    /// authority**, for when the current config authority is wrong or lost.
    ///
    /// ### Parameters
    /// - `new_authority` - The replacement authority (must not be the zero key)
    pub fn override_config_authority(
        ctx: Context<OverrideConfigAuthority>,
        new_authority: Pubkey,
    ) -> Result<()> {
        return instructions::override_config_authority::handler(ctx, new_authority);
    }

    /// Initialize the Vault state
    /// Mint Vault shares
    ///
    /// Requires the signer to be the protocol authority from `ProgramConfig`.
    ///
    /// ### Parameters
    /// - `admin` - The Admin of the Vault
    /// - `operator` - The Operator of the Vault
    /// - `fee_recipient` - The Fee recipient of the Vault
    /// - `vault_version` - Version number for the vault (allows multiple vaults per deposit mint)
    pub fn initialize(
        ctx: Context<Initialize>,
        admin: Pubkey,
        operator: Pubkey,
        fee_recipient: Pubkey,
        vault_version: u8,
    ) -> Result<()> {
        return instructions::initialize::handler(
            ctx,
            admin,
            operator,
            fee_recipient,
            vault_version,
        );
    }

    /// Deposit funds in the Vault
    /// Mint Vault shares
    ///
    /// ### Parameters
    /// - `amount` - The amount to deposit
    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        return instructions::deposit::handler(ctx, amount);
    }

    /// Redeem funds from the Vault
    /// Burn shared
    /// Get tokens out
    /// ### Parameters
    /// - `shares` - The amount of shares to burn
    pub fn redeem(ctx: Context<Redeem>, shares: u64) -> Result<()> {
        return instructions::redeem::handler(ctx, shares);
    }
    /// Operator withdraw funds from the Vault
    ///
    /// Get tokens out
    /// ### Parameters
    /// - `amount` - The amount of tokens to get out of the Vault
    pub fn operator_withdraw(ctx: Context<OperatorWithdraw>, amount: u64) -> Result<()> {
        return instructions::operator_withdraw::handler(ctx, amount);
    }
    /// Operator deposit funds in the Vault
    ///
    /// Get tokens in
    /// ### Parameters
    /// - `amount` - The amount of tokens to get in the Vault
    pub fn operator_deposit(ctx: Context<OperatorDeposit>, amount: u64) -> Result<()> {
        return instructions::operator_deposit::handler(ctx, amount);
    }

    /// Operator Updates the Deployed AUM
    ///
    /// Update must be within a +/- 10% bounderies of current deployed AUM
    /// Any update will increase / decrease the shares value
    /// ### Parameters
    /// - `new_aum` - The new deployed AUM value
    pub fn operator_update_aum(ctx: Context<OperatorUpdateAum>, new_aum: u64) -> Result<()> {
        return instructions::operator_update_aum::handler(ctx, new_aum);
    }

    /// Admin Updates the Withdrawal Fee
    ///
    /// Fee should discourage users from sandwiching operator_update_aum.
    /// ### Parameters
    /// - `new_fee` - Fee in units of `FEE_RATE_DENOMINATOR_VALUE` (full denominator
    ///   ≡ 100%). The handler caps `new_fee` strictly below
    ///   `FEE_RATE_DENOMINATOR_VALUE / 10` (i.e. < 10%); see
    ///   `set_withdrawal_fee::handler` for the exact cap expression. Higher
    ///   inputs revert with `WithdrawalFeeTooHigh`.
    pub fn set_withdrawal_fee(ctx: Context<SetWithdrawalFee>, new_fee: u32) -> Result<()> {
        return instructions::set_withdrawal_fee::handler(ctx, new_fee);
    }

    /// Admin Nominates a new Admin
    /// ### Parameters
    /// - `new_admin` - The new admin
    pub fn nominate_admin(ctx: Context<NominateAdmin>, new_admin: Pubkey) -> Result<()> {
        return instructions::nominate_admin::handler(ctx, new_admin);
    }

    /// New accepts the Admin Nomination
    /// ### Parameters
    pub fn accept_admin_nomination(ctx: Context<AcceptAdminNomination>) -> Result<()> {
        return instructions::accept_admin_nomination::handler(ctx);
    }

    /// Admin Updates the Operator
    /// ### Parameters
    /// - `new_operator` - The new operator
    pub fn set_operator(ctx: Context<SetOperator>, new_operator: Pubkey) -> Result<()> {
        return instructions::set_operator::handler(ctx, new_operator);
    }

    /// Admin Updates the Fee recipient
    /// ### Parameters
    /// - `new_fee_recipient` - The new fee recipient
    pub fn set_fee_recipient(
        ctx: Context<SetFeeRecipient>,
        new_fee_recipient: Pubkey,
    ) -> Result<()> {
        return instructions::set_fee_recipient::handler(ctx, new_fee_recipient);
    }

    /// Admin Updates the AUM Change Limits
    /// ### Parameters
    /// - `increase_limit` - Max increase in basis points (e.g., 20 = 0.2%, 100 = 1%)
    /// - `decrease_limit` - Max decrease in basis points (e.g., 20 = 0.2%, 100 = 1%)
    pub fn set_aum_limits(
        ctx: Context<SetAumLimits>,
        increase_limit: u32,
        decrease_limit: u32,
    ) -> Result<()> {
        return instructions::set_aum_limits::handler(ctx, increase_limit, decrease_limit);
    }

    /// Admin Pauses the Vault
    /// ### Parameters
    /// - `new_fee_recipient` - The new fee recipient
    pub fn pause(ctx: Context<Pause>) -> Result<()> {
        return instructions::pause::handler(ctx);
    }

    /// Admin Unpauses the Vault    
    /// ### Parameters
    /// - `new_fee_recipient` - The new fee recipient
    pub fn unpause(ctx: Context<Unpause>) -> Result<()> {
        return instructions::unpause::handler(ctx);
    }

    /// Admin Closes an empty Vault
    /// Reclaims rent from vault_state, share_mint, and vault_token_ata
    /// Requires vault to have zero shares outstanding and empty token account
    pub fn close_vault(ctx: Context<CloseVault>) -> Result<()> {
        return instructions::close_vault::handler(ctx);
    }

    /// Create metadata for the share token
    /// ### Parameters
    /// - `name` - The name of the token
    /// - `symbol` - The symbol of the token
    /// - `uri` - The URI for the token metadata
    pub fn create_share_token_metadata(
        ctx: Context<CreateShareTokenMetadata>,
        name: String,
        symbol: String,
        uri: String,
    ) -> Result<()> {
        return instructions::create_metadata::create_share_token_metadata(ctx, name, symbol, uri);
    }

    /// Update metadata for the share token
    /// ### Parameters
    /// - `name` - The new name of the token
    /// - `symbol` - The new symbol of the token
    /// - `uri` - The new URI for the token metadata
    pub fn update_share_token_metadata(
        ctx: Context<UpdateShareTokenMetadata>,
        name: String,
        symbol: String,
        uri: String,
    ) -> Result<()> {
        return instructions::update_metadata::update_share_token_metadata(ctx, name, symbol, uri);
    }
}
