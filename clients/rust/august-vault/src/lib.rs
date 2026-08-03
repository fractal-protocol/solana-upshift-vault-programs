//! Generated Rust client for the august-vault program.
//!
//! Source of truth: `target/idl/august_vault.json`. Regenerate via
//! `pnpm run generate-clients`. Do not edit `src/generated/` by hand.

mod generated;

pub use generated::{accounts::*, errors::*, instructions::*, programs::*};
pub use solana_pubkey::Pubkey;

// ---------------------------------------------------------------------------
// Hand-written extensions to the generated types.
//
// `src/generated/` is overwritten by `pnpm run generate-clients`, so anything
// that must survive regeneration lives here.
// ---------------------------------------------------------------------------

/// Default virtual-share offset, mirroring `EXTRA_SHARES` in the program.
///
/// Keep in step with `programs/august-vault/src/state/vault.rs`.
pub const EXTRA_SHARES: u128 = 1_000_000;

/// How far a vault's opening share supply must exceed its offset, mirroring
/// `MIN_SUPPLY_MULTIPLE` in the program.
pub const MIN_SUPPLY_MULTIPLE: u128 = 100;

impl VaultState {
    /// This vault's virtual-share offset, resolving the legacy zero.
    ///
    /// **Off-chain consumers must use this, not the raw `share_offset` field.**
    /// Vaults created before that field existed store 0, which is not a usable
    /// offset — both live mainnet vaults are in exactly that state. Pricing a
    /// deposit with a raw 0 collapses the formula to pure pro-rata, which agrees
    /// with the program only while the vault sits exactly at par and diverges the
    /// moment any AUM is reported. A quote computed that way, fed to
    /// `deposit_checked`, fails `SlippageExceeded` on every attempt.
    pub fn resolved_share_offset(&self) -> u128 {
        match self.share_offset {
            0 => EXTRA_SHARES,
            v => v as u128,
        }
    }

    /// Smallest first deposit this vault accepts, mirroring the program's
    /// `min_first_deposit_for`. Returns base units of the deposit mint.
    pub fn min_first_deposit(&self, decimals: u8) -> u64 {
        let by_decimals = match decimals {
            0..=3 => 1,
            d => 10_u64.saturating_pow(u32::from(d.saturating_sub(3))),
        };
        let by_offset = u64::try_from(MIN_SUPPLY_MULTIPLE * self.resolved_share_offset())
            .unwrap_or(u64::MAX);
        by_decimals.max(by_offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape both live mainnet vaults are in.
    ///
    /// The generated struct derives no `Default`, so build it field-by-field —
    /// which also means adding a field to `VaultState` makes this fail to compile
    /// rather than silently skipping the new field.
    #[test]
    fn a_zero_offset_resolves_to_the_default() {
        let zero = Pubkey::default();
        let mut vs = VaultState {
            discriminator: [0; 8],
            operator: zero,
            admin: zero,
            share_mint: zero,
            deposit_mint: zero,
            fee_recipient: zero,
            withdrawal_fee: 0,
            local_aum: 0,
            deployed_aum: 0,
            aum_increase_limit: 0,
            aum_decrease_limit: 0,
            pda_bump: [0; 1],
            vault_version: [0; 1],
            paused: false,
            share_offset: 0,
            padding: [0; 31],
        };
        assert_eq!(vs.resolved_share_offset(), EXTRA_SHARES);
        assert_eq!(vs.min_first_deposit(6), 100_000_000);

        vs.share_offset = 1_000;
        assert_eq!(vs.resolved_share_offset(), 1_000);
        assert_eq!(vs.min_first_deposit(6), 100_000);
    }
}
