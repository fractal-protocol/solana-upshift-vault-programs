// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! The vault state account is the main account of the vault.
//! It stores the operator pubkey and the paused state

use crate::errors::ErrorCode;
use anchor_lang::prelude::*;

pub const VAULT_STATE_SEED: &[u8] = b"VAULT_STATE";

/// Seed for the program-derived share-token mint owned by the vault.
pub const SHARE_MINT_SEED: &[u8] = b"mint";

/// Seed for the program-derived token account holding the vault's reserve assets.
pub const VAULT_TOKEN_SEED: &[u8] = b"token_vault";

pub const FEE_RATE_DENOMINATOR_VALUE: u32 = 1_000_000;

/// Virtual-share offset used by `shares_for_deposit` / `assets_for_redeem`.
///
/// The math treats every vault as if it had `EXTRA_SHARES` "ghost" share(s)
/// permanently outstanding. This defends against the OpenZeppelin ERC-4626
/// share-inflation attack: an attacker who is first to deposit a single unit
/// and then donates a large asset balance directly to the reserve would
/// otherwise be able to round subsequent depositors' shares down to zero.
/// See <https://docs.openzeppelin.com/contracts/5.x/erc4626#inflation-attack>.
///
/// This is **not** tunable — changing it changes the share-price math and
/// breaks the security property. Tests in this module pin the behaviour.
pub const EXTRA_SHARES: u128 = 1;

/// Companion virtual-asset offset; see `EXTRA_SHARES`. Both offsets must be
/// non-zero for the inflation defense to hold.
pub const VIRTUAL_ASSETS: u128 = 1;

#[account]
#[derive(Default, InitSpace)]
pub struct VaultState {
    pub operator: Pubkey,
    pub admin: Pubkey,
    pub share_mint: Pubkey,
    pub deposit_mint: Pubkey,
    pub fee_recipient: Pubkey,
    pub withdrawal_fee: u32,
    pub local_aum: u64,
    pub deployed_aum: u64,
    pub aum_increase_limit: u32, // Basis points (e.g., 20 = 0.2%)
    pub aum_decrease_limit: u32, // Basis points (e.g., 20 = 0.2%)
    pub pda_bump: [u8; 1],
    pub vault_version: [u8; 1], // Version number for vault PDAs (allows multiple vaults per deposit mint)
    pub paused: bool,
    pub padding: [u64; 32],
}

impl VaultState {
    pub const LEN: usize = 8 + Self::INIT_SPACE;
    /// Initialize the vault state
    pub fn init(
        &mut self,
        operator: Pubkey,
        admin: Pubkey,
        share_mint: Pubkey,
        deposit_mint: Pubkey,
        fee_recipient: Pubkey,
        withdrawal_fee: u32,
        pda_bump: [u8; 1],
        vault_version: [u8; 1],
    ) {
        self.operator = operator;
        self.admin = admin;
        self.share_mint = share_mint;
        self.deposit_mint = deposit_mint;
        self.fee_recipient = fee_recipient;
        self.withdrawal_fee = withdrawal_fee;
        self.deployed_aum = 0;
        self.aum_increase_limit = 20; // Default: 0.2% (20 basis points)
        self.aum_decrease_limit = 20; // Default: 0.2% (20 basis points)
        self.paused = false;
        self.pda_bump = pda_bump;
        self.vault_version = vault_version;
    }

    /// Get the seed for the vault state PDA
    pub fn seeds(&self) -> [&[u8]; 4] {
        [
            VAULT_STATE_SEED.as_ref(),
            self.deposit_mint.as_ref(),
            &self.vault_version,
            &self.pda_bump,
        ]
    }

    /// Sum of on-vault (`local_aum`) and externally-deployed (`deployed_aum`) assets.
    /// Returns `MathError` on overflow rather than saturating — saturation here
    /// would silently mint extra shares against a corrupted total.
    pub fn total_assets(&self) -> Result<u64> {
        self.local_aum
            .checked_add(self.deployed_aum)
            .ok_or_else(|| ErrorCode::MathError.into())
    }

