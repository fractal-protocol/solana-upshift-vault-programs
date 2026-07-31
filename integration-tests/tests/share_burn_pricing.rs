//! Share supply is read from the SPL mint, and SPL Token lets any holder burn
//! their own tokens. The vault therefore cannot stop supply from shrinking
//! underneath it — the share math has to stay sound anyway.
//!
//! Shrinking supply without shrinking recorded assets raises the price of every
//! remaining share. A holder who burns most of their position while keeping a
//! sliver makes one share expensive, and deposits truncate down to whole shares,
//! so later depositors forfeit up to one share's worth of value each. The
//! question is whether the burner's retained sliver collects enough of that
//! forfeited value to come out ahead.
//!
//! It does not, because `EXTRA_SHARES` ghost shares act as a co-holder that
//! cannot be burned and that dominates any small retained position.
//!
//! **The parameters below are not guesses.** The grid was derived by sweeping
//! `keep` x `deposit size` x `depositor count` against a build with the offsets
//! at 1, and it deliberately covers the whole region that was profitable there —
//! so these tests fail if the offsets are ever reduced. Against the current
//! constants no configuration in the grid pays, by several orders of magnitude.
//! Exact figures are recorded in the internal security review rather than here.

use august_vault::errors::ErrorCode;
use august_vault::state::vault::FEE_RATE_DENOMINATOR_VALUE;
use integration_tests::harness::{assert_anchor_err, VaultCtx, DEPOSIT_DECIMALS};

/// 1 whole token at the harness mint's 9 decimals.
const ONE_TOKEN: u64 = 10u64.pow(DEPOSIT_DECIMALS as u32);

/// Replay the manoeuvre and return the burner's final token balance.
///
/// Shape: sole first depositor stakes `ONE_TOKEN`, burns all but `keep` shares,
/// `depositors` honest parties each add `deposit_each`, then the burner exits.
/// Profit means ending with more than `ONE_TOKEN`.
fn burn_then_exit(keep: u64, deposit_each: u64, depositors: usize) -> u64 {
    let mut ctx = VaultCtx::fresh();

    let attacker = ctx.new_depositor(ONE_TOKEN);
    ctx.deposit_as(&attacker, ONE_TOKEN)
        .expect("first deposit mints 1:1");
    let minted = ctx.token_account_amount(&attacker.share_ata);
    assert_eq!(minted, ONE_TOKEN, "first deposit should mint 1:1");
    assert!(
        minted > keep,
        "test parameters: cannot keep more than minted"
    );

    // Collapse supply, keeping a sliver — the point of the manoeuvre.
    ctx.burn_shares_as(&attacker, minted - keep);
    assert_eq!(ctx.share_mint_supply(), keep);

    // Count the honest deposits that actually land. Historically a deposit worth
    // less than one whole share was rejected outright (ZeroAmount) — a refusal to
    // trade at a bad price, not a loss — so failures were tolerated here. At the
    // current offsets no configuration in this file can hit that (the smallest
    // mints ~399,600 shares), so a failure now means something else broke.
    //
    // This must be counted rather than ignored: the profit assertions below are
    // one-sided (`proceeds < ONE_TOKEN`), so a change that made every honest
    // deposit fail would leave the attacker with nothing to extract and turn the
    // entire anti-burn suite into a vacuous pass.
    let mut landed = 0usize;
    for _ in 0..depositors {
        let d = ctx.new_depositor(deposit_each);
        if ctx.deposit_as(&d, deposit_each).is_ok() {
            landed += 1;
        }
    }
    // EVERY deposit must land, not merely one: at the current offsets none of
    // these configurations can be refused, so a weaker `landed > 0` would let 24
    // of 25 deposits start failing while the sweep still passed against a far
    // smaller attack than it claims to test.
    assert_eq!(
        landed, depositors,
        "only {landed} of {depositors} honest deposits landed (keep={keep} \
         each={deposit_each}) — the sweep would be testing a weaker attack \
         than it claims"
    );

    // The vault is solvent and unpaused and the attacker holds shares, so this
    // exit must succeed. Swallowing a failure here would return 0 and pass the
    // profit assertion with a 100% margin while proving only that redeem broke.
    let held = ctx.token_account_amount(&attacker.share_ata);
    assert!(held > 0, "burner should still hold the retained sliver");
    ctx.redeem_as(&attacker, held)
        .expect("burner's exit must succeed against a solvent vault");
    ctx.token_account_amount(&attacker.deposit_ata)
}

