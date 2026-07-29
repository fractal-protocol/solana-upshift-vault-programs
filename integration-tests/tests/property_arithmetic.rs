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
use integration_tests::harness::vault_error_code;
use proptest::prelude::*;

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
        let den = total_assets as u128 + VIRTUAL_ASSETS;
        let result = VaultState::shares_for_deposit(supply, total_assets, amount);

        // `checked_mul`, not `*`: with the offsets at 10^6 the exact product
        // exceeds u128 when `amount` and `supply` are both near u64::MAX, so
        // computing it unchecked would panic inside this test rather than test
        // anything. When it does not fit, the program cannot compute it either
        // and must say so.
        match (amount as u128).checked_mul(supply as u128 + EXTRA_SHARES) {
            None => match result {
                Ok(v) => prop_assert!(
                    false,
                    "returned Ok({}) though the exact product exceeds u128", v
                ),
                Err(e) => prop_assert_eq!(
                    anchor_code(&e),
                    Some(vault_error_code(ErrorCode::MathError))
                ),
            },
            Some(num) => {
                let exact_floor = num / den;
                match result {
                    Ok(shares) => prop_assert_eq!(shares as u128, exact_floor),
                    Err(e) => {
                        // The product fit, so only the u64 narrowing may fail.
                        prop_assert!(exact_floor > u64::MAX as u128,
                            "error returned though result {} fits u64", exact_floor);
                        prop_assert_eq!(
                            anchor_code(&e),
                            Some(vault_error_code(ErrorCode::NumberOverflow))
                        );
                    }
                }
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
        let den = supply as u128 + EXTRA_SHARES;
        let result = VaultState::assets_for_redeem(supply, total_assets, shares);

        // See the companion property: the exact product can exceed u128.
        match (shares as u128).checked_mul(total_assets as u128 + VIRTUAL_ASSETS) {
            None => match result {
                Ok(v) => prop_assert!(
                    false,
                    "returned Ok({}) though the exact product exceeds u128", v
                ),
                Err(e) => prop_assert_eq!(
                    anchor_code(&e),
                    Some(vault_error_code(ErrorCode::MathError))
                ),
            },
            Some(num) => {
                let exact_floor = num / den;
                match result {
                    Ok(assets) => prop_assert_eq!(assets as u128, exact_floor),
                    Err(e) => {
                        prop_assert!(exact_floor > u64::MAX as u128,
                            "error returned though result {} fits u64", exact_floor);
                        prop_assert_eq!(
                            anchor_code(&e),
                            Some(vault_error_code(ErrorCode::NumberOverflow))
                        );
                    }
                }
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
