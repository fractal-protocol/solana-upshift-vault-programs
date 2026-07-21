# Verified Upgrade Runbook (P4)

How to upgrade the deployed `august_vault` program to a **reproducibly-built,
attested** binary and register it as **verified** on-chain (OtterSec →
explorers). This aligns the live mainnet bytecode with the public source, which
today it does **not** match (the current deployment predates verifiable builds —
see [VERIFY.md](../VERIFY.md)).

> **This changes live mainnet bytecode over real user funds.** Do not deviate
> from this runbook. Two transactions must be signed by the program's **Fordefi
> MPC upgrade authority**; everything else is unprivileged prep. Verification is
> **point-in-time** (the program stays upgradeable).

## Key facts

| | Mainnet | Devnet |
| --- | --- | --- |
| Program ID | `up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt` | `C8B1EpsSGVWK2vMrk3aDT3kL7RCE77otokUh4EC35kK7` |
| Upgrade authority | `B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM` (Fordefi MPC) | `APuzErEVGAbvhyj2hbmo6vp7pacNHRVXbu43UhcAne2i` |
| ProgramData account size (at writing) | 497,541 B | verify with `solana account <programData>` |

- **Expected reproducible hash** (mainnet build, committed source): the
  `exec_sha256` / `raw_sha256` / `size` in [`verified-hashes.txt`](../verified-hashes.txt)
  (currently `fca11d73…`, 505,216 B), built with `solana-verify` 0.5.1 in
  `solanafoundation/solana-verifiable-build@sha256:695f890e…` (Solana 2.3.0).
- **ProgramData must be extended first:** the new `.so` (505,216 B) is larger
  than the current allocation, so `solana program extend` is required or the
  upgrade fails. Deficit = `505,216 + 45 (loader header) − 497,541 = 7,720` bytes
  (re-derive if the sizes change).

## Prerequisites

- `solana-verify` 0.5.1 + Docker (for the reproducible build).
- Solana CLI.
- A **funded ops fee-payer** keypair (a few SOL) — pays for `extend` /
  `write-buffer`. This is NOT the upgrade authority.
- Fordefi access to the upgrade authority key, able to sign a
  `BPFLoaderUpgradeable::Upgrade` instruction and an arbitrary transaction
  (for the verify-PDA).
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
> differently from mainnet's `fca11d73…` — expected; the dry-run validates
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
# Extend ProgramData to fit the larger binary (re-derive the byte count if sizes changed).
solana program extend up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt 7720 \
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

Do **not** set the program immutable (no `--final`).

**Post-upgrade verification:**

```bash
solana-verify get-program-hash -u mainnet-beta up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt
#   Now equals verified-hashes.txt exec_sha256 (fca11d73…)
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
- The new code is **behaviorally equivalent** for the live (classic-SPL) vaults;
  the only functional delta (a Token-2022 operator-ATA fix) is latent.
- `integration-tests/tests/mainnet_fork_compat.rs` proves the new code reads +
  operates on the **real** on-chain vault accounts.
- This upgrade does **not** address the open audit items (virtual-offset size;
  Token-2022 extension whitelisting) — decide separately whether to bundle those
  (they would change behavior and need their own review).