    /// Shares minted for a deposit of `amount` underlying assets.
    ///
    /// Formula: `amount * (supply + EXTRA_SHARES) / (total_assets + VIRTUAL_ASSETS)`.
    ///
    /// Uses `u128` intermediates because `amount * (supply + 1)` exceeds `u64`
    /// at realistic balances. Rounded **down**: the LSB-of-precision goes to the
    /// vault, not the depositor. Inverse of `assets_for_redeem`; together the
    /// rounding policy guarantees `redeem(deposit(x)) ≤ x`. See the
    /// `rounding_direction_*` property tests below.
    pub fn shares_for_deposit(supply: u64, total_assets: u64, amount: u64) -> Result<u64> {
        let shares_eff = (supply as u128)
            .checked_add(EXTRA_SHARES)
            .ok_or(ErrorCode::MathError)?;
        let assets_eff = (total_assets as u128)
            .checked_add(VIRTUAL_ASSETS)
            .ok_or(ErrorCode::MathError)?;
        let shares = (amount as u128)
            .checked_mul(shares_eff)
            .ok_or(ErrorCode::MathError)?
            .checked_div(assets_eff)
            .ok_or(ErrorCode::MathError)?;
        u64::try_from(shares).map_err(|_| ErrorCode::NumberOverflow.into())
    }

    /// Underlying assets redeemed for `shares` burned.
    ///
    /// Formula: `shares * (total_assets + VIRTUAL_ASSETS) / (supply + EXTRA_SHARES)`.
    /// `u128` intermediates; rounded **down** (favours the vault). Companion of
    /// `shares_for_deposit`.
    pub fn assets_for_redeem(supply: u64, total_assets: u64, shares: u64) -> Result<u64> {
        let shares_eff = (supply as u128)
            .checked_add(EXTRA_SHARES)
            .ok_or(ErrorCode::MathError)?;
        let assets_eff = (total_assets as u128)
            .checked_add(VIRTUAL_ASSETS)
            .ok_or(ErrorCode::MathError)?;
        let assets = (shares as u128)
            .checked_mul(assets_eff)
            .ok_or(ErrorCode::MathError)?
            .checked_div(shares_eff)
            .ok_or(ErrorCode::MathError)?;
        u64::try_from(assets).map_err(|_| ErrorCode::NumberOverflow.into())
    }

