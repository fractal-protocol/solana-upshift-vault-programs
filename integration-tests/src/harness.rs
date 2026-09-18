//! Test harness: fresh LiteSVM + initialized vault + helper functions for
//! every instruction the pilot tests need.

use anchor_lang::{
    AccountDeserialize, AnchorDeserialize, Discriminator, InstructionData, ToAccountMetas,
};
use august_vault::{
    accounts as ix_accounts,
    errors::ErrorCode,
    instruction as ix_data,
    state::config::PROGRAM_CONFIG_SEED,
    state::nominated_admin::NOMINATED_ADMIN_PDA_SEED,
    state::vault::{
        withdrawal_queue_pda, FEE_RATE_DENOMINATOR_VALUE, SHARE_MINT_SEED, VAULT_STATE_SEED,
        VAULT_TOKEN_SEED,
    },
};
use august_withdrawal_queue::{
    accounts as q_accounts, instruction as q_ix,
    state::{WithdrawalQueue, WithdrawalRequest, WITHDRAWAL_REQUEST_SEED},
};
use litesvm::{types::FailedTransactionMetadata, LiteSVM};
// `solana_sdk::bpf_loader_upgradeable` is deprecated in favour of the
// `solana-loader-v3-interface` crate. We keep it rather than add a dependency
// for two constants used only by the ProgramData test fixture below.
#[allow(deprecated)]
use solana_sdk::bpf_loader_upgradeable::{self, UpgradeableLoaderState};
use solana_sdk::{
    account::Account as SolanaAccount,
    instruction::{Instruction, InstructionError},
    program_pack::Pack,
    pubkey::Pubkey,
    rent::Rent,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::{Transaction, TransactionError},
};
use spl_associated_token_account::{
    get_associated_token_address_with_program_id, instruction::create_associated_token_account,
};
use spl_token::state::{Account as SplAccount, Mint as SplMint};

pub const VAULT_VERSION: u8 = 0;
/// Unix time every fresh SVM starts at. LiteSVM's clock begins at 0, where a
/// forgotten `eligible_at` (still 0) and a correct one under a zero cooldown are
/// the same byte; a real epoch keeps that class of bug visible.
pub const HARNESS_EPOCH: i64 = 1_790_000_000;
/// Offset the harness vaults are created with. Uses the program's own default so
/// the suite exercises the value real vaults get unless a test says otherwise.
pub const HARNESS_SHARE_OFFSET: u128 = august_vault::state::vault::EXTRA_SHARES;

/// The same value as the `u64` the instruction actually takes.
pub const HARNESS_SHARE_OFFSET_U64: u64 = HARNESS_SHARE_OFFSET as u64;

/// The program's minimum first deposit for a harness vault.
///
/// Derived, never hardcoded: the floor is tied to the vault's offset, so a
/// literal here would silently drop below the program's floor the next time
/// either is retuned and every affected test would fail on `InsufficientAmount`
/// for reasons unrelated to what it is testing.
pub fn harness_min_first_deposit() -> u64 {
    august_vault::state::vault::VaultState::min_first_deposit_for(
        DEPOSIT_DECIMALS,
        HARNESS_SHARE_OFFSET,
    )
}
pub const DEPOSIT_DECIMALS: u8 = 9;

/// First 6000 user-facing Anchor error codes are reserved; user variants start
/// at 6000 and are assigned in declaration order. See
/// <https://docs.rs/anchor-lang/latest/anchor_lang/error/constant.ERROR_CODE_OFFSET.html>.
const ANCHOR_USER_ERROR_OFFSET: u32 = 6000;

/// Which SPL token program the vault is built against. Token-2022 exercises
/// the `InterfaceAccount` code path; legacy SPL exercises the older path.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TokenProgramKind {
    Spl,
    Token2022,
}

impl TokenProgramKind {
    pub fn id(self) -> Pubkey {
        match self {
            Self::Spl => spl_token::ID,
            Self::Token2022 => spl_token_2022::ID,
        }
    }
}

/// Token-2022 mint extensions the harness can create a deposit mint with. One
/// the queue allows and one it refuses, so the allow-list can be tested from
/// both sides.
#[derive(Clone, Copy, Debug)]
pub enum MintExtension {
    MetadataPointer,
    DefaultAccountStateFrozen,
}

/// A live vault + every key the tests need to interact with it.
pub struct VaultCtx {
    pub svm: LiteSVM,
    /// Token program the vault was initialized against. **Crate-private**: the
    /// four ATA fields were derived from this value at construction time, so
    /// mutating it post-`fresh_*` would silently desync the derivations from
    /// the program ID passed to instruction CPIs.
    pub(crate) token_program: TokenProgramKind,
    pub payer: Keypair,
    /// The `ProgramConfig` authority — the only key allowed to call
    /// `initialize`. Tests that need an unauthorized creator should use
    /// [`Self::new_funded_keypair`] instead.
    pub protocol_authority: Keypair,
    pub admin: Keypair,
    pub operator: Keypair,
    pub fee_recipient: Keypair,
    pub user: Keypair,
    pub deposit_mint: Pubkey,
    pub vault_state: Pubkey,
    pub share_mint: Pubkey,
    pub vault_token_pda: Pubkey,
    pub user_deposit_ata: Pubkey,
    pub user_share_ata: Pubkey,
    pub operator_deposit_ata: Pubkey,
    pub fee_recipient_deposit_ata: Pubkey,
}

impl VaultCtx {
    /// Fresh vault using the legacy SPL Token program.
    pub fn fresh() -> Self {
        Self::fresh_with_token_program(TokenProgramKind::Spl)
    }

    /// Fresh vault using Token-2022. Same vault logic, exercises the
    /// `InterfaceAccount` paths and the alternate token-program CPIs.
    pub fn fresh_token_2022() -> Self {
        Self::fresh_with_token_program(TokenProgramKind::Token2022)
    }

    /// Fresh Token-2022 vault whose deposit mint carries `extensions`. Nothing
    /// in the vault itself minds them; this exists so the queue's deposit-mint
    /// allow-list can be exercised against real mint bytes.
    pub fn fresh_token_2022_with_extensions(extensions: &[MintExtension]) -> Self {
        Self::fresh_inner(TokenProgramKind::Token2022, extensions)
    }

    fn fresh_with_token_program(token_program: TokenProgramKind) -> Self {
        Self::fresh_inner(token_program, &[])
    }

    fn fresh_inner(token_program: TokenProgramKind, extensions: &[MintExtension]) -> Self {
        let mut svm = LiteSVM::new();
        let mut clock: solana_sdk::clock::Clock = svm.get_sysvar();
        clock.unix_timestamp = HARNESS_EPOCH;
        svm.set_sysvar(&clock);

        svm.add_program(
            august_vault::ID,
            include_bytes!(concat!(env!("OUT_DIR"), "/august_vault.so")),
        )
        .expect("load august_vault.so — built by build.rs into OUT_DIR");
        // Loaded alongside the vault so every suite built on this harness carries
        // the two-program wiring, not only the ones that will drive the queue.
        // `mainnet_fork_compat.rs` builds its own SVM and loads the vault alone;
        // `embedded_artifacts.rs` covers the queue artifact on its own.
        svm.add_program(
            august_withdrawal_queue::ID,
            include_bytes!(concat!(env!("OUT_DIR"), "/august_withdrawal_queue.so")),
        )
        .expect("load august_withdrawal_queue.so — built by build.rs into OUT_DIR");

        let payer = airdrop_keypair(&mut svm, 100_000_000_000);
        // Vault creation is gated on the ProgramConfig authority, and the config
        // itself can only be bootstrapped by the program's upgrade authority.
        // LiteSVM loads programs under the non-upgradeable loader, so there is
        // no real ProgramData account — we install one naming this keypair.
        let protocol_authority = airdrop_keypair(&mut svm, 10_000_000_000);
        install_program_data(&mut svm, &protocol_authority.pubkey());
        initialize_program_config(&mut svm, &protocol_authority);
        let admin = airdrop_keypair(&mut svm, 1_000_000_000);
        let operator = airdrop_keypair(&mut svm, 1_000_000_000);
        let fee_recipient = airdrop_keypair(&mut svm, 1_000_000_000);
        let user = airdrop_keypair(&mut svm, 1_000_000_000);

        let deposit_mint_kp = Keypair::new();
        if extensions.is_empty() {
            create_mint(
                &mut svm,
                &payer,
                &deposit_mint_kp,
                &payer.pubkey(),
                DEPOSIT_DECIMALS,
                token_program,
            );
        } else {
            assert!(
                matches!(token_program, TokenProgramKind::Token2022),
                "mint extensions exist only on Token-2022"
            );
            create_mint_2022_with_extensions(
                &mut svm,
                &payer,
                &deposit_mint_kp,
                &payer.pubkey(),
                DEPOSIT_DECIMALS,
                extensions,
            );
        }
        let deposit_mint = deposit_mint_kp.pubkey();

        let (vault_state, _) = derive_vault_state(&deposit_mint, VAULT_VERSION);
        let (share_mint, _) = derive_share_mint(&deposit_mint, VAULT_VERSION);
        let (vault_token_pda, _) = derive_vault_token_pda(&deposit_mint, VAULT_VERSION);

        // Initialize the vault as the protocol authority.
        let init_ix = initialize_ix(
            deposit_mint,
            VAULT_VERSION,
            &protocol_authority.pubkey(),
            &protocol_authority.pubkey(),
            admin.pubkey(),
            operator.pubkey(),
            fee_recipient.pubkey(),
            token_program,
            HARNESS_SHARE_OFFSET_U64,
        );
        send_tx(
            &mut svm,
            &protocol_authority,
            &[init_ix],
            &[&protocol_authority],
        )
        .expect("initialize vault");

        let user_deposit_ata = create_ata(
            &mut svm,
            &payer,
            &user.pubkey(),
            &deposit_mint,
            token_program,
        );
        let user_share_ata =
            create_ata(&mut svm, &payer, &user.pubkey(), &share_mint, token_program);
        let operator_deposit_ata = create_ata(
            &mut svm,
            &payer,
            &operator.pubkey(),
            &deposit_mint,
            token_program,
        );
        let fee_recipient_deposit_ata = create_ata(
            &mut svm,
            &payer,
            &fee_recipient.pubkey(),
            &deposit_mint,
            token_program,
        );

        Self {
            svm,
            token_program,
            payer,
            protocol_authority,
            admin,
            operator,
            fee_recipient,
            user,
            deposit_mint,
            vault_state,
            share_mint,
            vault_token_pda,
            user_deposit_ata,
            user_share_ata,
            operator_deposit_ata,
            fee_recipient_deposit_ata,
        }
    }

    pub fn mint_to_user(&mut self, amount: u64) {
        let ix = mint_to_ix(
            self.token_program,
            &self.deposit_mint,
            &self.user_deposit_ata,
            &self.payer.pubkey(),
            amount,
        );
        let payer = self.payer.insecure_clone();
        send_tx(&mut self.svm, &payer, &[ix], &[&payer]).expect("mint to user");
    }

