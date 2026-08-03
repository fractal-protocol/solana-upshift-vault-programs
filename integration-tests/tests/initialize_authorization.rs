//! Authorization for vault creation.
//!
//! `initialize` previously accepted any paying signer. Creation is now gated on
//! a `ProgramConfig` authority, bootstrapped by the program's upgrade authority
//! (read from the loader's `ProgramData`) and rotatable without a program
//! upgrade.
//!
//! Why the gate matters operationally: `vault_version` is a `u8` seed byte, so a
//! deposit mint has 256 `(mint, version)` namespaces and each is single-use.
//! `close_vault` cannot close the share mint (classic SPL Token has no
//! close-mint instruction) and irreversibly revokes its mint authority, so a
//! retired version can never be re-initialized.
//!
//! Coverage here:
//! - bootstrap: who may create the config, only once, and against *this*
//!   program's `ProgramData` only
//! - gating: unauthorized creation is rejected and consumes no namespace
//! - rotation: authority hand-off, old-key lockout, role separation
//! - the known limitation, pinned deliberately: a closed version stays dead

use august_vault::errors::ErrorCode;
use integration_tests::harness::{
    assert_anchor_err, assert_anchor_framework_err, BareCtx, VaultCtx,
};
use litesvm::types::FailedTransactionMetadata;
use solana_sdk::{
    instruction::InstructionError, pubkey::Pubkey, signature::Keypair, signature::Signer,
    transaction::TransactionError,
};

/// Versions used by namespace tests. Deliberately not 0, which the harness
/// vault already occupies.
const SPARE_VERSIONS: [u8; 4] = [1, 2, 7, 255];

// ---- bootstrap: initialize_config ----

#[test]
fn initialize_config_by_upgrade_authority_succeeds() {
    let mut ctx = BareCtx::new();
    assert!(
        ctx.program_config().is_none(),
        "config must not exist before bootstrap"
    );

    let upgrade_authority = ctx.upgrade_authority.insecure_clone();
    ctx.try_initialize_config_as(&upgrade_authority, upgrade_authority.pubkey())
        .expect("the program's upgrade authority may bootstrap the config");

    let cfg = ctx.program_config().expect("config exists after bootstrap");
    assert_eq!(cfg.authority, upgrade_authority.pubkey());
}

#[test]
fn initialize_config_rejects_non_upgrade_authority() {
    let mut ctx = BareCtx::new();
    let impostor = ctx.new_funded_keypair(10_000_000_000);

    let err = ctx
        .try_initialize_config_as(&impostor, impostor.pubkey())
        .expect_err("only the upgrade authority may bootstrap the config");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);
    assert!(
        ctx.program_config().is_none(),
        "rejected bootstrap must not create the config"
    );
}

/// An **immutable** program can never be bootstrapped, and therefore can never
/// create a vault.
///
/// The check is `program_data.upgrade_authority_address == Some(signer)`, and a
/// program with no upgrade authority stores `None`, which no `Some(_)` can equal.
/// So this is not "someone else holds the key" — it is nobody, permanently.
///
/// Recorded as a test because it imposes an ordering rule on deployment that is
/// invisible from the code: the config must be bootstrapped **before** anyone
/// considers making the program immutable. Reversing that order permanently
/// prevents vault creation, with no on-chain remedy. See docs/UPGRADE.md Step 2.
#[test]
fn immutable_program_can_never_bootstrap_config() {
    let mut ctx = BareCtx::new();
    let upgrade_authority = ctx.upgrade_authority.insecure_clone();
    ctx.make_program_immutable();

    // Not the erstwhile upgrade authority...
    let err = ctx
        .try_initialize_config_as(&upgrade_authority, upgrade_authority.pubkey())
        .expect_err("an immutable program has no authority to authorize this");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);

    // ...and not anyone else either.
    let anyone = ctx.new_funded_keypair(10_000_000_000);
    let err = ctx
        .try_initialize_config_as(&anyone, anyone.pubkey())
        .expect_err("nor may an arbitrary signer step in");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);

    assert!(
        ctx.program_config().is_none(),
        "no config may exist after either attempt"
    );
}

/// The upgrade authority need not keep the role: it can name a separate
/// operational key as the vault creator, so routine deployments do not require
/// the Fordefi-held upgrade key.
#[test]
fn initialize_config_can_delegate_to_a_different_authority() {
    let mut ctx = BareCtx::new();
    let ops_key = Keypair::new().pubkey();
    let upgrade_authority = ctx.upgrade_authority.insecure_clone();

    ctx.try_initialize_config_as(&upgrade_authority, ops_key)
        .expect("bootstrap may delegate");

    assert_eq!(ctx.program_config().unwrap().authority, ops_key);
}

