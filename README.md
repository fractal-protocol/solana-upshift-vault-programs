# Upshift Vault Program

A share-based vault on Solana built with Anchor. Users deposit an SPL token and receive shares in return. An operator deploys vault funds externally (e.g., yield strategies), while an admin manages roles, fees, and emergency controls.

## Features

- Single configurable deposit token (SPL mint), set at initialization
- Compatible with both SPL Token and Token-2022 standards
- Multi-vault support: multiple vaults per program with different deposit tokens or versions
- Share-based accounting with proportional redemptions
- Withdrawal fee to prevent sandwich attacks
- Emergency pause/unpause

## Roles

| Role                   | Capabilities                                                        |
|------------------------|---------------------------------------------------------------------|
| **User**               | Deposit tokens, redeem shares                                       |
| **Operator**           | Withdraw/deposit funds, report deployed AUM                         |
| **Admin**              | Update fees, operator, admin (two-step), fee recipient, pause/unpause |
| **Protocol authority** | Create vaults. Program-wide (not per-vault), stored in `ProgramConfig`. Rotatable by itself, resettable by the upgrade authority. |
| **Upgrade authority**  | Upgrade the program; create `ProgramConfig`; reset the protocol authority |

Vault creation is **permissioned**. `ProgramConfig` is a singleton PDA naming the
one key allowed to call `initialize`; it is created once by the program's upgrade
authority. Until it exists, `initialize` fails closed. Note that a
`(deposit_mint, vault_version)` pair cannot be reused once its vault is closed,
so each deposit mint has a finite number of vault lifecycles.

## Instructions

| Instruction              | Access   | Description                                          |
|--------------------------|----------|------------------------------------------------------|
| `initialize_config`      | Upgrade authority | Create the singleton `ProgramConfig` (once)  |
| `set_config_authority`   | Protocol authority | Rotate the vault-creation authority         |
| `override_config_authority` | Upgrade authority | Reset the vault-creation authority (recovery) |
| `initialize`             | Protocol authority | Create vault, share mint, set roles, fix the share offset |
| `deposit`                | User     | Deposit tokens, receive shares                       |
| `deposit_checked`        | User     | As `deposit`, reverting below a caller-stated minimum share output |
| `redeem`                 | User     | Burn shares, receive tokens (minus fee)              |
| `redeem_checked`         | User     | As `redeem`, reverting below a caller-stated minimum payout (net of fee) |
| `operator_withdraw`      | Operator | Withdraw tokens for external deployment              |
| `operator_deposit`       | Operator | Return tokens to vault                               |
| `operator_update_aum`    | Operator | Update externally deployed AUM (per-vault bps limit) |
| `set_withdrawal_fee`     | Admin    | Set withdrawal fee (max 10%)                         |
| `nominate_admin`         | Admin    | Nominate new admin (two-step transfer)               |
| `accept_admin_nomination`| Nominee  | Accept admin role                                    |
| `set_operator`           | Admin    | Assign new operator                                  |
| `set_fee_recipient`      | Admin    | Change fee recipient                                 |
| `set_aum_limits`         | Admin    | Configure AUM limits                                 |
| `pause` / `unpause`      | Admin    | Emergency pause/unpause                              |
| `close_vault`            | Admin    | Close an empty vault                                 |
| `create_share_token_metadata` | Admin | Create token metadata for share mint            |
| `update_share_token_metadata` | Admin | Update token metadata                           |

## Development

```bash
# One-time per clone: install the repo's git hooks. The pre-commit hook refuses
# a commit that leaves a source file untracked — CI cannot catch that, because
# it only ever sees committed state.
./scripts/install-hooks.sh

# Build
anchor build

# Test
./scripts/run-tests.sh
```

> Use `scripts/run-tests.sh`, not a bare `anchor test`. Anchor's own validator
> preloads the program through genesis, which leaves its ProgramData upgrade
> authority set to the all-zero key — nobody can sign for it, so
> `initialize_config` is unsatisfiable and every suite that creates a vault fails
> in its `before` hook. The script starts a validator first and deploys through
> the loader (so the provider wallet becomes the upgrade authority), then runs
> `anchor test --skip-local-validator`. CI does the same.

## Deployment

```bash
# Setup config
cp deploy/deploy.config.example.json deploy/deploy.config.json
# Edit with your keys

# Deploy a NEW program + vault.
# --rewrite-declare-id is required: this REWRITES declare_id! in lib.rs and the
# mainnet entry in Anchor.toml so they name the freshly generated program.
# Revert both files afterwards, or every later build targets that program.
pnpm run deploy:devnet  -- --rewrite-declare-id
pnpm run deploy:mainnet -- --rewrite-declare-id

# Bootstrap the ProgramConfig of an ALREADY-DEPLOYED program.
# Vault creation is gated on this and it does not exist until run.
node deploy/bootstrap-config.mjs --program-id <ID> --url <RPC> \
  --authority <VAULT_CREATION_AUTHORITY> --keypair <upgrade-authority.json>

# Upgrade (devnet only — see below)
pnpm run upgrade:devnet
```