    pub fn deposit(&mut self, amount: u64) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::Deposit {
                vault_state: self.vault_state,
                vault_token_ata: self.vault_token_pda,
                sender_token_account: self.user_deposit_ata,
                sender_share_account: self.user_share_ata,
                share_mint: self.share_mint,
                deposit_mint: self.deposit_mint,
                signer: self.user.pubkey(),
                token_program: self.token_program.id(),
            }
            .to_account_metas(None),
            data: ix_data::Deposit { amount }.data(),
        };
        let user = self.user.insecure_clone();
        send_tx(&mut self.svm, &user, &[ix], &[&user]).map(|_| ())
    }

    pub fn operator_withdraw(&mut self, amount: u64) -> Result<(), FailedTransactionMetadata> {
        let operator = self.operator.insecure_clone();
        let operator_ata = self.operator_deposit_ata;
        self.operator_withdraw_as(&operator, operator_ata, amount)
    }

    /// `operator_withdraw` signed by an arbitrary keypair. Negative tests pass
    /// a non-operator signer (plus that signer's own deposit-mint ATA, so the
    /// `associated_token::authority` constraint resolves and the access-control
    /// constraint is what actually fires).
    pub fn operator_withdraw_as(
        &mut self,
        signer: &Keypair,
        operator_token_account: Pubkey,
        amount: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        self.operator_withdraw_with(signer, operator_token_account, None, amount)
    }

    /// `operator_withdraw` to a registered destination, passing both its ATA
    /// and its registry PDA.
    pub fn operator_withdraw_to(
        &mut self,
        sub: &Subaccount,
        amount: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        let operator = self.operator.insecure_clone();
        let (ata, pda) = (sub.deposit_ata, sub.pda);
        self.operator_withdraw_with(&operator, ata, Some(pda), amount)
    }

    /// `operator_withdraw` built with the **pre-registry account list** — six
    /// accounts, no optional slot at all. What an un-updated client sends.
    pub fn operator_withdraw_legacy_layout(
        &mut self,
        amount: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        let operator = self.operator.insecure_clone();
        let mut metas = ix_accounts::OperatorWithdraw {
            vault_state: self.vault_state,
            vault_deposit_ata: self.vault_token_pda,
            operator_token_account: self.operator_deposit_ata,
            subaccount: None,
            deposit_mint: self.deposit_mint,
            operator: operator.pubkey(),
            token_program: self.token_program.id(),
        }
        .to_account_metas(None);
        // Drop the trailing optional sentinel entirely.
        metas.pop();
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: metas,
            data: ix_data::OperatorWithdraw { amount }.data(),
        };
        self.send_as(&operator, ix)
    }

    /// The general form: the ATA and the registry PDA are named independently,
    /// so a test can omit the PDA or pass a mismatched one.
    pub fn operator_withdraw_with(
        &mut self,
        signer: &Keypair,
        operator_token_account: Pubkey,
        subaccount: Option<Pubkey>,
        amount: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::OperatorWithdraw {
                vault_state: self.vault_state,
                vault_deposit_ata: self.vault_token_pda,
                operator_token_account,
                subaccount,
                deposit_mint: self.deposit_mint,
                operator: signer.pubkey(),
                token_program: self.token_program.id(),
            }
            .to_account_metas(None),
            data: ix_data::OperatorWithdraw { amount }.data(),
        };
        self.send_as(signer, ix)
    }

    pub fn operator_deposit(&mut self, amount: u64) -> Result<(), FailedTransactionMetadata> {
        let operator = self.operator.insecure_clone();
        let operator_ata = self.operator_deposit_ata;
        self.operator_deposit_as(&operator, operator_ata, amount)
    }

    /// `operator_deposit` signed by an arbitrary keypair; see
    /// [`Self::operator_withdraw_as`] for the account-selection rationale.
    pub fn operator_deposit_as(
        &mut self,
        signer: &Keypair,
        operator_token_account: Pubkey,
        amount: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        self.operator_deposit_with(signer, operator_token_account, None, amount)
    }

    /// `operator_deposit` from a registered destination.
    pub fn operator_deposit_from(
        &mut self,
        sub: &Subaccount,
        amount: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        let operator = self.operator.insecure_clone();
        let (ata, pda) = (sub.deposit_ata, sub.pda);
        self.operator_deposit_with(&operator, ata, Some(pda), amount)
    }

    pub fn operator_deposit_with(
        &mut self,
        signer: &Keypair,
        operator_token_account: Pubkey,
        subaccount: Option<Pubkey>,
        amount: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::OperatorDeposit {
                vault_state: self.vault_state,
                vault_deposit_ata: self.vault_token_pda,
                operator_token_account,
                subaccount,
                deposit_mint: self.deposit_mint,
                operator: signer.pubkey(),
                token_program: self.token_program.id(),
            }
            .to_account_metas(None),
            data: ix_data::OperatorDeposit { amount }.data(),
        };
        self.send_as(signer, ix)
    }

    /// `operator_update_aum` signed by the configured operator.
    pub fn operator_update_aum(&mut self, new_aum: u64) -> Result<(), FailedTransactionMetadata> {
        let operator = self.operator.insecure_clone();
        self.operator_update_aum_as(&operator, new_aum)
    }

    /// `operator_update_aum` signed by an arbitrary keypair (non-operator
    /// signers must be rejected with `NotOperator`).
    pub fn operator_update_aum_as(
        &mut self,
        signer: &Keypair,
        new_aum: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::OperatorUpdateAum {
                vault_state: self.vault_state,
                deposit_mint: self.deposit_mint,
                operator: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix_data::OperatorUpdateAum { new_aum }.data(),
        };
        self.send_as(signer, ix)
    }

    pub fn redeem(&mut self, shares: u64) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::Redeem {
                vault_state: self.vault_state,
                vault_deposit_ata: self.vault_token_pda,
                sender_token_account: self.user_deposit_ata,
                sender_share_account: self.user_share_ata,
                fee_recipient_account: self.fee_recipient_deposit_ata,
                share_mint: self.share_mint,
                deposit_mint: self.deposit_mint,
                signer: self.user.pubkey(),
                token_program: self.token_program.id(),
            }
            .to_account_metas(None),
            data: ix_data::Redeem { shares }.data(),
        };
        let user = self.user.insecure_clone();
        send_tx(&mut self.svm, &user, &[ix], &[&user]).map(|_| ())
    }

    /// Admin-only: pause the vault. Used to test the paused-redeem CEI path.
    pub fn pause(&mut self) -> Result<(), FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        self.pause_as(&admin)
    }

    /// `pause` signed by an arbitrary keypair (non-admins must be rejected).
    pub fn pause_as(&mut self, signer: &Keypair) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::Pause {
                vault_state: self.vault_state,
                deposit_mint: self.deposit_mint,
                admin: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix_data::Pause {}.data(),
        };
        self.send_as(signer, ix)
    }

    /// Admin-only: unpause the vault.
    pub fn unpause(&mut self) -> Result<(), FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        self.unpause_as(&admin)
    }

    /// `unpause` signed by an arbitrary keypair (non-admins must be rejected).
    pub fn unpause_as(&mut self, signer: &Keypair) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::Unpause {
                vault_state: self.vault_state,
                deposit_mint: self.deposit_mint,
                admin: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix_data::Unpause {}.data(),
        };
        self.send_as(signer, ix)
    }

    /// Admin-only: configure the withdrawal fee (in 1e-6 units; 100_000 = 10%).
    /// Used by the fee-bearing redeem test.
    pub fn set_withdrawal_fee(&mut self, fee: u32) -> Result<(), FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        self.set_withdrawal_fee_as(&admin, fee)
    }

    /// `set_withdrawal_fee` signed by an arbitrary keypair.
    pub fn set_withdrawal_fee_as(
        &mut self,
        signer: &Keypair,
        fee: u32,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::SetWithdrawalFee {
                vault_state: self.vault_state,
                deposit_mint: self.deposit_mint,
                admin: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix_data::SetWithdrawalFee { new_fee: fee }.data(),
        };
        self.send_as(signer, ix)
    }

    /// Admin-only: configure the AUM change limits (basis points).
    pub fn set_aum_limits(
        &mut self,
        increase_limit: u32,
        decrease_limit: u32,
    ) -> Result<(), FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        self.set_aum_limits_as(&admin, increase_limit, decrease_limit)
    }

    /// `set_aum_limits` signed by an arbitrary keypair.
    pub fn set_aum_limits_as(
        &mut self,
        signer: &Keypair,
        increase_limit: u32,
        decrease_limit: u32,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::SetAumLimits {
                vault_state: self.vault_state,
                deposit_mint: self.deposit_mint,
                admin: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix_data::SetAumLimits {
                increase_limit,
                decrease_limit,
            }
            .data(),
        };
        self.send_as(signer, ix)
    }

    /// `set_operator` signed by an arbitrary keypair.
    pub fn set_operator_as(
        &mut self,
        signer: &Keypair,
        new_operator: Pubkey,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::SetOperator {
                vault_state: self.vault_state,
                deposit_mint: self.deposit_mint,
                admin: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix_data::SetOperator { new_operator }.data(),
        };
        self.send_as(signer, ix)
    }

    /// The registry PDA for `address` on this vault.
    pub fn subaccount_pda(&self, address: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[
                august_vault::state::subaccount::SUBACCOUNT_SEED,
                self.vault_state.as_ref(),
                address.as_ref(),
            ],
            &august_vault::ID,
        )
        .0
    }

    /// `register_subaccount` signed by the configured admin.
    pub fn register_subaccount(
        &mut self,
        sub: &Subaccount,
    ) -> Result<(), FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        self.register_subaccount_as(&admin, sub)
    }

    /// `register_subaccount` signed by an arbitrary keypair.
    pub fn register_subaccount_as(
        &mut self,
        signer: &Keypair,
        sub: &Subaccount,
    ) -> Result<(), FailedTransactionMetadata> {
        let address = sub.key();
        self.register_address_as(signer, address, sub.pda, sub.deposit_ata)
    }

    /// The general form: address, PDA and ATA are named independently so a test
    /// can mismatch them.
    pub fn register_address_as(
        &mut self,
        signer: &Keypair,
        address: Pubkey,
        pda: Pubkey,
        ata: Pubkey,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::RegisterSubaccount {
                vault_state: self.vault_state,
                subaccount: pda,
                deposit_mint: self.deposit_mint,
                subaccount_ata: ata,
                token_program: self.token_program.id(),
                admin: signer.pubkey(),
                system_program: solana_sdk::system_program::ID,
            }
            .to_account_metas(None),
            data: ix_data::RegisterSubaccount { address }.data(),
        };
        self.send_as(signer, ix)
    }

    /// `settle_subaccount_loss` signed by the configured admin.
    pub fn settle_subaccount_loss(
        &mut self,
        sub: &Subaccount,
        amount: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        self.settle_subaccount_loss_as(&admin, sub, amount)
    }

    pub fn settle_subaccount_loss_as(
        &mut self,
        signer: &Keypair,
        sub: &Subaccount,
        amount: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::SettleSubaccountLoss {
                vault_state: self.vault_state,
                subaccount: sub.pda,
                deposit_mint: self.deposit_mint,
                subaccount_ata: sub.deposit_ata,
                token_program: self.token_program.id(),
                admin: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix_data::SettleSubaccountLoss { amount }.data(),
        };
        self.send_as(signer, ix)
    }

    /// `deregister_subaccount` signed by the configured admin.
    pub fn deregister_subaccount(
        &mut self,
        sub: &Subaccount,
    ) -> Result<(), FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        self.deregister_subaccount_as(&admin, sub)
    }

    pub fn deregister_subaccount_as(
        &mut self,
        signer: &Keypair,
        sub: &Subaccount,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::DeregisterSubaccount {
                vault_state: self.vault_state,
                subaccount: sub.pda,
                deposit_mint: self.deposit_mint,
                admin: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix_data::DeregisterSubaccount {}.data(),
        };
        self.send_as(signer, ix)
    }

    /// The registry entry for `sub`, deserialized.
    pub fn subaccount_data(&self, sub: &Subaccount) -> august_vault::state::subaccount::Subaccount {
        use anchor_lang::AccountDeserialize;
        let acct = self
            .svm
            .get_account(&sub.pda)
            .expect("registry entry exists");
        august_vault::state::subaccount::Subaccount::try_deserialize(&mut acct.data.as_slice())
            .expect("deserialize registry entry")
    }

    /// A lamport-funded custody keypair with a deposit-mint ATA, holding
    /// `mint_amount` of the deposit token. Neither delegated nor registered —
    /// see [`Self::new_registered_subaccount`] for the usual case.
    pub fn new_subaccount(&mut self, mint_amount: u64) -> Subaccount {
        let keypair = airdrop_keypair(&mut self.svm, 1_000_000_000);
        let payer = self.payer.insecure_clone();
        let (deposit_mint, token_program) = (self.deposit_mint, self.token_program);
        let deposit_ata = create_ata(
            &mut self.svm,
            &payer,
            &keypair.pubkey(),
            &deposit_mint,
            token_program,
        );
        if mint_amount > 0 {
            let ix = mint_to_ix(
                token_program,
                &deposit_mint,
                &deposit_ata,
                &payer.pubkey(),
                mint_amount,
            );
            send_tx(&mut self.svm, &payer, &[ix], &[&payer]).expect("mint to subaccount");
        }
        let pda = self.subaccount_pda(&keypair.pubkey());
        Subaccount {
            keypair,
            deposit_ata,
            pda,
        }
    }

    /// A custody address that has delegated to the vault but is not yet
    /// registered, so it cannot receive funds.
    pub fn new_delegated_subaccount(&mut self, allowance: u64) -> Subaccount {
        let sub = self.new_subaccount(0);
        self.approve_vault_as_delegate(&sub, allowance);
        sub
    }

    /// A delegated custody address already registered on the vault — the state
    /// most tests start from.
    pub fn new_registered_subaccount(&mut self, allowance: u64) -> Subaccount {
        let sub = self.new_delegated_subaccount(allowance);
        self.register_subaccount(&sub).expect("register");
        sub
    }

    /// Have `subaccount` approve the vault PDA as delegate over its ATA for
    /// `amount`, which is what lets `operator_deposit` pull funds back from
    /// custody the operator cannot sign for. Spent down by each return, and SPL
    /// clears it at zero.
    pub fn approve_vault_as_delegate(&mut self, subaccount: &Subaccount, amount: u64) {
        let vault_state = self.vault_state;
        self.approve_delegate_as(subaccount, &vault_state, amount);
    }

    /// A plain owner-signed token transfer between two ATAs — used to simulate
    /// an unrelated party donating into a subaccount's ATA.
    pub fn transfer_tokens_as(&mut self, owner: &Keypair, from: &Pubkey, to: &Pubkey, amount: u64) {
        let ix = match self.token_program {
            TokenProgramKind::Spl => spl_token::instruction::transfer(
                &spl_token::ID,
                from,
                to,
                &owner.pubkey(),
                &[],
                amount,
            ),
            TokenProgramKind::Token2022 => spl_token_2022::instruction::transfer_checked(
                &spl_token_2022::ID,
                from,
                &self.deposit_mint,
                to,
                &owner.pubkey(),
                &[],
                amount,
                DEPOSIT_DECIMALS,
            ),
        }
        .unwrap();
        let signer = owner.insecure_clone();
        send_tx(&mut self.svm, &signer, &[ix], &[&signer]).expect("token transfer");
    }

    /// Have `subaccount` revoke whatever delegation its ATA carries.
    pub fn revoke_delegate(&mut self, subaccount: &Subaccount) {
        let ix = revoke_ix(
            self.token_program,
            &subaccount.deposit_ata,
            &subaccount.keypair.pubkey(),
        );
        let signer = subaccount.keypair.insecure_clone();
        send_tx(&mut self.svm, &signer, &[ix], &[&signer]).expect("revoke");
    }

    /// The delegation currently recorded on a token account, for tests that
    /// assert a delegation is still present after the vault stops honouring it.
    pub fn token_account_delegate(&self, pubkey: &Pubkey) -> Option<Pubkey> {
        let acct = self.svm.get_account(pubkey).expect("token account exists");
        let parsed = SplAccount::unpack(&acct.data[..SplAccount::LEN]).expect("unpack");
        parsed.delegate.into()
    }

    /// As [`Self::approve_vault_as_delegate`], but naming the delegate, so a
    /// test can grant one to the wrong party. Always signed by the subaccount:
    /// nobody else can grant it.
    pub fn approve_delegate_as(&mut self, subaccount: &Subaccount, delegate: &Pubkey, amount: u64) {
        let owner = subaccount.keypair.insecure_clone();
        let ata = subaccount.deposit_ata;
        self.approve_from(&owner, &ata, delegate, amount);
    }

    /// The underlying primitive: `owner` approves `delegate` over `source_ata`.
    /// Takes a bare keypair so a test can delegate an ATA that is not a
    /// `Subaccount` fixture — the operator's own, for instance.
    pub fn approve_from(
        &mut self,
        owner: &Keypair,
        source_ata: &Pubkey,
        delegate: &Pubkey,
        amount: u64,
    ) {
        let ix = approve_ix(
            self.token_program,
            source_ata,
            delegate,
            &owner.pubkey(),
            amount,
        );
        let signer = owner.insecure_clone();
        send_tx(&mut self.svm, &signer, &[ix], &[&signer]).expect("approve delegate");
    }

    /// The deposit-mint ATA for an arbitrary owner, derived the same way the
    /// program's `associated_token::authority` constraint derives it.
    pub fn deposit_ata_for(&self, owner: &Pubkey) -> Pubkey {
        get_associated_token_address_with_program_id(
            owner,
            &self.deposit_mint,
            &self.token_program.id(),
        )
    }

    /// `set_fee_recipient` signed by an arbitrary keypair.
    pub fn set_fee_recipient_as(
        &mut self,
        signer: &Keypair,
        new_fee_recipient: Pubkey,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::SetFeeRecipient {
                vault_state: self.vault_state,
                deposit_mint: self.deposit_mint,
                admin: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix_data::SetFeeRecipient { new_fee_recipient }.data(),
        };
        self.send_as(signer, ix)
    }

    /// Admin-only: nominate a new admin (two-step transfer, 24h window).
    pub fn nominate_admin(&mut self, nominee: Pubkey) -> Result<(), FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        self.nominate_admin_as(&admin, nominee)
    }

    /// `nominate_admin` signed by an arbitrary keypair (also pays the PDA rent).
    pub fn nominate_admin_as(
        &mut self,
        signer: &Keypair,
        nominee: Pubkey,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::NominateAdmin {
                vault_state: self.vault_state,
                deposit_mint: self.deposit_mint,
                nominated_admin_pda: self.nominated_admin_pda(),
                admin: signer.pubkey(),
                payer: signer.pubkey(),
                system_program: solana_sdk::system_program::ID,
            }
            .to_account_metas(None),
            data: ix_data::NominateAdmin { new_admin: nominee }.data(),
        };
        self.send_as(signer, ix)
    }

    /// `accept_admin_nomination` signed by `new_admin`, who also receives the
    /// closed nomination PDA's rent.
    ///
    /// On success this rotates [`Self::admin`] to `new_admin` so the no-suffix
    /// admin helpers (`pause`, `close_vault`, `set_withdrawal_fee`, …) keep
    /// signing with the key the vault actually recognizes. A test that needs
    /// the deposed keypair afterwards must clone it *before* calling this.
    pub fn accept_admin_nomination_as(
        &mut self,
        new_admin: &Keypair,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::AcceptAdminNomination {
                vault_state: self.vault_state,
                deposit_mint: self.deposit_mint,
                nominated_admin_pda: self.nominated_admin_pda(),
                new_admin: new_admin.pubkey(),
                receiver: new_admin.pubkey(),
                system_program: solana_sdk::system_program::ID,
            }
            .to_account_metas(None),
            data: ix_data::AcceptAdminNomination {}.data(),
        };
        let result = self.send_as(new_admin, ix);
        if result.is_ok() {
            self.admin = new_admin.insecure_clone();
        }
        result
    }

    /// Admin-only: close an empty vault.
    pub fn close_vault(&mut self) -> Result<(), FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        self.close_vault_as(&admin)
    }

    /// `close_vault` signed by an arbitrary keypair.
    pub fn close_vault_as(&mut self, signer: &Keypair) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::CloseVault {
                vault_state: self.vault_state,
                share_mint: self.share_mint,
                vault_token_ata: self.vault_token_pda,
                deposit_mint: self.deposit_mint,
                admin: signer.pubkey(),
                token_program: self.token_program.id(),
            }
            .to_account_metas(None),
            data: ix_data::CloseVault {}.data(),
        };
        self.send_as(signer, ix)
    }

    /// Load the Metaplex Token Metadata program into the SVM. The two share
    /// token-metadata instructions CPI into it; every other test can skip
    /// this. The fixture is a mainnet dump — see `tests/fixtures/README.md`
    /// for provenance.
    pub fn load_mpl_token_metadata(&mut self) {
        self.svm
            .add_program(
                mpl_token_metadata::ID,
                include_bytes!("../tests/fixtures/mpl_token_metadata.so"),
            )
            .expect("load mpl_token_metadata.so fixture");
    }

    /// The Metaplex metadata PDA for this vault's share mint, derived by the
    /// Metaplex crate itself so the seeds can never drift from canonical.
    pub fn share_metadata_pda(&self) -> Pubkey {
        mpl_token_metadata::accounts::Metadata::find_pda(&self.share_mint).0
    }

    /// `create_share_token_metadata` signed by `admin` (payer is the harness
    /// payer). SPL-only: the instruction's `token_program` account is the
    /// legacy Token program by type.
    pub fn create_share_token_metadata_as(
        &mut self,
        admin: &Keypair,
        name: &str,
        symbol: &str,
        uri: &str,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::CreateShareTokenMetadata {
                payer: self.payer.pubkey(),
                admin: admin.pubkey(),
                vault_state: self.vault_state,
                deposit_mint: self.deposit_mint,
                share_mint: self.share_mint,
                metadata_account: self.share_metadata_pda(),
                token_program: spl_token::ID,
                token_metadata_program: mpl_token_metadata::ID,
                system_program: solana_sdk::system_program::ID,
                rent: solana_sdk::sysvar::rent::ID,
            }
            .to_account_metas(None),
            data: ix_data::CreateShareTokenMetadata {
                name: name.to_string(),
                symbol: symbol.to_string(),
                uri: uri.to_string(),
            }
            .data(),
        };
        let payer = self.payer.insecure_clone();
        send_tx(&mut self.svm, &payer, &[ix], &[&payer, admin]).map(|_| ())
    }

    /// `update_share_token_metadata` signed by `admin`.
    pub fn update_share_token_metadata_as(
        &mut self,
        admin: &Keypair,
        name: &str,
        symbol: &str,
        uri: &str,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::UpdateShareTokenMetadata {
                admin: admin.pubkey(),
                vault_state: self.vault_state,
                deposit_mint: self.deposit_mint,
                share_mint: self.share_mint,
                metadata_account: self.share_metadata_pda(),
                token_program: spl_token::ID,
                token_metadata_program: mpl_token_metadata::ID,
            }
            .to_account_metas(None),
            data: ix_data::UpdateShareTokenMetadata {
                name: name.to_string(),
                symbol: symbol.to_string(),
                uri: uri.to_string(),
            }
            .data(),
        };
        self.send_as(admin, ix)
    }

    /// Deserialized Metaplex metadata for the share mint. Panics if the
    /// metadata account does not exist yet.
    pub fn share_metadata(&self) -> mpl_token_metadata::accounts::Metadata {
        let acct = self
            .svm
            .get_account(&self.share_metadata_pda())
            .expect("metadata account exists");
        mpl_token_metadata::accounts::Metadata::from_bytes(&acct.data).expect("valid metadata")
    }

    /// The nomination PDA for this vault (seeds mirror `nominate_admin.rs`).
    pub fn nominated_admin_pda(&self) -> Pubkey {
        Pubkey::find_program_address(
            &[
                NOMINATED_ADMIN_PDA_SEED,
                self.deposit_mint.as_ref(),
                &[VAULT_VERSION],
            ],
            &august_vault::ID,
        )
        .0
    }

    // ---- withdrawal queue authority ----

    /// Returns this vault's queue PDA under the hardcoded queue program, which is
    /// the one key `attach_withdrawal_queue` will store.
    pub fn withdrawal_queue_pda(&self) -> Pubkey {
        withdrawal_queue_pda(&self.vault_state)
    }

    /// Plants a non-empty account at `address` owned by `owner`, standing in for
    /// the queue program's `initialize_queue`. The vault checks only the owner and
    /// that the data is non-empty, never the contents.
    pub fn install_queue_account(&mut self, address: Pubkey, owner: Pubkey) {
        self.install_queue_account_of_len(address, owner, 64);
    }

    /// Plants the same account with an explicit data length. A length of `0` is
    /// the only state the `!data_is_empty()` check can catch.
    pub fn install_queue_account_of_len(&mut self, address: Pubkey, owner: Pubkey, len: usize) {
        let data = vec![0xA5u8; len];
        let lamports = Rent::default().minimum_balance(data.len());
        self.svm
            .set_account(
                address,
                SolanaAccount {
                    lamports,
                    data,
                    owner,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap_or_else(|e| panic!("set_account failed for {address}: {e:?}"));
    }

    /// Calls `attach_withdrawal_queue` as the admin with `queue` in the queue
    /// slot.
    pub fn attach_withdrawal_queue(
        &mut self,
        queue: Pubkey,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        self.attach_withdrawal_queue_as(&admin, queue)
    }

    /// Calls `attach_withdrawal_queue` signed by `signer`. The transaction
    /// metadata is returned so that tests can read the emitted event.
    pub fn attach_withdrawal_queue_as(
        &mut self,
        signer: &Keypair,
        queue: Pubkey,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let accounts = ix_accounts::AttachWithdrawalQueue {
            vault_state: self.vault_state,
            deposit_mint: self.deposit_mint,
            admin: signer.pubkey(),
            queue,
        }
        .to_account_metas(None);
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts,
            data: ix_data::AttachWithdrawalQueue {}.data(),
        };
        send_tx(&mut self.svm, signer, &[ix], &[signer])
    }

    /// Calls `detach_withdrawal_queue` as the admin. `queue` says which account
    /// fills the queue slot and whether it signs.
    pub fn detach_withdrawal_queue(
        &mut self,
        queue: QueueCoSigner<'_>,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        self.detach_withdrawal_queue_as(&admin, queue)
    }

    /// Calls `detach_withdrawal_queue` signed by `signer`. The transaction
    /// metadata is returned so that tests can read the emitted event.
    pub fn detach_withdrawal_queue_as(
        &mut self,
        signer: &Keypair,
        queue: QueueCoSigner<'_>,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let mut accounts = ix_accounts::DetachWithdrawalQueue {
            vault_state: self.vault_state,
            deposit_mint: self.deposit_mint,
            admin: signer.pubkey(),
            queue: queue.key(),
        }
        .to_account_metas(None);
        let mut signers: Vec<&Keypair> = vec![signer];
        match queue {
            QueueCoSigner::Signing(kp) => signers.push(kp),
            // The program types this slot as `Signer`, so the generated meta
            // already demands a signature. Clearing the flag sends the account
            // unsigned, which is the shape a caller who forgot the signature
            // produces. The slot is found by position: `queue` is declared last.
            QueueCoSigner::Unsigned(key) => {
                // The message compiler ORs `is_signer` per pubkey, so an unsigned
                // slot that aliases the payer would still arrive signed.
                assert_ne!(key, signer.pubkey(), "Unsigned cannot alias the payer");
                let slot = accounts.last_mut().expect("metas are never empty");
                assert_eq!(
                    slot.pubkey, key,
                    "queue is not the last meta; the accounts struct was reordered"
                );
                slot.is_signer = false;
            }
        }
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts,
            data: ix_data::DetachWithdrawalQueue {}.data(),
        };
        send_tx(&mut self.svm, signer, &[ix], &signers)
    }

    // ---- the queue program's admin instructions ----

    /// The queue's escrow ATA for `mint`, under this vault's token program.
    pub fn queue_escrow(&self, mint: &Pubkey) -> Pubkey {
        get_associated_token_address_with_program_id(
            &self.withdrawal_queue_pda(),
            mint,
            &self.token_program.id(),
        )
    }

    /// Decodes this vault's queue account. Panics if it does not exist.
    pub fn queue_state_data(&self) -> WithdrawalQueue {
        let account = self
            .svm
            .get_account(&self.withdrawal_queue_pda())
            .expect("queue account exists");
        WithdrawalQueue::try_deserialize(&mut account.data.as_slice()).expect("decode queue")
    }

    /// `initialize_queue` as the admin, with the harness payer funding it.
    pub fn initialize_queue(
        &mut self,
        cooldown_seconds: u64,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        let payer = self.payer.insecure_clone();
        self.initialize_queue_as(&admin, &payer, cooldown_seconds)
    }

    /// `initialize_queue` for this vault, signed by `admin` and funded by `payer`.
    pub fn initialize_queue_as(
        &mut self,
        admin: &Keypair,
        payer: &Keypair,
        cooldown_seconds: u64,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let (vault_state, deposit_mint, share_mint) =
            (self.vault_state, self.deposit_mint, self.share_mint);
        self.initialize_queue_for_vault_as(
            admin,
            payer,
            vault_state,
            deposit_mint,
            share_mint,
            cooldown_seconds,
        )
    }

    /// `initialize_queue` against arbitrary vault and mint accounts, so a test
    /// can hand the program something that is not a vault.
    pub fn initialize_queue_for_vault_as(
        &mut self,
        admin: &Keypair,
        payer: &Keypair,
        vault_state: Pubkey,
        deposit_mint: Pubkey,
        share_mint: Pubkey,
        cooldown_seconds: u64,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let queue = withdrawal_queue_pda(&vault_state);
        let token_program = self.token_program.id();
        let accounts = q_accounts::InitializeQueue {
            vault_state,
            admin: admin.pubkey(),
            payer: payer.pubkey(),
            deposit_mint,
            share_mint,
            queue,
            escrow_shares: get_associated_token_address_with_program_id(
                &queue,
                &share_mint,
                &token_program,
            ),
            escrow_assets: get_associated_token_address_with_program_id(
                &queue,
                &deposit_mint,
                &token_program,
            ),
            token_program,
            associated_token_program: spl_associated_token_account::ID,
            system_program: solana_sdk::system_program::ID,
        }
        .to_account_metas(None);
        let ix = Instruction {
            program_id: august_withdrawal_queue::ID,
            accounts,
            data: q_ix::InitializeQueue { cooldown_seconds }.data(),
        };
        // `payer` pays the fee so an intentionally-broke `admin` still reaches
        // the program instead of failing pre-flight.
        send_tx(&mut self.svm, payer, &[ix], &[payer, admin])
    }

    pub fn set_cooldown(
        &mut self,
        seconds: u64,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        self.set_cooldown_as(&admin, seconds)
    }

    pub fn set_cooldown_as(
        &mut self,
        admin: &Keypair,
        seconds: u64,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        self.queue_admin_call_as(admin, q_ix::SetCooldown { seconds }.data())
    }

    pub fn set_fulfillment_window(
        &mut self,
        seconds: u64,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let admin = self.admin.insecure_clone();
        self.set_fulfillment_window_as(&admin, seconds)
    }

    pub fn set_fulfillment_window_as(
        &mut self,
        admin: &Keypair,
        seconds: u64,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        self.queue_admin_call_as(admin, q_ix::SetFulfillmentWindow { seconds }.data())
    }

    /// One of the two admin setters, against this vault's queue and vault.
    fn queue_admin_call_as(
        &mut self,
        admin: &Keypair,
        data: Vec<u8>,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let (queue, vault_state) = (self.withdrawal_queue_pda(), self.vault_state);
        self.queue_admin_call_with_accounts_as(admin, queue, vault_state, data)
    }

    /// One of the two admin setters, against arbitrary queue and vault
    /// accounts, so a test can pair a queue with the wrong vault.
    pub fn queue_admin_call_with_accounts_as(
        &mut self,
        admin: &Keypair,
        queue: Pubkey,
        vault_state: Pubkey,
        data: Vec<u8>,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let accounts = q_accounts::QueueAdmin {
            queue,
            vault_state,
            admin: admin.pubkey(),
        }
        .to_account_metas(None);
        let ix = Instruction {
            program_id: august_withdrawal_queue::ID,
            accounts,
            data,
        };
        send_tx(&mut self.svm, admin, &[ix], &[admin])
    }

    /// Creates the ATA of `owner` for `mint` under this vault's token program,
    /// paid by the harness payer, as anyone may. Returns its address.
    pub fn create_ata_for(&mut self, owner: &Pubkey, mint: &Pubkey) -> Pubkey {
        let payer = self.payer.insecure_clone();
        create_ata(&mut self.svm, &payer, owner, mint, self.token_program)
    }

    /// Mints `amount` of the deposit token straight into `destination`.
    pub fn mint_deposit_to(&mut self, destination: &Pubkey, amount: u64) {
        let ix = mint_to_ix(
            self.token_program,
            &self.deposit_mint,
            destination,
            &self.payer.pubkey(),
            amount,
        );
        let payer = self.payer.insecure_clone();
        send_tx(&mut self.svm, &payer, &[ix], &[&payer]).expect("mint to destination");
    }

    /// The Clock sysvar's current `unix_timestamp`.
    pub fn now(&self) -> i64 {
        let clock: solana_sdk::clock::Clock = self.svm.get_sysvar();
        clock.unix_timestamp
    }

    // ---- the queue program's request instructions ----

    /// Initializes the queue, attaches it on the vault and opens it to requests.
    /// Returns the queue PDA.
    pub fn open_queue(&mut self, cooldown_seconds: u64) -> Pubkey {
        self.initialize_queue(cooldown_seconds)
            .expect("initialize_queue");
        let pda = self.withdrawal_queue_pda();
        self.attach_withdrawal_queue(pda).expect("attach");
        self.set_accepting_requests(true).expect("accept requests");
        pda
    }

    /// The request PDA for `owner`'s `request_id` on this vault's queue.
    pub fn request_pda(&self, owner: &Pubkey, request_id: u64) -> Pubkey {
        Pubkey::find_program_address(
            &[
                WITHDRAWAL_REQUEST_SEED,
                self.withdrawal_queue_pda().as_ref(),
                owner.as_ref(),
                &request_id.to_le_bytes(),
            ],
            &august_withdrawal_queue::ID,
        )
        .0
    }

    /// Decodes a request account. Panics if it does not exist.
    pub fn request_state_data(&self, owner: &Pubkey, request_id: u64) -> WithdrawalRequest {
        let account = self
            .svm
            .get_account(&self.request_pda(owner, request_id))
            .expect("request account exists");
        WithdrawalRequest::try_deserialize(&mut account.data.as_slice()).expect("decode request")
    }

    /// `request_withdrawal` as the user, from their share ATA, paying their
    /// deposit ATA, with no finalizer restriction.
    pub fn request_withdrawal(
        &mut self,
        request_id: u64,
        shares: u64,
        min_assets_out: u64,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let user = self.user.insecure_clone();
        let (share_account, recipient) = (self.user_share_ata, self.user_deposit_ata);
        self.request_withdrawal_as(
            &user,
            share_account,
            recipient,
            request_id,
            shares,
            min_assets_out,
            Pubkey::default(),
        )
    }

    /// `request_withdrawal` with every account and argument chosen by the test.
    #[allow(clippy::too_many_arguments)]
    pub fn request_withdrawal_as(
        &mut self,
        owner: &Keypair,
        owner_share_account: Pubkey,
        recipient_token_account: Pubkey,
        request_id: u64,
        shares: u64,
        min_assets_out: u64,
        finalizer: Pubkey,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let accounts = q_accounts::RequestWithdrawal {
            queue: self.withdrawal_queue_pda(),
            vault_state: self.vault_state,
            owner: owner.pubkey(),
            owner_share_account,
            escrow_shares: self.queue_escrow(&self.share_mint),
            share_mint: self.share_mint,
            recipient_token_account,
            request: self.request_pda(&owner.pubkey(), request_id),
            token_program: self.token_program.id(),
            system_program: solana_sdk::system_program::ID,
        }
        .to_account_metas(None);
        let ix = Instruction {
            program_id: august_withdrawal_queue::ID,
            accounts,
            data: q_ix::RequestWithdrawal {
                request_id,
                shares,
                min_assets_out,
                finalizer,
            }
            .data(),
        };
        send_tx(&mut self.svm, owner, &[ix], &[owner])
    }

    /// `update_request` as the user on their own request.
    pub fn update_request(
        &mut self,
        request_id: u64,
        expected_sequence: u64,
        min_assets_out: Option<u64>,
        new_recipient: Option<Pubkey>,
        finalizer: Option<Pubkey>,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let user = self.user.insecure_clone();
        let owner = user.pubkey();
        self.update_request_as(
            &user,
            &owner,
            request_id,
            expected_sequence,
            min_assets_out,
            new_recipient,
            finalizer,
        )
    }

    /// `update_request` signed by `signer` against `request_owner`'s request,
    /// so a test can have the wrong person try.
    #[allow(clippy::too_many_arguments)]
    pub fn update_request_as(
        &mut self,
        signer: &Keypair,
        request_owner: &Pubkey,
        request_id: u64,
        expected_sequence: u64,
        min_assets_out: Option<u64>,
        new_recipient: Option<Pubkey>,
        finalizer: Option<Pubkey>,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let accounts = q_accounts::UpdateRequest {
            queue: self.withdrawal_queue_pda(),
            vault_state: self.vault_state,
            owner: signer.pubkey(),
            request: self.request_pda(request_owner, request_id),
            new_recipient,
        }
        .to_account_metas(None);
        let ix = Instruction {
            program_id: august_withdrawal_queue::ID,
            accounts,
            data: q_ix::UpdateRequest {
                expected_sequence,
                min_assets_out,
                finalizer,
            }
            .data(),
        };
        send_tx(&mut self.svm, signer, &[ix], &[signer])
    }

    /// Create and fund a throwaway keypair (for impostor-signer tests).
    pub fn new_funded_keypair(&mut self, lamports: u64) -> Keypair {
        airdrop_keypair(&mut self.svm, lamports)
    }

    /// An additional independent depositor: funded keypair plus deposit and
    /// share ATAs, seeded with `mint_amount` of the deposit token.
    ///
    /// `VaultCtx` has one built-in `user`; share-price manipulation is inherently
    /// multi-party (an attacker's position has to be distinguishable from its
    /// victims'), so those tests need more.
    pub fn new_depositor(&mut self, mint_amount: u64) -> Depositor {
        let keypair = airdrop_keypair(&mut self.svm, 1_000_000_000);
        let payer = self.payer.insecure_clone();
        let (deposit_mint, share_mint, token_program) =
            (self.deposit_mint, self.share_mint, self.token_program);
        let deposit_ata = create_ata(
            &mut self.svm,
            &payer,
            &keypair.pubkey(),
            &deposit_mint,
            token_program,
        );
        let share_ata = create_ata(
            &mut self.svm,
            &payer,
            &keypair.pubkey(),
            &share_mint,
            token_program,
        );
        if mint_amount > 0 {
            let ix = mint_to_ix(
                token_program,
                &deposit_mint,
                &deposit_ata,
                &payer.pubkey(),
                mint_amount,
            );
            send_tx(&mut self.svm, &payer, &[ix], &[&payer]).expect("mint to depositor");
        }
        Depositor {
            keypair,
            deposit_ata,
            share_ata,
        }
    }

    /// `deposit` signed by an arbitrary depositor.
    pub fn deposit_as(
        &mut self,
        depositor: &Depositor,
        amount: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::Deposit {
                vault_state: self.vault_state,
                vault_token_ata: self.vault_token_pda,
                sender_token_account: depositor.deposit_ata,
                sender_share_account: depositor.share_ata,
                share_mint: self.share_mint,
                deposit_mint: self.deposit_mint,
                signer: depositor.keypair.pubkey(),
                token_program: self.token_program.id(),
            }
            .to_account_metas(None),
            data: ix_data::Deposit { amount }.data(),
        };
        self.send_as(&depositor.keypair, ix)
    }

    /// `deposit_checked` signed by an arbitrary depositor, with a slippage bound.
    pub fn deposit_checked_as(
        &mut self,
        depositor: &Depositor,
        amount: u64,
        min_shares_out: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::Deposit {
                vault_state: self.vault_state,
                vault_token_ata: self.vault_token_pda,
                sender_token_account: depositor.deposit_ata,
                sender_share_account: depositor.share_ata,
                share_mint: self.share_mint,
                deposit_mint: self.deposit_mint,
                signer: depositor.keypair.pubkey(),
                token_program: self.token_program.id(),
            }
            .to_account_metas(None),
            data: ix_data::DepositChecked {
                amount,
                min_shares_out,
            }
            .data(),
        };
        self.send_as(&depositor.keypair, ix)
    }

    /// `redeem` signed by an arbitrary depositor.
    pub fn redeem_as(
        &mut self,
        depositor: &Depositor,
        shares: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::Redeem {
                vault_state: self.vault_state,
                vault_deposit_ata: self.vault_token_pda,
                sender_token_account: depositor.deposit_ata,
                sender_share_account: depositor.share_ata,
                fee_recipient_account: self.fee_recipient_deposit_ata,
                share_mint: self.share_mint,
                deposit_mint: self.deposit_mint,
                signer: depositor.keypair.pubkey(),
                token_program: self.token_program.id(),
            }
            .to_account_metas(None),
            data: ix_data::Redeem { shares }.data(),
        };
        self.send_as(&depositor.keypair, ix)
    }

    /// `redeem_checked` signed by an arbitrary depositor, with a slippage bound
    /// on the **net** payout (after the withdrawal fee).
    pub fn redeem_checked_as(
        &mut self,
        depositor: &Depositor,
        shares: u64,
        min_assets_out: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::Redeem {
                vault_state: self.vault_state,
                vault_deposit_ata: self.vault_token_pda,
                sender_token_account: depositor.deposit_ata,
                sender_share_account: depositor.share_ata,
                fee_recipient_account: self.fee_recipient_deposit_ata,
                share_mint: self.share_mint,
                deposit_mint: self.deposit_mint,
                signer: depositor.keypair.pubkey(),
                token_program: self.token_program.id(),
            }
            .to_account_metas(None),
            data: ix_data::RedeemChecked {
                shares,
                min_assets_out,
            }
            .data(),
        };
        self.send_as(&depositor.keypair, ix)
    }

    /// Burn share tokens **directly through the token program**, bypassing the
    /// vault entirely.
    ///
    /// SPL Token lets any holder burn their own balance, which shrinks
    /// `share_mint.supply` — the value the vault reads to price deposits and
    /// redeems. The vault cannot prevent this, so the share math has to stay
    /// sound in spite of it.
    pub fn burn_shares_as(&mut self, depositor: &Depositor, amount: u64) {
        let ix = match self.token_program {
            TokenProgramKind::Spl => spl_token::instruction::burn(
                &spl_token::ID,
                &depositor.share_ata,
                &self.share_mint,
                &depositor.keypair.pubkey(),
                &[],
                amount,
            ),
            TokenProgramKind::Token2022 => spl_token_2022::instruction::burn(
                &spl_token_2022::ID,
                &depositor.share_ata,
                &self.share_mint,
                &depositor.keypair.pubkey(),
                &[],
                amount,
            ),
        }
        .expect("build burn instruction");
        self.send_as(&depositor.keypair, ix)
            .expect("an SPL holder can always burn their own shares");
    }

    /// The singleton program-config PDA.
    pub fn program_config_pda(&self) -> Pubkey {
        program_config_pda()
    }

    /// Deserialized program config.
    pub fn program_config(&self) -> august_vault::state::config::ProgramConfig {
        use anchor_lang::AccountDeserialize;
        let acct = self
            .svm
            .get_account(&program_config_pda())
            .expect("program config exists");
        august_vault::state::config::ProgramConfig::try_deserialize(&mut acct.data.as_slice())
            .expect("valid program config")
    }

    /// Create an additional deposit mint, so namespace tests can initialize
    /// vaults without colliding with the harness's own vault.
    pub fn create_extra_deposit_mint(&mut self) -> Pubkey {
        let payer = self.payer.insecure_clone();
        let mint_kp = Keypair::new();
        create_mint(
            &mut self.svm,
            &payer,
            &mint_kp,
            &payer.pubkey(),
            DEPOSIT_DECIMALS,
            self.token_program,
        );
        mint_kp.pubkey()
    }

    /// Attempt `initialize` for an arbitrary `(deposit_mint, vault_version)`
    /// signed by `signer`, who also pays. Returns the transaction result so
    /// negative tests can assert on the failure.
    pub fn try_initialize_vault(
        &mut self,
        signer: &Keypair,
        deposit_mint: Pubkey,
        vault_version: u8,
    ) -> Result<(), FailedTransactionMetadata> {
        let payer = signer.insecure_clone();
        self.try_initialize_vault_paid_by(signer, &payer, deposit_mint, vault_version)
    }

    /// As above, with the rent payer separate from the authorizing signer — the
    /// shape a cold or MPC-held protocol authority would use.
    pub fn try_initialize_vault_paid_by(
        &mut self,
        signer: &Keypair,
        payer: &Keypair,
        deposit_mint: Pubkey,
        vault_version: u8,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = initialize_ix(
            deposit_mint,
            vault_version,
            &signer.pubkey(),
            &payer.pubkey(),
            self.admin.pubkey(),
            self.operator.pubkey(),
            self.fee_recipient.pubkey(),
            self.token_program,
            HARNESS_SHARE_OFFSET_U64,
        );
        // `payer` pays the fee so an intentionally-broke `signer` still reaches
        // the program instead of failing pre-flight.
        send_tx(&mut self.svm, payer, &[ix], &[signer, payer]).map(|_| ())
    }

    /// Create a vault with an explicit share offset, for tests that care which
    /// offset a vault carries rather than accepting the harness default.
    pub fn try_initialize_vault_with_offset(
        &mut self,
        signer: &Keypair,
        deposit_mint: Pubkey,
        vault_version: u8,
        // `u64`, matching the wire type. Taking `u128` and narrowing with `as u64`
        // would silently truncate a boundary value a test meant to reject.
        share_offset: u64,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = initialize_ix(
            deposit_mint,
            vault_version,
            &signer.pubkey(),
            &signer.pubkey(),
            self.admin.pubkey(),
            self.operator.pubkey(),
            self.fee_recipient.pubkey(),
            self.token_program,
            share_offset,
        );
        let payer = signer.insecure_clone();
        send_tx(&mut self.svm, &payer, &[ix], &[signer]).map(|_| ())
    }

    /// Read a vault's stored offset straight from its account, so a test can
    /// assert what was actually persisted rather than what was requested.
    pub fn stored_share_offset_raw(&self, deposit_mint: Pubkey, vault_version: u8) -> u64 {
        use anchor_lang::AccountDeserialize;
        let addr = derive_vault_state(&deposit_mint, vault_version).0;
        let acct = self.svm.get_account(&addr).expect("vault state exists");
        august_vault::state::vault::VaultState::try_deserialize(&mut acct.data.as_slice())
            .expect("deserialize vault state")
            .share_offset
    }

    /// `override_config_authority` signed by an arbitrary keypair, which must be
    /// the upgrade authority named by the installed `ProgramData`.
    pub fn override_config_authority_as(
        &mut self,
        signer: &Keypair,
        new_authority: Pubkey,
    ) -> Result<(), FailedTransactionMetadata> {
        self.override_config_authority_with_program_data(signer, new_authority, program_data_pda())
    }

    /// As above, but with an arbitrary account passed as `program_data`.
    ///
    /// `override_config_authority` carries its **own** copy of the
    /// upgrade-authority constraint — it does not share one with
    /// `initialize_config` — so that copy needs its own spoofing test. Without
    /// this, dropping `seeds`/`seeds::program` from the override's `program_data`
    /// would let anyone who is the upgrade authority of *any* upgradeable program
    /// reset this program's vault-creation authority, and every test would pass.
    pub fn override_config_authority_with_program_data(
        &mut self,
        signer: &Keypair,
        new_authority: Pubkey,
        program_data: Pubkey,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::OverrideConfigAuthority {
                program_config: program_config_pda(),
                upgrade_authority: signer.pubkey(),
                program_data,
            }
            .to_account_metas(None),
            data: ix_data::OverrideConfigAuthority { new_authority }.data(),
        };
        self.send_as(signer, ix)
    }

    /// Install a `ProgramData` fixture at an arbitrary address, for spoofing
    /// tests. Mirrors [`BareCtx::install_foreign_program_data`].
    pub fn install_foreign_program_data(&mut self, address: Pubkey, upgrade_authority: &Pubkey) {
        install_program_data_at(&mut self.svm, address, upgrade_authority);
    }

    /// Drop the program's upgrade authority, modelling an immutable program.
    /// Mirrors [`BareCtx::make_program_immutable`].
    pub fn make_program_immutable(&mut self) {
        write_program_data(&mut self.svm, program_data_pda(), None);
    }

    /// `set_config_authority` signed by an arbitrary keypair.
    pub fn set_config_authority_as(
        &mut self,
        signer: &Keypair,
        new_authority: Pubkey,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::SetConfigAuthority {
                program_config: program_config_pda(),
                authority: signer.pubkey(),
            }
            .to_account_metas(None),
            data: ix_data::SetConfigAuthority { new_authority }.data(),
        };
        self.send_as(signer, ix)
    }

    /// Attempt `initialize_config` signed by `signer`. The harness already
    /// bootstrapped the config, so this exists for negative tests (wrong
    /// upgrade authority, double-initialization).
    pub fn try_initialize_config_as(
        &mut self,
        signer: &Keypair,
        authority: Pubkey,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = initialize_config_ix(&signer.pubkey(), authority, program_data_pda());
        self.send_as(signer, ix)
    }

    /// Rewrite the installed `ProgramData` fixture to name a different upgrade
    /// authority, mirroring an upgrade-authority rotation on a real cluster.
    pub fn set_program_data_upgrade_authority(&mut self, authority: &Pubkey) {
        install_program_data(&mut self.svm, authority);
    }

    /// Create an ATA for `owner` on the vault's deposit mint, returning its
    /// address. Impostor operator tests need this so the ATA-derivation
    /// constraint resolves and the access-control check is what fires. Also the
    /// way any third party can create one on mainnet — no signature from
    /// `owner` is required.
    pub fn create_deposit_ata_for(&mut self, owner: &Pubkey) -> Pubkey {
        let payer = self.payer.insecure_clone();
        let deposit_mint = self.deposit_mint;
        create_ata(
            &mut self.svm,
            &payer,
            owner,
            &deposit_mint,
            self.token_program,
        )
    }

    /// Advance the Clock sysvar's `unix_timestamp` by `secs`. LiteSVM does not
    /// tick wall-clock time on its own, so tests that exercise the 24-hour
    /// nomination expiry warp explicitly.
    pub fn warp_forward_seconds(&mut self, secs: i64) {
        let mut clock: solana_sdk::clock::Clock = self.svm.get_sysvar();
        clock.unix_timestamp += secs;
        self.svm.set_sysvar(&clock);
    }

    /// Overwrite the share mint's `supply` directly, bypassing the program.
    /// Pairs with `force_overwrite_vault_state` for tests that need share
    /// supply and recorded AUM at magnitudes unreachable through the public
    /// API (e.g. driving `local_aum + amount` past `u64::MAX`).
    pub fn force_overwrite_share_mint_supply(&mut self, new_supply: u64) {
        let mut acct = self
            .svm
            .get_account(&self.share_mint)
            .expect("share mint exists");
        let mut mint = SplMint::unpack(&acct.data[..SplMint::LEN]).expect("valid mint");
        mint.supply = new_supply;
        SplMint::pack(mint, &mut acct.data[..SplMint::LEN]).expect("repack mint");
        self.svm
            .set_account(self.share_mint, acct)
            .unwrap_or_else(|e| panic!("set_account failed for share_mint: {e:?}"));
    }

    /// Sign `ix` with `signer` (who also pays the fee) and send it.
    fn send_as(
        &mut self,
        signer: &Keypair,
        ix: Instruction,
    ) -> Result<(), FailedTransactionMetadata> {
        send_tx(&mut self.svm, signer, &[ix], &[signer]).map(|_| ())
    }

    pub fn token_account_amount(&self, pubkey: &Pubkey) -> u64 {
        let acct = self.svm.get_account(pubkey).expect("token account exists");
        // For Token-2022, the first 165 bytes are the same Account layout; reading
        // via spl_token::state::Account::unpack works for the base fields.
        SplAccount::unpack(&acct.data[..SplAccount::LEN])
            .expect("valid token account")
            .amount
    }

    /// The share mint's current mint authority, `None` once `close_vault` has
    /// revoked it.
    pub fn share_mint_authority(&self) -> Option<Pubkey> {
        let acct = self
            .svm
            .get_account(&self.share_mint)
            .expect("share mint exists");
        SplMint::unpack(&acct.data[..SplMint::LEN])
            .expect("valid mint")
            .mint_authority
            .into()
    }

    /// The three PDAs a `(deposit_mint, vault_version)` pair occupies.
    pub fn vault_pdas(&self, deposit_mint: Pubkey, vault_version: u8) -> [Pubkey; 3] {
        [
            derive_vault_state(&deposit_mint, vault_version).0,
            derive_share_mint(&deposit_mint, vault_version).0,
            derive_vault_token_pda(&deposit_mint, vault_version).0,
        ]
    }

    pub fn share_mint_supply(&self) -> u64 {
        let acct = self
            .svm
            .get_account(&self.share_mint)
            .expect("share mint exists");
        SplMint::unpack(&acct.data[..SplMint::LEN])
            .expect("valid mint")
            .supply
    }

    /// Deserialize the vault state via Anchor's `AccountDeserialize` so we don't
    /// have to manually skip the 8-byte discriminator.
    pub fn vault_state_data(&self) -> august_vault::state::vault::VaultState {
        use anchor_lang::AccountDeserialize;
        let acct = self
            .svm
            .get_account(&self.vault_state)
            .expect("vault state exists");
        august_vault::state::vault::VaultState::try_deserialize(&mut acct.data.as_slice())
            .expect("valid vault state")
    }

    /// Write a `VaultState` directly into the SVM, bypassing legal program
    /// instructions. Used by tests that need to engineer states unreachable
    /// via the public API (e.g. `deployed_aum = u64::MAX - 1`).
    ///
    /// Size-preservation contract: the caller must ensure the serialized form
    /// of `new_state` is no larger than the original account allocation
    /// (`VaultState::LEN` at construction time). The helper asserts this rather
    /// than silently truncating — otherwise a future field addition past
    /// `INIT_SPACE` would produce corrupt account bytes.
    pub fn force_overwrite_vault_state(
        &mut self,
        new_state: august_vault::state::vault::VaultState,
    ) {
        use anchor_lang::AccountSerialize;
        let mut acct = self
            .svm
            .get_account(&self.vault_state)
            .expect("vault_state account missing — did the test call VaultCtx::fresh()?");
        let mut data = Vec::with_capacity(acct.data.len());
        new_state
            .try_serialize(&mut data)
            .expect("serialize vault state");
        assert!(
            data.len() <= acct.data.len(),
            "force_overwrite_vault_state: serialized state is {} bytes but account is only {} \
             bytes — did VaultState grow past its INIT_SPACE?",
            data.len(),
            acct.data.len(),
        );
        // Pad with zeros so the account size stays put (resize NEVER truncates here,
        // since we asserted data.len() ≤ acct.data.len() above).
        data.resize(acct.data.len(), 0);
        acct.data = data;
        self.svm
            .set_account(self.vault_state, acct)
            .unwrap_or_else(|e| panic!("set_account failed for vault_state: {e:?}"));
    }

    /// Take a snapshot of every observable state slot a CEI test needs to
    /// prove unchanged. Use with `assert_eq!(before, after, ...)`.
    pub fn snapshot(&self) -> CeiSnapshot {
        let state = self.vault_state_data();
        CeiSnapshot {
            user_shares: self.token_account_amount(&self.user_share_ata),
            user_deposit: self.token_account_amount(&self.user_deposit_ata),
            vault_tokens: self.token_account_amount(&self.vault_token_pda),
            fee_recipient_tokens: self.token_account_amount(&self.fee_recipient_deposit_ata),
            share_supply: self.share_mint_supply(),
            local_aum: state.local_aum,
            deployed_aum: state.deployed_aum,
            withdrawal_queue_authority: state.withdrawal_queue_authority,
        }
    }
}

