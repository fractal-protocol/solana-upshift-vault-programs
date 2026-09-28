# Reproducible Build & On-Chain Verification

This repository produces **reproducible** Solana programs: building each with the
pinned toolchain below yields a `.so` whose SHA-256 is byte-for-byte identical on
any host. The expected hashes are committed in
[`verified-hashes.txt`](verified-hashes.txt) — one row per program — and asserted
in CI. Two programs are pinned today: `august_vault`, live on mainnet and devnet,
and `august_withdrawal_queue`, which is built and pinned but not yet deployed
anywhere.

> **Verification is point-in-time.** Both programs are **upgradeable** — the
> upgrade authority can replace the bytecode. A matching hash proves the deployed code
> **at the moment you fetch it**, not forever; re-fetch the live on-chain hash
> whenever you need fresh assurance.

## Pinned toolchain

- **Builder:** [`solana-verify`](https://github.com/solana-foundation/solana-verifiable-build) **v0.5.1** — the exact version the CI job pins (requires Docker). Use this version; a later CLI could change output/hashing behavior.
- **Base image:** `solanafoundation/solana-verifiable-build@sha256:695f890e620db8c39afe5112e048599f8ee395a0cab5a2e572f30a72c6366cb4`
  — Solana **v2.3.0**, container Rust **1.86.0**. The image is selected from the
  `solana-program` version pinned in the committed `Cargo.lock`; pinning the
  digest makes the build host-independent (an emulated `linux/amd64` build on
  Apple Silicon reproduces the same hash).

## Reproduce the build

```bash
# Pin the exact CLI the CI job uses (a later version could hash/behave differently):
cargo install solana-verify --version 0.5.1 --locked

git clone https://github.com/fractal-protocol/solana-upshift-vault-programs
cd solana-upshift-vault-programs
git checkout <release-commit-or-tag>

IMAGE=solanafoundation/solana-verifiable-build@sha256:695f890e620db8c39afe5112e048599f8ee395a0cab5a2e572f30a72c6366cb4

# One build per program. Never build the workspace in one go: the queue depends on
# the vault with `features = ["cpi"]`, which implies `no-entrypoint`, and cargo
# unifies features across a single build — producing a ~900-byte, entrypoint-less
# `august_vault.so`. See integration-tests/build.rs.
while read -r pkg _; do
  case "$pkg" in ''|\#*) continue ;; esac
  # Delete the artifact first. A cached compilation makes the build skip its copy
  # step, leaving the PREVIOUS .so in place — you then hash a stale file and
  # conclude the bytecode is unchanged. Removing it means a missing file, not a
  # wrong hash, if the build ever fails to produce one.
  rm -f "target/deploy/${pkg}.so"
  solana-verify build --library-name "$pkg" --base-image "$IMAGE"
  solana-verify get-executable-hash "target/deploy/${pkg}.so"   # exec_sha256
  shasum -a 256 "target/deploy/${pkg}.so"                       # raw_sha256
  wc -c "target/deploy/${pkg}.so"                               # size (bytes)
done < verified-hashes.txt
```

**Re-pinning after a source change.** Any edit to a program crate can change its
bytecode — doc comments included, since they can reach the artifact. Do not assume
a comment-only change is hash-neutral: clear the artifact as above, rebuild, and
compare. CI rebuilds from a clean checkout, so a stale local artifact is the one
way to convince yourself a pin is current when it is not.

At the commit these hashes were recorded, all three columns match every row of
[`verified-hashes.txt`](verified-hashes.txt). That file is the single source of
truth and is not reproduced here — the CI `reproducible-build` job rebuilds each
row and asserts all three columns, and additionally requires that the set of
pinned package names equals the set of directories under `programs/`, so a
program cannot be added without a row.

## Compare against the on-chain program

```bash
# solana-verify needs a full RPC URL; `-u mainnet-beta` fails with AccountNotFound.
solana-verify get-program-hash -u https://api.mainnet-beta.solana.com \
  up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt   # august_vault
solana-verify get-program-hash -u https://api.mainnet-beta.solana.com \
  upQhC7mgYwmHLVaatRWoGAu394piLT9th9FiQZHnPrW   # august_withdrawal_queue
```

## Current on-chain status

Checked 27 Sep 2026.

| Program | Mainnet | Executable hash | Release |
|---|---|---|---|
| `august_vault` | `up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt` | `cb1352a5dc4ab9513c10c1083534837bb4c666ed8ad9c3919f02f752bff52df9` (605,160 B) | [`v0.1.1`](https://github.com/fractal-protocol/solana-upshift-vault-programs/releases/tag/v0.1.1), deployed 6 Aug 2026 |
| `august_withdrawal_queue` | `upQhC7mgYwmHLVaatRWoGAu394piLT9th9FiQZHnPrW` | not deployed | — |

The vault's hash is OtterSec-verified
([status](https://verify.osec.io/status/up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt)).
The `up12…` program on devnet runs the same `cb1352a5…` build.

**`verified-hashes.txt` describes this source tree, not the deployed programs.**
The `reproducible-build` CI job rebuilds from source and asserts every row, so an
older record cannot live there. The rows now carry the vault with the withdrawal
queue gate and operator subaccounts, and the queue program itself; neither is on
chain until the [release runbook](docs/UPGRADE.md) runs. Until then, check the
deployed vault against the table above, and this tree against
`verified-hashes.txt`. After each deployment, update the table so the two agree
again.

## On-chain verification

Verification is registered through the OtterSec verified-programs flow, once per
program: the program's upgrade authority (the same Fordefi MPC key for both)
uploads an on-chain verification PDA, then an OtterSec remote job confirms the
on-chain hash matches this source.
**Creating the PDA alone is not sufficient** — the remote job must complete
successfully before Solana Explorer, SolanaFM, and Solscan display the program
as verified.

## Notes

- The `reproducible-build` CI job (`.github/workflows/ci.yml`) rebuilds every
  program in the pinned image and fails unless the executable hash, raw
  SHA-256, and size all match `verified-hashes.txt`; an intentional bytecode
  change must update that file in the same PR. The release workflow
  (`.github/workflows/release.yml`) rebuilds each row on its own runner, asserts
  the same three columns, and attests and publishes every `.so` in one release.
- **Line numbers are part of the bytecode; doc comments are not.** Anchor's
  `require!` / `err!` macros capture `file!()` and `line!()` at each error site,
  so the source path and the line number of every check are compiled into
  `.rodata`. Consequences, both measured on this program:
  - **Inserting or deleting *any* line above an error site changes the hash** —
    a plain `//` comment, a blank line, a reordered `use`. The size often stays
    **identical** (a line number is a fixed-width immediate), which makes it easy
    to assume nothing moved. Measured on a scratch build: adding one comment line
    above the first `require!` in `deposit.rs` changed the hash while the size
    stayed put at that build's 589,272 B. (That is a local `anchor build`, which
    differs from the verifiable-build size recorded above — the point is the
    unchanged size, not the number.)
  - **Rewording a doc comment in place does not.** `///` text is *not* in the
    binary — Anchor emits the IDL in a separate `idl-build` compilation to
    `target/idl/august_vault.json`. Rewording one `///` line produced a
    byte-identical `.so`.

  So a hash change after a comment-only edit is **not** automatically benign:
  it means a line shifted, and you should confirm that is all that happened. What
  *is* in the bytecode besides line numbers: `#[msg("…")]` error strings,
  instruction and account names, and `security_txt!`. Editing an error message is
  a real bytecode change and requires re-recording `verified-hashes.txt`.
- `security_txt!` is embedded in both programs (`programs/*/src/lib.rs`),
  exposing the security contact and source repository in the deployed bytecode.
- Devnet: `up12…` is the current devnet vault. `C8B1EpsSGVWK2vMrk3aDT3kL7RCE77otokUh4EC35kK7`
  is an older, non-reproducible devnet deployment that nothing here targets any more.
