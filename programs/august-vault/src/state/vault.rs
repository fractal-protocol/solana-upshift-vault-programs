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

/// Basis-point denominator for the AUM change limits: 10 000 bps = 100%.
///
/// **Widen before multiplying.** The AUM guard scales both sides of its
/// comparison by this value, and doing so in `u64` overflows once
/// `deployed_aum` exceeds `u64::MAX / (BPS_DENOMINATOR + increase_limit)`.
/// `operator_update_aum` therefore casts to `u128` first. Note that the *safe*
/// failure mode of the old u64 form depended on the root manifest's
/// `[profile.release] overflow-checks = true`: with checks on it panicked
/// (aborting the transaction), but built without them the products would wrap
/// and a wrapped comparison could satisfy the guard, permitting an arbitrarily
/// large AUM change. The `u128` form removes that dependency entirely.
pub const BPS_DENOMINATOR: u32 = 10_000;

/// Virtual-share offset used by `shares_for_deposit` / `assets_for_redeem`.
///
/// The math treats every vault as if it had `EXTRA_SHARES` "ghost" shares
/// permanently outstanding. This defends against the OpenZeppelin ERC-4626
/// share-inflation attack: an attacker who is first to deposit a single unit
/// and then donates a large asset balance directly to the reserve would
/// otherwise be able to round subsequent depositors' shares down to zero.
/// See <https://docs.openzeppelin.com/contracts/5.x/erc4626#inflation-attack>.
///
/// **Why 10^6 and not 1.** The ghost shares act as a permanent co-holder that
/// cannot be burned. With an offset of 1, a sole holder who burns their position
/// down to a handful of shares out-holds that co-holder and captures most of the
/// value that later depositors forfeit to rounding; at 10^6 the co-holder
/// dominates any such position and the manoeuvre is loss-making. Share supply is
/// read from the SPL mint, and SPL Token lets any holder burn their own tokens,
/// so the vault cannot prevent supply from shrinking — it can only make shrinking
/// it unprofitable.
///
/// **This constant is now on-chain state for pre-existing vaults.** Every vault
/// created before `share_offset` existed stores 0 and resolves to this value, so
/// changing it re-prices those vaults on the next upgrade. Treat it as frozen for
/// the live deployment; per-vault tuning is what `share_offset` is for.
///
/// **And `min_first_deposit` must move with them.** The ghost co-holder's claim
/// is `EXTRA_SHARES / (supply + EXTRA_SHARES)`, so it is only negligible while
/// supply stays far above the offsets. `min_first_deposit` is what guarantees
/// that for a new vault — it is floored at `MIN_SUPPLY_MULTIPLE * EXTRA_SHARES`.
/// Retuning this constant without it lets the co-holder own most of a new vault
/// and absorb its early holders' appreciation; `first_depositor_keeps_their_
/// appreciation` fails if that ever drifts apart.
pub const EXTRA_SHARES: u128 = 1_000_000;

/// Bounds on a per-vault offset, enforced at `initialize`.
///
/// The lower bound keeps the burn manoeuvre loss-making — but only just: the
/// margin narrows sharply as the offset falls, and `MIN_SHARE_OFFSET` is where it
/// is still negative across the swept region rather than comfortably so. Pinned
/// by `burn_manoeuvre_is_loss_making_at_every_permitted_offset`. The upper bound keeps
/// the resulting `min_first_deposit` from pricing a vault out of existence and
/// keeps `u64` headroom. Offsets must be a power of ten so the relationship to
/// the mint's decimals stays legible.
pub const MIN_SHARE_OFFSET: u128 = 1_000;
pub const MAX_SHARE_OFFSET: u128 = 1_000_000;

/// Companion virtual-asset offset; see `EXTRA_SHARES`.
///
/// **No longer read by the program.** Both pricing functions now add a single
/// per-vault `offset` to the share and asset terms, so "the two offsets must be
/// equal" is structural rather than a rule to maintain. This constant survives
/// only as the asset-side name in test expectations, which are correct *because*
/// it equals `EXTRA_SHARES` — pinned below so that coincidence cannot drift into
/// a set of tests that silently assert nothing.
pub const VIRTUAL_ASSETS: u128 = 1_000_000;

const _: () = assert!(
    VIRTUAL_ASSETS == EXTRA_SHARES,
    "test expectations use VIRTUAL_ASSETS as the asset-side offset; it must equal \
     EXTRA_SHARES or those expectations stop matching the program"
);

