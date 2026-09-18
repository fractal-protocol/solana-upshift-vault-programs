// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Design decision 12: which deposit mints a queue may be created for.

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
/// on each escrow hop and land the recipient below their floor while the
/// instruction succeeded; hooks need accounts the redeem CPI does not carry; a
/// permanent delegate can move escrowed funds; a frozen default state would
/// create frozen escrows. An allow-list rather than a deny-list, so a future
/// extension is refused until someone has reasoned about it.
pub fn require_supported_deposit_mint(mint: &InterfaceAccount<'_, Mint>) -> Result<()> {
    let info = mint.to_account_info();
    if *info.owner == anchor_spl::token::ID {
        return Ok(());
    }
    let data = info.try_borrow_data()?;
    let state = StateWithExtensions::<Mint2022>::unpack(&data)
        .map_err(|_| error!(ErrorCode::UnsupportedDepositMint))?;
    let extensions = state
        .get_extension_types()
        .map_err(|_| error!(ErrorCode::UnsupportedDepositMint))?;
    require!(
        extensions.iter().all(|e| matches!(
            e,
            ExtensionType::MetadataPointer | ExtensionType::TokenMetadata
        )),
        ErrorCode::UnsupportedDepositMint
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_lang::solana_program::program_pack::Pack;
    use anchor_spl::token_2022::spl_token_2022::state::Mint as MintState;

    /// Runs the check against a mint account with `owner` and `data`, through the
    /// same `InterfaceAccount` wrapper a handler holds.
    fn check(owner: Pubkey, mut data: Vec<u8>) -> Result<()> {
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
        require_supported_deposit_mint(&mint)
    }

    fn base_mint_bytes() -> Vec<u8> {
        let mint = MintState {
            mint_authority: Some(Pubkey::new_unique()).into(),
            supply: 0,
            decimals: 6,
            is_initialized: true,
            freeze_authority: None.into(),
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
}