/// Describes how a test fills the `queue` slot of `detach_withdrawal_queue`.
pub enum QueueCoSigner<'a> {
    /// The account is present and signs. This stands in for the signature
    /// `release_vault` will produce by CPI, which is faithful because the vault
    /// compares a signature against a stored key and never asks whether that key
    /// is a PDA.
    Signing(&'a Keypair),
    /// The account is present but does not sign.
    Unsigned(Pubkey),
}

impl QueueCoSigner<'_> {
    fn key(&self) -> Pubkey {
        match self {
            Self::Signing(kp) => kp.pubkey(),
            Self::Unsigned(k) => *k,
        }
    }
}

/// An independent vault participant created by [`VaultCtx::new_depositor`].
pub struct Depositor {
    pub keypair: Keypair,
    pub deposit_ata: Pubkey,
    pub share_ata: Pubkey,
}

/// A stand-in for the custody address a registered subaccount names. A plain
/// keypair where production wants Fordefi or a multisig; what the tests need is
/// the part the program can see — an ATA the operator does not own, and a
/// delegation it cannot grant itself.
pub struct Subaccount {
    pub keypair: Keypair,
    pub deposit_ata: Pubkey,
    /// The registry PDA for this address on the vault it was created against.
    pub pda: Pubkey,
}

