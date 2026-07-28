# Test fixtures

## Vault account snapshots (`*.bin`)

Raw mainnet account dumps used by `mainnet_fork_compat.rs`; see that file's
module docs for the addresses and capture details.

## `mpl_token_metadata.so`

The Metaplex Token Metadata program, required by the
`create_share_token_metadata` / `update_share_token_metadata` runtime tests
(the vault program CPIs into it).

Provenance:

- Program ID: `metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s`
- Dumped from Solana mainnet-beta at slot 435763474 (28 July 2026) via
  `solana program dump metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s -u m`
- The program's upgrade authority is `none` (frozen), last deployed in slot
  380725176, so the binary cannot drift from this snapshot.
- SHA-256: `31f0a627dba051a938de650464e55cc5397a4be0fd496929c1f9cf02fe5e9011`

To re-verify: re-run the dump command above and compare `shasum -a 256`.
