# Verified Upgrade Runbook (P4)

How to upgrade the deployed `august_vault` program to a **reproducibly-built,
attested** binary and register it as **verified** on-chain (OtterSec →
explorers). This aligns the live mainnet bytecode with the public source, which
today it does **not** match (the current deployment predates verifiable builds —
see [VERIFY.md](../VERIFY.md)).

> **This changes live mainnet bytecode over real user funds.** Do not deviate
> from this runbook. **Three** transactions must be signed by the program's
> **Fordefi MPC upgrade authority** — the `Upgrade` in Step 2, the verify-PDA
> upload in Step 3, and `initialize_config` in Step 4 — so book three approval
> ceremonies. Everything else is unprivileged prep. Verification is
> **point-in-time** (the program stays upgradeable).

## Key facts

| | Mainnet | Devnet |
| --- | --- | --- |
| Program ID | `up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt` | `C8B1EpsSGVWK2vMrk3aDT3kL7RCE77otokUh4EC35kK7` |
| Upgrade authority | `B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM` (Fordefi MPC) | `APuzErEVGAbvhyj2hbmo6vp7pacNHRVXbu43UhcAne2i` |
| ProgramData account size (at writing) | 507,781 B | verify with `solana account <programData>` |

- **Expected reproducible hash** (mainnet build, committed source): the
  `exec_sha256` / `raw_sha256` / `size` in [`verified-hashes.txt`](../verified-hashes.txt)
  (currently `555ffd9f…`, 595,912 B), built with `solana-verify` 0.5.1 in
  `solanafoundation/solana-verifiable-build@sha256:695f890e…` (Solana 2.3.0).
- **ProgramData must be extended first:** the new `.so` (595,912 B) is larger
  than the current allocation, so `solana program extend` is required or the
  upgrade fails. Deficit = `595,912 + 45 (loader header) − 507,781 = 88,176` bytes
  (re-derive if the sizes change).
