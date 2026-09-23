// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Design decision 12: which deposit mints a queue may be created for, and
//! what the queue assumes of the share mint.

use crate::errors::ErrorCode;
use anchor_lang::prelude::*;
use anchor_spl::token_2022::spl_token_2022::extension::{
    BaseStateWithExtensions, ExtensionType, StateWithExtensions,
};
use anchor_spl::token_2022::spl_token_2022::state::Mint as Mint2022;
use anchor_spl::token_interface::Mint;

/// A classic SPL mint always passes. A Token-2022 mint passes only if every
/// extension it carries is `MetadataPointer` or `TokenMetadata`.
///
/// Anything else, known or unknown, is refused. A transfer fee would take a cut
/// on each escrow hop, so the recipient would get less than the vault paid out
/// while the instruction succeeded; hooks need accounts the redeem CPI does not carry; a
/// permanent delegate can move escrowed funds; a frozen default state would
/// create frozen escrows. An allow-list rather than a deny-list, so a future
/// extension is refused until someone has reasoned about it.
pub fn require_supported_deposit_mint(mint: &InterfaceAccount<'_, Mint>) -> Result<()> {
    require!(
        only_metadata_extensions(mint)?,
        ErrorCode::UnsupportedDepositMint
    );
    Ok(())
}

/// The deposit mint's allow-list, and no freeze authority. `cancel_withdrawal`
/// is the exit that works in every state only because `escrow_shares` cannot
/// be frozen. The vault creates its share mint that way today; this makes it a
/// check rather than an inference across the program boundary.
pub fn require_supported_share_mint(mint: &InterfaceAccount<'_, Mint>) -> Result<()> {
    require!(
        mint.freeze_authority.is_none(),
        ErrorCode::UnsupportedShareMint
    );
    require!(
        only_metadata_extensions(mint)?,
        ErrorCode::UnsupportedShareMint
    );
    Ok(())
}

/// Whether `mint` is classic SPL, or Token-2022 carrying only metadata
/// extensions. Data that does not parse as a Token-2022 mint is `false`.
fn only_metadata_extensions(mint: &InterfaceAccount<'_, Mint>) -> Result<bool> {
    let info = mint.to_account_info();
    if *info.owner == anchor_spl::token::ID {
        return Ok(true);
    }
    let data = info.try_borrow_data()?;
    let Ok(state) = StateWithExtensions::<Mint2022>::unpack(&data) else {
        return Ok(false);
    };
    let Ok(extensions) = state.get_extension_types() else {
        return Ok(false);
    };
    for extension in extensions {
        match extension {
            ExtensionType::MetadataPointer | ExtensionType::TokenMetadata => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_lang::solana_program::program_pack::Pack;
    use anchor_spl::token_2022::spl_token_2022::state::Mint as MintState;

    /// Runs the check against a mint account with `owner` and `data`, through the
    /// same `InterfaceAccount` wrapper a handler holds.
    fn check(owner: Pubkey, data: Vec<u8>) -> Result<()> {
        with_mint(owner, data, require_supported_deposit_mint)
    }

    fn check_share(owner: Pubkey, data: Vec<u8>) -> Result<()> {
        with_mint(owner, data, require_supported_share_mint)
    }

    fn with_mint(
        owner: Pubkey,
        mut data: Vec<u8>,
        policy: fn(&InterfaceAccount<'_, Mint>) -> Result<()>,
    ) -> Result<()> {
        let key = Pubkey::new_unique();
        let mut lamports = 1;
        let info = AccountInfo::new(
            &key,
            false,
            false,
            &mut lamports,
            &mut data,
            &owner,
            false,
            0,
        );
        let mint = InterfaceAccount::<Mint>::try_from(&info).expect("a mint");
        policy(&mint)
    }

    fn base_mint_bytes() -> Vec<u8> {
        mint_bytes(None)
    }

    fn mint_bytes(freeze_authority: Option<Pubkey>) -> Vec<u8> {
        let mint = MintState {
            mint_authority: Some(Pubkey::new_unique()).into(),
            supply: 0,
            decimals: 6,
            is_initialized: true,
            freeze_authority: freeze_authority.into(),
        };
        let mut data = vec![0u8; MintState::LEN];
        Pack::pack_into_slice(&mint, &mut data);
        data
    }

    #[test]
    fn a_classic_spl_mint_passes() {
        check(anchor_spl::token::ID, base_mint_bytes()).expect("classic SPL always passes");
    }

    #[test]
    fn a_vanilla_token_2022_mint_passes() {
        check(anchor_spl::token_2022::ID, base_mint_bytes()).expect("no extensions is allowed");
    }

    /// A classic deposit mint may be freezable (USDC is); a share mint may not.
    #[test]
    fn a_freezable_share_mint_is_refused_on_either_program() {
        let freezable = mint_bytes(Some(Pubkey::new_unique()));
        check(anchor_spl::token::ID, freezable.clone()).expect("fine for a deposit mint");
        for owner in [anchor_spl::token::ID, anchor_spl::token_2022::ID] {
            let err = check_share(owner, freezable.clone()).expect_err("freezable share mint");
            assert_eq!(err, ErrorCode::UnsupportedShareMint.into());
            check_share(owner, base_mint_bytes()).expect("no freeze authority, no extensions");
        }
    }

    /// Not freezable, but carrying an extension outside the allow-list.
    #[test]
    fn a_share_mint_with_an_unlisted_extension_is_refused() {
        use anchor_spl::token_2022::spl_token_2022::extension::{
            mint_close_authority::MintCloseAuthority, BaseStateWithExtensionsMut,
            StateWithExtensionsMut,
        };
        let len = ExtensionType::try_calculate_account_len::<MintState>(&[
            ExtensionType::MintCloseAuthority,
        ])
        .expect("len");
        let mut data = vec![0u8; len];
        let mut state =
            StateWithExtensionsMut::<MintState>::unpack_uninitialized(&mut data).expect("unpack");
        state
            .init_extension::<MintCloseAuthority>(true)
            .expect("extension");
        state.base = MintState {
            mint_authority: Some(Pubkey::new_unique()).into(),
            supply: 0,
            decimals: 6,
            is_initialized: true,
            freeze_authority: None.into(),
        };
        state.pack_base();
        state.init_account_type().expect("account type");

        let err = check_share(anchor_spl::token_2022::ID, data).expect_err("unlisted extension");
        assert_eq!(err, ErrorCode::UnsupportedShareMint.into());
    }
}