impl Subaccount {
    pub fn key(&self) -> Pubkey {
        self.keypair.pubkey()
    }
}

/// A fresh SVM with the program loaded and a `ProgramData` fixture installed,
/// but **no program config and no vault**.
///
/// [`VaultCtx::fresh`] bootstraps the config as part of setup, which would mask
/// the authorization check on `initialize_config` behind an "account already in
/// use" failure. Tests that exercise the bootstrap itself start from here.
pub struct BareCtx {
    pub svm: LiteSVM,
    /// The key named as upgrade authority by the installed `ProgramData`.
    pub upgrade_authority: Keypair,
}

impl BareCtx {
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();
        svm.add_program(
            august_vault::ID,
            include_bytes!(concat!(env!("OUT_DIR"), "/august_vault.so")),
        )
        .expect("load august_vault.so — built by build.rs into OUT_DIR");
        // Loaded alongside the vault so every suite built on this harness carries
        // the two-program wiring, not only the ones that will drive the queue.
        // `mainnet_fork_compat.rs` builds its own SVM and loads the vault alone;
        // `embedded_artifacts.rs` covers the queue artifact on its own.
        svm.add_program(
            august_withdrawal_queue::ID,
            include_bytes!(concat!(env!("OUT_DIR"), "/august_withdrawal_queue.so")),
        )
        .expect("load august_withdrawal_queue.so — built by build.rs into OUT_DIR");
        let upgrade_authority = airdrop_keypair(&mut svm, 10_000_000_000);
        install_program_data(&mut svm, &upgrade_authority.pubkey());
        Self {
            svm,
            upgrade_authority,
        }
    }

    pub fn new_funded_keypair(&mut self, lamports: u64) -> Keypair {
        airdrop_keypair(&mut self.svm, lamports)
    }

    /// `initialize_config` signed by `signer`, who also pays.
    pub fn try_initialize_config_as(
        &mut self,
        signer: &Keypair,
        authority: Pubkey,
    ) -> Result<(), FailedTransactionMetadata> {
        self.try_initialize_config_with_program_data(signer, authority, program_data_pda())
    }

    /// As above, but with an arbitrary account passed as `program_data` — so a
    /// test can try to present some other program's `ProgramData`.
    pub fn try_initialize_config_with_program_data(
        &mut self,
        signer: &Keypair,
        authority: Pubkey,
        program_data: Pubkey,
    ) -> Result<(), FailedTransactionMetadata> {
        let ix = initialize_config_ix(&signer.pubkey(), authority, program_data);
        send_tx(&mut self.svm, signer, &[ix], &[signer]).map(|_| ())
    }

    /// Install a `ProgramData` fixture at an arbitrary address, for spoofing
    /// tests.
    pub fn install_foreign_program_data(&mut self, address: Pubkey, upgrade_authority: &Pubkey) {
        install_program_data_at(&mut self.svm, address, upgrade_authority);
    }

    /// Drop the program's upgrade authority, modelling an immutable program.
    pub fn make_program_immutable(&mut self) {
        write_program_data(&mut self.svm, program_data_pda(), None);
    }

    /// Attempt `initialize` on a program whose config has never been created,
    /// with a fully funded signer so the only possible objection is the missing
    /// config. Pins the "fails closed until bootstrapped" property.
    pub fn try_initialize_vault_without_config(&mut self) -> Result<(), FailedTransactionMetadata> {
        let payer = airdrop_keypair(&mut self.svm, 100_000_000_000);
        let mint_kp = Keypair::new();
        create_mint(
            &mut self.svm,
            &payer,
            &mint_kp,
            &payer.pubkey(),
            DEPOSIT_DECIMALS,
            TokenProgramKind::Spl,
        );
        let ix = initialize_ix(
            mint_kp.pubkey(),
            VAULT_VERSION,
            &payer.pubkey(),
            &payer.pubkey(),
            payer.pubkey(),
            payer.pubkey(),
            payer.pubkey(),
            TokenProgramKind::Spl,
            HARNESS_SHARE_OFFSET_U64,
        );
        send_tx(&mut self.svm, &payer, &[ix], &[&payer]).map(|_| ())
    }

    pub fn program_config(&self) -> Option<august_vault::state::config::ProgramConfig> {
        use anchor_lang::AccountDeserialize;
        let acct = self.svm.get_account(&program_config_pda())?;
        if acct.data.is_empty() {
            return None;
        }
        Some(
            august_vault::state::config::ProgramConfig::try_deserialize(&mut acct.data.as_slice())
                .expect("valid program config"),
        )
    }
}

