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
//! **The parameters below are not guesses.** They were found by sweeping
//! `keep` x `deposit size` x `depositor count` against a build with the offsets
//! at 1, which turned up 30 profitable configurations — the worst being
//! `keep = 4`, deposits of 0.4 token and 25 depositors, returning **+46.7%** on
//! the burner's stake. The grid here covers that whole profitable region, so
//! these tests fail if the offsets are ever reduced.

use august_vault::errors::ErrorCode;
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

    for _ in 0..depositors {
        let d = ctx.new_depositor(deposit_each);
        // A deposit worth less than one whole share is rejected outright
        // (ZeroAmount) — a refusal to trade at a bad price, not a loss. Only
        // completed deposits can be extracted from.
        let _ = ctx.deposit_as(&d, deposit_each);
    }

    let held = ctx.token_account_amount(&attacker.share_ata);
    if held > 0 {
        let _ = ctx.redeem_as(&attacker, held);
    }
    ctx.token_account_amount(&attacker.deposit_ata)
}

/// The single worst configuration found against the vulnerable build (+46.7%).
/// Kept as a fast, precise regression anchor.
#[test]
fn worst_known_configuration_is_loss_making() {
    let proceeds = burn_then_exit(4, ONE_TOKEN * 4 / 10, 25);
    assert!(
        proceeds < ONE_TOKEN,
        "burner recovered {proceeds} of {ONE_TOKEN} staked — this configuration \
         returned +46.7% before the ghost-share count was raised"
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
