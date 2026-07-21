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

- **Builder:** [`solana-verify`](https://github.com/solana-foundation/solana-verifiable-build) v0.5.1+ (requires Docker).
- **Base image:** `solanafoundation/solana-verifiable-build@sha256:695f890e620db8c39afe5112e048599f8ee395a0cab5a2e572f30a72c6366cb4`
  — Solana **v2.3.0**, container Rust **1.86.0**. The image is selected from the
  `solana-program` version pinned in the committed `Cargo.lock`; pinning the
  digest makes the build host-independent (an emulated `linux/amd64` build on
  Apple Silicon reproduces the same hash).

## Reproduce the build

```bash
git clone https://github.com/fractal-protocol/solana-upshift-vault-programs
cd solana-upshift-vault-programs
git checkout <release-commit-or-tag>

solana-verify build --library-name august_vault \
  --base-image solanafoundation/solana-verifiable-build@sha256:695f890e620db8c39afe5112e048599f8ee395a0cab5a2e572f30a72c6366cb4

solana-verify get-executable-hash target/deploy/august_vault.so
wc -c target/deploy/august_vault.so
```

At the commit these hashes were recorded, this reproduces the value in
[`verified-hashes.txt`](verified-hashes.txt):

```
august_vault  fca11d73ae5ba0635ee76964945c52ddcf78a167eb4c60317ae63df4a4e7cc1d   (505,216 bytes)
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

- The `reproducible-build` CI job (added with the CI workflow) rebuilds
  `august_vault` in the pinned image and fails if the hash drifts from
  `verified-hashes.txt`; an intentional bytecode change must update that file in
  the same PR.
- `security_txt!` is embedded in the program (`programs/august-vault/src/lib.rs`),
  exposing the security contact and source repository in the deployed bytecode.
- Devnet program: `C8B1EpsSGVWK2vMrk3aDT3kL7RCE77otokUh4EC35kK7`.
