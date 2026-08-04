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
mod tests;
