// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use anchor_lang::prelude::*;

/// **ABI WARNING**: variant declaration order is part of the on-chain ABI, and
/// the same discipline applies here as in the vault — Anchor numbers variants
/// sequentially from 6000 in declaration order, so reordering or inserting
/// anywhere but the end silently renumbers every variant after it.
///
/// This enum is deliberately near-empty: each instruction brings its own errors
/// as it lands, appended at the end and pinned below. The two here are the ones
/// every instruction needs.
#[error_code]
#[derive(PartialEq)]
pub enum ErrorCode {
    #[msg("Signer is not the vault's admin")]
    NotVaultAdmin,
    #[msg("Math error")]
    MathError,
}

/// Compile-time pin of the ABI above, generated from one list so completeness
/// and correctness cannot come apart. See the vault's `errors.rs` for the full
/// rationale — this is the same mechanism, kept identical on purpose so the two
/// programs are read the same way.
macro_rules! pin_error_abi {
    ($($variant:ident => $ordinal:literal),+ $(,)?) => {
        #[allow(dead_code)]
        const fn abi_ordinal(e: ErrorCode) -> u32 {
            // Exhaustive: an unpinned new variant is a build failure.
            match e {
                $(ErrorCode::$variant => $ordinal,)+
            }
        }

        const _: () = {
            $(assert!(ErrorCode::$variant as u32 == $ordinal);)+
        };

        /// Every pinned variant with its on-chain code, for a runtime canary.
        #[allow(dead_code)]
        pub const ABI_PINS: &[(ErrorCode, u32)] = &[
            $((ErrorCode::$variant, $ordinal + ANCHOR_USER_ERROR_OFFSET),)+
        ];
    };
}

/// Anchor reserves the first 6000 codes; user variants start here.
pub const ANCHOR_USER_ERROR_OFFSET: u32 = 6000;

pin_error_abi! {
    NotVaultAdmin => 0,
    MathError => 1,
}
