// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Unit tests for [`super::VaultState`] — pricing, layout and ABI pins.
//!
//! A child module of `state::vault` rather than a separate integration test, so
//! it keeps access to the parent's private items and stays next to the invariants
//! it enforces. Several of these tests are named by doc comments on the constants
//! they guard (`MIN_SHARE_OFFSET`, `share_offset`, `MIN_SUPPLY_MULTIPLE`); keep
//! those references working if you rename anything here.
//!
//! Split out of `vault.rs` purely for navigability — it was ~650 of that file's
//! 1,155 lines. `#[cfg(test)]` means none of it reaches the SBF artifact, and
//! because it sat at the end of the file the move shifted no line above it, so
//! the verified build hash is unchanged (line numbers are in the bytecode via
//! `file!()`/`line!()` — see VERIFY.md).

/// Decimals the sweep below sizes its stake against (the harness mint).
const DEFAULT_TEST_DECIMALS: u8 = 9;

use super::*;
use test_case::test_case;

// ---- shares_for_deposit: exact-value cases ----

// The offsets are 10^6, so cases below that magnitude are dominated by them
// (which is the point — see `EXTRA_SHARES`). Ratio cases therefore use
// magnitudes where the offsets are negligible, and the offset-dominated
// regime gets its own cases.
#[test_case(0, 0, 1_000_000, 1_000_000; "first deposit: 1:1 mint")]
#[test_case(0, 0, 1, 1; "first deposit: single unit")]
#[test_case(0, 0, u64::MAX, u64::MAX; "first deposit: max amount preserved")]
#[test_case(1_000_000, 1_000_000, 1_000_000, 1_000_000; "1:1 ratio mid-life")]
#[test_case(1_000_000_000_000, 2_000_000_000_000, 1_000_000_000_000, 500_000_249_999;
        "share price 2x: about half the shares")]
#[test_case(1_000_000, 3_000_000, 1, 0; "rounds down to zero on tiny deposit")]
fn shares_for_deposit_cases(supply: u64, total_assets: u64, amount: u64, expected: u64) {
    let got =
        VaultState::shares_for_deposit_with_offset(supply, total_assets, amount, EXTRA_SHARES)
            .unwrap();
    assert_eq!(got, expected);
}

// ---- assets_for_redeem: exact-value cases ----

#[test_case(1_000_000, 1_000_000, 1_000_000, 1_000_000; "1:1 ratio: offsets cancel")]
#[test_case(1_000_000_000_000, 2_000_000_000_000, 500_000_000_000, 999_999_500_000;
        "share price 2x: about 2 per share")]
// Inflation-defense: a tiny supply against a large balance does NOT let the
// single-share holder drain the vault. With 10^6 ghost shares the sole real
// share is worth ~1 unit of a 10^6 balance, not half of it — the defence is
// far stronger than it was with a single ghost share.
#[test_case(1, 1_000_000, 1, 1; "tiny supply: ghost shares absorb nearly everything")]
#[test_case(1, 1_000_000_000_000, 1, 1_000_000; "tiny supply against a huge balance")]
fn assets_for_redeem_cases(supply: u64, total_assets: u64, shares: u64, expected: u64) {
    let got = VaultState::assets_for_redeem_with_offset(supply, total_assets, shares, EXTRA_SHARES)
        .unwrap();
    assert_eq!(got, expected);
}

// ---- loss states: deposits must mint at least pro-rata ----
//
// The mirror of the redemption cap. Below par the offsets under-mint, so a
// depositor would hand part of their deposit to incumbents on arrival. These
// pin the fair outcome and the boundary conditions around it.

