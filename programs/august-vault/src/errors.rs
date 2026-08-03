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

/// Compile-time pin of the ABI described above, placed next to the enum it
/// guards, generated from **one** list so the two properties it needs cannot
/// come apart:
///
/// * **Completeness** — the generated match is exhaustive, so appending a
///   variant to `ErrorCode` without adding it here is a build error.
/// * **Correctness** — each entry also generates `assert!(variant as u32 ==
///   ordinal)`, so reordering two existing variants fails the build even though
///   the match stays exhaustive.
///
/// Writing those as two hand-maintained lists (the previous form) left the
/// second one optional: appending a variant broke the match, the author added an
/// arm with a copy-pasted ordinal, and nothing forced a matching assertion — so
/// the pin compiled clean while claiming the wrong code. One list, one place to
/// edit, both guarantees. That is also how the runtime canary in
/// `state/vault.rs` drifted: it consumes [`ABI_PINS`] now rather than repeating
/// the table.
///
/// Anchor adds `ANCHOR_USER_ERROR_OFFSET` (6000) to these ordinals on-chain.
macro_rules! pin_error_abi {
    ($($variant:ident => $ordinal:literal),+ $(,)?) => {
        #[allow(dead_code)]
        const fn abi_ordinal(e: ErrorCode) -> u32 {
            // Exhaustive: this is what makes an unpinned new variant a build
            // failure rather than a silently unpinned error code.
            match e {
                $(ErrorCode::$variant => $ordinal,)+
            }
        }

        const _: () = {
            $(assert!(ErrorCode::$variant as u32 == $ordinal);)+
        };

        /// Every pinned variant with its on-chain code, for the runtime canary.
        /// Derived from the same list as the compile-time pin, so it cannot omit
        /// a variant the pin covers.
        #[allow(dead_code)]
        pub const ABI_PINS: &[(ErrorCode, u32)] = &[
            $((ErrorCode::$variant, $ordinal + ANCHOR_USER_ERROR_OFFSET),)+
        ];
    };
}

/// Anchor reserves the first 6000 codes; user variants start here.
pub const ANCHOR_USER_ERROR_OFFSET: u32 = 6000;

pin_error_abi! {
    NotOperator => 0,
    ZeroAmount => 1,
    InsufficientAmount => 2,
    AumIncreaseTooBig => 3,
    AumDecreaseTooBig => 4,
    WithdrawalFeeTooHigh => 5,
    AumLimitTooHigh => 6,
    NotAdmin => 7,
    VaultPaused => 8,
    InvalidNominatedAdmin => 9,
    NominationExpired => 10,
    MathError => 11,
    NumberOverflow => 12,
    NotEnoughLiquidity => 13,
    UnauthorizedAdmin => 14,
    VaultNotEmpty => 15,
    NotProtocolAuthority => 16,
    InvalidAuthority => 17,
    SlippageExceeded => 18,
    SharePriceUndefined => 19,
    InvalidShareOffset => 20,
}
