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
august_vault    555ffd9f17b0c0d2d81547e239634e4d898f702ecb06335455199014fee96b64  0fc8ae3d6f6349d4090deae09ec12bab239987ad7a47cda5499b54ee1f170bad  595912
```

## Compare against the on-chain program

```bash
solana-verify get-program-hash -u https://api.mainnet-beta.solana.com \
  up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt
```

## Current on-chain status

The mainnet program `up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt` was deployed
**before** verifiable builds were introduced, so its current on-chain bytecode
(`dcd22bfad72992fdd9688945f5ca44b94929a1e43a88ca258f410dcfd9aa8cd6`) is **not
yet reproducible** from this source. A **verified upgrade** to a
reproducibly-built binary is planned; after it lands, `get-program-hash` will
equal the published `get-executable-hash` for a named release commit. Until
then, this document establishes the **source → bytecode** reproducibility of the
release build; the on-chain match follows the upgrade.

## On-chain verification (registered at/after the verified upgrade)

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
- **Doc comments are part of the bytecode.** Anchor embeds the IDL — including
  instruction and account doc comments — into the program binary, so editing a
  `///` comment changes the hash even though no logic changed (and can leave the
  size identical, which makes it easy to assume nothing moved). Rewording a
  comment therefore requires re-running the build above and updating
  `verified-hashes.txt`. Verified by rebuilding twice from identical sources: the
  hash is stable run-to-run, so a changed hash always means changed input.
- `security_txt!` is embedded in the program (`programs/august-vault/src/lib.rs`),
  exposing the security contact and source repository in the deployed bytecode.
- Devnet program: `C8B1EpsSGVWK2vMrk3aDT3kL7RCE77otokUh4EC35kK7`.