/// The same zero-key guard both rotation instructions have. Bootstrapping to the
/// zero key would create a config nobody can authorize with, and because the
/// config is create-once the only remedy would be an upgrade-authority override —
/// or nothing at all, if the program had since been made immutable.
#[test]
fn initialize_config_rejects_the_zero_key() {
    let mut ctx = BareCtx::new();
    let upgrade_authority = ctx.upgrade_authority.insecure_clone();

    let err = ctx
        .try_initialize_config_as(&upgrade_authority, Pubkey::default())
        .expect_err("the zero key must not be installable as the authority");
    assert_anchor_err(&err, ErrorCode::InvalidAuthority);
    assert!(
        ctx.program_config().is_none(),
        "rejected bootstrap must not create the config"
    );

    // And the slot is still free for a real authority afterwards.
    ctx.try_initialize_config_as(&upgrade_authority, upgrade_authority.pubkey())
        .expect("a valid authority must still be installable");
}

#[test]
fn initialize_config_is_singleton() {
    let mut ctx = BareCtx::new();
    let upgrade_authority = ctx.upgrade_authority.insecure_clone();
    ctx.try_initialize_config_as(&upgrade_authority, upgrade_authority.pubkey())
        .expect("first bootstrap");

    let second = Keypair::new().pubkey();
    ctx.try_initialize_config_as(&upgrade_authority, second)
        .expect_err("the config may only be created once");

    assert_eq!(
        ctx.program_config().unwrap().authority,
        upgrade_authority.pubkey(),
        "failed re-bootstrap must not overwrite the stored authority"
    );
}

/// The `program_data` account is pinned by seeds to this program. Presenting
/// another program's `ProgramData` — one the caller genuinely controls — must
/// not authorize them here.
#[test]
fn initialize_config_rejects_foreign_program_data() {
    let mut ctx = BareCtx::new();
    let impostor = ctx.new_funded_keypair(10_000_000_000);

    // A ProgramData account at an unrelated address naming the impostor.
    let foreign = Keypair::new().pubkey();
    let impostor_key = impostor.pubkey();
    ctx.install_foreign_program_data(foreign, &impostor_key);

    let err = ctx
        .try_initialize_config_with_program_data(&impostor, impostor_key, foreign)
        .expect_err("program_data must be this program's own");
    // Pin the reason, matching the override twin: a bare `expect_err` would pass
    // on any failure, including a harness-side mistake such as a wrong account
    // order or a missing signer. The seeds constraint rejects this before the
    // upgrade-authority comparison is ever reached.
    assert_anchor_framework_err(&err, 2006);
    assert!(ctx.program_config().is_none());
}

// ---- gating: initialize ----

/// The property the whole rollout rests on: a program whose config has never
/// been created cannot create vaults at all. Everything else in this file runs
/// on `VaultCtx`, which bootstraps the config during setup, so without this test
/// a change that relaxed `program_config` (to `UncheckedAccount`, `Option<_>`, or
/// `init_if_needed`) would silently restore permissionless creation while every
/// other test kept passing.
#[test]
fn initialize_fails_closed_when_config_is_missing() {
    let mut ctx = BareCtx::new();
    assert!(ctx.program_config().is_none(), "precondition: no config");

    let err = ctx
        .try_initialize_vault_without_config()
        .expect_err("no vault may be created before the config exists");
    // Anchor's AccountNotInitialized — the config account cannot be loaded.
    assert_anchor_framework_err(&err, 3012);
}

