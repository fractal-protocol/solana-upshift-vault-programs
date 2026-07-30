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
/// without a compile error. The exhaustive `abi_ordinal` match below pins every
/// variant's ordinal at **compile time**, so appending or reordering will not
/// build until it is updated.
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
    #[msg("Received fewer shares than the caller's minimum")]
    SlippageExceeded,
    #[msg("Vault holds no assets while shares are outstanding; share price is undefined")]
    SharePriceUndefined,
    #[msg("Share offset must be a power of ten within the permitted range")]
    InvalidShareOffset,
}

/// Compile-time pin of the ABI described above, placed next to the enum it guards.
///
/// This is an **exhaustive match**, not a list of assertions, and that is the
/// point: a non-exhaustive match is a hard compile error, so appending a variant
/// breaks the build until it is pinned here. A value-only assertion block cannot
/// do that — it silently keeps passing for the variants it happens to list, which
/// is exactly how the previous runtime canary drifted and stopped covering the
/// most recently added code.
///
/// Anchor adds `ANCHOR_USER_ERROR_OFFSET` (6000) to these ordinals on-chain.
#[allow(dead_code)]
const fn abi_ordinal(e: ErrorCode) -> u32 {
    match e {
        ErrorCode::NotOperator => 0,
        ErrorCode::ZeroAmount => 1,
        ErrorCode::InsufficientAmount => 2,
        ErrorCode::AumIncreaseTooBig => 3,
        ErrorCode::AumDecreaseTooBig => 4,
        ErrorCode::WithdrawalFeeTooHigh => 5,
        ErrorCode::AumLimitTooHigh => 6,
        ErrorCode::NotAdmin => 7,
        ErrorCode::VaultPaused => 8,
        ErrorCode::InvalidNominatedAdmin => 9,
        ErrorCode::NominationExpired => 10,
        ErrorCode::MathError => 11,
        ErrorCode::NumberOverflow => 12,
        ErrorCode::NotEnoughLiquidity => 13,
        ErrorCode::UnauthorizedAdmin => 14,
        ErrorCode::VaultNotEmpty => 15,
        ErrorCode::NotProtocolAuthority => 16,
        ErrorCode::InvalidAuthority => 17,
        ErrorCode::SlippageExceeded => 18,
        ErrorCode::SharePriceUndefined => 19,
        ErrorCode::InvalidShareOffset => 20,
    }
}

const _: () = {
    // Two independent halves, and both are needed:
    //   * the exhaustive match above forces COMPLETENESS — appending a variant
    //     without pinning it is a non-exhaustive-match build error;
    //   * these asserts force CORRECTNESS — each pinned ordinal is compared to
    //     the variant's real discriminant, so reordering two existing variants
    //     fails here even though the match stays exhaustive.
    // A value-only block (the previous form) had neither property, which is how
    // it silently stopped covering the most recently added variant.
    assert!(ErrorCode::NotOperator as u32 == abi_ordinal(ErrorCode::NotOperator));
    assert!(ErrorCode::ZeroAmount as u32 == abi_ordinal(ErrorCode::ZeroAmount));
    assert!(ErrorCode::InsufficientAmount as u32 == abi_ordinal(ErrorCode::InsufficientAmount));
    assert!(ErrorCode::AumIncreaseTooBig as u32 == abi_ordinal(ErrorCode::AumIncreaseTooBig));
    assert!(ErrorCode::AumDecreaseTooBig as u32 == abi_ordinal(ErrorCode::AumDecreaseTooBig));
    assert!(ErrorCode::WithdrawalFeeTooHigh as u32 == abi_ordinal(ErrorCode::WithdrawalFeeTooHigh));
    assert!(ErrorCode::AumLimitTooHigh as u32 == abi_ordinal(ErrorCode::AumLimitTooHigh));
    assert!(ErrorCode::NotAdmin as u32 == abi_ordinal(ErrorCode::NotAdmin));
    assert!(ErrorCode::VaultPaused as u32 == abi_ordinal(ErrorCode::VaultPaused));
    assert!(
        ErrorCode::InvalidNominatedAdmin as u32 == abi_ordinal(ErrorCode::InvalidNominatedAdmin)
    );
    assert!(ErrorCode::NominationExpired as u32 == abi_ordinal(ErrorCode::NominationExpired));
    assert!(ErrorCode::MathError as u32 == abi_ordinal(ErrorCode::MathError));
    assert!(ErrorCode::NumberOverflow as u32 == abi_ordinal(ErrorCode::NumberOverflow));
    assert!(ErrorCode::NotEnoughLiquidity as u32 == abi_ordinal(ErrorCode::NotEnoughLiquidity));
    assert!(ErrorCode::UnauthorizedAdmin as u32 == abi_ordinal(ErrorCode::UnauthorizedAdmin));
    assert!(ErrorCode::VaultNotEmpty as u32 == abi_ordinal(ErrorCode::VaultNotEmpty));
    assert!(ErrorCode::NotProtocolAuthority as u32 == abi_ordinal(ErrorCode::NotProtocolAuthority));
    assert!(ErrorCode::InvalidAuthority as u32 == abi_ordinal(ErrorCode::InvalidAuthority));
    assert!(ErrorCode::SlippageExceeded as u32 == abi_ordinal(ErrorCode::SlippageExceeded));
    assert!(ErrorCode::SharePriceUndefined as u32 == abi_ordinal(ErrorCode::SharePriceUndefined));
    assert!(ErrorCode::InvalidShareOffset as u32 == abi_ordinal(ErrorCode::InvalidShareOffset));
};
