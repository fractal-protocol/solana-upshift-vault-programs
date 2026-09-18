# Upshift Vault Programs

A share-based vault on Solana built with Anchor. Users deposit an SPL token and receive shares in return. An operator deploys vault funds externally (e.g., yield strategies), while an admin manages roles, fees, and emergency controls.

This workspace builds two programs:

| Crate | Artifact | Status |
|---|---|---|
| `programs/august-vault` | `august_vault.so` | Live on mainnet and devnet. Everything below describes this program. |
| `programs/august-withdrawal-queue` | `august_withdrawal_queue.so` | `initialize_queue`, two config setters, `request_withdrawal` and `update_request` over the `WithdrawalQueue` / `WithdrawalRequest` state accounts. Finalization, cancellation and `release_vault` are not implemented. **Not deployed anywhere, and must not be until `release_vault` lands.** |

The queue will let a vault route redemptions through a request-and-cooldown flow
instead of paying out instantly. The vault side of that is already in place: a
vault can name a `withdrawal_queue_authority` and will then redeem only for that
key. No vault names this program, and none is set, so every vault still redeems
instantly. Until the queue has instructions it builds and deploys but does
nothing.

**Both programs are built one crate at a time, never as a whole workspace.** The
queue depends on the vault with `features = ["cpi"]`, and `cpi` implies
`no-entrypoint`. Cargo unifies a dependency's features across a single build, so a
workspace-wide `cargo build-sbf` compiles the vault once under
`default ∪ cpi ∪ no-entrypoint` and emits a ~900-byte `august_vault.so` with no
entrypoint for the loader to dispatch to. `integration-tests/build.rs` builds per
manifest for this reason; CI uses `anchor build`, which already builds each program
separately. Two guards catch a regression: `integration-tests/tests/embedded_artifacts.rs`
and the size floor in CI's program-size step.

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
| **Admin**              | Update fees, operator, operator subaccounts, admin (two-step), fee recipient, pause/unpause |
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
| `redeem`                 | User, or the withdrawal queue | Burn shares, receive tokens (minus fee)  |
| `redeem_checked`         | User, or the withdrawal queue | As `redeem`, reverting below a caller-stated minimum payout (net of fee) |
| `operator_withdraw`      | Operator | Withdraw tokens for external deployment, to a registered subaccount if any |
| `operator_deposit`       | Operator | Return tokens to vault, from a registered subaccount if any |
| `operator_update_aum`    | Operator | Update externally deployed AUM (per-vault bps limit) |
| `set_withdrawal_fee`     | Admin    | Set withdrawal fee (max 10%)                         |
| `nominate_admin`         | Admin    | Nominate new admin (two-step transfer)               |
| `accept_admin_nomination`| Nominee  | Accept admin role                                    |
| `set_operator`           | Admin    | Assign new operator                                  |
| `register_subaccount`    | Admin    | Register a permitted operator destination             |
| `deregister_subaccount`  | Admin    | Remove one, once its outstanding principal is zero    |
| `settle_subaccount_loss` | Admin    | Write down principal a destination will never return  |
| `set_fee_recipient`      | Admin    | Change fee recipient                                 |
| `set_aum_limits`         | Admin    | Configure AUM limits                                 |
| `attach_withdrawal_queue` | Admin    | Attach this vault's withdrawal queue, once the queue has initialized it |
| `detach_withdrawal_queue` | Admin, plus the attached queue | Detach the queue, restoring direct redemption; pending requests still finalize or cancel |
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

### Withdrawal queue (optional, per vault)

`VaultState.withdrawal_queue_authority` decides who may redeem. It is
**`Pubkey::default()` on every vault today**, which means redemption is instant
and open to any share holder — the behaviour described above, and the behaviour
every vault created before this field existed keeps without a migration, since
those accounts carry zeroes at that offset.

When an admin attaches a withdrawal queue, the field holds that queue's PDA and
becomes the *only* key `redeem` and `redeem_checked` accept; a direct holder
redemption is refused with `WithdrawalQueueRequired` (6021). Holders then exit by
requesting through the queue and waiting out its cooldown, and the queue redeems
on their behalf by CPI, signing as that PDA. It is a per-vault setting, not a
program-wide one: vaults on the same program can differ.

