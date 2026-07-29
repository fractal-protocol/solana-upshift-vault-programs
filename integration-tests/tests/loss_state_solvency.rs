//! Solvency after a reported loss.
//!
//! `total_assets < supply` is reachable whenever the operator reports a loss on
//! deployed capital. In that state the share-price offsets pull the effective
//! price *up* toward 1.0, so a redemption priced purely by the offset formula
//! pays more than the position's pro-rata share of what is actually left.
//!
//! That is not a rounding curiosity: whoever redeems first takes the excess, and
//! later holders find the reserve short. `assets_for_redeem` therefore caps the
//! payout at pro-rata, and this file proves the property end to end — every
//! holder can still exit after a loss, and nobody can take more than their share.
//!
//! The same offsets under-price the mirror direction: a deposit made into a vault
//! below par would be minted *fewer* shares than its money is worth, donating the
//! difference to the incumbents. `shares_for_deposit` therefore floors the mint at
//! pro-rata, proven here by depositing into a loss-state vault and redeeming
//! straight back out. A vault that has lost everything has no price at all, and
//! deposits into it are rejected rather than mispriced.
//!
//! The magnitudes are chosen so the effect is large rather than dust. The
//! divergence is worst at the *smallest* reachable supply and shrinks
//! monotonically as supply grows: uncapped, a deposit into a vault carrying a 50%
//! loss lost roughly a third of its value at the smallest reachable supply
//! (the minimum first deposit for a 6-decimal mint), a fifth at a supply equal
//! to the offsets, and negligible amounts once supply is orders of magnitude
//! above them.

use august_vault::errors::ErrorCode;
use integration_tests::harness::{assert_anchor_err, VaultCtx, DEPOSIT_DECIMALS};

/// The minimum first deposit for a 9-decimal mint, which is the same order as
/// the share-price offsets. Note this is *not* the worst case for the divergence
/// — smaller supplies diverge more (see the module docs) — but it is the regime
/// the harness mint puts us in.
const MIN_FIRST: u64 = 10u64.pow(DEPOSIT_DECIMALS as u32 - 3);

/// Drive a vault into a loss state: two equal depositors, the operator takes
/// everything out, reports half of it lost, and returns only what is left.
fn vault_after_50_percent_loss(
    each: u64,
) -> (VaultCtx, Vec<integration_tests::harness::Depositor>) {
    let mut ctx = VaultCtx::fresh();

    let a = ctx.new_depositor(each);
    let b = ctx.new_depositor(each);
    ctx.deposit_as(&a, each).expect("first deposit");
    ctx.deposit_as(&b, each).expect("second deposit");
    let supply = ctx.share_mint_supply();
    assert_eq!(supply, each * 2, "1:1 mint expected at this point");

    // Deploy everything, then widen the AUM window so a large loss can be
    // reported in one step (the default is ±20 bps).
    ctx.operator_withdraw(each * 2)
        .expect("operator deploys all");
    ctx.set_aum_limits(10_000, 10_000)
        .expect("admin widens the AUM window");

    // Report half the deployed capital lost, and return only the surviving half.
    ctx.operator_update_aum(each)
        .expect("operator reports a loss");
    ctx.operator_deposit(each)
        .expect("operator returns what is left");

    let state = ctx.vault_state_data();
    let total = state.total_assets().unwrap();
    assert_eq!(total, each, "assets should now be half the share supply");
    assert!(total < supply, "precondition: this must be a loss state");

    (ctx, vec![a, b])
}

/// Every holder must be able to exit, and the vault must not run dry partway.
///
/// Uncapped, the first redeemer would be paid `2/3` of the reserve for a half
/// position, and the second would hit `NotEnoughLiquidity`.
#[test]
fn all_holders_can_exit_after_a_loss() {
    let each = MIN_FIRST;
    let (mut ctx, holders) = vault_after_50_percent_loss(each);

    let reserve_before = ctx.token_account_amount(&ctx.vault_token_pda);
    let mut paid_out = 0u64;

    for (i, h) in holders.iter().enumerate() {
        let shares = ctx.token_account_amount(&h.share_ata);
        assert!(shares > 0, "holder {i} should hold shares");
        let before = ctx.token_account_amount(&h.deposit_ata);
        ctx.redeem_as(h, shares)
            .unwrap_or_else(|e| panic!("holder {i} could not exit after the loss: {e:?}"));
        let received = ctx.token_account_amount(&h.deposit_ata) - before;
        paid_out += received;

        // Nobody may take more than their pro-rata share of what existed when
        // they redeemed.
        assert!(
            received <= reserve_before / 2 + 1,
            "holder {i} took {received}, more than half of the {reserve_before} reserve"
        );
    }

    assert!(
        paid_out <= reserve_before,
        "paid out {paid_out} against a reserve of {reserve_before}"
    );
}