impl Default for BareCtx {
    fn default() -> Self {
        Self::new()
    }
}

/// A snapshot of every state slot the CEI invariant covers. The CEI property
/// says: when a failure-mode `require!` fires, every slot here is unchanged.
///
/// **Maintenance contract**: this struct is the canonical list of "state the
/// CEI invariant guards." When you add a new mutable field to `VaultState`,
/// add a new token account that any state-changing instruction touches, or
/// introduce a new observable side effect, add the corresponding field here
/// and to `VaultCtx::snapshot()`. Otherwise CEI tests can pass while real
/// regressions slip through unobserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CeiSnapshot {
    pub user_shares: u64,
    pub user_deposit: u64,
    pub vault_tokens: u64,
    pub fee_recipient_tokens: u64,
    pub share_supply: u64,
    pub local_aum: u64,
    pub deployed_aum: u64,
    /// Not a balance, but it decides who may redeem, so an unintended change here
    /// is an exit freeze.
    pub withdrawal_queue_authority: Pubkey,
}

/// The on-chain custom error code for an `ErrorCode` variant.
///
/// **Variant-order dependency**: `expected as u32` returns the variant's
/// declaration ordinal (0..N) because Anchor's `#[error_code]` macro leaves
/// `ErrorCode` without explicit discriminants, and we add
/// `ANCHOR_USER_ERROR_OFFSET` to get the on-chain code. If anyone reorders
/// `errors.rs`, every caller silently matches the wrong variant — tests pass
/// against the wrong code. The `errors_discriminant_canary` test in
/// `programs/august-vault/src/state/vault.rs` pins the expected codes and
/// fails-fast on any drift. Keep the canary updated when adding variants.
///
/// Prefer [`assert_anchor_err`] for failed transactions; this is for callers
/// that hold a raw code instead (e.g. `AnchorError::error_code_number` from a
/// directly invoked `VaultState` helper).
pub fn vault_error_code(expected: ErrorCode) -> u32 {
    (expected as u32) + ANCHOR_USER_ERROR_OFFSET
}