/// How far the opening share supply must exceed the vault's offset.
///
/// The offset behaves like a co-holder that can never be burned or redeemed, so
/// it holds a permanent claim of `offset / (supply + offset)`. At 100x that is
/// under 1%, so an early holder keeps >99% of any appreciation. Raising this
/// multiplies the minimum deposit one-for-one while barely changing the burn
/// margin, so 100 is the point where a high-value mint stays launchable.
pub const MIN_SUPPLY_MULTIPLE: u128 = 100;

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
    /// This vault's virtual-share offset, in base units.
    ///
    /// Per-vault rather than global because the right value depends on what a
    /// base unit of the deposit mint is *worth*. The offset must dominate a
    /// 1-unit retained sliver (so it is an absolute count), while
    /// `MIN_SUPPLY_MULTIPLE * offset` is the minimum first deposit — whose cost
    /// is that count times the base-unit price. One global constant cannot serve
    /// both a 6-decimal dollar stablecoin and an 8-decimal asset worth ~$100k.
    ///
    /// **Zero means "not set" and maps to [`EXTRA_SHARES`].** Vaults created
    /// before this field existed have zeroed padding, so they read 0 — both live
    /// mainnet vaults are in that state. Never read this field directly: on-chain
    /// use `VaultState::share_offset()`, and off-chain use
    /// `resolved_share_offset()` from the generated Rust client, which mirrors it.
    /// A raw 0 collapses the pricing to pure pro-rata, which agrees with the
    /// program only while the vault sits exactly at par.
    pub share_offset: u64,
    /// Reserved. New fields must be carved **out of** this array so `LEN` stays
    /// 455, the size of the live mainnet vault accounts — enforced by the `const`
    /// assertion below the struct.
    ///
    /// **Declare them AFTER `share_offset`, never before it.** Inserting a field
    /// earlier shifts `share_offset` off byte 199, and every vault storing a
    /// non-default offset would then silently read 0 and fall back to the default.
    /// `share_offset_stays_at_its_byte_offset` fails if that happens.
    pub padding: [u64; 31],
}

