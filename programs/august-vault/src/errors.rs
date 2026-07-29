// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use anchor_lang::prelude::*;

/// **ABI WARNING**: variant declaration order is part of the on-chain ABI.
/// Anchor's `#[error_code]` macro assigns numeric codes sequentially starting
/// at 6000 in declaration order (6000, 6001, …). Reordering variants, inserting
/// new variants anywhere except the end, or assigning explicit discriminants
/// silently renumbers downstream variants — every off-chain client matching on
/// numeric codes (and every test using `(ErrorCode as u32) + 6000`) breaks
/// without a compile error. The `errors_discriminant_canary` test in
/// `state/vault.rs` pins the expected codes; update it in lockstep if you
/// must reorder.
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
    #[msg("Signer is not the protocol authority")]
    NotProtocolAuthority,
    #[msg("Authority must not be the zero key")]
    InvalidAuthority,
}