/// The withdrawal fee the program charges on a redeem of `assets`: mirrors the
/// private `Redeem::ceil_div` rounding (fees round **up**, in the vault's
/// favour). Single test-side mirror of that formula — update it here if the
/// on-chain rounding ever changes.
pub fn expected_withdrawal_fee(assets: u64, fee_rate: u32) -> u64 {
    let numerator = (assets as u128) * (fee_rate as u128);
    numerator.div_ceil(FEE_RATE_DENOMINATOR_VALUE as u128) as u64
}

/// Every event of type `E` the transaction emitted, decoded from its
/// `Program data:` logs, in order.
pub fn events_of<E: AnchorDeserialize + Discriminator>(
    meta: &litesvm::types::TransactionMetadata,
) -> Vec<E> {
    use base64::Engine;
    let disc = E::DISCRIMINATOR;
    meta.logs
        .iter()
        .filter_map(|line| line.strip_prefix("Program data: "))
        .map(|b64| {
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .expect("Program data line is base64")
        })
        .filter(|bytes| bytes.starts_with(disc))
        .map(|bytes| E::try_from_slice(&bytes[disc.len()..]).expect("event body decodes"))
        .collect()
}

/// Assert a raw Anchor **framework** error code (the 2000/3000 ranges), for
/// failures that are not program `ErrorCode` variants — e.g. 3012
/// `AccountNotInitialized` or 2006 `ConstraintSeeds`.
pub fn assert_anchor_framework_err(err: &FailedTransactionMetadata, expected_code: u32) {
    assert_custom_code(
        err,
        expected_code,
        &format!("framework code {expected_code}"),
    );
}

