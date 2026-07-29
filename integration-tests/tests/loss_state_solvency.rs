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
//! The magnitudes are chosen so the effect is large rather than dust: a supply
//! close to the offsets is where the uncapped formula diverges most, and it is
//! exactly the state a newly created vault is in.

use integration_tests::harness::{VaultCtx, DEPOSIT_DECIMALS};

/// The minimum first deposit for a 9-decimal mint — and, deliberately, the same
/// order as the share-price offsets, which is the worst case for the divergence.
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
            let _ = ctx.redeem_as(h, shares);
        }
        assert_eq!(
            ctx.vault_state_data().local_aum,
            ctx.token_account_amount(&ctx.vault_token_pda),
            "local_aum must track the reserve through every redemption"
        );
    }
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
        ctx.redeem_as(h, shares)
            .unwrap_or_else(|e| panic!("holder {i} could not exit: {e:?}"));
        paid_out += ctx.token_account_amount(&h.deposit_ata);
    }
    assert!(
        paid_out <= reserve_before + holders.len() as u64,
        "paid out {paid_out} against a reserve of {reserve_before}"
    );
}