#[test_case(1_000_000, 500_000, 500_000, 1_000_000; "supply == offsets, 50% loss")]
#[test_case(1_000, 500, 500, 1_000; "supply far below offsets, 50% loss")]
#[test_case(1_000_000_000, 500_000_000, 500_000_000, 1_000_000_000; "supply above offsets")]
fn shares_for_deposit_never_mints_below_pro_rata(
    supply: u64,
    total_assets: u64,
    amount: u64,
    expected: u64,
) {
    let got =
        VaultState::shares_for_deposit_with_offset(supply, total_assets, amount, EXTRA_SHARES)
            .unwrap();
    assert_eq!(got, expected, "must mint pro-rata in a loss state");
    let pro_rata = ((amount as u128 * supply as u128) / total_assets as u128) as u64;
    assert!(got >= pro_rata, "{got} mints below pro-rata {pro_rata}");
}

/// A deposit made after a loss must be immediately redeemable for what it
/// paid, give or take rounding — no value transfer to incumbents on arrival.
#[test_case(1_000_000, 500_000, 500_000; "supply == offsets")]
#[test_case(1_000, 500, 500; "supply far below offsets")]
#[test_case(1_000_000, 999_999, 100_000; "1 unit of loss")]
#[test_case(2_166_176_445, 1_083_088_222, 500_000_000; "live-vault magnitude")]
fn depositing_after_a_loss_does_not_donate_to_incumbents(
    supply: u64,
    total_assets: u64,
    amount: u64,
) {
    let minted =
        VaultState::shares_for_deposit_with_offset(supply, total_assets, amount, EXTRA_SHARES)
            .unwrap();
    let new_supply = supply + minted;
    let new_total = total_assets + amount;

    let redeemable =
        VaultState::assets_for_redeem_with_offset(new_supply, new_total, minted, EXTRA_SHARES)
            .unwrap();
    assert!(
        redeemable + 2 >= amount,
        "deposited {amount} but could only redeem {redeemable} straight back"
    );

    // And the incumbents' claim must not have grown at the depositor's expense.
    let incumbent_before =
        VaultState::assets_for_redeem_with_offset(supply, total_assets, supply, EXTRA_SHARES)
            .unwrap();
    let incumbent_after =
        VaultState::assets_for_redeem_with_offset(new_supply, new_total, supply, EXTRA_SHARES)
            .unwrap();
    assert!(
        incumbent_after <= incumbent_before + 2,
        "incumbents' claim rose from {incumbent_before} to {incumbent_after}"
    );
}

/// No assets but outstanding shares has no defined price — refuse rather than
/// mint against the ghost shares alone. The first-ever deposit is unaffected.
#[test]
fn deposit_into_a_zero_asset_vault_with_shares_is_rejected() {
    let err = VaultState::shares_for_deposit_with_offset(1_000_000, 0, 500_000, EXTRA_SHARES)
        .unwrap_err();
    assert_eq!(
        err_code(&err).unwrap(),
        ErrorCode::SharePriceUndefined as u32 + ANCHOR_USER_ERROR_OFFSET,
    );
    // supply == 0 is the first deposit and must still mint 1:1.
    assert_eq!(
        VaultState::shares_for_deposit_with_offset(0, 0, 500_000, EXTRA_SHARES).unwrap(),
        500_000
    );
}

// ---- loss states: redemptions must stay within pro-rata ----
//
// `total_assets < supply` is reachable whenever an operator reports a loss.
// The offsets pull the price toward 1.0, which below 1.0 means *over*-paying,
// so redemptions are capped at pro-rata. The overpayment grew with the
// offset/supply ratio: unbounded in the limit, and on a 6-decimal mint whose
// minimum first deposit is 1,000 units it approached paying out the whole
// reserve to whoever redeemed first.

#[test_case(1_000_000, 500_000, 500_000, 250_000; "supply == offsets, 50% loss")]
#[test_case(1_000, 500, 500, 250; "supply far below offsets, 50% loss")]
#[test_case(1_000_000_000, 500_000_000, 500_000_000, 250_000_000; "supply above offsets")]
#[test_case(1_000_000, 1, 500_000, 0; "near-total loss floors to zero")]
fn assets_for_redeem_never_exceeds_pro_rata(
    supply: u64,
    total_assets: u64,
    shares: u64,
    expected: u64,
) {
    let got = VaultState::assets_for_redeem_with_offset(supply, total_assets, shares, EXTRA_SHARES)
        .unwrap();
    assert_eq!(got, expected, "must equal pro-rata in a loss state");
    let pro_rata = ((shares as u128 * total_assets as u128) / supply as u128) as u64;
    assert!(got <= pro_rata, "{got} exceeds pro-rata {pro_rata}");
}

