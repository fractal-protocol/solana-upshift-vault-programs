//! Runtime tests for `create_share_token_metadata` and
//! `update_share_token_metadata` — the two instructions the July 2026
//! due-diligence review found had no passing runtime test anywhere
//! (instruction-entrypoint coverage was 15/17 without them).
//!
//! Both instructions CPI into Metaplex Token Metadata, so the SVM loads the
//! real (frozen) mainnet program from `tests/fixtures/mpl_token_metadata.so`;
//! see `tests/fixtures/README.md` for provenance. The instructions only
//! accept the legacy SPL Token program by type, so every vault here is
//! `VaultCtx::fresh()`.

use august_vault::errors::ErrorCode;
use integration_tests::harness::{assert_anchor_err, VaultCtx};

const NAME: &str = "August Vault Share";
const SYMBOL: &str = "AVS";
const URI: &str = "https://example.com/avs.json";

fn vault_with_mpl() -> VaultCtx {
    let mut ctx = VaultCtx::fresh();
    ctx.load_mpl_token_metadata();
    ctx
}

/// Metaplex stores name/symbol/uri right-padded with NULs to fixed widths;
/// trim before comparing.
fn trimmed(s: &str) -> &str {
    s.trim_end_matches('\0')
}

// ---- create: happy path ----

#[test]
fn create_share_token_metadata_writes_expected_fields() {
    let mut ctx = vault_with_mpl();
    let admin = ctx.admin.insecure_clone();

    ctx.create_share_token_metadata_as(&admin, NAME, SYMBOL, URI)
        .expect("admin creates metadata");

    let md = ctx.share_metadata();
    assert_eq!(trimmed(&md.name), NAME);
    assert_eq!(trimmed(&md.symbol), SYMBOL);
    assert_eq!(trimmed(&md.uri), URI);
    assert_eq!(md.mint, ctx.share_mint, "metadata must point at share mint");
    assert_eq!(
        md.update_authority, ctx.vault_state,
        "vault state PDA must be the update authority"
    );
    assert!(md.is_mutable, "metadata is created mutable");
    assert_eq!(md.seller_fee_basis_points, 0);
}

// ---- create: access control + pause gate ----

#[test]
fn create_share_token_metadata_rejects_non_admin() {
    let mut ctx = vault_with_mpl();
    let impostor = ctx.new_funded_keypair(1_000_000_000);

    let err = ctx
        .create_share_token_metadata_as(&impostor, NAME, SYMBOL, URI)
        .expect_err("non-admin must not create metadata");
    assert_anchor_err(&err, ErrorCode::UnauthorizedAdmin);
    assert!(
        ctx.svm.get_account(&ctx.share_metadata_pda()).is_none(),
        "no metadata account may exist after a rejected create"
    );
}

#[test]
fn create_share_token_metadata_rejects_when_paused() {
    let mut ctx = vault_with_mpl();
    let admin = ctx.admin.insecure_clone();
    ctx.pause().expect("admin pauses");

    let err = ctx
        .create_share_token_metadata_as(&admin, NAME, SYMBOL, URI)
        .expect_err("paused vault must reject metadata creation");
    assert_anchor_err(&err, ErrorCode::VaultPaused);
}

// ---- update: happy path ----

#[test]
fn update_share_token_metadata_replaces_fields() {
    let mut ctx = vault_with_mpl();
    let admin = ctx.admin.insecure_clone();
    ctx.create_share_token_metadata_as(&admin, NAME, SYMBOL, URI)
        .expect("create first");

    ctx.update_share_token_metadata_as(
        &admin,
        "Renamed Share",
        "RNS",
        "https://example.com/renamed.json",
    )
    .expect("admin updates metadata");

    let md = ctx.share_metadata();
    assert_eq!(trimmed(&md.name), "Renamed Share");
    assert_eq!(trimmed(&md.symbol), "RNS");
    assert_eq!(trimmed(&md.uri), "https://example.com/renamed.json");
    assert_eq!(
        md.update_authority, ctx.vault_state,
        "update authority must not drift on update"
    );
}

// ---- update: access control + pause gate ----

#[test]
fn update_share_token_metadata_rejects_non_admin() {
    let mut ctx = vault_with_mpl();
    let admin = ctx.admin.insecure_clone();
    ctx.create_share_token_metadata_as(&admin, NAME, SYMBOL, URI)
        .expect("create first");

    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let err = ctx
        .update_share_token_metadata_as(&impostor, "Evil", "EVL", "https://evil.example")
        .expect_err("non-admin must not update metadata");
    assert_anchor_err(&err, ErrorCode::UnauthorizedAdmin);

    let md = ctx.share_metadata();
    assert_eq!(
        trimmed(&md.name),
        NAME,
        "rejected update must not change name"
    );
}

#[test]
fn update_share_token_metadata_rejects_when_paused() {
    let mut ctx = vault_with_mpl();
    let admin = ctx.admin.insecure_clone();
    ctx.create_share_token_metadata_as(&admin, NAME, SYMBOL, URI)
        .expect("create first");

    ctx.pause().expect("admin pauses");
    let err = ctx
        .update_share_token_metadata_as(&admin, "Renamed", "RN", URI)
        .expect_err("paused vault must reject metadata updates");
    assert_anchor_err(&err, ErrorCode::VaultPaused);
}
