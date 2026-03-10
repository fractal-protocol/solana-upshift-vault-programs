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

| Role         | Capabilities                                                        |
|--------------|---------------------------------------------------------------------|
| **User**     | Deposit tokens, redeem shares                                       |
| **Operator** | Withdraw/deposit funds, report deployed AUM                         |
| **Admin**    | Update fees, operator, admin (two-step), fee recipient, pause/unpause |

## Instructions

| Instruction              | Access   | Description                                          |
|--------------------------|----------|------------------------------------------------------|
| `initialize`             | Deployer | Create vault, share mint, set roles                  |
| `deposit`                | User     | Deposit tokens, receive shares                       |
| `redeem`                 | User     | Burn shares, receive tokens (minus fee)              |
| `operator_withdraw`      | Operator | Withdraw tokens for external deployment              |
| `operator_deposit`       | Operator | Return tokens to vault                               |
| `operator_update_aum`    | Operator | Update externally deployed AUM (+-10% limit)         |
| `set_withdrawal_fee`     | Admin    | Set withdrawal fee (max 10%)                         |
| `nominate_admin`         | Admin    | Nominate new admin (two-step transfer)               |
| `accept_admin_nomination`| Nominee  | Accept admin role                                    |
| `set_operator`           | Admin    | Assign new operator                                  |
| `set_fee_recipient`      | Admin    | Change fee recipient                                 |
| `set_aum_limits`         | Admin    | Configure AUM limits                                 |
| `pause` / `unpause`      | Admin    | Emergency pause/unpause                              |
| `close_vault`            | Admin    | Close an empty vault                                 |
| `create_metadata`        | Admin    | Create token metadata for share mint                 |
| `update_metadata`        | Admin    | Update token metadata                                |

## Development

```bash
# Build
anchor build

# Test (runs local validator automatically)
anchor test
```

## Deployment

```bash
# Setup config
cp deploy/deploy.config.example.json deploy/deploy.config.json
# Edit with your keys

# Deploy
pnpm run deploy:devnet
pnpm run deploy:mainnet

# Initialize vault
pnpm run initialize:devnet
pnpm run initialize:mainnet

# Upgrade
pnpm run upgrade:devnet
pnpm run upgrade:mainnet
```

## Security

- **Operator Trust**: The operator can withdraw funds and report off-chain balances. Trust assumptions are critical.
- **Withdrawal Fee**: Protects against front-running of AUM updates.
- **Emergency Pause**: Disables all user operations.
- **Upgrade Authority**: Should be transferred to an admin multisig after production deployment.
- **Token-2022**: Supported, but token extensions may require program upgrades for additional accounts.

## License

See [LICENSE](LICENSE).