    /// Minimum amount of the deposit token required for the *first* deposit,
    /// expressed in the mint's native units.
    ///
    /// Why: depositing a single unit as the very first depositor enables a
    /// classic share-inflation attack. The minimum forces the first deposit to
    /// be large enough that the virtual-share offset cannot be diluted by a
    /// follow-up donation. For mints with ≥4 decimals the floor is `10^(d-3)`
    /// (≈ 0.001 of a whole token); for 0–3 decimals a single base unit is the
    /// most we can require without rejecting all deposits.
    pub fn min_first_deposit(decimals: u8) -> u64 {
        match decimals {
            0..=3 => 1,
            d => 10_u64.saturating_pow(d.saturating_sub(3) as u32),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    // ---- shares_for_deposit: exact-value cases ----

    #[test_case(0, 0, 1_000_000, 1_000_000; "first deposit: 1:1 mint")]
    #[test_case(0, 0, 1, 1; "first deposit: single unit")]
    #[test_case(0, 0, u64::MAX, u64::MAX; "first deposit: max amount preserved")]
    #[test_case(1_000_000, 1_000_000, 1_000_000, 1_000_000; "1:1 ratio mid-life")]
    #[test_case(1_000_000, 2_000_000, 1_000_000, 500_000; "share price 2x: half shares")]
    #[test_case(1_000_000, 3_000_000, 1, 0; "rounds down to zero on tiny deposit")]
    fn shares_for_deposit_cases(supply: u64, total_assets: u64, amount: u64, expected: u64) {
        let got = VaultState::shares_for_deposit(supply, total_assets, amount).unwrap();
        assert_eq!(got, expected);
    }

    // ---- assets_for_redeem: exact-value cases ----

    #[test_case(1_000_000, 1_000_000, 1_000_000, 1_000_000; "1:1 ratio: offsets cancel")]
    #[test_case(1_000_000, 2_000_000, 500_000, 999_999; "share price 2x: ~2 per share (rounded)")]
    // Inflation-defense: a tiny supply against a huge balance does NOT let the
    // single-share holder drain the vault — the virtual share absorbs ~half.
    #[test_case(1, 1_000_000, 1, 500_000; "tiny supply: virtual share absorbs half")]
    fn assets_for_redeem_cases(supply: u64, total_assets: u64, shares: u64, expected: u64) {
        let got = VaultState::assets_for_redeem(supply, total_assets, shares).unwrap();
        assert_eq!(got, expected);
    }

    // ---- rounding direction is a security property: pin it explicitly ----
    //
    // The exact rational value `r = amount * (supply + 1) / (total_assets + 1)`
    // may be non-integer. `shares_for_deposit` must return `floor(r)`:
    //   `got * (total_assets + 1)  <=  amount * (supply + 1)`
    //   `(got + 1) * (total_assets + 1)  >  amount * (supply + 1)`
    // A future refactor flipping `checked_div` to `div_ceil` would break this.

    #[test_case(1_000_000, 3_000_000, 7; "non-divisible: 7 * 1e6+1 / 3e6+1")]
    #[test_case(2, 5, 1; "non-divisible: 1 * 3 / 6")]
    #[test_case(11, 13, 17; "non-divisible: 17 * 12 / 14")]
    fn shares_for_deposit_rounds_down(supply: u64, total_assets: u64, amount: u64) {
        let got = VaultState::shares_for_deposit(supply, total_assets, amount).unwrap() as u128;
        let num = (amount as u128) * (supply as u128 + EXTRA_SHARES);
        let den = total_assets as u128 + VIRTUAL_ASSETS;
        assert!(
            got * den <= num,
            "rounded up: got*den={} > num={}",
            got * den,
            num
        );
        assert!((got + 1) * den > num, "lost more than 1 LSB");
    }

    #[test_case(1_000_000, 3_000_000, 7; "non-divisible: 7 * 3e6+1 / 1e6+1")]
    #[test_case(5, 2, 1; "non-divisible: 1 * 3 / 6")]
    #[test_case(13, 11, 17; "non-divisible: 17 * 12 / 14")]
    fn assets_for_redeem_rounds_down(supply: u64, total_assets: u64, shares: u64) {
        let got = VaultState::assets_for_redeem(supply, total_assets, shares).unwrap() as u128;
        let num = (shares as u128) * (total_assets as u128 + VIRTUAL_ASSETS);
        let den = supply as u128 + EXTRA_SHARES;
        assert!(
            got * den <= num,
            "rounded up: got*den={} > num={}",
            got * den,
            num
        );
        assert!((got + 1) * den > num, "lost more than 1 LSB");
    }

    // ---- arithmetic narrowing path: only the final `u64::try_from` can fail
    // for u64 inputs. The intermediates are all bounded:
    //   (a) `(supply as u128) + 1` fits in u128 (u64::MAX + 1 = 2^64 < u128::MAX).
    //   (b) `(total_assets as u128) + 1` — same bound.
    //   (c) `u64 * (u64 + 1)` = `(2^64 - 1) * 2^64 = 2^128 - 2^64 < u128::MAX`.
    //   (d) `checked_div` cannot overflow.
    // So `MathError` is structurally unreachable for `shares_for_deposit` /
    // `assets_for_redeem` with u64 inputs; only the `u64::try_from` narrowing
    // can fail, and by convention it returns `NumberOverflow`.

    /// Extract Anchor's numeric error code from an `anchor_lang::error::Error`.
    /// Returns `None` for non-AnchorError variants (e.g. raw ProgramError).
    fn err_code(e: &anchor_lang::error::Error) -> Option<u32> {
        match e {
            anchor_lang::error::Error::AnchorError(b) => Some(b.error_code_number),
            _ => None,
        }
    }

    /// Anchor user errors start at this code; `(ErrorCode as u32) + this`
    /// gives the on-chain code for each variant.
    const ANCHOR_USER_ERROR_OFFSET: u32 = 6000;

    #[test]
    fn shares_for_deposit_narrowing_overflow_returns_number_overflow() {
        let err = VaultState::shares_for_deposit(u64::MAX, 1, u64::MAX).unwrap_err();
        let code = err_code(&err).expect("AnchorError expected");
        assert_eq!(
            code,
            ErrorCode::NumberOverflow as u32 + ANCHOR_USER_ERROR_OFFSET,
            "expected NumberOverflow (narrowing path), got code {code}",
        );
    }

    #[test]
    fn assets_for_redeem_narrowing_overflow_returns_number_overflow() {
        // Symmetric boundary case to `shares_for_deposit`: with assets_eff
        // small (supply=0 → shares_eff=1, after the +1 offset), a maximal
        // `shares` * (`total_assets` + 1) overflows the u64 narrowing.
        let err = VaultState::assets_for_redeem(0, u64::MAX, u64::MAX).unwrap_err();
        let code = err_code(&err).expect("AnchorError expected");
        assert_eq!(
            code,
            ErrorCode::NumberOverflow as u32 + ANCHOR_USER_ERROR_OFFSET,
            "expected NumberOverflow (narrowing path), got code {code}",
        );
    }

    // ---- canary: pin every ErrorCode variant's on-chain code ----
    //
    // `assert_anchor_err` in the integration harness and many tests compute
    // expected error codes as `(ErrorCode as u32) + 6000`. That only works
    // because `errors.rs` declares variants without explicit discriminants,
    // so Anchor's #[error_code] assigns them sequentially in declaration
    // order. If anyone reorders or inserts variants, the code shifts silently
    // and tests start matching the wrong variant. This canary fails fast on
    // any drift — update the expected values in lockstep with errors.rs.

    #[test]
    fn errors_discriminant_canary() {
        // (variant, expected on-chain code)
        let expected = [
            (ErrorCode::NotOperator, 6000),
            (ErrorCode::ZeroAmount, 6001),
            (ErrorCode::InsufficientAmount, 6002),
            (ErrorCode::AumIncreaseTooBig, 6003),
            (ErrorCode::AumDecreaseTooBig, 6004),
            (ErrorCode::WithdrawalFeeTooHigh, 6005),
            (ErrorCode::AumLimitTooHigh, 6006),
            (ErrorCode::NotAdmin, 6007),
            (ErrorCode::VaultPaused, 6008),
            (ErrorCode::InvalidNominatedAdmin, 6009),
            (ErrorCode::NominationExpired, 6010),
            (ErrorCode::MathError, 6011),
            (ErrorCode::NumberOverflow, 6012),
            (ErrorCode::NotEnoughLiquidity, 6013),
            (ErrorCode::UnauthorizedAdmin, 6014),
            (ErrorCode::VaultNotEmpty, 6015),
        ];
        for (variant, code) in expected {
            assert_eq!(
                variant as u32 + ANCHOR_USER_ERROR_OFFSET,
                code,
                "ErrorCode declaration order shifted — see the ABI warning in errors.rs",
            );
        }
    }

    // ---- round-trip property: redeem(deposit(x)) <= x, across many regimes ----
    //
    // Seeds chosen to exercise the regimes where rounding actually bites:
    // total_assets >> supply (high share price), supply >> total_assets (post-
    // donation inflation regime), near-overflow boundaries, and the trivial
    // 1:1 cases. Intermediate `checked_add` overflows skip the case rather
    // than panic, so adding new seeds is safe.

    #[test_case(0, 0, 1; "fresh vault: single unit")]
    #[test_case(0, 0, 1_000_000; "fresh vault: round number")]
    #[test_case(1_000_000, 1_000_000, 500_000; "mid-life 1:1")]
    #[test_case(123_456, 789_012, 12_345; "asymmetric small")]
    #[test_case(1, 1_000_000_000, 1_000; "high share price")]
    #[test_case(1_000_000_000, 1, 100; "post-donation inflation regime")]
    #[test_case(u64::MAX / 2, u64::MAX / 2, 1; "near-overflow boundary: single unit")]
    #[test_case(0, 0, u64::MAX / 4; "large fresh deposit")]
    fn round_trip_never_extracts_value(supply: u64, total_assets: u64, deposit: u64) {
        let Ok(minted) = VaultState::shares_for_deposit(supply, total_assets, deposit) else {
            return; // Overflow in shares math: not a useful seed for this property.
        };
        let (Some(new_supply), Some(new_total)) = (
            supply.checked_add(minted),
            total_assets.checked_add(deposit),
        ) else {
            return; // Post-mint state overflows u64: skip rather than panic.
        };
        let Ok(redeemed) = VaultState::assets_for_redeem(new_supply, new_total, minted) else {
            return;
        };
        assert!(
            redeemed <= deposit,
            "round-trip extracted value: supply={supply} total={total_assets} deposit={deposit} minted={minted} redeemed={redeemed}"
        );
    }

    // ---- total_assets ----

    #[test]
    fn total_assets_sums_local_and_deployed() {
        let vault = VaultState {
            local_aum: 100,
            deployed_aum: 250,
            ..Default::default()
        };
        assert_eq!(vault.total_assets().unwrap(), 350);
    }

    #[test]
    fn total_assets_detects_overflow() {
        let vault = VaultState {
            local_aum: u64::MAX,
            deployed_aum: 1,
            ..Default::default()
        };
        let err = vault.total_assets().unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("MathError"), "got: {msg}");
    }

    // ---- min_first_deposit ----

    #[test_case(0, 1; "0 decimals")]
    #[test_case(1, 1; "1 decimal")]
    #[test_case(2, 1; "2 decimals")]
    #[test_case(3, 1; "3 decimals (boundary: still 1 unit)")]
    #[test_case(4, 10; "4 decimals (formula kicks in)")]
    #[test_case(5, 100; "5 decimals")]
    #[test_case(6, 1_000; "6 decimals (USDC-style)")]
    #[test_case(7, 10_000; "7 decimals")]
    #[test_case(8, 100_000; "8 decimals")]
    #[test_case(9, 1_000_000; "9 decimals (SOL-style)")]
    #[test_case(10, 10_000_000; "10 decimals")]
    #[test_case(18, 1_000_000_000_000_000; "18 decimals (USDC-EVM-style)")]
    fn min_first_deposit_matches_table(decimals: u8, expected: u64) {
        assert_eq!(VaultState::min_first_deposit(decimals), expected);
    }
}