/// **Compile-time layout guard.** Two live mainnet vaults are 455-byte accounts.
/// Growing `VaultState` past that makes every existing vault fail to deserialize
/// — user funds become unreachable without a migration. Anchor's `init` sizes new
/// accounts from `INIT_SPACE`, so a new field silently changes this number; the
/// shrink direction is silent too, because `try_deserialize` ignores trailing
/// bytes. Adding a field therefore requires removing the same number of bytes
/// from `padding`, and this assertion fails the build if it is forgotten.
const _: () = assert!(
    VaultState::LEN == 455,
    "VaultState::LEN must stay 455 — live mainnet accounts are this size"
);

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
        share_offset: u64,
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
        self.share_offset = share_offset;
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

    /// This vault's offset, resolving the legacy zero.
    ///
    /// Always use this rather than reading `share_offset` directly: a vault
    /// created before the field existed stores 0, and 0 is not a usable offset —
    /// it would disable the inflation and burn defences entirely.
    pub fn share_offset(&self) -> u128 {
        match self.share_offset {
            0 => EXTRA_SHARES,
            v => v as u128,
        }
    }

    // ---- pricing, bound to this vault's own offset ----
    //
    // The three `*_with_offset` / `*_for` associated functions below take the
    // offset as a plain parameter, which every on-chain call site would then
    // have to remember to fill with `vault_state.share_offset()`. That is a bug
    // class, not a hypothetical: replacing `vault_state.share_offset()` with
    // `EXTRA_SHARES` in both handlers once left the whole suite green, and two
    // bespoke discriminator tests had to be written to compensate.
    //
    // These wrappers make the mistake unrepresentable — the offset comes from
    // `self` and cannot be passed at all. **Handlers must use these.** The
    // explicit-offset forms remain for the unit sweeps and the fork reference,
    // which legitimately need to price against an arbitrary offset.

    /// [`Self::shares_for_deposit_with_offset`] at this vault's offset.
    pub fn shares_for_deposit(&self, supply: u64, total_assets: u64, amount: u64) -> Result<u64> {
        Self::shares_for_deposit_with_offset(supply, total_assets, amount, self.share_offset())
    }

    /// [`Self::assets_for_redeem_with_offset`] at this vault's offset.
    pub fn assets_for_redeem(&self, supply: u64, total_assets: u64, shares: u64) -> Result<u64> {
        Self::assets_for_redeem_with_offset(supply, total_assets, shares, self.share_offset())
    }

    /// [`Self::min_first_deposit_for`] at this vault's offset.
    pub fn min_first_deposit(&self, decimals: u8) -> u64 {
        Self::min_first_deposit_for(decimals, self.share_offset())
    }

    /// Smallest first deposit this vault may accept, given its offset.
    ///
    /// Both floors apply: the historical decimals floor, and
    /// `MIN_SUPPLY_MULTIPLE * offset` so the offset co-holder's permanent claim
    /// stays negligible. See [`MIN_SUPPLY_MULTIPLE`].
    ///
    /// **The offset floor is not optional.** The offset and this minimum must move
    /// together: a large offset without a correspondingly large floor lets the
    /// offset co-holder own most of a new vault and absorb the early holders'
    /// appreciation — a real loss to them, not a rounding artifact. Pinned by
    /// `first_depositor_keeps_their_appreciation`.
    ///
    /// **This is an amount, and the invariant is about supply.** The two coincide
    /// only when the vault opens empty, where minting is 1:1. They come apart
    /// when share supply is 0 while assets remain — reachable via the pro-rata
    /// cap's residual after the last holder exits above par, an external burn of
    /// the whole supply, or `operator_update_aum` raising the AUM at zero supply.
    /// There a deposit of exactly this amount mints
    /// `amount * offset / (total_assets + offset)`, which is less than the amount
    /// and can floor to zero. So the opening-supply invariant is enforced
    /// separately, on the minted shares, by [`Self::min_opening_supply`]; this
    /// function is the advertised amount and is exact only for an empty vault.
    ///
    /// Consequence worth knowing: at the default offset this floor is 10^8 base
    /// units, so mints with few decimals are effectively excluded (10^8 units of
    /// a 0-decimal token is not a realistic deposit). That is deliberate and
    /// fails closed — the offset cannot be made negligible for such a mint, so a
    /// vault on one would silently mistreat its depositors. A smaller
    /// `share_offset` is the supported way to make such a mint launchable.
    pub fn min_first_deposit_for(decimals: u8, offset: u128) -> u64 {
        let by_decimals = match decimals {
            0..=3 => 1,
            d => 10_u64.saturating_pow(d.saturating_sub(3) as u32),
        };
        let by_offset =
            u64::try_from(MIN_SUPPLY_MULTIPLE.saturating_mul(offset)).unwrap_or(u64::MAX);
        by_decimals.max(by_offset)
    }

    /// Smallest share supply a vault may open with.
    ///
    /// This is the invariant [`Self::min_first_deposit_for`] exists to serve,
    /// stated on the quantity it is actually about: the offset behaves like a
    /// co-holder holding `offset / (supply + offset)`, so supply must open far
    /// above the offset for an early holder to keep their appreciation. Checked
    /// against the *minted shares*, which is the same thing as the deposited
    /// amount for a vault opening empty and deliberately is not when assets are
    /// present at zero supply.
    ///
    /// A vault in that residual state therefore needs a proportionally larger
    /// deposit to reopen, and says so with `InsufficientAmount` rather than
    /// `ZeroAmount`. `operator_withdraw` alone does NOT restore a 1:1 reopen —
    /// it leaves `total_assets` unchanged; the residual tests show what does.
    pub fn min_opening_supply(&self) -> u64 {
        u64::try_from(MIN_SUPPLY_MULTIPLE.saturating_mul(self.share_offset())).unwrap_or(u64::MAX)
    }

    /// Whether `offset` may be stored on a new vault.
    ///
    /// Powers of ten only, within [`MIN_SHARE_OFFSET`]..=[`MAX_SHARE_OFFSET`].
    /// Restricting the shape keeps the relationship to the mint's decimals
    /// legible and stops a caller choosing a value that quietly disables the
    /// defences.
    pub fn is_valid_share_offset(offset: u128) -> bool {
        if !(MIN_SHARE_OFFSET..=MAX_SHARE_OFFSET).contains(&offset) {
            return false;
        }
        // `checked_mul`, not `saturating_mul`: saturation would stick at
        // `u128::MAX`, so `p == offset` would ACCEPT `u128::MAX` if the range
        // check above were ever loosened. This way the predicate is correct on
        // its own rather than only in combination with that check.
        let mut p = 1_u128;
        while p < offset {
            match p.checked_mul(10) {
                Some(next) => p = next,
                None => return false,
            }
        }
        p == offset
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
    /// Formula, where `offset` is this vault's [`VaultState::share_offset`]:
    ///   `floor(max( amount * (supply + offset) / (total_assets + offset),`
    ///   `           amount * supply / total_assets ))`.
    ///
    /// Above par the offset term is the larger and binds; below par
    /// (`total_assets < supply`, reachable after a reported loss) the pro-rata
    /// floor binds, so a depositor never hands value to incumbents on arrival.
    ///
    /// Note the pro-rata floor has **no upper bound**, where the offset-only
    /// formula was implicitly bounded. As `total_assets` approaches 1 against a
    /// large supply the mint count approaches `amount * supply`, so in a
    /// deep-loss state deposits eventually exceed `u64` and revert with
    /// `NumberOverflow` — deposits brick rather than misprice.
    ///
    /// That state is reachable by the operator alone, within the AUM bounds, and
    /// does not require admin action — see the internal security review for the
    /// mechanism. A vault in it is already worthless to its holders, and only
    /// `operator_deposit` can recapitalise it, so this is a documented
    /// consequence of an operator who has already destroyed the vault's value
    /// rather than a separately guarded
    /// case.
    /// Errors with `SharePriceUndefined` when `total_assets == 0` while shares are
    /// outstanding — that state has no share price.
    ///
    /// Uses `u128` intermediates because `amount * (supply + EXTRA_SHARES)`
    /// exceeds `u64` at realistic balances. Rounded **down**: the
    /// LSB-of-precision goes to the vault, not the depositor. Inverse of
    /// `assets_for_redeem`; together the rounding policy guarantees
    /// `redeem(deposit(x)) <= x`, pinned by `shares_for_deposit_rounds_down` /
    /// `assets_for_redeem_rounds_down` below and by the property suite in
    /// `integration-tests/tests/property_arithmetic.rs`.
    pub fn shares_for_deposit_with_offset(
        supply: u64,
        total_assets: u64,
        amount: u64,
        offset: u128,
    ) -> Result<u64> {
        // A vault holding nothing while shares are outstanding has no meaningful
        // share price: pro-rata is a division by zero, and the offset formula
        // would mint a token amount against the ghost shares alone, handing the
        // depositor a negligible stake in exchange for funding the entire
        // reserve. Refuse instead of mispricing. `operator_deposit` can
        // recapitalise without minting, after which deposits work again. The
        // first-ever deposit (supply == 0) is unaffected and still mints 1:1.
        require!(
            total_assets > 0 || supply == 0,
            ErrorCode::SharePriceUndefined
        );

        let shares_eff = (supply as u128)
            .checked_add(offset)
            .ok_or(ErrorCode::MathError)?;
        let assets_eff = (total_assets as u128)
            .checked_add(offset)
            .ok_or(ErrorCode::MathError)?;
        let with_offsets = (amount as u128)
            .checked_mul(shares_eff)
            .ok_or(ErrorCode::MathError)?
            .checked_div(assets_eff)
            .ok_or(ErrorCode::MathError)?;

        // Mirror of the cap in `assets_for_redeem`, and required for the same
        // reason: the offsets pull the price toward 1.0, so below par
        // (`total_assets < supply`) they *under*-mint, and a depositor would hand
        // part of their deposit straight to incumbents. At a supply equal to the
        // offsets, a deposit into a vault carrying a 50% loss lost 20% of its
        // value on arrival. Minting at least pro-rata removes that.
        //
        // Above par the offset value is the larger of the two and still wins, so
        // the inflation defence is unchanged — that defence works precisely by
        // minting *more* than pro-rata when `total_assets` has been inflated,
        // where pro-rata would round to zero.
        let shares = if total_assets == 0 {
            with_offsets
        } else {
            let pro_rata = (amount as u128)
                .checked_mul(supply as u128)
                .ok_or(ErrorCode::MathError)?
                .checked_div(total_assets as u128)
                .ok_or(ErrorCode::MathError)?;
            with_offsets.max(pro_rata)
        };

        u64::try_from(shares).map_err(|_| ErrorCode::NumberOverflow.into())
    }

    /// Underlying assets redeemed for `shares` burned.
    ///
    /// Formula, where `offset` is this vault's [`VaultState::share_offset`]:
    ///   `floor(min( shares * (total_assets + offset) / (supply + offset),`
    ///   `           shares * total_assets / supply ))`.
    ///
    /// The mirror of the floor in `shares_for_deposit`: above par the offset term
    /// is the smaller and binds, below par the pro-rata cap does, so the first
    /// redeemer cannot take more than its share and leave later holders short.
    /// `u128` intermediates; rounded **down** (favours the vault).
    pub fn assets_for_redeem_with_offset(
        supply: u64,
        total_assets: u64,
        shares: u64,
        offset: u128,
    ) -> Result<u64> {
        // Mirror of the guard in `shares_for_deposit`. A vault holding nothing
        // against outstanding shares has no price in this direction either: the
        // pro-rata cap collapses to zero, so `redeem` would revert with
        // `ZeroAmount` ("Amount must be > 0") even though the caller passed a
        // perfectly good share count. Say what is actually wrong instead. Both
        // paths still revert, so no previously-failing redeem now succeeds.
        require!(
            total_assets > 0 || supply == 0,
            ErrorCode::SharePriceUndefined
        );

        let shares_eff = (supply as u128)
            .checked_add(offset)
            .ok_or(ErrorCode::MathError)?;
        let assets_eff = (total_assets as u128)
            .checked_add(offset)
            .ok_or(ErrorCode::MathError)?;
        let with_offsets = (shares as u128)
            .checked_mul(assets_eff)
            .ok_or(ErrorCode::MathError)?
            .checked_div(shares_eff)
            .ok_or(ErrorCode::MathError)?;

        // Never pay more than the position's pro-rata share of the assets that
        // actually exist.
        //
        // The offsets pull the effective price toward 1.0. Above 1.0 that is
        // conservative — the offset formula pays *less* than pro-rata. Below 1.0,
        // reachable whenever an operator reports a loss, it pays *more*, and the
        // excess grows as supply shrinks relative to the offsets: at a supply
        // equal to the offsets a 50% loss overpays a half-position by 50%, and on
        // a mint whose minimum first deposit is far below the offsets it
        // approaches paying out the entire reserve. Whoever redeems first would
        // take more than their share and leave later holders short of liquidity.
        //
        // Capping at pro-rata removes that without weakening anything: wherever
        // the offsets matter as a defence — an inflated `total_assets`, or a
        // supply collapsed by external burns, both of which put the price above
        // 1.0 — the offset value is already the smaller of the two and still wins.
        let capped = if supply == 0 {
            // No shares outstanding, so pro-rata is undefined; a caller holding
            // no shares can only redeem zero anyway.
            with_offsets
        } else {
            let pro_rata = (shares as u128)
                .checked_mul(total_assets as u128)
                .ok_or(ErrorCode::MathError)?
                .checked_div(supply as u128)
                .ok_or(ErrorCode::MathError)?;
            with_offsets.min(pro_rata)
        };

        u64::try_from(capped).map_err(|_| ErrorCode::NumberOverflow.into())
    }

    // NOTE: there is deliberately no `min_first_deposit(decimals)` convenience
    // wrapper. One existed and immediately attracted a call site that held a
    // vault and should have used that vault's own offset — precisely the bug
    // per-vault offsets exist to prevent. Callers must name the offset they mean:
    // a vault's `share_offset()`, or `EXTRA_SHARES` for the default explicitly.
}

#[cfg(test)]
mod tests {
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
        let got =
            VaultState::assets_for_redeem_with_offset(supply, total_assets, shares, EXTRA_SHARES)
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
        let got =
            VaultState::assets_for_redeem_with_offset(supply, total_assets, shares, EXTRA_SHARES)
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
        let out =
            VaultState::assets_for_redeem_with_offset(supply, total_assets, supply, EXTRA_SHARES)
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
        let got =
            VaultState::assets_for_redeem_with_offset(supply, total_assets, shares, EXTRA_SHARES)
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
        let got =
            VaultState::shares_for_deposit_with_offset(supply, total_assets, amount, EXTRA_SHARES)
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
        let got =
            VaultState::assets_for_redeem_with_offset(supply, total_assets, shares, EXTRA_SHARES)
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
        let err = VaultState::assets_for_redeem_with_offset(0, u64::MAX, u64::MAX, EXTRA_SHARES)
            .unwrap_err();
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
        let out =
            VaultState::assets_for_redeem_with_offset(shares, reserve, shares, offset).unwrap();

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
                            let minted = VaultState::shares_for_deposit_with_offset(
                                supply, total, each, offset,
                            )
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
}