/// An unauthorized caller creates nothing whatever their balance — but *which*
/// error they see depends on it, and that is a property of Anchor's codegen
/// rather than of this account list.
///
/// The previous version of this test funded the impostor's rent from a separate
/// solvent payer, so the System Program never had the chance to fail and the
/// test could not detect the behaviour it claimed to pin. Both balances are
/// covered here.
///
/// `anchor_syn::codegen::accounts::try_accounts::generate_constraints` emits
/// every `init` field's creation CPI ahead of *all* non-init access checks, so
/// the authority `constraint` on `signer` runs after `vault_state` is created.
/// With a solvent payer that creation succeeds and the constraint rejects with
/// `NotProtocolAuthority`; with the impostor paying their own way and unable to
/// afford the rent, the System Program rejects first. The security property —
/// no vault, no namespace consumed — holds either way.
///
/// If a future Anchor release evaluates access checks before `init`, the second
/// assertion fails and the comment on `Initialize::program_config` should be
/// revisited.
#[test]
fn unauthorized_signer_creates_nothing_whatever_their_balance() {
    let mut ctx = VaultCtx::fresh();
    let mint = ctx.create_extra_deposit_mint();

    // Solvent impostor with a separate solvent payer: the rent is funded, so
    // the authority constraint is reached and is what rejects.
    let rich_impostor = ctx.new_funded_keypair(100_000_000);
    let payer = ctx.payer.insecure_clone();
    let err = ctx
        .try_initialize_vault_paid_by(&rich_impostor, &payer, mint, SPARE_VERSIONS[0])
        .expect_err("unauthorized");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);
    assert_vault_pdas_absent(&ctx, mint, SPARE_VERSIONS[0]);

    // Impostor paying their own rent, far too poor to fund even one of the
    // three accounts (~0.0076 SOL total). Still rejected, still creates
    // nothing, but the System Program gets there first.
    let broke_impostor = ctx.new_funded_keypair(1_000_000);
    let err = ctx
        .try_initialize_vault_paid_by(&broke_impostor, &broke_impostor, mint, SPARE_VERSIONS[1])
        .expect_err("unauthorized and unable to fund rent");
    assert_system_program_insufficient_lamports(&err);
    assert_vault_pdas_absent(&ctx, mint, SPARE_VERSIONS[1]);
}

/// A rejected `initialize` must leave all three PDAs untouched.
fn assert_vault_pdas_absent(ctx: &VaultCtx, deposit_mint: Pubkey, vault_version: u8) {
    for pda in ctx.vault_pdas(deposit_mint, vault_version) {
        assert!(
            ctx.svm.get_account(&pda).is_none_or(|a| a.lamports == 0),
            "rejected initialize created {pda}"
        );
    }
}

/// `SystemError::ResultWithNegativeLamports` — surfaced as `Custom(1)` from the
/// `init` CPI, i.e. not one of this program's error codes.
fn assert_system_program_insufficient_lamports(err: &FailedTransactionMetadata) {
    match &err.err {
        TransactionError::InstructionError(_, InstructionError::Custom(code)) => assert_eq!(
            *code, 1,
            "expected the System Program's insufficient-lamports error, got custom code {code}"
        ),
        other => panic!("expected an insufficient-lamports failure, got {other:?}"),
    }
}

/// A cold or MPC-held protocol authority must be able to authorize creation
/// without holding SOL — the reason `payer` is a separate account.
#[test]
fn initialize_allows_a_separate_rent_payer() {
    let mut ctx = VaultCtx::fresh();
    let mint = ctx.create_extra_deposit_mint();
    let authority = ctx.protocol_authority.insecure_clone();
    let payer = ctx.payer.insecure_clone();

    // Drain nothing, just prove the authority never has to pay: it signs, the
    // separate payer funds.
    let before = ctx.svm.get_balance(&authority.pubkey()).unwrap_or(0);
    ctx.try_initialize_vault_paid_by(&authority, &payer, mint, SPARE_VERSIONS[1])
        .expect("a separate payer may fund creation");
    let after = ctx.svm.get_balance(&authority.pubkey()).unwrap_or(0);
    assert_eq!(
        before, after,
        "the protocol authority must not be charged rent or fees"
    );
}

#[test]
fn initialize_accepts_the_protocol_authority() {
    let mut ctx = VaultCtx::fresh();
    let mint = ctx.create_extra_deposit_mint();
    let authority = ctx.protocol_authority.insecure_clone();

    ctx.try_initialize_vault(&authority, mint, SPARE_VERSIONS[0])
        .expect("the protocol authority may create vaults");
}

/// The core of the finding: an arbitrary funded signer can no longer create a
/// vault.
#[test]
fn initialize_rejects_unauthorized_signer() {
    let mut ctx = VaultCtx::fresh();
    let mint = ctx.create_extra_deposit_mint();
    let impostor = ctx.new_funded_keypair(10_000_000_000);

    let err = ctx
        .try_initialize_vault(&impostor, mint, SPARE_VERSIONS[0])
        .expect_err("an arbitrary signer must not create a vault");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);
}

