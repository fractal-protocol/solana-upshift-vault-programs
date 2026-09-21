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
/// as it lands, appended at the end and pinned below.
#[error_code]
#[derive(PartialEq)]
pub enum ErrorCode {
    #[msg("Signer is not the vault's admin")]
    NotVaultAdmin,
    #[msg("Math error")]
    MathError,
    #[msg("vault_state is not the vault this queue serves")]
    VaultMismatch,
    #[msg("Cooldown exceeds the 30-day maximum")]
    CooldownOutOfBounds,
    #[msg("Fulfillment window exceeds the 90-day maximum")]
    FulfillmentWindowOutOfBounds,
    #[msg("The vault's withdrawal_queue_authority does not point at this queue")]
    QueueNotActiveOnVault,
    #[msg("Deposit mint carries a Token-2022 extension the queue does not support")]
    UnsupportedDepositMint,
    #[msg("A request must escrow at least one share")]
    ZeroShares,
    #[msg(
        "Recipient must be a deposit-mint token account that is not an escrow or the vault reserve"
    )]
    InvalidRecipient,
    #[msg("Signer is not the request's owner")]
    NotRequestOwner,
    #[msg("The request's fulfillment window has closed; it can only be cancelled")]
    RequestExpired,
    #[msg("expected_sequence does not match the request; it may have been recreated")]
    StaleRequestSequence,
    #[msg("update_request would change nothing; pass a field or the new recipient account")]
    NothingToUpdate,
    #[msg("The request's cooldown has not elapsed")]
    CooldownNotElapsed,
    #[msg("Signer is neither the request's owner nor its finalizer")]
    FinalizerNotAllowed,
    #[msg("The recipient received less than the request's min_assets_out")]
    PayoutBelowFloor,
}

/// Compile-time pin of the ABI above, generated from one list so completeness
/// and correctness cannot come apart. See the vault's `errors.rs` for the full
/// rationale — deliberately duplicated rather than shared, since sharing would
/// mean `#[macro_export]` on the live mainnet crate, and the macro captures
/// `ErrorCode`/`ANCHOR_USER_ERROR_OFFSET` by bare name. Nothing enforces that the
/// two copies stay in step; they pin independent ABIs and need not.
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

        /// Every pinned variant with its on-chain code. Consumed by
        /// `errors_discriminant_canary` below.
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
    VaultMismatch => 2,
    CooldownOutOfBounds => 3,
    FulfillmentWindowOutOfBounds => 4,
    QueueNotActiveOnVault => 5,
    UnsupportedDepositMint => 6,
    ZeroShares => 7,
    InvalidRecipient => 8,
    NotRequestOwner => 9,
    RequestExpired => 10,
    StaleRequestSequence => 11,
    NothingToUpdate => 12,
    CooldownNotElapsed => 13,
    FinalizerNotAllowed => 14,
    PayoutBelowFloor => 15,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Anchor assigns discriminants sequentially from 6000 in declaration order,
    /// so reordering or inserting a variant silently shifts every code after it.
    /// The compile-time pin above catches that at build time; this catches the
    /// case where the pinned list itself is what drifted. Mirrors the vault's
    /// `errors_discriminant_canary`.
    #[test]
    fn errors_discriminant_canary() {
        assert!(
            !ABI_PINS.is_empty(),
            "ABI_PINS is empty — this canary would assert nothing"
        );
        for (variant, code) in ABI_PINS.iter().copied() {
            assert_eq!(
                variant as u32 + ANCHOR_USER_ERROR_OFFSET,
                code,
                "ErrorCode declaration order shifted — see the ABI warning in errors.rs",
            );
        }
    }
}
