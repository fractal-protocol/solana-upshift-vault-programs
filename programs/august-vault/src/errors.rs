// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE.BSL file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use anchor_lang::prelude::*;

#[error_code]
#[derive(PartialEq)]
pub enum ErrorCode {
    #[msg("Signer must be the admin")]
    NotOperator,
    #[msg("Amount must be > 0")]
    ZeroAmount,
    #[msg("Insufficient amount for initial deposit")]
    InsufficientAmount,
    #[msg("Over 10% aum increase ?")]
    AumIncreaseTooBig,
    #[msg("Over 10% aum decrease ?")]
    AumDecreaseTooBig,
    #[msg("Withdrawal fee too high")]
    WithdrawalFeeTooHigh,
    #[msg("AUM limit exceeds maximum allowed value")]
    AumLimitTooHigh,
    #[msg("Signer is Not Admin")]
    NotAdmin,
    #[msg("Vault is paused")]
    VaultPaused,
    #[msg("Nominated Admin is incorrect")]
    InvalidNominatedAdmin,
    #[msg("Admin nomination window expired")]
    NominationExpired,
    #[msg("Math error")]
    MathError,
    #[msg("Number Overflow")]
    NumberOverflow,
    #[msg("Not Enough Liquidity")]
    NotEnoughLiquidity,
    #[msg("Unauthorized admin for metadata operation")]
    UnauthorizedAdmin,
    #[msg("Vault must be empty to close")]
    VaultNotEmpty,
}