/// The single worst configuration found against the vulnerable build. Kept as a
/// fast, precise regression anchor.
#[test]
fn worst_known_configuration_is_loss_making() {
    let proceeds = burn_then_exit(4, ONE_TOKEN * 4 / 10, 25);
    assert!(
        proceeds < ONE_TOKEN,
        "burner recovered {proceeds} of {ONE_TOKEN} staked — this configuration \
         was the worst configuration before the ghost-share count was raised"
    );
}

/// Sweep the whole region that was profitable at offsets of 1: retained sliver,
/// deposit size, and depositor count. None of it may pay.
#[test]
fn burning_is_never_profitable_across_the_profitable_region() {
    let mut worst = (0u64, 0u64, 0u64, 0usize);
    for keep in [3u64, 4, 8] {
        for tenths in [2u64, 3, 4, 5] {
            let deposit_each = ONE_TOKEN * tenths / 10;
            for depositors in [2usize, 5, 10, 25] {
                let proceeds = burn_then_exit(keep, deposit_each, depositors);
                if proceeds > worst.0 {
                    worst = (proceeds, keep, tenths, depositors);
                }
                assert!(
                    proceeds < ONE_TOKEN,
                    "profitable: keep={keep} each=0.{tenths} token \
                     depositors={depositors} -> {proceeds} of {ONE_TOKEN}"
                );
            }
        }
    }
    // Report how much headroom is left, so a future change that erodes the
    // margin without crossing zero is still visible in the log.
    let shortfall = ONE_TOKEN - worst.0;
    println!(
        "  worst case: keep={} each=0.{} token depositors={} -> {} of {} \
         (short by {}, {:.1}%)",
        worst.1,
        worst.2,
        worst.3,
        worst.0,
        ONE_TOKEN,
        shortfall,
        shortfall as f64 / ONE_TOKEN as f64 * 100.0
    );
}

/// Keeping a very large or very small sliver must not help either — the two
/// ends of the trade-off (a big position limits the price move; a tiny one
/// forfeits more up front).
#[test]
fn extreme_retained_sizes_are_loss_making() {
    for keep in [1u64, 2, 16, 1_000, 1_000_000, 100_000_000] {
        let proceeds = burn_then_exit(keep, ONE_TOKEN * 4 / 10, 10);
        assert!(
            proceeds < ONE_TOKEN,
            "keeping {keep} shares recovered {proceeds} of {ONE_TOKEN} — profitable"
        );
    }
}

// ---- deposit_checked: caller-stated worst acceptable rate ----

/// The complement to the ghost-share defence: a depositor can refuse a bad rate
/// outright instead of relying on the rate being good. Here the rate has been
/// moved by a burn, and the depositor's bound catches it.
#[test]
fn deposit_checked_rejects_a_rate_moved_by_a_burn() {
    let mut ctx = VaultCtx::fresh();
    let attacker = ctx.new_depositor(ONE_TOKEN);
    ctx.deposit_as(&attacker, ONE_TOKEN).expect("seed deposit");

    // Quote the rate a depositor would see before the burn.
    let amount = ONE_TOKEN * 4 / 10;
    let quoted = august_vault::state::vault::VaultState::shares_for_deposit(
        ctx.share_mint_supply(),
        ctx.vault_state_data().total_assets().unwrap(),
        amount,
    )
    .unwrap();

    // The rate moves underneath them.
    ctx.burn_shares_as(&attacker, ONE_TOKEN - 4);

    let victim = ctx.new_depositor(amount);
    let err = ctx
        .deposit_checked_as(&victim, amount, quoted)
        .expect_err("a deposit below the stated minimum must revert");
    assert_anchor_err(&err, ErrorCode::SlippageExceeded);
    assert_eq!(
        ctx.token_account_amount(&victim.deposit_ata),
        amount,
        "a reverted deposit must leave the depositor's funds untouched"
    );
}