/// Vault admin is a per-vault role; holding it must not confer the
/// program-wide right to create vaults.
#[test]
fn initialize_rejects_vault_admin() {
    let mut ctx = VaultCtx::fresh();
    let mint = ctx.create_extra_deposit_mint();
    let admin = ctx.admin.insecure_clone();

    let err = ctx
        .try_initialize_vault(&admin, mint, SPARE_VERSIONS[0])
        .expect_err("a vault admin must not create vaults");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);
}

/// A rejected attempt must leave the namespace pristine — otherwise the DoS
/// survives the fix, just with an extra step.
#[test]
fn rejected_initialize_consumes_no_namespace() {
    let mut ctx = VaultCtx::fresh();
    let mint = ctx.create_extra_deposit_mint();
    let impostor = ctx.new_funded_keypair(10_000_000_000);
    let version = SPARE_VERSIONS[1];

    ctx.try_initialize_vault(&impostor, mint, version)
        .expect_err("unauthorized");

    // No account of the failed attempt survives...
    for pda in ctx.vault_pdas(mint, version) {
        assert!(
            ctx.svm.get_account(&pda).is_none_or(|a| a.lamports == 0),
            "failed initialize left {pda} allocated"
        );
    }

    // ...and the authority can still take the version afterwards.
    let authority = ctx.protocol_authority.insecure_clone();
    ctx.try_initialize_vault(&authority, mint, version)
        .expect("version must remain available after a rejected attempt");
}

/// The finding's attack, end to end: an unauthorized party sweeping versions
/// gets nowhere, and every version stays available to the protocol.
#[test]
fn unauthorized_signer_cannot_squat_the_version_namespace() {
    let mut ctx = VaultCtx::fresh();
    let mint = ctx.create_extra_deposit_mint();
    let impostor = ctx.new_funded_keypair(50_000_000_000);

    for version in SPARE_VERSIONS {
        let err = ctx
            .try_initialize_vault(&impostor, mint, version)
            .unwrap_err();
        assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);
    }

    let authority = ctx.protocol_authority.insecure_clone();
    for version in SPARE_VERSIONS {
        ctx.try_initialize_vault(&authority, mint, version)
            .unwrap_or_else(|e| panic!("version {version} should still be free: {e:?}"));
    }
}

/// Multiple versions per deposit mint is the intended Model B behaviour; the
/// gate must not break it.
#[test]
fn protocol_authority_can_create_several_versions_per_mint() {
    let mut ctx = VaultCtx::fresh();
    let mint = ctx.create_extra_deposit_mint();
    let authority = ctx.protocol_authority.insecure_clone();

    for version in SPARE_VERSIONS {
        ctx.try_initialize_vault(&authority, mint, version)
            .unwrap_or_else(|e| panic!("version {version} must be creatable: {e:?}"));
    }
    // And a version cannot be taken twice.
    ctx.try_initialize_vault(&authority, mint, SPARE_VERSIONS[0])
        .expect_err("an occupied version must not be re-initialized");
}

// ---- rotation: set_config_authority ----

#[test]
fn set_config_authority_hands_over_creation_rights() {
    let mut ctx = VaultCtx::fresh();
    let old = ctx.protocol_authority.insecure_clone();
    let new = ctx.new_funded_keypair(10_000_000_000);
    let mint = ctx.create_extra_deposit_mint();

    ctx.set_config_authority_as(&old, new.pubkey())
        .expect("current authority may rotate");
    assert_eq!(ctx.program_config().authority, new.pubkey());

    ctx.try_initialize_vault(&new, mint, SPARE_VERSIONS[0])
        .expect("the new authority may create vaults");
}

#[test]
fn rotated_out_authority_is_locked_out() {
    let mut ctx = VaultCtx::fresh();
    let old = ctx.protocol_authority.insecure_clone();
    let new = ctx.new_funded_keypair(10_000_000_000);
    let mint = ctx.create_extra_deposit_mint();

    ctx.set_config_authority_as(&old, new.pubkey())
        .expect("rotate");

    let err = ctx
        .try_initialize_vault(&old, mint, SPARE_VERSIONS[0])
        .expect_err("the previous authority must lose creation rights");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);

    let err = ctx
        .set_config_authority_as(&old, old.pubkey())
        .expect_err("the previous authority must not rotate the role back");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);
}

#[test]
fn set_config_authority_rejects_non_authority() {
    let mut ctx = VaultCtx::fresh();
    let impostor = ctx.new_funded_keypair(10_000_000_000);
    let before = ctx.program_config().authority;

    let err = ctx
        .set_config_authority_as(&impostor, impostor.pubkey())
        .expect_err("only the current authority may rotate");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);
    assert_eq!(
        ctx.program_config().authority,
        before,
        "rejected rotation must not change the stored authority"
    );
}