Two instructions write the field. `attach_withdrawal_queue` takes no key
argument: the only key it can store is this vault's queue PDA,
`["withdrawal_queue", vault_state]` under the queue program id hardcoded in the
vault, and it stores that key only once the queue program has initialized the
account there. The `queue` account passed must sit at that address, be owned by
the queue program and hold data, or the call fails with
`InvalidWithdrawalQueueAuthority` (6022); a vault that already has a queue
attached fails with `WithdrawalQueueAlreadyAttached` (6024). Because the PDA is a
pure function of the vault, there is no swap to a different queue, only attach
and detach. `detach_withdrawal_queue` clears the field and needs the attached
queue's signature: a different signer fails with `WrongWithdrawalQueueSigner`
(6023), an unsigned slot with Anchor's 3010, and a vault with no queue with
`WithdrawalQueueNotAttached` (6025). The queue will give that signature only from
its `release_vault`; pending requests survive a release and finalize or cancel
afterwards.
Together those rules mean the field can only ever hold a key under the queue
program's control, so no admin mistake can freeze exits while deposits keep
working.
Attach emits `WithdrawalQueueAttached { vault, queue }` and detach emits
`WithdrawalQueueDetached { vault, queue }`.

The queue program's admin side exists in source: `initialize_queue` creates the
queue PDA and its two escrow token accounts, for a classic SPL mint
or a Token-2022 mint carrying at most the two metadata extensions
(`UnsupportedDepositMint`, 6006, otherwise); `set_cooldown` (at most 30 days)
and `set_fulfillment_window` (zero disables expiry, else at most 90 days)
configure it. Attaching it on the vault is what makes it live.

Holders use `request_withdrawal(request_id, shares, min_assets_out, finalizer)`:
their shares move to the queue's escrow and a request account is created at
`["withdrawal_request", queue, owner, request_id]`, stamped with the cooldown and
window in force at that moment. It requires the queue to be accepting
(`NotAcceptingRequests`, 6007) and the vault's gate to point at the queue, a
nonzero amount (`ZeroShares`, 6008), and a recipient that holds the deposit mint
and belongs to neither the queue nor the vault (`InvalidRecipient`, 6009).
`update_request(expected_sequence, min?, finalizer?)` plus an optional new
recipient account lets the owner adjust those three fields while the request is
pending and unexpired (`RequestExpired`, 6011); the sequence stamp must match
(`StaleRequestSequence`, 6012) so a delayed instruction cannot land on a
recreated request. Finalization, cancellation and `release_vault` are not
implemented yet.

**Do not deploy the queue program, or attach a queue, on any cluster yet.** Once
`initialize_queue` is deployable the queue PDA can be created, so attaching
becomes possible, while detaching needs `release_vault`, which does not exist.
Attaching before it lands would be a one-way door: a gated vault could be
reopened only by a vault program upgrade. Recorded as a merge blocker on
AUGUST-7669 and AUGUST-7672.

### Operator subaccounts (optional, per vault)

`VaultState.subaccount_count` decides whether operator funds go to a registered
destination, separating the power to *move* vault funds from the addresses that
*receive* them. It is **zero on every vault created before the registry existed**
— which is what the fork fixtures pin — so both operator transfers use the
operator's own ATA and the feature arrives without a migration.

Each permitted destination is a PDA seeded `["SUBACCOUNT", vault_state, address]`,
created by `register_subaccount`. Membership is therefore a seed derivation
rather than a list: no scan, no cap, and rent paid per address instead of charged
to every vault. Once any is registered, `operator_withdraw` and
`operator_deposit` must name one and the operator's own ATA is refused.