#[test]
fn deposit_checked_accepts_a_satisfied_bound() {
    let mut ctx = VaultCtx::fresh();
    let d = ctx.new_depositor(ONE_TOKEN);
    // First deposit mints 1:1, so exactly `amount` shares are expected.
    ctx.deposit_checked_as(&d, ONE_TOKEN, ONE_TOKEN)
        .expect("an exactly-met bound must be accepted");
    assert_eq!(ctx.token_account_amount(&d.share_ata), ONE_TOKEN);
}

/// The bound must be exact, not approximate.
///
/// `deposit_checked_accepts_a_satisfied_bound` pins the accept side at exactly
/// equal, so together these fix the comparison as `>=`. Without a one-above case
/// the only rejection test in this file uses a ~1000x gap, which any tolerance
/// regression (`shares + 1 >= min`, or a basis-point slack) would still pass.
#[test]
fn deposit_checked_rejects_one_share_above_the_mint() {
    let mut ctx = VaultCtx::fresh();
    let d = ctx.new_depositor(ONE_TOKEN);
    // First deposit mints 1:1, so exactly ONE_TOKEN shares are available.
    let err = ctx
        .deposit_checked_as(&d, ONE_TOKEN, ONE_TOKEN + 1)
        .expect_err("a bound one share above what is minted must revert");
    assert_anchor_err(&err, ErrorCode::SlippageExceeded);
    assert_eq!(
        ctx.token_account_amount(&d.deposit_ata),
        ONE_TOKEN,
        "a reverted deposit must leave the depositor's funds untouched"
    );
}

/// A deposit that rounds to zero shares reports which check actually objected.
///
/// Both `require!`s are violated when the mint rounds to zero against a non-zero
/// bound, so the order decides the error code. The slippage check runs first, so
/// an integrator sees `SlippageExceeded` — "the rate was worse than you allowed"
/// — rather than `ZeroAmount` ("Amount must be > 0"), which is actively
/// misleading when they passed a perfectly good non-zero amount. With no bound,
/// `ZeroAmount` is still the answer.
#[test]
fn zero_share_deposit_reports_slippage_when_a_bound_was_set() {
    // Any share price above 1.0 makes a 1-unit deposit round to zero, in both the
    // offset term and the pro-rata floor. Doubling the AUM is the largest single
    // step `set_aum_limits` permits (limits are capped at BPS_DENOMINATOR).
    let mut ctx = VaultCtx::fresh();
    let seed = ctx.new_depositor(ONE_TOKEN);
    ctx.deposit_as(&seed, ONE_TOKEN).expect("seed deposit");
    ctx.operator_withdraw(ONE_TOKEN).expect("deploy capital");
    ctx.set_aum_limits(10_000, 10_000)
        .expect("widen the window");
    ctx.operator_update_aum(ONE_TOKEN * 2)
        .expect("report a gain, taking the price to 2.0");

    let d = ctx.new_depositor(ONE_TOKEN);
    let minted = august_vault::state::vault::VaultState::shares_for_deposit(
        ctx.share_mint_supply(),
        ctx.vault_state_data().total_assets().unwrap(),
        1,
    )
    .unwrap();
    assert_eq!(minted, 0, "test setup: a 1-unit deposit must mint nothing");

    let err = ctx
        .deposit_checked_as(&d, 1, 5)
        .expect_err("zero shares cannot satisfy a bound of 5");
    assert_anchor_err(&err, ErrorCode::SlippageExceeded);

    let err = ctx
        .deposit_checked_as(&d, 1, 0)
        .expect_err("zero shares is still a zero-share deposit");
    assert_anchor_err(&err, ErrorCode::ZeroAmount);
}