- **Bootstrap the program config after upgrading:** vault creation is gated on a
  `ProgramConfig` authority that does not exist yet. Until `initialize_config` is
  run — by the upgrade authority, so a Fordefi-signed transaction on mainnet —
  `initialize` fails closed and no new vault can be created. Existing vaults are
  unaffected. See [Step 4](#step-4--bootstrap-the-program-config).

## Prerequisites

- `solana-verify` 0.5.1 + Docker (for the reproducible build).
- Solana CLI.
- A **funded ops fee-payer** keypair (a few SOL) — pays for `extend` /
  `write-buffer`. This is NOT the upgrade authority.
- Fordefi access to the upgrade authority key, able to sign a
  `BPFLoaderUpgradeable::Upgrade` instruction and two arbitrary transactions
  (the verify-PDA upload in Step 3 and `initialize_config` in Step 4).
- The repository protections below provisioned. (The workflow fails closed
  without the machine-enforced ones — `release` env reviewers, immutable
  releases, and the admin-read token; the `v*` tag ruleset is human-audited.)

## Repository protections (one-time setup)

All four are prerequisites for a release. The workflow **enforces** (fails
closed on) #1 (`release` environment reviewers), #2 (immutable releases), and #4
(the token). The `v*` tag ruleset (#3) is **human-audited** — the workflow does
not machine-verify it (see #3), so a missing or weakened ruleset will NOT stop a
release; audit it out of band. (GitHub would otherwise auto-create a referenced
missing environment *without* protection, and the default `GITHUB_TOKEN` can't
read these settings.)

1. **`release` environment** (Settings → Environments) with **required
   reviewers** — gates the privileged `publish` job (attest + release).
2. **Immutable releases** enabled (`PUT /repos/{owner}/{repo}/immutable-releases`)
   — so published assets can't be swapped after publication.
3. **A `v*` tag ruleset** (Settings → Rules → Rulesets), **human-audited** (the
   workflow does not machine-verify it — see below): enforcement **Active**,
   target **Tags**, ref-name condition **exactly `refs/tags/v*`** with **no
   exclusion** that re-opens it, rules **Restrict creations + Restrict updates +
   Restrict deletions**, and a **bypass list containing only repo admins** (no
   non-admin user/team/app with always/exempt bypass — a bypass actor defeats
   the "admin-only tag" guarantee). This is audited by a human because a sound
   automated check needs `bypass_actors`, which GitHub returns only with *write*
   access to the ruleset (more than the read token below has). The runtime
   backstop is the workflow asserting the trigger tag resolves to the CI-green
   default-branch commit.
4. **`RELEASE_ADMIN_TOKEN`** secret scoped to the `release` environment — a
   **fine-grained PAT** with **Administration: read** (for the immutable-release
   setting) + **Actions: read** (the `GET /environments/{name}` endpoint maps to
   Actions, not Environments) on this repo — a static, reusable secret. The guard
   uses it to verify #1 and #2 (the default `GITHUB_TOKEN` can't read those) and
   fails closed if it is missing. *If you prefer a GitHub App:* its installation token **expires
   hourly**, so do NOT store it as this secret — instead store the App ID +
   private key and mint a fresh token per run (e.g. `actions/create-github-app-token`).

---

## Step 0 — Cut an attested release

Tag a commit on the default branch; `release.yml` builds it reproducibly,
asserts `verified-hashes.txt`, attests provenance, and publishes the `.so`.

```bash
git checkout <commit-on-default-branch>
git tag v0.1.0 && git push origin v0.1.0     # v* tags are admin-only per the tag ruleset
```

Download the released `august_vault_v0.1.0.so`, and confirm it matches:

```bash
solana-verify get-executable-hash august_vault_v0.1.0.so   # == verified-hashes.txt exec_sha256
sha256sum august_vault_v0.1.0.so                            # == raw_sha256
```

Also confirm the GitHub **build attestation** exists for the asset
(`…/attestations`). Everything below deploys THIS artifact.

---

## Step 1 — Devnet dry-run (rehearse everything)

> `declare_id!` is baked into the bytecode, so the **devnet** artifact must
> declare the devnet program ID. Build a devnet-targeted `.so` (this hashes
> differently from mainnet's `555ffd9f…` — expected; the dry-run validates
> mechanics + state compatibility, not the mainnet bytes):

```bash
# On a scratch checkout of the release commit:
sed -i 's/declare_id!("up12byto[^"]*")/declare_id!("C8B1EpsSGVWK2vMrk3aDT3kL7RCE77otokUh4EC35kK7")/' \
  programs/august-vault/src/lib.rs
solana-verify build --library-name august_vault \
  --base-image solanafoundation/solana-verifiable-build@sha256:695f890e620db8c39afe5112e048599f8ee395a0cab5a2e572f30a72c6366cb4
```

Then upgrade the **devnet** program (`C8B1Eps…`). Its upgrade authority
(`APuzEr…`) is a **regular keypair the team holds — NOT a Fordefi MPC** — so
sign the devnet upgrade **directly with the CLI**. (This is the one place the
dry-run differs from mainnet: on mainnet the authority is Fordefi and the buffer
is handed off to it; on devnet the same held keypair can buffer + deploy in one
step.)

```bash
DEVNET_SO=target/deploy/august_vault.so   # the devnet-ID build from above

# Extend ProgramData first if the new .so is larger than the current allocation.
solana program show C8B1EpsSGVWK2vMrk3aDT3kL7RCE77otokUh4EC35kK7 -u devnet   # inspect ProgramData len
# solana program extend C8B1Eps… <deficit_bytes> -u devnet -k <ops-payer.json>   # if needed

# Upgrade, signed directly by the devnet authority keypair (writes buffer + deploys):
solana program deploy "$DEVNET_SO" \
  --program-id C8B1EpsSGVWK2vMrk3aDT3kL7RCE77otokUh4EC35kK7 \
  --upgrade-authority <devnet-authority-keypair.json> \
  -u devnet
```

Afterwards:

- Refresh the fork-test fixtures for a devnet vault and run
  `cargo test --manifest-path integration-tests/Cargo.toml --test mainnet_fork_compat`,
  **or** interact with a devnet vault (deposit/redeem/operator) and confirm
  correct behavior.
- Confirm `solana-verify get-program-hash -u devnet C8B1Eps…` equals the
  devnet build's hash.

Only proceed to mainnet once the devnet rehearsal is clean.

---

## Step 2 — Mainnet verified upgrade

**Pre-flight (abort on any mismatch):**

```bash
# 1. Confirm the upgrade authority is still the expected Fordefi key.
solana program show up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt -u mainnet-beta
#    Authority must be B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM

# 2. Re-run the mainnet-fork layout guard against fresh fixtures (refresh them
#    first per integration-tests/tests/mainnet_fork_compat.rs), and full CI green.

# 3. Confirm the release .so is the verified artifact.
solana-verify get-executable-hash august_vault_v0.1.0.so   # == verified-hashes.txt

# 4. Preserve the CURRENTLY-deployed binary for byte-exact rollback. It is not
#    reproducible from source, but it IS byte-recoverable right now — archive it.
solana program dump up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt \
  pre-upgrade-august_vault.so -u mainnet-beta
solana-verify get-program-hash -u mainnet-beta up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt   # dcd22bfa…
sha256sum pre-upgrade-august_vault.so
#    Store pre-upgrade-august_vault.so + these hashes in secure archival (this
#    is the only way to byte-restore the pre-upgrade program — see Rollback).
```

**Prepare the buffer (unprivileged, ops fee-payer):**

```bash
# Extend ProgramData to fit the larger binary.
# Re-derive first — this value is release-specific:
#   solana program show up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt -u m   # current allocation
#   additional_bytes = <new .so size> + 45 - <current ProgramData account size>
# For the hashes in verified-hashes.txt: 595,912 + 45 - 507,781 = 88,176.
solana program extend up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt 88176 \
  -u mainnet-beta -k <ops-payer.json>

# Upload the verified .so into a buffer.
solana program write-buffer august_vault_v0.1.0.so -u mainnet-beta -k <ops-payer.json>
#   -> Buffer: <BUFFER_ADDRESS>

# Hand the buffer to the upgrade authority so Fordefi can consume it.
solana program set-buffer-authority <BUFFER_ADDRESS> \
  --new-buffer-authority B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM \
  -u mainnet-beta -k <ops-payer.json>
```

**(Optional) pause user operations** via the admin key during the window
(note: pause does not stop operator withdrawals).

**Execute the upgrade (Fordefi-signed):** via Fordefi, submit a
`BPFLoaderUpgradeable::Upgrade` instruction:

- program = `up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt`
- buffer = `<BUFFER_ADDRESS>`
- authority = `B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM`
- spill = the ops fee-payer

Do **not** set the program immutable (no `--final`) — and note this is now
load-bearing beyond keeping future upgrades possible. `initialize_config`
authorizes against `program_data.upgrade_authority_address == Some(signer)`, and
an immutable program stores `None`, which no signer can ever match. Making the
program immutable therefore **permanently prevents the config from being created,
and so permanently prevents any new vault from being created**, with no on-chain
remedy. If immutability is ever wanted, Step 4 must happen first. Pinned by
`immutable_program_can_never_bootstrap_config` in
`integration-tests/tests/initialize_authorization.rs`.

**Post-upgrade verification:**

```bash
solana-verify get-program-hash -u mainnet-beta up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt
#   Now equals verified-hashes.txt exec_sha256 (555ffd9f…)
```

If you paused, **unpause first** — `deposit` and `redeem` are pause-gated, so
the smoke test fails with `VaultPaused` while paused. Then smoke-test one vault
with a tiny deposit + redeem and confirm the expected shares/assets.

---

## Step 3 — Register on-chain verification (Fordefi-signed)

```bash
# Build the (unsigned) verify-PDA upload transaction for the authority as uploader.
# Pin the exact release commit, library, image, and cluster — an omitted
# --commit-hash resolves to the remote default-branch HEAD (wrong if `init`
# advanced after tagging) and an omitted -u uses the CLI's configured cluster
# (which Step 1 pointed at devnet).
solana-verify export-pda-tx \
  https://github.com/fractal-protocol/solana-upshift-vault-programs \
  --program-id up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt \
  --uploader B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM \
  --commit-hash <RELEASE_COMMIT_SHA> \
  --library-name august_vault \
  --base-image solanafoundation/solana-verifiable-build@sha256:695f890e620db8c39afe5112e048599f8ee395a0cab5a2e572f30a72c6366cb4 \
  -u mainnet-beta \
  --encoding base58
```

- Sign + submit that transaction via **Fordefi** (the uploader must be the
  upgrade authority for explorers to trust the record).
- Submit the OtterSec remote job and poll it (mainnet explicit):

```bash
solana-verify remote submit-job --program-id up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt \
  --uploader B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM -u mainnet-beta
solana-verify remote get-job --job-id <JOB_ID> -u mainnet-beta   # wait for success
```

**Uploading the PDA alone is not sufficient — the remote job must succeed.**
Then confirm the "verified" badge on Solana Explorer, SolanaFM, Solscan, and
`https://verify.osec.io/status/up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt`.

---

## Step 4 — Bootstrap the program config

`initialize` is gated on the `ProgramConfig` authority, and that account does not
exist on a program that has never had it created. Until it does, **`initialize`
fails closed and no new vault can be created** — existing vaults keep working
normally, so this is not urgent, but the first new-vault deployment after the
upgrade will fail without it.

Only the program's **current upgrade authority** can create the config, verified
on-chain against the loader's `ProgramData`. On mainnet that is the Fordefi MPC
wallet, so this is a Fordefi-signed transaction like the upgrade itself.

Choose the authority deliberately: it can be the upgrade authority itself, or a
separate operational key so routine vault creation does not need Fordefi. It is
rotatable later with `set_config_authority` (signed by the current config
authority), so this is not a one-way decision.

```bash
# Emit an unsigned initialize_config transaction for the Fordefi authority.
# --authority is the key that will be allowed to create vaults; omit it to use
# the upgrade authority itself. The script prints every derived account so they
# can be checked against the Fordefi review screen before signing.
node deploy/bootstrap-config.mjs \
  --program-id up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt \
  --url https://api.mainnet-beta.solana.com \
  --authority <VAULT_CREATION_AUTHORITY> \
  --unsigned bootstrap-config.json

# Have Fordefi sign and submit it, then re-run WITHOUT --unsigned to read back
# and confirm the stored authority:
node deploy/bootstrap-config.mjs \
  --program-id up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt \
  --url https://api.mainnet-beta.solana.com --keypair <any-readonly.json>
```

On devnet, where the team holds the upgrade authority directly, the same script
signs and sends in one step with `--keypair <devnet-authority.json>`.

Do **not** use `deploy/new-vault.mjs` for this. That script generates a fresh
program keypair and deploys a **new** program, so it would bootstrap that
program's config and leave the just-upgraded one still gated — while rewriting
`declare_id!` in your working tree.

Then create one vault end-to-end as the smoke test.

**Downstream clients must be updated in lockstep.** This release changes
`initialize`'s account list: `program_config` and a separate `payer` are added,
and `signer` must now be the config authority. Any consumer built against the
previous IDL will fail. Concretely, after the upgrade:

1. Bump the `solana-upshift-vault-programs` submodule in the private
   `solana-vaults` repo and re-run `scripts/sync-idl.sh` so `frontend/idl/`
   carries the new IDL (its CI drift-guards the committed copy).
2. Provision the admin UI's signer as the config authority, or rotate the
   authority to whatever key that UI signs with — otherwise its create-vault
   flow fails with `6016 NotProtocolAuthority` even with a fresh IDL.
3. Rebuild any Rust consumers of `clients/rust/august-vault` (the generated
   client in this repo is already regenerated).

---

## Rollback

The program stays upgradeable, so a bad upgrade is recoverable by another
Fordefi-signed upgrade. Two options:

- **Byte-exact rollback** to the pre-upgrade binary archived in Step 2
  (`pre-upgrade-august_vault.so`). Verify the archived file's hash, then
  `solana program write-buffer pre-upgrade-august_vault.so` (ops payer) →
  `set-buffer-authority` to the Fordefi authority → Fordefi-signed `Upgrade`.
  Confirm `get-program-hash` returns the archived `dcd22bfa…`. (The old binary
  isn't source-reproducible, but the Step 2 dump makes it byte-recoverable.)
- **Roll forward** to a rebuilt, verified hotfix release — preferred once the
  regression is understood.

Pause user ops while rolling back or forward. The behavioral equivalence
established by the fork test + devnet rehearsal makes a rollback unlikely.

## Why this is safe for the live vaults

- The `VaultState` account **layout is byte-identical** to the deployed version
  (verified), so existing accounts are read unchanged — no migration.
- The new code is **behaviorally equivalent for existing vaults**: deposit,
  redeem, the operator instructions and every admin instruction are unchanged, so
  live user flows are unaffected. (The one latent functional delta is a
  Token-2022 operator-ATA fix, which no live vault exercises.)
- **The share-price offsets change, and the effect on the live vaults is exactly
  zero.** `EXTRA_SHARES` / `VIRTUAL_ASSETS` go from 1 to 10^6, which alters the
  deposit and redeem formulas. Both live vaults currently hold `total_assets`
  exactly equal to share supply (no yield reported yet), and with equal offsets
  the price is `(T + O)/(S + O)` — identically 1.0 for any `O` when `T == S`. Every
  deposit and redeem amount checked against both vaults' real on-chain state
  returns a byte-identical result before and after. Once yield is reported the two
  formulas diverge, bounded well under 0.01% (worst case measured: −0.0077% on a
  full-supply redeem of the smaller vault at +20% yield). No migration, no
  rebasing, no action for holders.

  The divergence is **not** uniformly in the vault's favour, so be precise about
  it: larger offsets damp the share price toward 1.0 in both directions. Above
  1.0, redeemers receive marginally less (favours the vault) while depositors
  receive marginally more shares for the same assets (marginally dilutes existing
  holders). Measured on the larger live vault at +5% yield: a 185,265,261,060-unit
  deposit mints 4,535 more shares out of 176 billion, and the equivalent redeem
  returns 5,000 fewer units out of 194 billion. The floor-rounding on every
  operation still favours the vault, as before; it is the offset change itself
  that is two-sided.
- **`initialize` is the exception, and it is a breaking change.** Vault *creation*
  now requires the `ProgramConfig` account and a signer equal to its authority,
  and gains a separate `payer`. This affects no existing vault, but it does mean
  (a) no vault can be created between the upgrade and Step 4, and (b) every
  client that creates vaults must be rebuilt against the new IDL — see the
  lockstep list in Step 4.
- `integration-tests/tests/mainnet_fork_compat.rs` proves the new code reads +
  operates on the **real** on-chain vault accounts.
- This upgrade does **not** address the open audit items (virtual-offset size;
  Token-2022 extension whitelisting) — decide separately whether to bundle those
  (they would change behavior and need their own review).