/// Assert that a failed transaction's underlying error is the given
/// `ErrorCode` variant. Reads the raw `InstructionError::Custom(code)` from
/// the transaction-level error so we never depend on log-message wording.
/// See [`vault_error_code`] for the variant-order caveat this inherits.
pub fn assert_anchor_err(err: &FailedTransactionMetadata, expected: ErrorCode) {
    let expected_code = vault_error_code(expected);
    assert_custom_code(err, expected_code, &format!("{expected:?}"));
}

/// Shared by [`assert_anchor_err`] and [`assert_anchor_framework_err`]:
/// pull `InstructionError::Custom(code)` off the transaction-level error and
/// compare it, so neither depends on log-message wording.
fn assert_custom_code(err: &FailedTransactionMetadata, expected_code: u32, label: &str) {
    match &err.err {
        TransactionError::InstructionError(_, InstructionError::Custom(code)) => {
            assert_eq!(
                *code,
                expected_code,
                "expected {} (code {}), got code {}; logs:\n{}",
                label,
                expected_code,
                code,
                err.meta.logs.join("\n"),
            );
        }
        other => panic!(
            "expected Custom instruction error for {label}, got {other:?}; logs:\n{}",
            err.meta.logs.join("\n")
        ),
    }
}

// ---- low-level helpers ----

fn airdrop_keypair(svm: &mut LiteSVM, lamports: u64) -> Keypair {
    let kp = Keypair::new();
    svm.airdrop(&kp.pubkey(), lamports).expect("airdrop");
    kp
}

fn create_mint(
    svm: &mut LiteSVM,
    payer: &Keypair,
    mint_kp: &Keypair,
    mint_authority: &Pubkey,
    decimals: u8,
    token_program: TokenProgramKind,
) {
    let (mint_size, init_ix) = match token_program {
        TokenProgramKind::Spl => (
            SplMint::LEN,
            spl_token::instruction::initialize_mint(
                &spl_token::ID,
                &mint_kp.pubkey(),
                mint_authority,
                None,
                decimals,
            )
            .unwrap(),
        ),
        TokenProgramKind::Token2022 => (
            spl_token_2022::state::Mint::LEN,
            spl_token_2022::instruction::initialize_mint(
                &spl_token_2022::ID,
                &mint_kp.pubkey(),
                mint_authority,
                None,
                decimals,
            )
            .unwrap(),
        ),
    };
    let rent = Rent::default().minimum_balance(mint_size);
    let create_ix = system_instruction::create_account(
        &payer.pubkey(),
        &mint_kp.pubkey(),
        rent,
        mint_size as u64,
        &token_program.id(),
    );
    send_tx(svm, payer, &[create_ix, init_ix], &[payer, mint_kp]).expect("create mint");
}