/// `min_shares_out = 0` must behave exactly like `deposit`, so the additive
/// instruction is a strict superset rather than a subtly different path.
#[test]
fn deposit_checked_with_zero_bound_matches_deposit() {
    let plain = {
        let mut ctx = VaultCtx::fresh();
        let d = ctx.new_depositor(ONE_TOKEN);
        ctx.deposit_as(&d, ONE_TOKEN / 3).expect("deposit");
        ctx.token_account_amount(&d.share_ata)
    };
    let checked = {
        let mut ctx = VaultCtx::fresh();
        let d = ctx.new_depositor(ONE_TOKEN);
        ctx.deposit_checked_as(&d, ONE_TOKEN / 3, 0)
            .expect("deposit_checked with no bound");
        ctx.token_account_amount(&d.share_ata)
    };
    assert_eq!(plain, checked, "the two paths must mint identically");
}

// ---- redeem_checked: the same bound on the way out ----

/// The redemption-side twin of `deposit_checked_rejects_a_rate_moved_by_a_burn`.
///
/// A holder quotes an exit and the operator lowers the reported AUM before the
/// transaction lands. Without a bound the shares burn at the new, lower price
/// and the holder simply receives less than quoted, with no way to say "revert
/// instead" — `set_withdrawal_fee`'s own doc comment notes the design already
/// assumes the operator moves the price.
#[test]
fn redeem_checked_rejects_a_payout_moved_by_an_aum_drop() {
    let mut ctx = VaultCtx::fresh();
    let holder = ctx.new_depositor(ONE_TOKEN);
    ctx.deposit_as(&holder, ONE_TOKEN).expect("seed deposit");
    ctx.operator_withdraw(ONE_TOKEN).expect("deploy capital");

    // Quote the exit at the current price...
    let shares = ONE_TOKEN / 2;
    let quoted = august_vault::state::vault::VaultState::assets_for_redeem(
        ctx.share_mint_supply(),
        ctx.vault_state_data().total_assets().unwrap(),
        shares,
    )
    .unwrap();

    // ...then the operator reports a loss, and the price moves underneath it.
    ctx.set_aum_limits(10_000, 10_000)
        .expect("widen the window");
    ctx.operator_update_aum(ONE_TOKEN / 2)
        .expect("report a 50% loss");
    ctx.operator_deposit(ONE_TOKEN / 2)
        .expect("return the remaining capital so liquidity is not the binding constraint");

    let err = ctx
        .redeem_checked_as(&holder, shares, quoted)
        .expect_err("a payout below the stated minimum must revert");
    assert_anchor_err(&err, ErrorCode::SlippageExceeded);
    assert_eq!(
        ctx.token_account_amount(&holder.share_ata),
        ONE_TOKEN,
        "a reverted redemption must leave the holder's shares untouched"
    );
}

#[test]
fn redeem_checked_accepts_a_satisfied_bound() {
    let mut ctx = VaultCtx::fresh();
    let holder = ctx.new_depositor(ONE_TOKEN);
    ctx.deposit_as(&holder, ONE_TOKEN).expect("seed deposit");

    // At par the exit is 1:1, so exactly `shares` assets come back.
    let shares = ONE_TOKEN / 2;
    ctx.redeem_checked_as(&holder, shares, shares)
        .expect("an exactly-met bound must be accepted");
    assert_eq!(ctx.token_account_amount(&holder.deposit_ata), shares);
}