/// The first redeemer specifically must not be able to take more than pro-rata —
/// the mechanism by which later holders would be left short.
#[test]
fn first_redeemer_cannot_exceed_pro_rata_after_a_loss() {
    let each = MIN_FIRST;
    let (mut ctx, holders) = vault_after_50_percent_loss(each);

    let supply = ctx.share_mint_supply();
    let total = ctx.vault_state_data().total_assets().unwrap();
    let shares = ctx.token_account_amount(&holders[0].share_ata);
    let pro_rata = ((shares as u128 * total as u128) / supply as u128) as u64;

    let before = ctx.token_account_amount(&holders[0].deposit_ata);
    ctx.redeem_as(&holders[0], shares).expect("first exit");
    let received = ctx.token_account_amount(&holders[0].deposit_ata) - before;

    assert!(
        received <= pro_rata,
        "first redeemer received {received}, above pro-rata {pro_rata}"
    );
    // And enough is left for the remaining holder to be made whole pro-rata.
    let left = ctx.token_account_amount(&ctx.vault_token_pda);
    let remaining_shares = ctx.token_account_amount(&holders[1].share_ata);
    let remaining_supply = ctx.share_mint_supply();
    let remaining_claim =
        ((remaining_shares as u128 * left as u128) / remaining_supply as u128) as u64;
    assert!(
        remaining_claim <= left,
        "the remaining holder's claim {remaining_claim} exceeds the {left} left"
    );
}

/// Accounting must stay consistent through the loss: `local_aum` tracks the
/// reserve balance at every step.
#[test]
fn accounting_stays_consistent_through_a_loss() {
    let each = MIN_FIRST;
    let (mut ctx, holders) = vault_after_50_percent_loss(each);

    assert_eq!(
        ctx.vault_state_data().local_aum,
        ctx.token_account_amount(&ctx.vault_token_pda),
        "local_aum must equal the reserve after the loss is booked"
    );

    for h in &holders {
        let shares = ctx.token_account_amount(&h.share_ata);
        if shares > 0 {
            // Must not be swallowed: reverting the pro-rata cap makes the second
            // holder's exit fail with NotEnoughLiquidity, and because a failed
            // transaction commits nothing, `local_aum == reserve` would still
            // hold below — so this test would pass against the very regression
            // the file exists to catch.
            ctx.redeem_as(h, shares)
                .expect("every holder must be able to exit after a loss");
        }
        assert_eq!(
            ctx.vault_state_data().local_aum,
            ctx.token_account_amount(&ctx.vault_token_pda),
            "local_aum must track the reserve through every redemption"
        );
    }
}