/// `set_config_authority` is the *config authority's* instruction: holding the
/// upgrade authority does not grant it. (The upgrade authority's own path is
/// `override_config_authority`, covered below.)
#[test]
fn upgrade_authority_cannot_use_set_config_authority() {
    let mut ctx = VaultCtx::fresh();
    let ops = ctx.new_funded_keypair(10_000_000_000);
    let original = ctx.protocol_authority.insecure_clone();
    ctx.set_config_authority_as(&original, ops.pubkey())
        .expect("delegate to an ops key");

    // The harness's protocol authority is also the installed upgrade authority,
    // so this proves the two roles are checked independently rather than that
    // the signer merely happens to be the wrong key.
    let err = ctx
        .set_config_authority_as(&original, original.pubkey())
        .expect_err("upgrade authority must not rotate via set_config_authority");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);
    assert_eq!(ctx.program_config().authority, ops.pubkey());
}

#[test]
fn set_config_authority_rejects_the_zero_key() {
    let mut ctx = VaultCtx::fresh();
    let authority = ctx.protocol_authority.insecure_clone();

    let err = ctx
        .set_config_authority_as(&authority, Pubkey::default())
        .expect_err("the zero key is unsignable and must be rejected");
    assert_anchor_err(&err, ErrorCode::InvalidAuthority);
    assert_eq!(ctx.program_config().authority, authority.pubkey());
}

// ---- recovery: override_config_authority ----

/// The recovery path that makes single-step rotation safe: if the config
/// authority is wrong or lost, the upgrade authority can reset it without a
/// program upgrade.
#[test]
fn upgrade_authority_can_override_a_lost_config_authority() {
    let mut ctx = VaultCtx::fresh();
    let original = ctx.protocol_authority.insecure_clone();
    let mint = ctx.create_extra_deposit_mint();

    // Rotate to a key nobody holds — the "lost key" scenario.
    let unreachable = Keypair::new().pubkey();
    ctx.set_config_authority_as(&original, unreachable)
        .expect("rotate away");
    let err = ctx
        .try_initialize_vault(&original, mint, SPARE_VERSIONS[0])
        .expect_err("creation is now impossible for anyone we can sign as");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);

    // The upgrade authority recovers it.
    ctx.override_config_authority_as(&original, original.pubkey())
        .expect("upgrade authority may reset the config authority");
    assert_eq!(ctx.program_config().authority, original.pubkey());
    ctx.try_initialize_vault(&original, mint, SPARE_VERSIONS[0])
        .expect("creation works again after recovery");
}

#[test]
fn override_config_authority_rejects_non_upgrade_authority() {
    let mut ctx = VaultCtx::fresh();
    let impostor = ctx.new_funded_keypair(10_000_000_000);
    let before = ctx.program_config().authority;

    let err = ctx
        .override_config_authority_as(&impostor, impostor.pubkey())
        .expect_err("only the upgrade authority may override");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);
    assert_eq!(ctx.program_config().authority, before);
}

/// The recovery path must verify it is reading **this** program's `ProgramData`,
/// not merely some account the loader owns.
///
/// `override_config_authority` declares its own `program_data` constraint rather
/// than sharing one with `initialize_config`, so the spoofing test on that
/// instruction gives this one no cover. Without `seeds` + `seeds::program`,
/// `Account<'_, ProgramData>` would still check loader ownership — so an attacker
/// need only be the upgrade authority of *any* upgradeable program, deploy a
/// throwaway one, and present its `ProgramData` to seize vault creation on this
/// program.
#[test]
fn override_config_authority_rejects_foreign_program_data() {
    let mut ctx = VaultCtx::fresh();
    let impostor = ctx.new_funded_keypair(10_000_000_000);
    let before = ctx.program_config().authority;

    // A ProgramData account at an unrelated address naming the impostor — i.e.
    // the impostor genuinely is an upgrade authority, just not of this program.
    let foreign = Keypair::new().pubkey();
    let impostor_key = impostor.pubkey();
    ctx.install_foreign_program_data(foreign, &impostor_key);

    let err = ctx
        .override_config_authority_with_program_data(&impostor, impostor_key, foreign)
        .expect_err("program_data must be this program's own");
    // The seeds constraint rejects it before the authority comparison is reached.
    assert_anchor_framework_err(&err, 2006);
    assert_eq!(
        ctx.program_config().authority,
        before,
        "a rejected override must not move the authority"
    );
}