**The address must prove it can return funds before it can be named.** Its
deposit-mint ATA must already carry the vault PDA as SPL delegate with a nonzero
allowance, granted by the subaccount itself — so the rollout is *custody
approves, then admin switches*. That is the only proof available on-chain, and
it needs the owner's signature, unlike ATA existence, which
`create_associated_token_account` lets any third party manufacture. Judging the
address by shape instead would miss an uncreated ATA address (System-owned and
empty, so it reads as an ordinary wallet) and would wrongly refuse an SPL token
multisig, which can sign.

**Custody grants the delegation, and only custody can.** SPL `Approve` is
authorized by the token account's owner, so neither the admin nor the operator
can do it. Custody signs `approve(subaccount_ata, vault_state, n)`, where
`vault_state` is the vault PDA derived
`["VAULT_STATE", deposit_mint, vault_version]`. Note `approve` **overwrites**
rather than accumulates: topping up from 100 to 150 means approving 150, not 50.

**The allowance bounds deployments, not just returns.** `operator_withdraw`
requires the destination's delegation to cover **that destination's** outstanding
principal plus the amount being sent. What that guarantees is narrow and worth
stating exactly: the vault can pull from the destination's ATA, up to the
allowance, without custody signing again. It does **not** guarantee the funds
will be in that ATA. Once custody deploys them to a venue they leave it
entirely, and a return then fails inside the token program with
`InsufficientFunds` until custody brings them back. That leg is a trust
assumption on custody, not something this program can enforce — it never sees
it.

Principal is tracked per destination, on its registry PDA — a vault-wide figure
would make every destination's custody cover every other's exposure. It counts
tokens actually sent and not returned: not reported AUM, since
`operator_update_aum` marks value with no tokens moving and a mark-down would
otherwise reopen capacity; and not the destination ATA's balance, which anyone
can inflate with a donation. `VaultState.deployed_principal` carries the total for
monitoring only — nothing on-chain trusts it. Size the grant to the cycle you
intend to deploy: returns spend it down and SPL clears the delegation once it
reaches zero, so it must be re-granted per cycle, and a lapsed or short one fails
with `SubaccountDelegationMissing` (6023). The program's check covers the
delegation only — a short *balance* or a frozen source ATA still surface as the
token program's own errors.

**One subaccount address serves one vault per deposit mint.** An ATA is derived
from (owner, mint), and an SPL token account has a single delegate slot that
`approve` overwrites — so two vaults sharing a deposit mint cannot share a
subaccount address. Give custody a distinct address per vault. Naming an address
already delegated to another vault is refused at config time, but the reverse
order is not preventable from here: if custody later approves for a second vault,
the first vault's next transfer fails with `SubaccountDelegationMissing` (6023),
which is the non-obvious cause of that error. No two live vaults share a deposit
mint today.

**Use an address that holds nothing else.** SPL cannot tell the vault's tokens
from the destination's own, so anything in that ATA is reachable by the vault up
to the allowance — approve 600 against a balance of 600 and a return of 600
succeeds, booking the surplus as vault assets. A pre-existing balance also
*blocks* loss settlement, since the shortfall is `principal - ata.amount` and so
reads zero while unrelated funds sit there, leaving the destination impossible to
deregister and the vault impossible to close until they are moved out. A
dedicated address per vault avoids both.

**The allowance is also the compromise radius.** A compromised *operator* needs
no admin involvement to pull the whole standing allowance into the vault; a
compromised admin can additionally rotate the operator to itself, roll the
subaccount back and withdraw to its own ATA. Because the same number now bounds
what can be deployed, keeping it to one cycle bounds both at once — a
compromised key reaches roughly what is already deployed, which it could reach
via the reserve anyway.

**Deregistering stops this vault honouring the delegation — it does not revoke
it.** The destination's ATA stops being accepted, so the vault cannot pull; but
the allowance stands, and an admin can re-register the same address and resume
without any new approval from custody. It is therefore a lever against a
compromised *operator*, or against custody gone unreachable — not against a
compromised admin. Only custody's `revoke` ends the exposure.