/// Fixes the comparison as `>=` rather than `>`, and rules out any tolerance
/// slack — the paired accept case above sits at exactly equal.
#[test]
fn redeem_checked_rejects_one_unit_above_the_payout() {
    let mut ctx = VaultCtx::fresh();
    let holder = ctx.new_depositor(ONE_TOKEN);
    ctx.deposit_as(&holder, ONE_TOKEN).expect("seed deposit");

    let shares = ONE_TOKEN / 2;
    let err = ctx
        .redeem_checked_as(&holder, shares, shares + 1)
        .expect_err("a bound one unit above the payout must revert");
    assert_anchor_err(&err, ErrorCode::SlippageExceeded);
    assert_eq!(
        ctx.token_account_amount(&holder.share_ata),
        ONE_TOKEN,
        "a reverted redemption must leave the holder's shares untouched"
    );
}

/// The bound is on what the caller **receives**, not on the gross redemption
/// value. Bounding the gross figure would let a fee increase between quote and
/// execution take the difference while the bound still passed — and the fee is
/// admin-settable at any time.
#[test]
fn redeem_checked_bound_is_net_of_the_withdrawal_fee() {
    let mut ctx = VaultCtx::fresh();
    let holder = ctx.new_depositor(ONE_TOKEN);
    ctx.deposit_as(&holder, ONE_TOKEN).expect("seed deposit");

    // 1% of the gross redemption goes to the fee recipient.
    let fee_bps = FEE_RATE_DENOMINATOR_VALUE / 100;
    ctx.set_withdrawal_fee(fee_bps).expect("set a 1% fee");

    let shares = ONE_TOKEN / 2;
    let gross = shares; // at par
    let fee = gross / 100;
    let net = gross - fee;

    // A bound at the gross figure must fail: that is not what arrives.
    let err = ctx
        .redeem_checked_as(&holder, shares, gross)
        .expect_err("the gross figure is not what the caller receives");
    assert_anchor_err(&err, ErrorCode::SlippageExceeded);

    // A bound at the net figure must pass, exactly.
    ctx.redeem_checked_as(&holder, shares, net)
        .expect("the net payout must satisfy a bound set at the net payout");
    assert_eq!(ctx.token_account_amount(&holder.deposit_ata), net);
}

/// `min_assets_out = 0` must behave exactly like `redeem`, so the additive
/// instruction is a strict superset rather than a subtly different path.
#[test]
fn redeem_checked_with_zero_bound_matches_redeem() {
    let exit = |checked: bool| {
        let mut ctx = VaultCtx::fresh();
        let holder = ctx.new_depositor(ONE_TOKEN);
        ctx.deposit_as(&holder, ONE_TOKEN).expect("seed deposit");
        let shares = ONE_TOKEN / 3;
        if checked {
            ctx.redeem_checked_as(&holder, shares, 0)
                .expect("redeem_checked with no bound");
        } else {
            ctx.redeem_as(&holder, shares).expect("redeem");
        }
        ctx.token_account_amount(&holder.deposit_ata)
    };
    assert_eq!(
        exit(false),
        exit(true),
        "the two paths must pay out identically"
    );
}

/// An external burn must never make the vault's own accounting inconsistent: it
/// reduces supply, and `local_aum` must continue to match the reserve balance.
#[test]
fn external_burn_leaves_vault_accounting_intact() {
    let mut ctx = VaultCtx::fresh();
    let holder = ctx.new_depositor(ONE_TOKEN);
    ctx.deposit_as(&holder, ONE_TOKEN).expect("deposit");

    let before = ctx.vault_state_data();
    ctx.burn_shares_as(&holder, ONE_TOKEN / 2);
    let after = ctx.vault_state_data();

    assert_eq!(
        before.local_aum, after.local_aum,
        "a burn must not change recorded AUM"
    );
    assert_eq!(
        after.local_aum,
        ctx.token_account_amount(&ctx.vault_token_pda),
        "local_aum must still equal the reserve balance"
    );
    assert_eq!(
        ctx.share_mint_supply(),
        ctx.token_account_amount(&holder.share_ata),
        "supply must still equal the sole holder's balance"
    );

    // And the vault stays usable afterwards.
    let next = ctx.new_depositor(ONE_TOKEN);
    ctx.deposit_as(&next, ONE_TOKEN)
        .expect("the vault must remain usable after an external burn");
}
