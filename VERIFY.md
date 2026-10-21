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
- **Base image:** `solanafoundation/solana-verifiable-build@sha256:12fd4c0a0790f0fc41ef74b0cdb6bccc167ba6137eb1adb749bac649481c86bd`
  — Solana **v4.3.0**, platform-tools **v1.57**. Passed explicitly: auto-selection
  from `Cargo.lock`'s `solana-program` would pick the Solana 2.3.0 image the
  v0 builds used. Pinning the digest makes the build host-independent (an emulated
  `linux/amd64` build on Apple Silicon reproduces the same hash).
- **Architecture:** `--arch v3` (SBPFv3). `solana-verify` defaults to v0, so
  every build and verification command passes it.

## Reproduce the build

```bash
# Pin the exact CLI the CI job uses (a later version could hash/behave differently):
cargo install solana-verify --version 0.5.1 --locked

git clone https://github.com/fractal-protocol/solana-upshift-vault-programs
cd solana-upshift-vault-programs
git checkout <release-commit-or-tag>

IMAGE=solanafoundation/solana-verifiable-build@sha256:12fd4c0a0790f0fc41ef74b0cdb6bccc167ba6137eb1adb749bac649481c86bd

# One build per program. Never build the workspace in one go: the queue depends on
# the vault with `features = ["cpi"]`, which implies `no-entrypoint`, and cargo
# unifies features across a single build — producing a 544-byte, entrypoint-less
# `august_vault.so`. See integration-tests/build.rs.
while read -r pkg _; do
  case "$pkg" in ''|\#*) continue ;; esac
  # Delete the artifact first. A cached compilation makes the build skip its copy
  # step, leaving the PREVIOUS .so in place — you then hash a stale file and
  # conclude the bytecode is unchanged. Removing it means a missing file, not a
  # wrong hash, if the build ever fails to produce one.
  rm -f "target/deploy/${pkg}.so"
  solana-verify build --library-name "$pkg" --base-image "$IMAGE" --arch v3
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

Mainnet, checked 14 Oct 2026:

| Program | Mainnet | Executable hash | Release | OtterSec |
|---|---|---|---|---|
| `august_vault` | `up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt` | `f258ab095f0235e75186dfb1c5cc2d4aaeaa71b4d326549dbdb85fc7da17df31` (681,832 B) | [`v0.3.0`](https://github.com/fractal-protocol/solana-upshift-vault-programs/releases/tag/v0.3.0), deployed 14 Oct 2026 | [verified](https://verify.osec.io/status/up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt) at `87c0f2d` |
| `august_withdrawal_queue` | `upQhC7mgYwmHLVaatRWoGAu394piLT9th9FiQZHnPrW` | `72351d599f81769a98a1facb65f5af852d3aa08018cef8ad3d871a150e41ff01` (480,128 B) | [`v0.3.0`](https://github.com/fractal-protocol/solana-upshift-vault-programs/releases/tag/v0.3.0), deployed 14 Oct 2026 | [verified](https://verify.osec.io/status/upQhC7mgYwmHLVaatRWoGAu394piLT9th9FiQZHnPrW) at `87c0f2d` |

Devnet runs the same `v0.3.0` artifacts for both programs (9 Oct 2026).

Those are SBPFv0 builds. This tree builds SBPFv3, so its hashes differ from the
deployed ones until the next upgrade; reproduce a deployed release from that
tag's own VERIFY.md.

**`verified-hashes.txt` describes this source tree, not the deployed programs.**
The `reproducible-build` CI job rebuilds from source and asserts every row, so an
older record cannot live there: the table above is the record of what is deployed
and verified. Check the deployed programs against that table, and this tree
against `verified-hashes.txt`. After each deployment, update the table.

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