A destination cannot be deregistered while the vault is still owed principal
there, so funds cannot be stranded by removing the record of them. After a
realized loss that principal will never return in tokens, so
`settle_subaccount_loss` writes it down — admin only, since the operator
reducing principal is the capacity-reopening move the coverage rule exists to
prevent, and bounded by the principal *not* sitting at the destination's ATA, so
a live obligation cannot be written off. It leaves `deployed_aum` alone; reported
value stays the operator's to move under its bps limits. Without it a vault that
took a loss could never be closed. Removing the last registration returns the
vault to paying the operator's own ATA, which also recovers funds left there from
before the first registration.

The first registration adopts the vault's existing `deployed_principal`, and its
allowance must cover it — so a vault with funds already out stays covered. That
principal is physically in the *operator's* ATA, which is no longer an accepted
source, so the operator should move it to the new destination rather than leave it
to be written off. Registering while `deployed_aum == 0` avoids the question.

**Token-2022 CPI Guard.** The return transfer is the shape the guard permits: it
blocks CPI transfers authorized by the account's *owner*, not by a delegate. The
guard also blocks `Approve` inside a CPI, so on such an ATA the delegation must
be granted as a top-level instruction — which a wallet or MPC signer does
anyway, and a PDA-owned ATA cannot enable the extension in the first place. A
PDA-based multisig therefore cannot combine CPI Guard with this feature, since
it can only issue `Approve` via CPI.

The registry is **only as good as its addresses**: each must be custody the
operator cannot unilaterally sweep — an MPC wallet such as Fordefi, which is an
ordinary System-owned account signing directly, or a multisig such as Squads,
whose vault is a PDA owned by its own program — and it depends on admin being a
different *party* from the operator, which is not enforced and not checkable
on-chain.

## Security

- **Operator Trust**: The operator can move funds out of the vault and report
  off-chain balances. Trust assumptions are critical. A registered subaccount
  narrows this — the operator still moves funds, but only to an address the
  admin named — and nothing changes while none is registered or where admin and
  operator are the same key. See the operator-subaccount section for what
  deregistering does and does not neutralise.
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

### Writing up a security fix in this repository

This repository is **public, and the program it builds is upgradeable and live**.
A fix therefore becomes readable the moment it is pushed, while the deployed
program is still vulnerable — the merge and the remediation are separate events,
often days apart.

Two consequences, and they pull in opposite directions:

1. **A fix in public discloses the weakness it fixes, and that is unavoidable.**
   The reasoning behind a security-relevant constant has to live next to it, and
   the file has to stay public for the build to be reproducibly verifiable. Do not
   try to obscure it — an unexplained constant is worse than a disclosed one,
   because the next person to touch it will not know what it is load-bearing for.
2. **Quantified exploit results are a different matter, and they are avoidable.**
   Sweep outputs, profitability percentages and worked attack parameters convert
   "there is a weakness here" into "here is how well it pays". They add nothing a
   reviewer of this repository needs.

So: **explain the mechanism, keep the numbers out.** State what property is being
defended and why the chosen value defends it; put sweep regions, profitability
figures and worked attack configurations in the internal security review and
reference it. `share_burn_pricing.rs` follows this pattern and says so explicitly;
`state/vault.rs`'s `EXTRA_SHARES` comment explains the manoeuvre without a single
figure.

This applies to **commit messages and pull-request descriptions as much as to
code** — they are the same public artifact, and a commit message cannot be edited
after the fact without rewriting history, which on this repository would re-SHA
the commits that `verified-hashes.txt`'s CI provenance is pinned to. Get it right
the first time; a PR body can be edited, a commit message effectively cannot.

## Reproducible Builds & Verification

The program builds reproducibly: `solana-verify build` in a pinned Docker image (Solana 2.3.0) yields a byte-identical `.so` whose SHA-256 is recorded in [`verified-hashes.txt`](verified-hashes.txt) and asserted in CI, and the program embeds a `security_txt!` contact + source-repo pointer in its bytecode. Because the program is **upgradeable**, on-chain verification is point-in-time. See **[VERIFY.md](VERIFY.md)** for the pinned toolchain, exact commands, expected hashes, and how to verify the live on-chain program.

## License

See [LICENSE](LICENSE).
