//! Property-based tests for the share/asset conversion arithmetic in
//! `VaultState`, addressing the due-diligence recommendation to add
//! fuzz/property testing over "share and asset conversion" math.
//!
//! The unit suite in `programs/august-vault/src/state/vault.rs` pins these
//! properties at hand-picked seeds; this suite searches the full `u64` input
//! space (proptest explores boundaries and random interiors, and shrinks any
//! counterexample it finds).
//!
//! Determinism note: proptest derives its RNG per run; a failing case is
//! persisted to `proptest-regressions/` so it replays on every later run.

use august_vault::errors::ErrorCode;
use august_vault::state::vault::{VaultState, EXTRA_SHARES, VIRTUAL_ASSETS};
use proptest::prelude::*;

/// On-chain code for `ErrorCode::NumberOverflow` (declaration ordinal +
/// Anchor's 6000 user-error offset — pinned by `errors_discriminant_canary`).
const NUMBER_OVERFLOW_CODE: u32 = ErrorCode::NumberOverflow as u32 + 6000;

fn anchor_code(e: &anchor_lang::error::Error) -> Option<u32> {
    match e {
        anchor_lang::error::Error::AnchorError(b) => Some(b.error_code_number),
        _ => None,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    /// `shares_for_deposit` is exactly `floor(amount * (supply+1) / (total+1))`
    /// whenever the result fits in u64. A refactor that changes the rounding
    /// direction (the OtterSec-audited security property) fails here.
    #[test]
    fn shares_for_deposit_is_exact_floor(
        supply in any::<u64>(),
        total_assets in any::<u64>(),
        amount in any::<u64>(),
    ) {
        let num = (amount as u128) * (supply as u128 + EXTRA_SHARES);
        let den = total_assets as u128 + VIRTUAL_ASSETS;
        let exact_floor = num / den;

        match VaultState::shares_for_deposit(supply, total_assets, amount) {
            Ok(shares) => prop_assert_eq!(shares as u128, exact_floor),
            Err(e) => {
                // Only the final u64 narrowing may fail for u64 inputs.
                prop_assert!(exact_floor > u64::MAX as u128,
                    "error returned though result {} fits u64", exact_floor);
                prop_assert_eq!(anchor_code(&e), Some(NUMBER_OVERFLOW_CODE));
            }
        }
    }

    /// Companion property for `assets_for_redeem`.
    #[test]
    fn assets_for_redeem_is_exact_floor(
        supply in any::<u64>(),
        total_assets in any::<u64>(),
        shares in any::<u64>(),
    ) {
        let num = (shares as u128) * (total_assets as u128 + VIRTUAL_ASSETS);
        let den = supply as u128 + EXTRA_SHARES;
        let exact_floor = num / den;

        match VaultState::assets_for_redeem(supply, total_assets, shares) {
            Ok(assets) => prop_assert_eq!(assets as u128, exact_floor),
            Err(e) => {
                prop_assert!(exact_floor > u64::MAX as u128,
                    "error returned though result {} fits u64", exact_floor);
                prop_assert_eq!(anchor_code(&e), Some(NUMBER_OVERFLOW_CODE));
            }
        }
    }

    /// Depositing then redeeming the minted shares can never extract more
    /// than was deposited, in any reachable regime. Generalizes the seeded
    /// `round_trip_never_extracts_value` unit test.
    #[test]
    fn round_trip_never_extracts_value(
        supply in any::<u64>(),
        total_assets in any::<u64>(),
        deposit in any::<u64>(),
    ) {
        let Ok(minted) = VaultState::shares_for_deposit(supply, total_assets, deposit) else {
            return Ok(()); // narrowing overflow: not a round-trippable state
        };
        let (Some(new_supply), Some(new_total)) =
            (supply.checked_add(minted), total_assets.checked_add(deposit))
        else {
            return Ok(()); // post-mint state itself overflows u64
        };
        let Ok(redeemed) = VaultState::assets_for_redeem(new_supply, new_total, minted) else {
            return Ok(());
        };
        prop_assert!(
            redeemed <= deposit,
            "extracted value: minted={} redeemed={} > deposit={}",
            minted, redeemed, deposit
        );
    }

    /// First deposit into a fresh vault mints exactly 1:1.
    #[test]
    fn first_deposit_mints_one_to_one(amount in any::<u64>()) {
        prop_assert_eq!(
            VaultState::shares_for_deposit(0, 0, amount).unwrap(),
            amount
        );
    }

    /// More assets in never means fewer shares out (same vault state).
    #[test]
    fn shares_for_deposit_is_monotone_in_amount(
        supply in any::<u64>(),
        total_assets in any::<u64>(),
        a in any::<u64>(),
        b in any::<u64>(),
    ) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        if let (Ok(s_lo), Ok(s_hi)) = (
            VaultState::shares_for_deposit(supply, total_assets, lo),
            VaultState::shares_for_deposit(supply, total_assets, hi),
        ) {
            prop_assert!(s_lo <= s_hi, "monotonicity violated: {} > {}", s_lo, s_hi);
        }
    }

    /// More shares burned never means fewer assets out (same vault state).
    #[test]
    fn assets_for_redeem_is_monotone_in_shares(
        supply in any::<u64>(),
        total_assets in any::<u64>(),
        a in any::<u64>(),
        b in any::<u64>(),
    ) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        if let (Ok(r_lo), Ok(r_hi)) = (
            VaultState::assets_for_redeem(supply, total_assets, lo),
            VaultState::assets_for_redeem(supply, total_assets, hi),
        ) {
            prop_assert!(r_lo <= r_hi, "monotonicity violated: {} > {}", r_lo, r_hi);
        }
    }

    /// The first-deposit floor is monotone in mint decimals and never zero,
    /// so no decimals value can disable the inflation-attack defense.
    #[test]
    fn min_first_deposit_is_monotone_and_nonzero(d in 0u8..=200) {
        let cur = VaultState::min_first_deposit(d);
        prop_assert!(cur >= 1);
        if d > 0 {
            prop_assert!(VaultState::min_first_deposit(d - 1) <= cur);
        }
    }
}
