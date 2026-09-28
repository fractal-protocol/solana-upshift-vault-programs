# Release Runbook

How to ship a release of this repository's two programs: build and attest both,
upgrade `august_vault`, deploy `august_withdrawal_queue` for the first time,
register both as **verified** (OtterSec → explorers), and turn the queue on per
vault.

The release this runbook targets moves the vault from `v0.1.1` (`cb1352a5…`, live
since 6 Aug 2026) to a build that adds the withdrawal-queue gate and operator
subaccounts, and brings the queue program up at its fixed id. The `v0.1.1`
ceremony is recorded under [Previous releases](#previous-releases).

> **This changes live mainnet bytecode over real user funds.** Do not deviate
> from this runbook. **Three** transactions must be signed by the **Fordefi MPC
> upgrade authority**: the vault `Upgrade` (Step 4) and one verify-PDA upload per
> program (Step 5). Book three approval ceremonies, plus one per vault in Step 6
> if the vault admin is a Fordefi key too. Everything else is unprivileged prep.
> Verification is **point-in-time** (both programs stay upgradeable).

## Key facts

| | `august_vault` | `august_withdrawal_queue` |
| --- | --- | --- |
| Program ID (mainnet and devnet) | `up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt` | `upQhC7mgYwmHLVaatRWoGAu394piLT9th9FiQZHnPrW` |
| Mainnet upgrade authority | `B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM` (Fordefi MPC) | same, after Step 3's handoff |
| Devnet upgrade authority | `APuzErEVGAbvhyj2hbmo6vp7pacNHRVXbu43UhcAne2i` (team-held keypair) | same |
| Deployed now | `cb1352a5…`, `Data Length` 605,160 B on both clusters (27 Sep 2026) | not deployed on any cluster |
| Library name | `august_vault` | `august_withdrawal_queue` |

- **Expected hashes:** the `exec_sha256` / `raw_sha256` / `size` rows in
  [`verified-hashes.txt`](../verified-hashes.txt), built with `solana-verify`
  0.5.1 in `solanafoundation/solana-verifiable-build@sha256:695f890e…`
  (Solana 2.3.0). The release asserts both.
- **The vault binary hardcodes the queue's id** (`WITHDRAWAL_QUEUE_PROGRAM_ID`
  in `programs/august-vault/src/state/vault.rs`), and `attach_withdrawal_queue`
  accepts only a queue PDA owned by that program. So the queue must exist at
  `upQhC7…` under the Fordefi authority **before** the vault upgrade. That is why
  Step 3 comes before Step 4.
- **Vault ProgramData must be extended first.** The new vault `.so` is larger
  than the current allocation. With `Data Length` from `solana program show`,
  `additional_bytes = <new .so size> − <Data Length>`; for the current pins,
  687,352 − 605,160 = 82,192. Re-derive when the pin changes.
- **Devnet runs the same ids**, so the released mainnet artifacts deploy to
  devnet unchanged and the rehearsal (Step 2) exercises the exact bytes. The
  older `C8B1Eps…` devnet program is not part of this release.

## Program keypair custody

A program keypair is needed once per cluster, to create the program account at
its address. After that it has no authority over the program; the upgrade
authority does. **Before a program is first deployed on a cluster, though, whoever
holds its keypair can claim that address under an upgrade authority of their
choosing.** For the queue on mainnet that is the live risk until Step 3 is done:
the upgraded vault trusts whatever program sits at `upQhC7…`.

