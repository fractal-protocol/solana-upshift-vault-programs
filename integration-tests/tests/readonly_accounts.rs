//! Pins accounts that must be declared read-only.
//!
//! Wallets that inject state assertions (Phantom via Lighthouse) snapshot every
//! writable account at simulation and fail the transaction if it changed before
//! landing. A writable `deposit_mint` made user deposits and redeems fail
//! whenever anyone minted or burned USDC in between. The harness builds its
//! metas from the same structs, so the runtime suites cannot catch `mut` being
//! re-added; this checks the declared metas directly.

use anchor_lang::{prelude::Pubkey, ToAccountMetas};
use august_vault::accounts as v;
use august_withdrawal_queue::accounts as q;

fn k() -> Pubkey {
    Pubkey::new_unique()
}

fn assert_readonly(metas: impl ToAccountMetas, key: Pubkey, what: &str) {
    let metas = metas.to_account_metas(None);
    let meta = metas
        .iter()
        .find(|m| m.pubkey == key)
        .unwrap_or_else(|| panic!("{what}: account missing from metas"));
    assert!(!meta.is_writable, "{what} must be read-only");
}

#[test]
fn deposit_mint_and_signer_are_readonly_on_deposit() {
    let (mint, signer) = (k(), k());
    let accounts = || v::Deposit {
        vault_state: k(),
        vault_token_ata: k(),
        sender_token_account: k(),
        sender_share_account: k(),
        share_mint: k(),
        deposit_mint: mint,
        signer,
        token_program: k(),
    };
    assert_readonly(accounts(), mint, "deposit.deposit_mint");
    assert_readonly(accounts(), signer, "deposit.signer");
}

#[test]
fn deposit_mint_and_signer_are_readonly_on_redeem() {
    let (mint, signer) = (k(), k());
    let accounts = || v::Redeem {
        vault_state: k(),
        vault_deposit_ata: k(),
        sender_token_account: k(),
        sender_share_account: k(),
        fee_recipient_account: k(),
        share_mint: k(),
        deposit_mint: mint,
        signer,
        token_program: k(),
    };
    assert_readonly(accounts(), mint, "redeem.deposit_mint");
    assert_readonly(accounts(), signer, "redeem.signer");
}

#[test]
fn deposit_mint_is_readonly_on_operator_transfers() {
    let mint = k();
    assert_readonly(
        v::OperatorDeposit {
            vault_state: k(),
            vault_deposit_ata: k(),
            operator_token_account: k(),
            deposit_mint: mint,
            operator: k(),
            token_program: k(),
            subaccount: None,
        },
        mint,
        "operator_deposit.deposit_mint",
    );
    assert_readonly(
        v::OperatorWithdraw {
            vault_state: k(),
            vault_deposit_ata: k(),
            operator_token_account: k(),
            deposit_mint: mint,
            operator: k(),
            token_program: k(),
            subaccount: None,
        },
        mint,
        "operator_withdraw.deposit_mint",
    );
}

#[test]
fn deposit_mint_is_readonly_on_finalize_withdrawal() {
    let mint = k();
    assert_readonly(
        q::FinalizeWithdrawal {
            queue: k(),
            vault_state: k(),
            vault_deposit_ata: k(),
            fee_recipient_account: k(),
            escrow_shares: k(),
            escrow_assets: k(),
            share_mint: k(),
            deposit_mint: mint,
            finalizer: k(),
            request: k(),
            owner: k(),
            recipient_token_account: k(),
            vault_program: k(),
            token_program: k(),
            event_authority: k(),
            program: k(),
        },
        mint,
        "finalize_withdrawal.deposit_mint",
    );
}

#[test]
fn admin_instructions_do_not_lock_unwritten_accounts() {
    let (admin, vault_state, share_mint) = (k(), k(), k());
    let create = || v::CreateShareTokenMetadata {
        payer: k(),
        admin,
        vault_state,
        deposit_mint: k(),
        share_mint,
        metadata_account: k(),
        token_program: k(),
        token_metadata_program: k(),
        system_program: k(),
        rent: k(),
    };
    assert_readonly(create(), admin, "create_metadata.admin");
    assert_readonly(create(), vault_state, "create_metadata.vault_state");
    assert_readonly(create(), share_mint, "create_metadata.share_mint");

    assert_readonly(
        v::UpdateShareTokenMetadata {
            admin: k(),
            vault_state,
            deposit_mint: k(),
            share_mint: k(),
            metadata_account: k(),
            token_program: k(),
            token_metadata_program: k(),
        },
        vault_state,
        "update_metadata.vault_state",
    );
    assert_readonly(
        v::SetAumLimits {
            vault_state: k(),
            deposit_mint: k(),
            admin,
        },
        admin,
        "set_aum_limits.admin",
    );
    assert_readonly(
        v::NominateAdmin {
            vault_state,
            deposit_mint: k(),
            nominated_admin_pda: k(),
            admin: k(),
            payer: k(),
            system_program: k(),
        },
        vault_state,
        "nominate_admin.vault_state",
    );
}