**Mainnet upgrades are not driven from these scripts.** The mainnet upgrade
authority is a Fordefi MPC key, which cannot sign the way `anchor upgrade`
requires, so `upgrade:mainnet` does not exist. Follow
**[docs/UPGRADE.md](docs/UPGRADE.md)** — buffer upload, `solana program extend`,
and three Fordefi-signed transactions. For an externally held authority,
`bootstrap-config.mjs --unsigned out.json --nonce-account <PUBKEY>` emits a
transaction that does not expire mid-ceremony.

**There is currently no script that adds a vault to an already-deployed
program.** `deploy/new-vault.mjs` is not that tool — it generates a *fresh*
program keypair, rewrites `declare_id!` in your working tree, rebuilds and
deploys a **new program**, which on mainnet costs several SOL and leaves you with
a second, unverified deployment. The removed `initialize:*` scripts did not work
either (they predate `vault_version`, `program_config` and the separate `payer`).

To create a vault on an existing program today, build the `initialize`
instruction directly against the IDL, signed by the `ProgramConfig` authority,
supplying:

| argument / account | notes |
|---|---|
| `program_config` | the singleton PDA; the signer must equal its stored authority |
| `payer` | funds the new accounts; may differ from the signer |
| `vault_version` | `u8`; each `(deposit_mint, vault_version)` pair is **single-use** and cannot be reused once its vault is closed |
| `share_offset` | **required and permanent.** A power of ten from `1000` to `1000000` inclusive; anything else is rejected with `InvalidShareOffset` (6020) |

`share_offset` cannot be changed after creation, and it sets both the share
pricing and the minimum first deposit (`100 x share_offset`, or the mint's
`10^(decimals-3)` floor, whichever is larger). Choose it for what a **base unit**
of the deposit mint is worth — a larger offset widens the margin against
share-burn price manipulation, a smaller one keeps a high unit-value mint
launchable. For a 6-decimal dollar stablecoin `1000000` gives a 100-token
minimum; for an 8-decimal asset worth ~$100k that same offset would demand a
six-figure opening deposit, where `1000` asks roughly a thousandth of that.

## Security

- **Operator Trust**: The operator can withdraw funds and report off-chain balances. Trust assumptions are critical.
- **Withdrawal Fee**: Protects against front-running of AUM updates.
- **Emergency Pause**: Disables all user operations.
- **Upgrade Authority**: Should be transferred to an admin multisig after production deployment.
- **Vault creation is gated.** `initialize` requires a signature from the
  `ProgramConfig` authority, a singleton PDA created once by the program's
  upgrade authority (`initialize_config`). The upgrade authority can also reset
  that key unilaterally via `override_config_authority` — a recovery path for a
  lost config key, and a power it effectively already had by virtue of being able
  to replace the program.
- **Do not make this program immutable.** Revoking the upgrade authority
  permanently prevents `ProgramConfig` from ever being created, and vault
  creation is gated on it — so an immutable program with no config can never
  create another vault, and one with a config can never recover that config's
  authority. Existing vaults keep working; there is no on-chain remedy for
  either case.
- **Share offset is per vault and permanent.** `initialize` fixes the virtual-
  share offset for the vault's life. It must dominate a one-unit retained sliver,
  so it is an absolute count; and `MIN_SUPPLY_MULTIPLE x offset` is the minimum
  first deposit, whose *cost* is that count times what a base unit of the mint is
  worth. Choose it per asset: a 6-decimal dollar stablecoin and an 8-decimal
  asset worth ~$100k cannot share one value — at an offset sized for the former,
  opening the latter would cost six figures. Larger offset = wider margin against
  share-burn price manipulation; smaller = a reachable opening deposit.
- **Token-2022**: Supported, but token extensions may require program upgrades
  for additional accounts. **Vaults must only be created on plain mints without
  transfer-altering extensions**; the supported mint types are agreed as part of
  vault onboarding.

## Reproducible Builds & Verification

The program builds reproducibly: `solana-verify build` in a pinned Docker image (Solana 2.3.0) yields a byte-identical `.so` whose SHA-256 is recorded in [`verified-hashes.txt`](verified-hashes.txt) and asserted in CI, and the program embeds a `security_txt!` contact + source-repo pointer in its bytecode. Because the program is **upgradeable**, on-chain verification is point-in-time. See **[VERIFY.md](VERIFY.md)** for the pinned toolchain, exact commands, expected hashes, and how to verify the live on-chain program.

## License

See [LICENSE](LICENSE).