| Key | Address | Used for | Custody |
| --- | --- | --- | --- |
| Vault program keypair | `up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt` | Creating `up12…` on a new cluster. Mainnet and devnet are done | Ops laptop, `~/.config/solana/programs/august_vault-keypair-mainnet-up12byto.json` (mode 600). Offline copy in the team 1Password (28 Sep 2026). **Owner: TBD** |
| Queue program keypair | `upQhC7mgYwmHLVaatRWoGAu394piLT9th9FiQZHnPrW` | First deployment on devnet (Step 2) and mainnet (Step 3) | Ops laptop, `~/.config/solana/programs/august_withdrawal_queue-keypair-mainnet-upQhC7mg.json` (mode 600). Generated 28 Sep 2026, replacing an earlier id whose keypair was lost before any deployment. Offline copy in the team 1Password (28 Sep 2026). **Owner: TBD** |
| Upgrade authority (mainnet) | `B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM` | Upgrades, verify PDAs, `ProgramConfig` | Fordefi MPC |
| Upgrade authority (devnet) | `APuzErEVGAbvhyj2hbmo6vp7pacNHRVXbu43UhcAne2i` | Devnet upgrades and deploys | Team-held keypair |

Rules for both program keypairs:

- Keep them outside any build directory: `cargo clean` or `anchor clean` deletes
  `target/` and every key in it. Never commit them, and keep at least two
  offline copies. Losing the queue keypair before Step 3 means a new id, a vault
  rebuild and a re-pin.
- Check the key before every use: `solana-keygen pubkey <file>` must print the
  address in the table.
- **`target/deploy/*-keypair.json` in this repository is not either key.**
  `anchor build` writes a random keypair there when none exists, and CI does the
  same for localnet. Never run `anchor deploy` or `anchor keys sync` against
  mainnet or devnet: both use that file and would deploy to, or rewrite
  `declare_id!` with, a throwaway address.

## Prerequisites

- `solana-verify` 0.5.1 + Docker (to reproduce the build).
- Solana CLI **2.x**, the version `ci.yml` pins as `SOLANA_VERSION`. Agave 3.x
  and later break the permissionless `extend` in Step 4 (see the note there), and
  newer CLIs also refuse a bare `solana program show` with `No default signer
  found`. Put 2.x on PATH and check before starting:
  `sh -c "$(curl -sSfL https://release.anza.xyz/v2.1.14/install)"`, then
  `solana --version`.
- A **funded ops fee-payer** keypair (about 10 SOL at the current sizes). It is
  NOT the upgrade authority. It pays the queue's ProgramData rent (2.43 SOL,
  `solana rent 479189`) plus an equal buffer that the deploy refunds, the vault
  `extend` (0.42 SOL), and the vault buffer (3.49 SOL, refunded to the spill
  account by the `Upgrade`).
- The queue program keypair, checked as above.
- Fordefi access to the upgrade authority, able to sign a
  `BPFLoaderUpgradeable::Upgrade` instruction and two arbitrary transactions.
  Use a **durable nonce** for each: a recent blockhash expires in 60 to 90 s,
  well inside a Fordefi review, and the submission then fails with `Blockhash
  not found` after the approvals were collected. The nonce authority must be the
  signer, since `AdvanceNonceAccount` is signed by it. The `v0.1.1` ceremony's
  nonce account `3fp7jaxa2wqLPR5CfBeo9n1cKWfUV8JoVCwLDXJVcrTj` (authority
  `B75DMr…`) is reusable; one per concurrent transaction is needed, so create
  more as:

  ```bash
  solana-keygen new -o nonce.json
  solana create-nonce-account nonce.json 0.0015 \
    --nonce-authority B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM \
    -u mainnet-beta -k <ops-payer.json>
  ```

  `solana-verify export-pda-tx` has no nonce option, so Step 5's transactions
  carry a recent blockhash. Rebuild each on the nonce before submitting it to
  Fordefi: prepend `AdvanceNonceAccount` and use the nonce value as the
  blockhash.
- `anchor build` run once in the checkout, and `pnpm install`, if you encode
  Step 6's instructions with the generated clients. The deployed bytecode still
  comes from the release artifacts.
