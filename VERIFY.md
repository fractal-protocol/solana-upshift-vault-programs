# Reproducible Build & On-Chain Verification

This repository produces a **reproducible** Solana program: building `august_vault`
with the pinned toolchain below yields a `.so` whose SHA-256 is byte-for-byte
identical on any host. The expected hash is committed in
[`verified-hashes.txt`](verified-hashes.txt) and asserted in CI.

> **Verification is point-in-time.** The program is **upgradeable** — the upgrade
> authority can replace the bytecode. A matching hash proves the deployed code
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

solana-verify build --library-name august_vault \
  --base-image solanafoundation/solana-verifiable-build@sha256:695f890e620db8c39afe5112e048599f8ee395a0cab5a2e572f30a72c6366cb4

solana-verify get-executable-hash target/deploy/august_vault.so   # exec_sha256
shasum -a 256 target/deploy/august_vault.so                        # raw_sha256
wc -c target/deploy/august_vault.so                                # size (bytes)
```

At the commit these hashes were recorded, all three columns match
[`verified-hashes.txt`](verified-hashes.txt) (the CI `reproducible-build` job
asserts the same three):

```
# program       exec_sha256                                                       raw_sha256                                                        size
august_vault    f9f43ca48589f82a8690e802c1d75cd016b01bff5404bd3f8b5a18d6348c79a8  5159566ba012f1f96e10bee6991b57db67e2d800c2475e01d15cbb6571b14747  598592
```

## Compare against the on-chain program

```bash
solana-verify get-program-hash -u https://api.mainnet-beta.solana.com \
  up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt
```

## Current on-chain status

The mainnet program `up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt` **is** running a
reproducibly-built binary. Its on-chain executable hash is

```
fca11d73ae5ba0635ee76964945c52ddcf78a167eb4c60317ae63df4a4e7cc1d   (505,216 B)
```

which is the build recorded in `verified-hashes.txt` **before** the current
release — i.e. the program as it stood prior to the `ProgramConfig` gate and the
share-price offset retune. The hash above is what `get-program-hash` returns
today; the hash in the block above (`f9f43ca4…`) is the *pending* release and
will only match on-chain once [the upgrade runbook](docs/UPGRADE.md) has been
executed. Both values being present is expected while an upgrade is in flight —
compare against the one matching the deployed release, not simply the newest
line in `verified-hashes.txt`.

## On-chain verification

Verification is registered through the OtterSec verified-programs flow: the
program's upgrade authority (a Fordefi MPC key) uploads an on-chain verification
PDA, then an OtterSec remote job confirms the on-chain hash matches this source.
**Creating the PDA alone is not sufficient** — the remote job must complete
successfully before Solana Explorer, SolanaFM, and Solscan display the program
as verified.

## Notes

- The `reproducible-build` CI job (`.github/workflows/ci.yml`) rebuilds
  `august_vault` in the pinned image and fails unless the executable hash, raw
  SHA-256, and size all match `verified-hashes.txt`; an intentional bytecode
  change must update that file in the same PR.
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
- `security_txt!` is embedded in the program (`programs/august-vault/src/lib.rs`),
  exposing the security contact and source repository in the deployed bytecode.
- Devnet program: `C8B1EpsSGVWK2vMrk3aDT3kL7RCE77otokUh4EC35kK7`.
