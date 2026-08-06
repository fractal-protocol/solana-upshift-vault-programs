# Verified Upgrade Runbook (P4)

How to upgrade the deployed `august_vault` program to a **reproducibly-built,
attested** binary and register it as **verified** on-chain (OtterSec →
explorers).

The live mainnet program is **already** running a reproducible build
(`fca11d73…`, the release recorded in `verified-hashes.txt` before this one — see
[VERIFY.md](../VERIFY.md)). This runbook therefore moves it from one verified
build to the next: the `ProgramConfig` gate on vault creation, the share-price
offset retune, and the two slippage-bounded instructions `deposit_checked` and
`redeem_checked`.

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
  (currently `cb1352a5…`, 605,160 B), built with `solana-verify` 0.5.1 in
  `solanafoundation/solana-verifiable-build@sha256:695f890e…` (Solana 2.3.0).
- **ProgramData must be extended first:** the new `.so` (605,160 B) is larger
  than the current allocation, so `solana program extend` is required or the
  upgrade fails. Deficit = `605,160 + 45 (loader header) − 507,781 = 97,424` bytes
  (re-derive if the sizes change).
- **Bootstrap the program config after upgrading:** vault creation is gated on a
  `ProgramConfig` authority that does not exist yet. Until `initialize_config` is
  run — by the upgrade authority, so a Fordefi-signed transaction on mainnet —
  `initialize` fails closed and no new vault can be created. Existing vaults are
  unaffected. See [Step 4](#step-4--bootstrap-the-program-config).

## Prerequisites

- `solana-verify` 0.5.1 + Docker (for the reproducible build).
- Solana CLI.
- **`anchor build` run once in the checkout**, and `pnpm install`. Step 4's
  script encodes its instruction from `target/idl/august_vault.json`, which
  `solana-verify build` does *not* emit — without it both Step 4 commands abort.
  The IDL is only used locally to encode the instruction; the bytecode being
  deployed still comes from the verifiable build.
- A **funded ops fee-payer** keypair (a few SOL) — pays for `extend` /
  `write-buffer`. This is NOT the upgrade authority.
- Fordefi access to the upgrade authority key, able to sign a
  `BPFLoaderUpgradeable::Upgrade` instruction and two arbitrary transactions
  (the verify-PDA upload in Step 3 and `initialize_config` in Step 4).
- **Step 4's cost is paid by the ops fee-payer**, not the Fordefi key. Pass
  `--payer <ops-keypair.json>`: `initialize_config` takes `payer` as a `Signer`
  separate from `upgrade_authority`, so the ops key covers the `ProgramConfig`
  rent (169 bytes, 0.00207 SOL) plus the fee and signs locally, leaving only the
  authority's signature for Fordefi. The script verifies the payer's balance
  before writing the file and refuses if it is short — otherwise the approvals
  get collected and the submission then fails for insufficient lamports. If you
  omit `--payer` the Fordefi key pays instead and must itself hold SOL; the same
  balance check applies.
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
git tag v0.1.1 && git push origin v0.1.1     # v* tags are admin-only per the tag ruleset
#   (v0.1.1, not v0.1.0: the first attempt never published — see the note below)
```

Releasing a commit that is **behind** the default branch is supported (the guard
accepts `identical|behind`), and is the right choice when the tip has since
gained commits that do not change the program — the release then names the exact
commit the audit and the frontend's `EXPECTED_BUILD` refer to.

> **Do not add `--target` to `gh release create`.** If the tagged commit's
> `.github/workflows/` differs from the default branch's — which any workflow
> change merged after it produces — `POST /releases` demands the `workflows`
> scope. That is not a valid `permissions:` key and `GITHUB_TOKEN` can never hold
> it, so publishing fails with `403 Resource not accessible by integration`,
> naming a permission rather than its cause. This sank the first `v0.1.0`
> attempt. Note the tag itself is unaffected — only the release object fails, so
> the fix requires a NEW tag on a commit carrying the corrected workflow (a
> tag-triggered run always uses the workflow file as of the tagged commit).

Download the released `august_vault_v0.1.1.so`, and confirm it matches:

```bash
solana-verify get-executable-hash august_vault_v0.1.1.so   # == verified-hashes.txt exec_sha256
sha256sum august_vault_v0.1.1.so                            # == raw_sha256
```

Also confirm the GitHub **build attestation** exists for the asset
(`…/attestations`). Everything below deploys THIS artifact.

---

## Step 1 — Devnet dry-run (rehearse everything)

> `declare_id!` is baked into the bytecode, so the **devnet** artifact must
> declare the devnet program ID. Build a devnet-targeted `.so` (this hashes
> differently from mainnet's `cb1352a5…` — expected; the dry-run validates
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
# solana program extend C8B1Eps… <deficit_bytes> -u devnet -k <devnet-authority.json>   # if needed
#   NOTE: on Agave 3.x this must be signed by the program's UPGRADE AUTHORITY, not
#   the ops payer — see the mainnet note in Step 2. On devnet the team holds that
#   key, so it is only a question of which -k to pass.

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
solana-verify get-executable-hash august_vault_v0.1.1.so   # == verified-hashes.txt

# 4. Preserve the CURRENTLY-deployed binary for byte-exact rollback. It IS
#    reproducible from source (it is the previous verified release), so this dump
#    is a convenience, not the only recovery route — archive it anyway.
solana program dump up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt \
  pre-upgrade-august_vault.so -u mainnet-beta
solana-verify get-program-hash -u mainnet-beta up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt
#    Expect fca11d73ae5ba0635ee76964945c52ddcf78a167eb4c60317ae63df4a4e7cc1d
#    (505,216 B) — the release recorded in verified-hashes.txt before this one.
#    If you get something else, STOP: an unrecorded upgrade has happened.
sha256sum pre-upgrade-august_vault.so
#    Store pre-upgrade-august_vault.so + these hashes in secure archival.
```

**Prepare the buffer (unprivileged, ops fee-payer):**

```bash
# Extend ProgramData to fit the larger binary.
# Re-derive first — this value is release-specific:
#   solana program show up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt -u m
#   additional_bytes = <new .so size> - <Data Length from the command above>
#
# NOTE which size you read. `solana program show` reports `Data Length`, which is
# the ProgramData account size MINUS the 45-byte loader header (verified live:
# 507,736 vs the account's 507,781). So with `program show` the +45 cancels and
# you subtract directly. Using `Data Length` in the `+ 45 - size` form instead
# over-extends by exactly 45 bytes.
# For the hashes in verified-hashes.txt: 605,160 + 45 - 507,781 = 97,424.
# ⚠ THE CLI COMMAND BELOW FAILS ON AGAVE 3.x. Read this first.
#
#   Error: Upgrade authority B75DM… does not match <ops-payer>
#
# Agave 3.x sends `ExtendProgramChecked`, which requires the UPGRADE AUTHORITY to
# sign, and `solana program extend` has no --authority flag: it signs with -k. On
# mainnet that key is Fordefi, so the documented ops-payer command cannot work.
#
# The ON-CHAIN instruction is still permissionless. The plain `ExtendProgram`
# (loader instruction 6) needs only a payer signature — verified by simulation and
# then executed on mainnet 6 Aug 2026 (tx 4zAQ7aGaxwc576hGJJCK2yUqG9yCYEHPTgrHybL4SFRdFoiagSrqX5w1yEZXVqYrotxpUYkgYKBoW8BwFLv74hMa),
# taking ProgramData from 507,736 to 605,160 bytes with the ops payer alone.
#
# So this does NOT need a fourth Fordefi signature. Two ways to do it:
#
#   (a) Use a Solana 2.x CLI, which sends the unchecked instruction. This is the
#       version CI pins (SOLANA_VERSION in ci.yml), so it matches the toolchain the
#       release was built with.
#
#   (b) Send loader instruction 6 directly — 8 bytes of data: u32 LE 6, then u32 LE
#       additional_bytes. Accounts, in order: programdata (w), program (w),
#       system program, payer (signer, w). SIMULATE FIRST and confirm the log line
#       "Extended ProgramData account by <n> bytes" before sending.
#
# Either way, verify afterwards that `Data Length` equals the new .so size exactly.
solana program extend up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt 97424 \
  -u mainnet-beta -k <ops-payer.json>   # ← Agave 2.x only; see the note above

# Upload the verified .so into a buffer.
solana program write-buffer august_vault_v0.1.1.so -u mainnet-beta -k <ops-payer.json>
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
#   Now equals verified-hashes.txt exec_sha256 (cb1352a5…)
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
separate operational key so routine vault creation does not need Fordefi. Two
things to know before signing, because `initialize_config` uses Anchor `init` and
so can **never** be re-run:

- **If you can sign with the key you set**, rotate it with `set_config_authority`
  (signed by the *current* config authority). Routine, no Fordefi needed unless
  the config authority is itself the Fordefi key.
- **If you set a key you cannot sign with** — a typo, or a key nobody holds —
  `set_config_authority` is useless, because it requires a signature from exactly
  that key. The remedy is `override_config_authority`, signed by the **upgrade
  authority**, i.e. a second Fordefi ceremony. Verify the authority in the
  read-back below before you consider Step 4 done.

The authority may not be the zero key (`InvalidAuthority`, 6017) on any of the
three instructions.

```bash
# Emit an unsigned initialize_config transaction for the Fordefi authority.
# --authority is the key that will be allowed to create vaults; omit it to use
# the upgrade authority itself. The script prints every derived account so they
# can be checked against the Fordefi review screen before signing.
# --nonce-account is STRONGLY recommended here. Without it the exported
# transaction carries an ordinary recent blockhash that expires in ~60-90s, and a
# Fordefi review-and-approve ceremony will almost certainly outlast it — the
# submission then fails with "Blockhash not found" AFTER the approvals were
# collected, and the whole ceremony has to be repeated. A durable nonce does not
# expire. Its nonce authority must be the upgrade authority, since
# AdvanceNonceAccount is signed by the nonce authority:
#   solana-keygen new -o nonce.json
#   solana create-nonce-account nonce.json 0.0015 \
#     --nonce-authority B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM \
#     -u mainnet-beta -k <ops-payer.json>
node deploy/bootstrap-config.mjs \
  --program-id up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt \
  --url https://api.mainnet-beta.solana.com \
  --authority <VAULT_CREATION_AUTHORITY> \
  --payer <ops-payer.json> \
  --nonce-account <NONCE_ACCOUNT_PUBKEY> \
  --unsigned bootstrap-config.json

# The written file records who already signed and who is still needed:
#   "feePayer": "<ops payer>", "signedBy": ["<ops payer>"],
#   "awaitingSignatureFrom": "<upgrade authority>"
# Fordefi supplies that one remaining signature.

# Have Fordefi sign and submit it, then re-run WITHOUT --unsigned to read back
# and confirm the stored authority. Pass the SAME --authority: that is what arms
# the script's comparison, and it exits non-zero on a mismatch. No --keypair is
# needed — the read-back path never signs. (--url matters: it defaults to DEVNET,
# so an omitted --url silently reads the wrong cluster.)
node deploy/bootstrap-config.mjs \
  --program-id up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt \
  --url https://api.mainnet-beta.solana.com \
  --authority <VAULT_CREATION_AUTHORITY>
```

If that read-back reports a mismatch, vault creation is now gated behind the
wrong key. Existing vaults are unaffected and user funds are not at risk, but no
new vault can be created until `override_config_authority` is run — a second
Fordefi ceremony. Do not treat Step 4 as complete until this command exits 0.

On devnet, where the team holds the upgrade authority directly, the same script
signs and sends in one step with `--keypair <devnet-authority.json>`.

Do **not** use `deploy/new-vault.mjs` for this. That script generates a fresh
program keypair and deploys a **new** program, so it would bootstrap that
program's config and leave the just-upgraded one still gated — while rewriting
`declare_id!` in your working tree.

Then create one vault end-to-end as the smoke test.

**Downstream clients must be updated in lockstep.** This release changes
`initialize`'s account list *and* its arguments: `program_config` and a separate
`payer` are added, `signer` must now be the config authority, and a new
`share_offset: u64` argument is **required**. Any consumer built against the
previous IDL will fail. Concretely, after the upgrade:

> **The admin UI must now send a share offset when creating a vault.** It is a
> power of ten from 1,000 to 1,000,000, it is **permanent for that vault**, and it
> sets both the share pricing and the minimum first deposit. It cannot be
> defaulted server-side without making a permanent economic decision on the
> operator's behalf — surface it as a deliberate choice. See the guidance in the
> [root README](../README.md#security).


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
  Confirm `get-program-hash` returns `fca11d73…`. That binary is also
  source-reproducible from the previous release commit, so the archive is a
  convenience rather than the only route back.
- **Roll forward** to a rebuilt, verified hotfix release — preferred once the
  regression is understood.

**A bytecode rollback does not undo Step 4.** The `ProgramConfig` PDA is a
separate account: rolling back the binary leaves it in place, program-owned and
simply unused by the old code. Three consequences worth deciding on *before* you
run Step 4:

- `initialize_config` uses Anchor `init`, so it can **never** be re-run. Rolling
  back and later rolling forward does not give you a second attempt at
  bootstrapping — the account created in Step 4 is the one you keep, and only
  `override_config_authority` can change who controls it.
- The old binary's `initialize` takes neither `program_config` nor `payer`, so
  any client already rebuilt against the new IDL (see the lockstep list in
  Step 4) **breaks on rollback**. Roll clients back too.
- Therefore: do not run Step 4 until you are confident you will not roll back.
  Steps 3 and 4 are independent of each other, so this does not block
  verification.

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
- **Deposits are floored at pro-rata and redemptions capped at pro-rata**, the
  two semantic changes to user-facing instructions in this release. Both exist
  for the same reason and are symmetric: the offsets pull the price toward 1.0,
  so below par (`total_assets < supply`, reachable after an operator reports a
  loss) they would *under*-mint on deposit and *over*-pay on redeem. Uncorrected,
  a deposit made after a 50% loss would have handed a material fraction of its
  value to incumbents on arrival — worst at the smallest reachable supply and
  shrinking rapidly as supply grows past the offsets — and the first redeemer
  would have taken more than its share, leaving later holders short. Neither is
  reachable in the state the live vaults are in. `shares_for_deposit` now mints
  `max(offset_formula, pro_rata)` and `assets_for_redeem` pays
  `min(offset_formula, pro_rata)`. Above par the offset value is the binding one
  in both cases, so the anti-inflation and anti-burn behaviour is unchanged, and
  at `total_assets == supply` — where both live vaults sit — all three formulas
  agree. A vault holding no assets while shares are outstanding has no defined
  price and now rejects **both deposits and redemptions** with
  `SharePriceUndefined` — on redeem that replaces the previous `ZeroAmount`
  (6001 -> 6019), so any client matching on the old code needs updating; `operator_deposit`
  can recapitalise it without minting. Proven end to end in
  `integration-tests/tests/loss_state_solvency.rs`.
- **`initialize` is also a breaking change.** Vault *creation*
  now requires the `ProgramConfig` account and a signer equal to its authority,
  gains a separate `payer`, and takes a `share_offset` argument that is fixed for
  the vault's life (a power of ten in the program's permitted band). The offset
  sets both the pricing and the minimum first deposit, so it must be chosen for
  what a base unit of the deposit mint is worth — see the note in README.
  Existing vaults store zero there and resolve to the default, so their pricing
  is unchanged. This affects no existing vault, but it does mean
  (a) no vault can be created between the upgrade and Step 4, and (b) every
  client that creates vaults must be rebuilt against the new IDL — see the
  lockstep list in Step 4.
- `integration-tests/tests/mainnet_fork_compat.rs` proves the new code reads +
  operates on the **real** on-chain vault accounts.
- This upgrade **does** address the virtual-offset-size item: the offsets go from
  1 to 10^6, with the pro-rata floor and cap described above. It does **not**
  address Token-2022 extension whitelisting — decide separately whether to bundle
  that (it would change behavior and need its own review).