- The repository protections below provisioned. (The workflow fails closed
  without the machine-enforced ones: `release` env reviewers, immutable
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

## Step 0 — Dry-run tag

Tag a commit on the default branch with a prerelease suffix. `release.yml` runs
exactly as for a real release: one build job per program, each asserting its
`verified-hashes.txt` row, then one attested **prerelease** carrying both.

```bash
TAG=v0.2.0-rc.1
git checkout <commit-on-default-branch>
git tag "$TAG" && git push origin "$TAG"      # v* tags are admin-only per the tag ruleset
```

Check that the prerelease has four assets (`<program>_$TAG.so` and
`<program>_$TAG.hashes.txt` for each program) and that each `.so` matches its
row and carries an attestation:

```bash
gh release download "$TAG" --dir "rel-$TAG"
for pkg in august_vault august_withdrawal_queue; do
  solana-verify get-executable-hash "rel-$TAG/${pkg}_$TAG.so"   # == exec_sha256
  shasum -a 256 "rel-$TAG/${pkg}_$TAG.so"                       # == raw_sha256
  gh attestation verify "rel-$TAG/${pkg}_$TAG.so" \
    --repo fractal-protocol/solana-upshift-vault-programs
done
```

The release is immutable, so a failed dry run needs a new suffix (`-rc.2`), not
a re-run: a tag-triggered run always uses the workflow file as of the tagged
commit.

**Tag a commit whose `ci.yml` push run executed `Reproducible Build`.** A
docs-only commit skips that job, and the guard refuses anything but `success`.
Updating VERIFY.md after a deployment produces exactly such commits, so tag the
last code commit instead: releasing one behind the tip is supported.

## Step 1 — Cut the release

Same as Step 0 with the final tag on the same commit:

```bash
TAG=v0.2.0
git tag "$TAG" <same-commit> && git push origin "$TAG"
```

Releasing a commit that is **behind** the default branch is supported (the guard
accepts `identical|behind`), and is the right choice when the tip has since
gained commits that do not change the programs.

> **Do not add `--target` to `gh release create`.** If the tagged commit's
> `.github/workflows/` differs from the default branch's — which any workflow
> change merged after it produces — `POST /releases` demands the `workflows`
> scope. That is not a valid `permissions:` key and `GITHUB_TOKEN` can never hold
> it, so publishing fails with `403 Resource not accessible by integration`,
> naming a permission rather than its cause. This sank the first `v0.1.0`
> attempt.

Download both `.so` files and repeat the Step 0 checks. Everything below deploys
THESE artifacts.

---

## Step 2 — Devnet rehearsal

Both ids are the same on devnet and `declare_id!` matches, so rehearse with the
release artifacts themselves. The devnet authority is a team-held keypair, so
every step signs directly with the CLI.

```bash
URL=https://api.devnet.solana.com
DEV=<devnet-authority-keypair.json>        # APuzEr…

# Queue: first deployment, straight under the devnet authority.
solana-keygen pubkey <queue-program-keypair.json>   # must print upQhC7…
solana program deploy "rel-$TAG/august_withdrawal_queue_$TAG.so" \
  --program-id <queue-program-keypair.json> \
  --upgrade-authority "$DEV" -u "$URL" -k "$DEV"

# Vault: extend, then upgrade. On devnet the authority signs the extend itself,
# so the Agave 3.x restriction in Step 4 does not matter here.
solana program show up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt -u "$URL"   # Data Length
solana program extend up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt <additional_bytes> -u "$URL" -k "$DEV"
solana program deploy "rel-$TAG/august_vault_$TAG.so" \
  --program-id up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt \
  --upgrade-authority "$DEV" -u "$URL" -k "$DEV"

solana-verify get-program-hash -u "$URL" upQhC7mgYwmHLVaatRWoGAu394piLT9th9FiQZHnPrW   # == queue row
solana-verify get-program-hash -u "$URL" up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt   # == vault row
```

Then run Step 6 on one devnet vault and walk a request through: request,
expedite, finalize, a second request cancelled, `release_vault`, re-attach. Also
confirm that an existing devnet vault with no queue still deposits and redeems
as before. Only proceed to mainnet once the rehearsal is clean.

---

## Step 3 — Mainnet: deploy the queue program

No Fordefi signature: the ops payer deploys, then hands the upgrade authority to
Fordefi at once. Until the handoff lands, the ops key can replace the queue's
bytecode; that is harmless only because no vault trusts `upQhC7…` yet (the vault
upgrade is Step 4). So run the two commands back to back and check the result
before going on.

(Deploying straight under the Fordefi key would need Fordefi to co-sign the
loader's `DeployWithMaxDataLen` together with the program keypair, a
multi-signer transaction for no gain in safety over the checked handoff.)

```bash
URL=https://api.mainnet-beta.solana.com
OPS=<ops-payer.json>

# Pre-flight
solana-keygen pubkey <queue-program-keypair.json>        # must print upQhC7…
solana account upQhC7mgYwmHLVaatRWoGAu394piLT9th9FiQZHnPrW -u "$URL"
#   must fail with AccountNotFound. If it exists, STOP. Anyone can send lamports
#   to the address, which blocks the deploy without claiming the id; if it is
#   instead a program, someone else deployed at this id, and the vault must not
#   be upgraded to trust it. Either way the id needs a decision before going on.
solana-verify get-executable-hash "rel-$TAG/august_withdrawal_queue_$TAG.so"   # == queue row

# Deploy (writes a buffer, then deploys from it; the buffer's rent is refunded).
# ProgramData is sized to this binary, so a larger fix later needs an extend;
# add --max-len <bytes> here to buy headroom at 6,960 lamports a byte.
solana program deploy "rel-$TAG/august_withdrawal_queue_$TAG.so" \
  --program-id <queue-program-keypair.json> \
  --upgrade-authority "$OPS" -u "$URL" -k "$OPS"

# Hand the upgrade authority to Fordefi. The skip flag is required because
# Fordefi cannot co-sign here; the read-back below is the check it skips.
solana program set-upgrade-authority upQhC7mgYwmHLVaatRWoGAu394piLT9th9FiQZHnPrW \
  --new-upgrade-authority B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM \
  --skip-new-upgrade-authority-signer-check \
  -u "$URL" -k "$OPS"

# Read back. Do not continue unless both hold.
solana program show upQhC7mgYwmHLVaatRWoGAu394piLT9th9FiQZHnPrW -u "$URL"
#   Authority: B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM
solana-verify get-program-hash -u "$URL" upQhC7mgYwmHLVaatRWoGAu394piLT9th9FiQZHnPrW
#   == verified-hashes.txt exec_sha256 for august_withdrawal_queue
```

Never `solana program close` the queue. Closing a program retires its address
for good, and the vault hardcodes this one.

---

## Step 4 — Mainnet: upgrade the vault (Fordefi-signed)

**Pre-flight (abort on any mismatch):**

```bash
# 1. The upgrade authority is still the Fordefi key.
solana program show up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt -u "$URL"
#    Authority must be B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM

# 2. Step 3 is done: upQhC7… exists under B75DMr… with the pinned hash.

# 3. The mainnet-fork layout guard passes against fresh fixtures (refresh them
#    per integration-tests/tests/mainnet_fork_compat.rs), and CI is green.

# 4. The release .so is the verified artifact.
solana-verify get-executable-hash "rel-$TAG/august_vault_$TAG.so"   # == vault row

# 5. Preserve the deployed binary for byte-exact rollback.
solana program dump up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt pre-upgrade-august_vault.so -u "$URL"
solana-verify get-program-hash -u "$URL" up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt
#    Expect cb1352a5dc4ab9513c10c1083534837bb4c666ed8ad9c3919f02f752bff52df9 (v0.1.1).
#    Anything else means an unrecorded upgrade happened: STOP.
sha256sum pre-upgrade-august_vault.so   # archive the file and both hashes
```

> **`solana-verify` takes a full RPC URL, not a cluster moniker.** It hands the
> value straight to its HTTP client, so `-u mainnet-beta` fails with
> `AccountNotFound` in `get-program-hash` (which mid-ceremony reads as "the
> program is gone"), `builder error` in `export-pda-tx`, and `relative URL
> without a base` in `remote submit-job`. The `solana` CLI accepts both. Omitting
> `-u` falls back to the CLI's configured cluster, which Step 2 pointed at devnet.

**Prepare the buffer (unprivileged, ops payer):**

```bash
# Extend ProgramData to fit the larger binary. `Data Length` from `program show`
# already excludes the 45-byte loader header, so subtract directly:
#   additional_bytes = <new .so size> - <Data Length>   (82,192 for the current pins)
solana program extend up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt <additional_bytes> \
  -u "$URL" -k "$OPS"   # ← Solana 2.x CLI only; see below
# Afterwards `Data Length` must equal the new .so size exactly.

solana program write-buffer "rel-$TAG/august_vault_$TAG.so" -u "$URL" -k "$OPS"
#   -> Buffer: <BUFFER_ADDRESS>
solana program set-buffer-authority <BUFFER_ADDRESS> \
  --new-buffer-authority B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM \
  -u "$URL" -k "$OPS"
```

> **The extend fails on an Agave 3.x CLI** with `Upgrade authority B75DM… does
> not match <ops-payer>`. Agave 3.x sends `ExtendProgramChecked`, which needs the
> upgrade authority's signature, and `solana program extend` signs with `-k`.
> The on-chain `ExtendProgram` (loader instruction 6) is still permissionless: it
> needs only a payer. That was executed on mainnet on 6 Aug 2026 with the ops
> payer alone (tx
> `4zAQ7aGaxwc576hGJJCK2yUqG9yCYEHPTgrHybL4SFRdFoiagSrqX5w1yEZXVqYrotxpUYkgYKBoW8BwFLv74hMa`).
> So use a Solana 2.x CLI, or send instruction 6 directly: 8 bytes of data (u32
> LE 6, then u32 LE `additional_bytes`), accounts programdata (w), program (w),
> system program, payer (signer, w). Simulate first and look for `Extended
> ProgramData account by <n> bytes`.

**(Optional) pause user operations** via the admin key during the window (pause
does not stop operator withdrawals).

**Execute the upgrade (Fordefi-signed):** submit a
`BPFLoaderUpgradeable::Upgrade` instruction with program `up12…`, buffer
`<BUFFER_ADDRESS>`, authority `B75DMr…`, spill = the ops payer.

Do **not** set either program immutable (no `--final`). `initialize_config` and
`override_config_authority` authorize against the vault's recorded upgrade
authority, which an immutable program stores as `None`; a queue fix would need
an upgrade too. Pinned by `immutable_program_can_never_bootstrap_config` in
`integration-tests/tests/initialize_authorization.rs`.

**Post-upgrade:**

```bash
solana-verify get-program-hash -u "$URL" up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt   # == vault row
```

If you paused, **unpause first** (`deposit` and `redeem` are pause-gated), then
smoke-test one vault with a tiny deposit and redeem. No vault is gated yet, so
both must behave exactly as before.

---

## Step 5 — Register on-chain verification for both programs (Fordefi-signed)

One verify-PDA transaction per program, both uploaded by the upgrade authority
(explorers trust only that uploader). The vault's PDA already exists
(`AUHMtzx…`), so its transaction records an `update`; the queue's is a first
`init`. The same command covers both.

Pin the **tag's** commit: an omitted `--commit-hash` resolves to the remote
default branch's HEAD, which is wrong once `init` advances. (`v0.1.1` recorded
the tag's parent; sound, since the program sources were identical, but pin the
tag itself.)

```bash
COMMIT=$(git rev-list -n1 "$TAG")
IMAGE=solanafoundation/solana-verifiable-build@sha256:695f890e620db8c39afe5112e048599f8ee395a0cab5a2e572f30a72c6366cb4

for pair in up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt:august_vault \
            upQhC7mgYwmHLVaatRWoGAu394piLT9th9FiQZHnPrW:august_withdrawal_queue; do
  solana-verify export-pda-tx \
    https://github.com/fractal-protocol/solana-upshift-vault-programs \
    --program-id "${pair%%:*}" \
    --uploader B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM \
    --commit-hash "$COMMIT" \
    --library-name "${pair##*:}" \
    --base-image "$IMAGE" \
    -u "$URL" --encoding base58
done
```

Sign and submit each via **Fordefi**. **Then submit the OtterSec remote job for
each.** This is a required step: the PDA only records *what to build*, and until
the job runs the API keeps serving the previous result, so after an upgrade the
vault shows as *failing* verification.

```bash
for id in up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt upQhC7mgYwmHLVaatRWoGAu394piLT9th9FiQZHnPrW; do
  solana-verify remote submit-job --program-id "$id" \
    --uploader B75DMrVVhSgjjFQyVrYdDWMw9nCHLBU8UnsSXBGHkfYM -u "$URL"
done
# submit-job polls to completion; `remote get-job --job-id <id>` re-reads a finished job.
```

Confirm `is_verified: true` and `on_chain_hash == executable_hash` at
`https://verify.osec.io/status/<program-id>` for both, check the badges on Solana
Explorer, SolanaFM and Solscan, and update the status table in
[VERIFY.md](../VERIFY.md#current-on-chain-status).

---

## Step 6 — Turn the queue on, one vault at a time

Nothing changes for a vault until its admin attaches a queue. Every instruction
here is signed by the vault's **`admin`** (read it from the `VaultState`); if
that key is a Fordefi wallet, each is a Fordefi ceremony, with a durable nonce.

**Lockstep first.** Once a vault is attached, a holder's direct `redeem` fails
with `WithdrawalQueueRequired` (6021). Before attaching, the frontend and SDK
must route that vault's redemptions through `request_withdrawal`, and the
finalize keeper must be running for it.

1. **`initialize_queue(cooldown_seconds)`**, signed by the admin plus a payer
   for rent. Creates the queue PDA `["withdrawal_queue", vault_state]` and its
   two escrow ATAs. The cooldown is at most 30 days; start short on the first
   vault and watch a keeper cycle. It checks the deposit and share mints against
   the queue's extension allow-list and runs once per vault.
2. **Optional: `set_fulfillment_window(seconds)`.** A new queue starts at `0`
   (requests never expire); at most 90 days. Both settings apply to new requests
   only.
3. **`attach_withdrawal_queue`** on the vault, signed by the admin. The queue is
   live at once. Steps 1 and 3 may share one transaction.

No script emits these as unsigned transactions yet (the admin UI's queue flows
will). Until then, build them with the generated clients and export them unsigned
for Fordefi, on a durable nonce.

Smoke test on the first vault: request a small withdrawal, `expedite_request` it
(admin or operator), finalize, and check the payout; then confirm a direct
`redeem` now fails with 6021.

**Turning it off:** `release_vault` on the queue (admin) detaches it, and direct
redemption reopens. Pending requests survive and can still be finalized or
cancelled. Release only when the vault's liquidity comfortably covers what is
still pending: nothing on-chain checks it. `attach_withdrawal_queue` re-enables
the same queue with its state intact.

**Downstream in lockstep with Step 4**, whether or not any vault is attached yet:

1. Bump the `solana-upshift-vault-programs` submodule in the private
   `solana-vaults` repo and re-run `scripts/sync-idl.sh`, so `frontend/idl/`
   carries both IDLs.
2. Rebuild Rust consumers of `clients/rust/august-vault` and
   `clients/rust/august-withdrawal-queue`.
3. Operator tooling: `operator_withdraw` and `operator_deposit` gain an optional
   subaccount account. It must be passed exactly when the vault has registered
   subaccounts, so nothing changes until the first `register_subaccount`.

---

## Rollback

Both programs stay upgradeable, so a bad upgrade is recoverable by another
Fordefi-signed upgrade.

- **Queue misbehaves:** `release_vault` on every attached vault first. That
  restores direct redemption without touching either binary. Then roll the queue
  forward to a fixed, verified release as in Step 4: extend if the fix is larger
  (Solana 2.x CLI or loader instruction 6), write-buffer, set-buffer-authority,
  Fordefi `Upgrade`.
- **Vault misbehaves:** release every attached queue first, then roll back
  byte-exact to `pre-upgrade-august_vault.so` from Step 4 (write-buffer →
  set-buffer-authority → Fordefi `Upgrade`; no extend, it is smaller; confirm `get-program-hash` returns
  `cb1352a5…`), or forward to a hotfix. Two things to know:
  - The old binary does not read `withdrawal_queue_authority`, so a vault left
    attached through a rollback reopens direct redemption while the queue still
    holds escrowed shares. Those still finalize (the old `redeem` accepts the
    queue PDA as an ordinary holder) or cancel.
  - The old binary ignores `subaccount_count`, `deployed_principal` and the
    `Subaccount` accounts. A later roll-forward reads them as they were left, so
    principal moved while rolled back is not counted.

Pause user operations while rolling back or forward.

## Why this is safe for the live vaults

- **The `VaultState` layout is unchanged in size and in every existing field.**
  `LEN` stays 455, enforced by a `const` assertion; the new fields
  (`withdrawal_queue_authority`, `subaccount_count`, `deployed_principal`) are
  carved from `padding`, which is zero on the live accounts. So they read "no
  queue", "no subaccounts" and zero principal with no migration.
  `integration-tests/tests/mainnet_fork_compat.rs` runs the new code against the
  real on-chain vault accounts.
- **User flows are unchanged until an admin acts.** `deposit` and `redeem` behave
  as in `v0.1.1` for an unattached vault, and `operator_withdraw` pays the
  operator's own ATA while no subaccount is registered.
- **`deployed_principal` starts at zero** even where capital is already out:
  `v0.1.1` did not track it. It is informational (coverage reads each
  `Subaccount::principal`), but the first `register_subaccount` adopts it as the
  inherited principal, so capital deployed before the upgrade is not carried
  over.
- **New error codes are appended** (6021 to 6029); no existing code moves. One
  message changed: `NotOperator` said "Signer must be the admin" and now says
  "Signer must be the operator".
- **The queue holds nothing until attached.** It escrows shares only for
  requests on attached vaults, redeems through the vault's own `redeem` as the
  queue PDA, and never holds the vault's assets beyond one finalize.

---

## Previous releases

**`v0.1.1` (deployed 6 Aug 2026)** moved `up12…` from `fca11d73…` (505,216 B) to
`cb1352a5dc4ab9513c10c1083534837bb4c666ed8ad9c3919f02f752bff52df9` (605,160 B):
the `ProgramConfig` gate on vault creation, the share-price offset retune, and
`deposit_checked` / `redeem_checked`.

- ProgramData extended 507,736 → 605,160 B by the ops payer alone (tx `4zAQ7aGa…`
  above).
- Verify PDA `AUHMtzxmdhHaRvKXi3jmYyae7zg3Ki4Hh4XFGdBvcqwb` updated in tx
  `3B97dPhHRPivZphWPbzPohwhbxj2f5BtqeYBuKo71fQnWUZfPBVnwTQH2gCD8C56DLMHiCCyAoNSrbYomvGNcA6w`;
  OtterSec job `e5774e7e-853c-4688-8452-59a2e5497090` verified `cb1352a5…` at
  commit `6a250947` (the tag's parent; `programs/`, `Cargo.lock` and `Cargo.toml`
  are byte-identical to the tag).
- `ProgramConfig` PDA `7D5Jjs3NgWs26TVvng31MpmD5nqciynyN8swndazc35K` bootstrapped
  with `deploy/bootstrap-config.mjs`; its authority is the Fordefi key `B75DMr…`.
  `initialize_config` uses Anchor `init` and can never run again, so a rollback
  and roll-forward keeps that account; only `override_config_authority` (upgrade
  authority) can change who controls it.
- The first `v0.1.0` tag never published (the `--target` trap above).