/// An immutable program has no upgrade authority at all, so the recovery path is
/// permanently closed — `Some(signer)` can never equal `None`.
///
/// This pins the `None` handling in `override_config_authority`'s own copy of the
/// constraint. The natural-looking "handle the None case" refactor
/// (`map_or(true, |a| a == signer)`) would make this instruction callable by
/// **anyone** once the program is set `--final`, and the equivalent test on
/// `initialize_config` would not notice.
///
/// Operationally this is the trade-off behind single-step rotation: making the
/// program immutable also permanently forfeits config-authority recovery.
#[test]
fn immutable_program_can_never_override_config_authority() {
    let mut ctx = VaultCtx::fresh();
    let original = ctx.protocol_authority.insecure_clone();
    let before = ctx.program_config().authority;

    ctx.make_program_immutable();

    // Not the erstwhile upgrade authority...
    let err = ctx
        .override_config_authority_as(&original, original.pubkey())
        .expect_err("an immutable program has no upgrade authority to satisfy");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);

    // ...and not anyone else either.
    let anyone = ctx.new_funded_keypair(10_000_000_000);
    let err = ctx
        .override_config_authority_as(&anyone, anyone.pubkey())
        .expect_err("nobody can satisfy a None upgrade authority");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);

    assert_eq!(ctx.program_config().authority, before);
}

/// Unlike the previous test, this one is sensitive to the `ProgramData` contents
/// the instruction actually reads: rotating the on-chain upgrade authority moves
/// who may override.
#[test]
fn override_config_authority_follows_the_program_data_authority() {
    let mut ctx = VaultCtx::fresh();
    let original = ctx.protocol_authority.insecure_clone();
    let new_upgrade_authority = ctx.new_funded_keypair(10_000_000_000);
    let key = new_upgrade_authority.pubkey();

    // Before rotating ProgramData, the newcomer cannot override.
    ctx.override_config_authority_as(&new_upgrade_authority, key)
        .expect_err("not yet the upgrade authority");

    ctx.set_program_data_upgrade_authority(&key);

    // Now it can, and the previous upgrade authority cannot.
    ctx.override_config_authority_as(&new_upgrade_authority, key)
        .expect("the current ProgramData authority may override");
    assert_eq!(ctx.program_config().authority, key);

    let err = ctx
        .override_config_authority_as(&original, original.pubkey())
        .expect_err("the superseded upgrade authority must lose the power");
    assert_anchor_err(&err, ErrorCode::NotProtocolAuthority);
}

#[test]
fn override_config_authority_rejects_the_zero_key() {
    let mut ctx = VaultCtx::fresh();
    let authority = ctx.protocol_authority.insecure_clone();

    let err = ctx
        .override_config_authority_as(&authority, Pubkey::default())
        .expect_err("the zero key must be rejected here too");
    assert_anchor_err(&err, ErrorCode::InvalidAuthority);
}

// ---- known limitation, pinned on purpose ----

/// Closing a vault does not free its version. `close_vault` cannot close the
/// share mint and revokes its authority to `None`, which no one can restore, so
/// the `(mint, version)` pair is spent for good.
///
/// This is asserted deliberately rather than left to chance: it is the reason
/// the 256-version namespace is a finite resource, and it is why gating
/// `initialize` was the fix rather than making closed versions reusable. If a
/// future change makes reuse possible, this test should fail and be rewritten.
#[test]
fn closed_version_cannot_be_reinitialized() {
    let mut ctx = VaultCtx::fresh();
    let mint = ctx.deposit_mint;
    let authority = ctx.protocol_authority.insecure_clone();

    // A fresh vault is already empty, so it can be closed immediately.
    ctx.close_vault().expect("admin closes the empty vault");

    // The share mint survives with its authority revoked — the root cause.
    let share_mint = ctx.share_mint;
    let acct = ctx
        .svm
        .get_account(&share_mint)
        .expect("share mint account outlives the vault");
    assert!(
        acct.lamports > 0,
        "share mint rent is stranded, not reclaimed"
    );
    assert!(
        ctx.share_mint_authority().is_none(),
        "close_vault must revoke the mint authority (irreversibly)"
    );

    ctx.try_initialize_vault(&authority, mint, integration_tests::harness::VAULT_VERSION)
        .expect_err("a closed version can never be re-initialized");
}