/// The whole supply must never be able to claim more than the whole reserve —
/// the property that keeps later redeemers from being left short.
#[test_case(1_000, 500; "supply far below offsets")]
#[test_case(1_000_000, 500_000; "supply equal to offsets")]
#[test_case(1_000_000, 999_999; "1 unit of loss")]
#[test_case(2_166_176_445, 1_082_000_000; "live-vault magnitude, ~50% loss")]
fn redeeming_all_shares_never_exceeds_reserves(supply: u64, total_assets: u64) {
    let out = VaultState::assets_for_redeem_with_offset(supply, total_assets, supply, EXTRA_SHARES)
        .unwrap();
    assert!(
        out <= total_assets,
        "redeeming the entire supply requested {out} against reserves of {total_assets}"
    );
}

/// The cap must not weaken the offsets where they are the defence: above 1.0
/// (an inflated `total_assets`, or a supply collapsed by external burns) the
/// offset value is the smaller one and must still be what is paid.
#[test_case(1, 1_000_000, 1, 1; "supply collapsed to 1 against a large balance")]
#[test_case(4, 1_000_000_000, 4, 4_003; "supply collapsed to 4")]
#[test_case(1_000_000, 2_000_000, 500_000, 750_000; "price 2x")]
fn offsets_still_bind_above_par(supply: u64, total_assets: u64, shares: u64, expected: u64) {
    let got = VaultState::assets_for_redeem_with_offset(supply, total_assets, shares, EXTRA_SHARES)
        .unwrap();
    assert_eq!(got, expected);
    let pro_rata = ((shares as u128 * total_assets as u128) / supply as u128) as u64;
    assert!(
        got < pro_rata,
        "above par the offsets must pay strictly less than pro-rata \
             (got {got}, pro-rata {pro_rata})"
    );
}

// ---- rounding direction is a security property: pin it explicitly ----
//
// With `E = EXTRA_SHARES` and `V = VIRTUAL_ASSETS`, the exact rational value
// `r = amount * (supply + E) / (total_assets + V)` may be non-integer.
// `shares_for_deposit` must return `floor(r)`:
//   `got * (total_assets + V)  <=  amount * (supply + E)`
//   `(got + 1) * (total_assets + V)  >  amount * (supply + E)`
// A future refactor flipping `checked_div` to `div_ceil` would break this.