/// The mirror direction: money deposited into a vault below par must buy shares
/// worth what was paid, not fewer.
///
/// Without the pro-rata floor the offsets under-mint here, and the shortfall is
/// not dust: at this supply a deposit of `MIN_FIRST` into a vault carrying a 50%
/// loss was minted 1,500,000 shares instead of 2,000,000 and could redeem only
/// 857,142 of the 1,000,000 it paid — 14.3% handed to the incumbents on arrival.
/// This is a mid-range case, not the worst one: the shortfall grows as supply
/// falls (see the module docs). With the floor the round trip is exact, and the
/// assertions below allow only dust.
#[test]
fn depositing_after_a_loss_is_not_a_donation_to_incumbents() {
    let each = MIN_FIRST;
    let (mut ctx, holders) = vault_after_50_percent_loss(each);

    // What the incumbents could claim before the new money arrives.
    let supply_before = ctx.share_mint_supply();
    let reserve_before = ctx.token_account_amount(&ctx.vault_token_pda);
    let incumbent_shares: Vec<u64> = holders
        .iter()
        .map(|h| ctx.token_account_amount(&h.share_ata))
        .collect();
    let incumbent_claim_before: u64 = incumbent_shares
        .iter()
        .map(|s| ((*s as u128 * reserve_before as u128) / supply_before as u128) as u64)
        .sum();

    let c = ctx.new_depositor(each);
    ctx.deposit_as(&c, each).expect("deposit into a loss state");
    let minted = ctx.token_account_amount(&c.share_ata);
    assert!(minted > 0, "deposit minted no shares at all");

    // The new money must be worth what it paid, immediately.
    let before = ctx.token_account_amount(&c.deposit_ata);
    ctx.redeem_as(&c, minted).expect("exit straight back out");
    let received = ctx.token_account_amount(&c.deposit_ata) - before;
    assert!(
        received <= each,
        "extracted value: paid {each}, took {received} back out"
    );
    assert!(
        each - received <= 4,
        "deposited {each} but could only redeem {received} straight back — \
         the difference went to the incumbents"
    );

    // And the incumbents are no richer than they were.
    let supply_after = ctx.share_mint_supply();
    let reserve_after = ctx.token_account_amount(&ctx.vault_token_pda);
    let incumbent_claim_after: u64 = incumbent_shares
        .iter()
        .map(|s| ((*s as u128 * reserve_after as u128) / supply_after as u128) as u64)
        .sum();
    assert!(
        incumbent_claim_after <= incumbent_claim_before + 4,
        "incumbents' claim rose from {incumbent_claim_before} to {incumbent_claim_after} \
         on someone else's deposit"
    );
}

/// A vault that has lost everything while shares are still outstanding has no
/// share price. Depositing must be refused rather than priced against the ghost
/// shares alone — which would fund the whole reserve for a negligible stake.
/// The operator can recapitalise without minting, and deposits then work again.
#[test]
fn deposit_is_rejected_when_the_vault_has_lost_everything() {
    let each = MIN_FIRST;
    let mut ctx = VaultCtx::fresh();

    let a = ctx.new_depositor(each);
    ctx.deposit_as(&a, each).expect("first deposit");
    ctx.operator_withdraw(each).expect("operator deploys all");
    ctx.set_aum_limits(10_000, 10_000)
        .expect("admin widens the AUM window");
    ctx.operator_update_aum(0)
        .expect("operator reports a total loss");

    let state = ctx.vault_state_data();
    assert_eq!(state.total_assets().unwrap(), 0, "precondition: no assets");
    assert!(
        ctx.share_mint_supply() > 0,
        "precondition: shares outstanding"
    );

    let b = ctx.new_depositor(each);
    let err = ctx
        .deposit_as(&b, each)
        .expect_err("deposit into a priceless vault must be refused");
    assert_anchor_err(&err, ErrorCode::SharePriceUndefined);

    // Recapitalising mints nothing, so it cannot be used to dilute anyone; it
    // just restores a defined price.
    let supply_before = ctx.share_mint_supply();
    ctx.operator_deposit(each).expect("operator recapitalises");
    assert_eq!(
        ctx.share_mint_supply(),
        supply_before,
        "recapitalisation must not mint shares"
    );
    ctx.deposit_as(&b, each)
        .expect("deposits work again once the vault holds assets");
}

/// A larger vault should behave the same way — the cap is not specific to the
/// small-supply regime, it just matters most there.
#[test]
fn all_holders_can_exit_after_a_loss_at_larger_scale() {
    let each = MIN_FIRST * 1_000;
    let (mut ctx, holders) = vault_after_50_percent_loss(each);

    let reserve_before = ctx.token_account_amount(&ctx.vault_token_pda);
    let mut paid_out = 0u64;
    for (i, h) in holders.iter().enumerate() {
        let shares = ctx.token_account_amount(&h.share_ata);
        let before = ctx.token_account_amount(&h.deposit_ata);
        ctx.redeem_as(h, shares)
            .unwrap_or_else(|e| panic!("holder {i} could not exit: {e:?}"));
        // Delta, not the absolute balance: these holders deposited their whole
        // balance so it happens to be 0 beforehand, but that is a property of the
        // fixture, not of the assertion.
        paid_out += ctx.token_account_amount(&h.deposit_ata) - before;
    }
    assert!(
        paid_out <= reserve_before + holders.len() as u64,
        "paid out {paid_out} against a reserve of {reserve_before}"
    );
}