/// A Token-2022 mint carrying `extensions`, initialized in the order the token
/// program requires: account, extension initializers, then the mint itself.
fn create_mint_2022_with_extensions(
    svm: &mut LiteSVM,
    payer: &Keypair,
    mint_kp: &Keypair,
    mint_authority: &Pubkey,
    decimals: u8,
    extensions: &[MintExtension],
) {
    use spl_token_2022::extension::{default_account_state, metadata_pointer, ExtensionType};
    use spl_token_2022::state::{AccountState, Mint as Mint2022};

    let types: Vec<ExtensionType> = extensions
        .iter()
        .map(|e| match e {
            MintExtension::MetadataPointer => ExtensionType::MetadataPointer,
            MintExtension::DefaultAccountStateFrozen => ExtensionType::DefaultAccountState,
        })
        .collect();
    let size = ExtensionType::try_calculate_account_len::<Mint2022>(&types).expect("mint size");
    let mint = mint_kp.pubkey();
    let mut ixs = vec![system_instruction::create_account(
        &payer.pubkey(),
        &mint,
        Rent::default().minimum_balance(size),
        size as u64,
        &spl_token_2022::ID,
    )];
    for e in extensions {
        ixs.push(match e {
            MintExtension::MetadataPointer => metadata_pointer::instruction::initialize(
                &spl_token_2022::ID,
                &mint,
                Some(*mint_authority),
                Some(mint),
            )
            .unwrap(),
            MintExtension::DefaultAccountStateFrozen => {
                default_account_state::instruction::initialize_default_account_state(
                    &spl_token_2022::ID,
                    &mint,
                    &AccountState::Frozen,
                )
                .unwrap()
            }
        });
    }
    // A frozen default state is only meaningful with a freeze authority to thaw
    // accounts, and Token-2022 refuses the mint without one.
    let freeze_authority = extensions
        .iter()
        .any(|e| matches!(e, MintExtension::DefaultAccountStateFrozen))
        .then_some(mint_authority);
    ixs.push(
        spl_token_2022::instruction::initialize_mint2(
            &spl_token_2022::ID,
            &mint,
            mint_authority,
            freeze_authority,
            decimals,
        )
        .unwrap(),
    );
    send_tx(svm, payer, &ixs, &[payer, mint_kp]).expect("create Token-2022 mint with extensions");
}

fn create_ata(
    svm: &mut LiteSVM,
    payer: &Keypair,
    owner: &Pubkey,
    mint: &Pubkey,
    token_program: TokenProgramKind,
) -> Pubkey {
    let ata = get_associated_token_address_with_program_id(owner, mint, &token_program.id());
    let ix = create_associated_token_account(&payer.pubkey(), owner, mint, &token_program.id());
    send_tx(svm, payer, &[ix], &[payer]).expect("create ATA");
    ata
}

/// SPL `approve` — what makes `operator_deposit` work once a vault has a
/// subaccount: the owner delegates its ATA to the vault PDA, which pulls against
/// that allowance.
fn approve_ix(
    token_program: TokenProgramKind,
    source_ata: &Pubkey,
    delegate: &Pubkey,
    owner: &Pubkey,
    amount: u64,
) -> Instruction {
    match token_program {
        TokenProgramKind::Spl => spl_token::instruction::approve(
            &spl_token::ID,
            source_ata,
            delegate,
            owner,
            &[],
            amount,
        )
        .unwrap(),
        TokenProgramKind::Token2022 => spl_token_2022::instruction::approve(
            &spl_token_2022::ID,
            source_ata,
            delegate,
            owner,
            &[],
            amount,
        )
        .unwrap(),
    }
}

/// SPL `revoke` — custody withdrawing the delegation. The incident-response
/// counterpart to `approve_ix`.
fn revoke_ix(token_program: TokenProgramKind, source_ata: &Pubkey, owner: &Pubkey) -> Instruction {
    match token_program {
        TokenProgramKind::Spl => {
            spl_token::instruction::revoke(&spl_token::ID, source_ata, owner, &[]).unwrap()
        }
        TokenProgramKind::Token2022 => {
            spl_token_2022::instruction::revoke(&spl_token_2022::ID, source_ata, owner, &[])
                .unwrap()
        }
    }
}

fn mint_to_ix(
    token_program: TokenProgramKind,
    mint: &Pubkey,
    dest: &Pubkey,
    authority: &Pubkey,
    amount: u64,
) -> Instruction {
    match token_program {
        TokenProgramKind::Spl => {
            spl_token::instruction::mint_to(&spl_token::ID, mint, dest, authority, &[], amount)
                .unwrap()
        }
        TokenProgramKind::Token2022 => spl_token_2022::instruction::mint_to(
            &spl_token_2022::ID,
            mint,
            dest,
            authority,
            &[],
            amount,
        )
        .unwrap(),
    }
}

fn send_tx(
    svm: &mut LiteSVM,
    payer: &Keypair,
    instructions: &[Instruction],
    signers: &[&Keypair],
) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
    // Rotate the blockhash before every send. LiteSVM rejects a byte-identical
    // transaction replayed under the same blockhash with `AlreadyProcessed`,
    // which surfaces as an opaque failure in any test that intentionally
    // repeats a call (a paused-then-unpaused deposit, a re-nomination, a
    // property walk that draws the same op twice). Giving every transaction a
    // fresh blockhash removes the hazard for all call sites at once.
    svm.expire_blockhash();
    let blockhash = svm.latest_blockhash();
    let tx =
        Transaction::new_signed_with_payer(instructions, Some(&payer.pubkey()), signers, blockhash);
    svm.send_transaction(tx)
}

// ---- program config bootstrap ----

/// The singleton program-config PDA.
pub fn program_config_pda() -> Pubkey {
    Pubkey::find_program_address(&[PROGRAM_CONFIG_SEED], &august_vault::ID).0
}

/// The loader-derived `ProgramData` address for this program.
fn program_data_pda() -> Pubkey {
    Pubkey::find_program_address(&[august_vault::ID.as_ref()], &bpf_loader_upgradeable::ID).0
}

/// Install a `ProgramData` account naming `upgrade_authority`.
///
/// LiteSVM's `add_program` installs programs under the non-upgradeable loader,
/// so no real `ProgramData` exists and `initialize_config` (which authenticates
/// against it) could not otherwise be exercised. The bytes are the bincode form
/// of `UpgradeableLoaderState::ProgramData`: a 4-byte enum index (3), an 8-byte
/// slot, then `Option<Pubkey>` as a 1-byte `Some` tag plus the 32-byte key.
fn install_program_data(svm: &mut LiteSVM, upgrade_authority: &Pubkey) {
    install_program_data_at(svm, program_data_pda(), upgrade_authority);
}

fn install_program_data_at(svm: &mut LiteSVM, address: Pubkey, upgrade_authority: &Pubkey) {
    write_program_data(svm, address, Some(upgrade_authority));
}

/// `upgrade_authority = None` models an **immutable** program (deployed or set
/// with `--final`), which is a materially different state from "some other key
/// holds it": `Some(signer)` can never equal `None`, so no one at all can pass
/// the `initialize_config` check.
fn write_program_data(svm: &mut LiteSVM, address: Pubkey, upgrade_authority: Option<&Pubkey>) {
    const PROGRAM_DATA_VARIANT: u32 = 3;
    let mut data = vec![0u8; UpgradeableLoaderState::size_of_programdata_metadata()];
    data[0..4].copy_from_slice(&PROGRAM_DATA_VARIANT.to_le_bytes());
    // data[4..12] is the deployment slot; 0 is fine, nothing reads it.
    if let Some(authority) = upgrade_authority {
        data[12] = 1; // Option::Some
        data[13..45].copy_from_slice(authority.as_ref());
    } // else leave the Option tag at 0 (None) and the key bytes zeroed

    let acct = SolanaAccount {
        lamports: Rent::default().minimum_balance(data.len()),
        data,
        owner: bpf_loader_upgradeable::ID,
        executable: false,
        rent_epoch: 0,
    };
    svm.set_account(address, acct)
        .unwrap_or_else(|e| panic!("set_account failed for ProgramData: {e:?}"));
}

/// The single construction site for `initialize_config`.
///
/// `BareCtx` previously built this account list inline as well. Two independent
/// copies are behaviourally identical today, but if this instruction ever gains a
/// separate rent payer (mirroring `try_initialize_vault_paid_by`) the inline copy
/// would keep sending `payer = signer`, both would still compile, and the tests
/// using it would silently stop covering the shape they claim to.
fn initialize_config_ix(
    upgrade_authority: &Pubkey,
    authority: Pubkey,
    program_data: Pubkey,
) -> Instruction {
    Instruction {
        program_id: august_vault::ID,
        accounts: ix_accounts::InitializeConfig {
            program_config: program_config_pda(),
            upgrade_authority: *upgrade_authority,
            payer: *upgrade_authority,
            program_data,
            system_program: solana_sdk::system_program::ID,
        }
        .to_account_metas(None),
        data: ix_data::InitializeConfig { authority }.data(),
    }
}

/// Bootstrap the config with `authority` as both the upgrade authority (per the
/// installed `ProgramData`) and the resulting vault-creation authority.
fn initialize_program_config(svm: &mut LiteSVM, authority: &Keypair) {
    let ix = initialize_config_ix(&authority.pubkey(), authority.pubkey(), program_data_pda());
    send_tx(svm, authority, &[ix], &[authority]).expect("initialize program config");
}

/// Build an `initialize` instruction for an arbitrary `(deposit_mint, version)`.
#[allow(clippy::too_many_arguments)]
fn initialize_ix(
    deposit_mint: Pubkey,
    vault_version: u8,
    signer: &Pubkey,
    payer: &Pubkey,
    admin: Pubkey,
    operator: Pubkey,
    fee_recipient: Pubkey,
    token_program: TokenProgramKind,
    share_offset: u64,
) -> Instruction {
    Instruction {
        program_id: august_vault::ID,
        accounts: ix_accounts::Initialize {
            program_config: program_config_pda(),
            signer: *signer,
            payer: *payer,
            deposit_mint,
            vault_state: derive_vault_state(&deposit_mint, vault_version).0,
            share_mint: derive_share_mint(&deposit_mint, vault_version).0,
            vault_token_ata: derive_vault_token_pda(&deposit_mint, vault_version).0,
            system_program: solana_sdk::system_program::ID,
            token_program: token_program.id(),
            rent: solana_sdk::sysvar::rent::ID,
        }
        .to_account_metas(None),
        data: ix_data::Initialize {
            admin,
            operator,
            fee_recipient,
            vault_version,
            share_offset,
        }
        .data(),
    }
}

// ---- PDA derivations (use program-side seed constants to prevent drift) ----

fn derive_vault_state(deposit_mint: &Pubkey, vault_version: u8) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[VAULT_STATE_SEED, deposit_mint.as_ref(), &[vault_version]],
        &august_vault::ID,
    )
}

fn derive_share_mint(deposit_mint: &Pubkey, vault_version: u8) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[SHARE_MINT_SEED, deposit_mint.as_ref(), &[vault_version]],
        &august_vault::ID,
    )
}

fn derive_vault_token_pda(deposit_mint: &Pubkey, vault_version: u8) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[VAULT_TOKEN_SEED, deposit_mint.as_ref(), &[vault_version]],
        &august_vault::ID,
    )
}