#[test_case(1_000_000, 3_000_000, 7; "non-divisible: 7 * (1e6+E) / (3e6+V)")]
#[test_case(2, 5, 1; "non-divisible: 1 * (2+E) / (5+V)")]
#[test_case(11, 13, 17; "non-divisible: 17 * (11+E) / (13+V)")]
fn shares_for_deposit_rounds_down(supply: u64, total_assets: u64, amount: u64) {
    // These assertions describe the OFFSET term only, which is the binding
    // one at or above par. Below par `shares_for_deposit` returns the
    // pro-rata floor instead, and a case added there would fail with a
    // misleading "rounded up" rather than a wrong-branch message. Guard the
    // precondition so the next maintainer gets told which it is.
    assert!(
        total_assets >= supply,
        "this test pins the offset term, which only binds at or above par \
             (supply={supply}, total_assets={total_assets}). Below par the \
             pro-rata floor governs — assert against that instead."
    );
    let got = VaultState::shares_for_deposit_with_offset(supply, total_assets, amount, EXTRA_SHARES)
        .unwrap() as u128;
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

#[test_case(1_000_000, 3_000_000, 7; "non-divisible: 7 * (3e6+V) / (1e6+E)")]
#[test_case(5, 2, 1; "non-divisible: 1 * (2+V) / (5+E)")]
#[test_case(13, 11, 17; "non-divisible: 17 * (11+V) / (13+E)")]
fn assets_for_redeem_rounds_down(supply: u64, total_assets: u64, shares: u64) {
    let got = VaultState::assets_for_redeem_with_offset(supply, total_assets, shares, EXTRA_SHARES)
        .unwrap() as u128;
    // Two bounds apply, and the payout is the floor of whichever is tighter:
    // the offset formula, and pro-rata (which binds only below par). Asserting
    // the exact value is stronger than the old "within 1 LSB" check, and it
    // still pins the direction — rounding never favours the redeemer.
    let offset_floor = ((shares as u128) * (total_assets as u128 + VIRTUAL_ASSETS))
        / (supply as u128 + EXTRA_SHARES);
    let pro_rata_floor = ((shares as u128) * (total_assets as u128)) / (supply as u128);
    assert_eq!(
        got,
        offset_floor.min(pro_rata_floor),
        "must be the floor of the tighter bound (offset {offset_floor}, \
             pro-rata {pro_rata_floor})"
    );
    assert!(got <= offset_floor, "rounded up past the offset formula");
    assert!(got <= pro_rata_floor, "paid more than pro-rata");
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
/// gives the on-chain code for each variant. Re-exported from `errors.rs`
/// rather than redeclared — a second `= 6000` here is one more mirrored
/// constant that can drift from the value the ABI pin actually uses.
use crate::errors::ANCHOR_USER_ERROR_OFFSET;

// Two distinct arithmetic failure paths, pinned separately. Which one fires
// depends on the magnitude of the inputs, and the boundary moved when the
// offsets grew to 10^6: `u64::MAX * (u64::MAX + 1)` still fits `u128`, but
// `u64::MAX * (u64::MAX + 10^6)` does not. Both revert the transaction; the
// codes differ only in which check caught it.

#[test]
fn shares_for_deposit_narrowing_overflow_returns_number_overflow() {
    // Both u128 products fit; only the final narrowing to u64 fails. (Here it
    // is the pro-rata branch that exceeds u64, at a supply of u64::MAX against
    // a single unit of assets.)
    let err = VaultState::shares_for_deposit_with_offset(u64::MAX, 1, 1_000_000, EXTRA_SHARES)
        .unwrap_err();
    let code = err_code(&err).expect("AnchorError expected");
    assert_eq!(
        code,
        ErrorCode::NumberOverflow as u32 + ANCHOR_USER_ERROR_OFFSET,
        "expected NumberOverflow (narrowing path), got code {code}",
    );
}

#[test]
fn shares_for_deposit_u128_product_overflow_returns_math_error() {
    // `amount * (supply + EXTRA_SHARES)` exceeds u128 before any division.
    let err = VaultState::shares_for_deposit_with_offset(u64::MAX, 1, u64::MAX, EXTRA_SHARES)
        .unwrap_err();
    let code = err_code(&err).expect("AnchorError expected");
    assert_eq!(
        code,
        ErrorCode::MathError as u32 + ANCHOR_USER_ERROR_OFFSET,
        "expected MathError (u128 product path), got code {code}",
    );
}

#[test]
fn assets_for_redeem_narrowing_overflow_returns_number_overflow() {
    // Symmetric to `shares_for_deposit`: the u128 product fits, the u64
    // narrowing does not.
    let err = VaultState::assets_for_redeem_with_offset(0, u64::MAX, 1_000_000, EXTRA_SHARES)
        .unwrap_err();
    let code = err_code(&err).expect("AnchorError expected");
    assert_eq!(
        code,
        ErrorCode::NumberOverflow as u32 + ANCHOR_USER_ERROR_OFFSET,
        "expected NumberOverflow (narrowing path), got code {code}",
    );
}

#[test]
fn assets_for_redeem_u128_product_overflow_returns_math_error() {
    let err =
        VaultState::assets_for_redeem_with_offset(0, u64::MAX, u64::MAX, EXTRA_SHARES).unwrap_err();
    let code = err_code(&err).expect("AnchorError expected");
    assert_eq!(
        code,
        ErrorCode::MathError as u32 + ANCHOR_USER_ERROR_OFFSET,
        "expected MathError (u128 product path), got code {code}",
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
    // Consumes `ABI_PINS` rather than repeating the table. The previous
    // hand-copied list had already drifted — it stopped at 6019 and never
    // covered `InvalidShareOffset`, the variant added by the same change
    // that wrote it — while still passing, because a loop over a short list
    // simply checks fewer things. Sharing the macro's list makes that
    // impossible: a variant the compile-time pin covers is a variant this
    // canary covers.
    assert!(
        !crate::errors::ABI_PINS.is_empty(),
        "ABI_PINS is empty — this canary would assert nothing"
    );
    for (variant, code) in crate::errors::ABI_PINS.iter().copied() {
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
    let Ok(minted) =
        VaultState::shares_for_deposit_with_offset(supply, total_assets, deposit, EXTRA_SHARES)
    else {
        return; // Overflow in shares math: not a useful seed for this property.
    };
    let (Some(new_supply), Some(new_total)) = (
        supply.checked_add(minted),
        total_assets.checked_add(deposit),
    ) else {
        return; // Post-mint state overflows u64: skip rather than panic.
    };
    let Ok(redeemed) =
        VaultState::assets_for_redeem_with_offset(new_supply, new_total, minted, EXTRA_SHARES)
    else {
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

// The offset floor (MIN_SUPPLY_MULTIPLE * EXTRA_SHARES = 10^8) dominates
// until the decimals term overtakes it at 11 decimals.
#[test_case(0, 100_000_000; "0 decimals (offset floor)")]
#[test_case(6, 100_000_000; "6 decimals, USDC-style: 100 whole tokens")]
#[test_case(9, 100_000_000; "9 decimals, SOL-style: 0.1 whole token")]
#[test_case(11, 100_000_000; "11 decimals (the crossover)")]
#[test_case(12, 1_000_000_000; "12 decimals (decimals term takes over)")]
#[test_case(18, 1_000_000_000_000_000; "18 decimals (USDC-EVM-style)")]
fn min_first_deposit_matches_table(decimals: u8, expected: u64) {
    assert_eq!(
        VaultState::min_first_deposit_for(decimals, EXTRA_SHARES),
        expected
    );
}

/// The property the floor exists for, checked at **every permitted offset**:
/// after the minimum first deposit the non-burnable offset co-holder must own
/// a negligible slice, so an early holder keeps essentially all of the
/// vault's appreciation.
///
/// This is what ties `MIN_SUPPLY_MULTIPLE` to the offset. Without the
/// offset-derived floor a first depositor could forfeit their entire gain.
#[test_case(6, MIN_SHARE_OFFSET; "6 decimals, min offset")]
#[test_case(6, MAX_SHARE_OFFSET; "6 decimals, max offset")]
#[test_case(8, 10_000; "8 decimals, BTC-style mid offset")]
#[test_case(9, MAX_SHARE_OFFSET; "9 decimals, max offset")]
#[test_case(18, MAX_SHARE_OFFSET; "18 decimals, max offset")]
fn first_depositor_keeps_their_appreciation(decimals: u8, offset: u128) {
    let principal = VaultState::min_first_deposit_for(decimals, offset);
    let shares = VaultState::shares_for_deposit_with_offset(0, 0, principal, offset).unwrap();
    assert_eq!(shares, principal, "the first deposit mints 1:1");

    // The vault doubles, then the sole holder exits completely.
    let reserve = principal.checked_mul(2).expect("test input fits");
    let out = VaultState::assets_for_redeem_with_offset(shares, reserve, shares, offset).unwrap();

    let gain = principal; // 100% appreciation
    let kept = out.saturating_sub(principal);
    // A FIXED policy threshold, deliberately not derived from
    // MIN_SUPPLY_MULTIPLE. At the floor the retained fraction is always
    // `M / (M + 1)` by construction, so asserting against that expression is
    // tautological for any M — it moves with whatever value is chosen. 99%
    // encodes the policy instead: it holds at M = 100 (99.01%) and fails if
    // the multiple is halved to 50 (98.04%), which is the drift that matters.
    assert!(
        (kept as u128) * 100 >= (gain as u128) * 99,
        "sole holder kept {kept} of a {gain} gain at {decimals} decimals with \
             offset {offset} — the offset co-holder absorbed it; the offset and \
             MIN_SUPPLY_MULTIPLE must move together"
    );
}

/// The point of making the offset per-vault: a high unit-value mint (8
/// decimals, ~$100k a token) is unlaunchable at the default offset, where
/// the opening deposit is a six-figure cheque. A smaller offset brings that
/// down by three orders of magnitude, and the supply still dominates the
/// offset by the same multiple, so the co-holder's claim is unchanged.
#[test]
fn a_smaller_offset_makes_a_high_value_mint_launchable() {
    const BTC_DECIMALS: u8 = 8;
    let at_default = VaultState::min_first_deposit_for(BTC_DECIMALS, EXTRA_SHARES);
    let at_min = VaultState::min_first_deposit_for(BTC_DECIMALS, MIN_SHARE_OFFSET);
    assert_eq!(at_default / at_min, 1_000, "three orders of magnitude");
    assert!(at_min as u128 >= MIN_SUPPLY_MULTIPLE * MIN_SHARE_OFFSET);
}

/// The security property that justifies `MIN_SHARE_OFFSET`, checked at every
/// permitted offset rather than only the default.
///
/// The integration sweep in `share_burn_pricing.rs` runs against harness
/// vaults, which all carry the default (== `MAX_SHARE_OFFSET`), so it says
/// nothing about the smaller offsets this change newly permits. The manoeuvre
/// is pure share math, so it replays exactly here: stake the offset's own
/// minimum first deposit, burn down to a sliver, let honest deposits land,
/// then exit.
///
/// Note the margin is thin at the bottom of the band (well under 1%) and very
/// wide at the top. That asymmetry is the reason `MIN_SHARE_OFFSET` exists and
/// is why it must not be lowered without re-running this.
#[test]
fn burn_manoeuvre_is_loss_making_at_every_permitted_offset() {
    for offset in [MIN_SHARE_OFFSET, 10_000, 100_000, MAX_SHARE_OFFSET] {
        let stake = VaultState::min_first_deposit_for(DEFAULT_TEST_DECIMALS, offset);
        let mut worst: i128 = i128::MIN;

        for keep in [1u64, 2, 4, 16, 256, 1_000, 100_000] {
            if keep >= stake {
                continue;
            }
            for tenths in [1u64, 2, 3, 5, 8] {
                let each = stake / 10 * tenths;
                if each == 0 {
                    continue;
                }
                for depositors in [1usize, 2, 5, 25, 100] {
                    // Attacker deposits `stake` 1:1, then burns down to `keep`.
                    let (mut supply, mut total) = (keep, stake);
                    for _ in 0..depositors {
                        let minted =
                            VaultState::shares_for_deposit_with_offset(supply, total, each, offset)
                                .expect("honest deposit prices");
                        if minted == 0 {
                            continue;
                        }
                        supply += minted;
                        total += each;
                    }
                    let out =
                        VaultState::assets_for_redeem_with_offset(supply, total, keep, offset)
                            .expect("attacker exit prices");
                    worst = worst.max(out as i128 - stake as i128);
                }
            }
        }

        assert!(
            worst < 0,
            "offset {offset}: the burn manoeuvre returned {worst} on a stake of \
                 {stake} — profitable, so this offset must not be permitted"
        );
    }
}

/// `share_offset` must stay at account byte 199 — the first word of the old
/// `padding`.
///
/// `LEN == 455` constrains the total size, not the field order, and the
/// comment on `padding` tells the next author to "carve new fields out of this
/// array" — the natural reading of which is to declare them beside the other
/// scalars, i.e. *before* `share_offset`. That would shift this field into
/// what is now `padding[1]`, so every vault carrying a non-default offset
/// would silently read 0 there and fall back to the default. No error, no log,
/// correct deserialization, `LEN` still 455, build green.
///
/// The live-vault fork fixtures cannot catch it: their padding is entirely
/// zero, so any shift within that region is invisible. This serializes a
/// sentinel and checks where it actually lands.
#[test]
fn share_offset_stays_at_its_byte_offset() {
    use anchor_lang::AccountSerialize;

    const SENTINEL: u64 = 0x00A1_B2C3_D4E5_F607;
    let state = VaultState {
        share_offset: SENTINEL,
        ..Default::default()
    };
    let mut bytes = Vec::new();
    state.try_serialize(&mut bytes).expect("serialize");

    let at = bytes
        .windows(8)
        .position(|w| w == SENTINEL.to_le_bytes())
        .expect("sentinel must appear in the serialized account");
    assert_eq!(
        at, 199,
        "share_offset moved from byte 199 to {at}. Every vault with a \
             non-default offset would now read 0 there and silently fall back to \
             the default. Carve new fields from the END of `padding`, after \
             `share_offset`, never before it."
    );
}

/// Only powers of ten inside the permitted band may be stored on a vault.
#[test]
fn share_offset_validation_is_exact() {
    for ok in [MIN_SHARE_OFFSET, 10_000, 100_000, MAX_SHARE_OFFSET] {
        assert!(
            VaultState::is_valid_share_offset(ok),
            "{ok} should be valid"
        );
    }
    for bad in [
        0,
        1,
        MIN_SHARE_OFFSET - 1,
        MIN_SHARE_OFFSET + 1,
        5_000,
        MAX_SHARE_OFFSET + 1,
        MAX_SHARE_OFFSET * 10,
        u128::MAX,
    ] {
        assert!(
            !VaultState::is_valid_share_offset(bad),
            "{bad} should be rejected"
        );
    }
}

/// A vault created before `share_offset` existed reads 0 from padding, which
/// must resolve to the default rather than disabling the defences.
#[test]
fn legacy_zero_offset_resolves_to_the_default() {
    let legacy = VaultState::default();
    assert_eq!(legacy.share_offset, 0, "legacy accounts store zero");
    assert_eq!(legacy.share_offset(), EXTRA_SHARES);

    let explicit = VaultState {
        share_offset: MIN_SHARE_OFFSET as u64,
        ..Default::default()
    };
    assert_eq!(explicit.share_offset(), MIN_SHARE_OFFSET);
}

/// The offset term must remain part of the floor.
///
/// Note what this can and cannot catch: because `min_first_deposit_for`
/// returns `max(by_decimals, MIN_SUPPLY_MULTIPLE * offset)`, both sides of the
/// assertion move together, so it is insensitive to the *value* of
/// `MIN_SUPPLY_MULTIPLE`. What it does catch is the offset term being dropped
/// from the floor entirely. The value is pinned instead by
/// `first_depositor_keeps_their_appreciation`.
#[test]
fn min_first_deposit_dominates_the_offsets() {
    for d in 0..=18u8 {
        for offset in [MIN_SHARE_OFFSET, 10_000, MAX_SHARE_OFFSET] {
            let min = VaultState::min_first_deposit_for(d, offset) as u128;
            assert!(
                min >= MIN_SUPPLY_MULTIPLE * offset,
                "min_first_deposit_for({d}, {offset}) = {min} does not clear the offset"
            );
        }
    }
}
