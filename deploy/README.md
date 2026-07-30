# Solana Vault Deployment Guide

> **Scope — read first.** This guide covers deploying a **brand-new program**
> together with its first vault. It does **not** cover:
>
> - **Upgrading the live mainnet program.** That is a Fordefi-signed ceremony —
>   follow [`docs/UPGRADE.md`](../docs/UPGRADE.md). There is deliberately no
>   `upgrade:mainnet` script.
> - **Adding a vault to an already-deployed program.** No script does this today;
>   see the table in the [root README](../README.md).
>
> Two things changed that this guide predates:
>
> 1. **Vault creation is permissioned.** A program cannot create any vault until
>    its singleton `ProgramConfig` has been bootstrapped by the upgrade authority
>    (`deploy/bootstrap-config.mjs`). `new-vault.mjs` does this for the program it
>    deploys; an existing program needs it run explicitly.
> 2. **`initialize` takes a permanent `share_offset`.** Set `shareOffset` in
>    `vaultConfig` (a power of ten from 1,000 to 1,000,000). It is validated
>    before any SOL is spent, and it fixes both the share pricing and the minimum
>    first deposit — choose it for what a base unit of your deposit mint is worth.
>    See the root README for how to pick it.

> **One script to deploy everything.** No manual steps, no configuration hassle.

## 🚀 Quick Start (3 Steps)

### 1. Configure Your Vault

Edit `deploy/deploy.config.json`:

```json
{
  "network": "mainnet",
  "rpcEndpoint": "https://mainnet.helius-rpc.com/?api-key=YOUR_KEY",
  "deployerPrivateKey": "YOUR_BASE58_PRIVATE_KEY",
  
  "vaultConfig": {
    "shareOffset": 1000000,
    "depositMint": "YOUR_TOKEN_MINT_ADDRESS",
    "admin": "YOUR_ADMIN_WALLET",
    "operator": "YOUR_OPERATOR_WALLET", 
    "feeRecipient": "YOUR_FEE_WALLET",
    "shareTokenName": "My Vault Token",
    "shareTokenSymbol": "MVT",
    "shareTokenUri": ""
  }
}
```

**Important:**
- ⚠️ Symbol must be **≤10 characters** (Metaplex requirement)
- ✅ Share token decimals **match the deposit mint's** (`mint::decimals = deposit_mint.decimals`)
- ✅ For simplest deployment, set admin/operator/feeRecipient to your deployer wallet
- 🔒 Never commit private keys to git

### 2. Fund Your Wallet

**Mainnet:** Ensure deployer wallet has **5-10 SOL**  
**Devnet:** Use faucet at https://faucet.solana.com/

```bash
# Check balance
solana balance YOUR_WALLET --url mainnet
```

### 3. Deploy

```bash
# Deploy to mainnet
npm run deploy:mainnet -- --rewrite-declare-id

# Deploy to devnet  
npm run deploy:devnet -- --rewrite-declare-id
```

**That's it!** ✨ The script handles everything automatically.

---

## 📦 What Gets Deployed

The `new-vault.mjs` script performs **all 7 steps** automatically:

1. ✅ Generates new program keypair (unique Program ID)
2. ✅ Updates source code with Program ID
3. ✅ Builds the Anchor program
4. ✅ Deploys program to Solana
5. ✅ Initializes vault with your config
6. ✅ Creates token metadata (name, symbol)
7. ✅ Saves deployment record

**Time:** ~3-5 minutes  
**Cost:** ~2-3 SOL (mainnet)

---

## 📋 Example Output

```
======================================================================
🎉 DEPLOYMENT COMPLETE 🎉
======================================================================

📦 PROGRAM:
   ID: FFtotWvN9y5yQvdRY5QuxeujbqddypHcvUjaoJ9N4niD
   Explorer: https://explorer.solana.com/address/...

🏦 VAULT:
   State: 8G37vgJKho7cA3zTcDM8T6KfreauDKPmu4St73vzobGk
   Explorer: https://explorer.solana.com/address/...

🪙 SHARE TOKEN:
   Name: xBTC Vault
   Symbol: xBTCs
   Decimals: 8
   Mint: LsNbjbd2ASdeNmzpWKNH2LPhZfLSGF2w2u48muoNuea

✅ All steps completed successfully!
======================================================================
```

---

## 🔧 Configuration Reference

### Network Options
- `"mainnet"` - Solana mainnet-beta
- `"devnet"` - Solana devnet (for testing)

### Vault Config

| Field | Notes |
|-------|-------|
| `vaultVersion` | `u8`. Each `(depositMint, vaultVersion)` pair is single-use and cannot be reused once its vault is closed. Optional, defaults to 0. |
| `shareOffset` | Power of ten, 1,000–1,000,000. **Permanent** — sets the share pricing and the minimum first deposit (`100 x shareOffset`, or the mint's decimals floor, whichever is larger). Choose it for what a base unit of the deposit mint is worth. Optional, defaults to 1,000,000. |


| Field | Description | Required |
|-------|-------------|----------|
| `depositMint` | Token users will deposit | ✅ Yes |
| `admin` | Full vault control | ✅ Yes |
| `operator` | Manages deployments/AUM | ✅ Yes |
| `feeRecipient` | Receives withdrawal fees | ✅ Yes |
| `shareTokenName` | Display name | ✅ Yes |
| `shareTokenSymbol` | Token symbol (max 10 chars) | ✅ Yes |
| `shareTokenUri` | Metadata URI | ❌ Optional |

### Getting Your Private Key

**From Phantom:**
- Settings → Security & Privacy → Export Private Key

**From Solana CLI:**
```bash
solana-keygen pubkey ~/.config/solana/id.json
```

---

## 🆘 Troubleshooting

### "Symbol too long" Error
**Problem:** Token symbol > 10 characters  
**Solution:** Shorten symbol in config (e.g., `"xBTCvault"` → `"xBTCv"`)

### "Insufficient funds" Error
**Problem:** Not enough SOL for deployment  
**Solution:** Add 5-10 SOL to deployer wallet

```bash
# Check balance
solana balance YOUR_WALLET --url mainnet
```

### "Account not initialized" Error
**Problem:** Deposit mint doesn't exist on chosen network  
**Solution:** Verify deposit mint exists on mainnet/devnet

### "Build failed" Error
**Problem:** Anchor build issue  
**Solution:**
```bash
anchor clean
anchor build
```

### Rate Limited (Devnet Only)
**Problem:** Faucet rate limit reached  
**Solution:** Use web faucet at https://faucet.solana.com/ or wait 5-10 minutes

---

## 📁 Files Created

After deployment:

```
deployments/
  └── {SYMBOL}-{NETWORK}-{TIMESTAMP}.json  # Complete deployment record

target/deploy/
  ├── august_vault-keypair.json            # New program keypair
  └── august_vault-keypair-backup.json     # Previous keypair backup
```

### Deployment Record Example

```json
{
  "program": {
    "programId": "FFtotWvN9y...",
    "deployTx": "2NeAYLMd..."
  },
  "vault": {
    "vaultState": "8G37vgJK...",
    "shareMint": "LsNbjbd2...",
    "depositMint": "FaDsEu2n...",
    "name": "xBTC Vault",
    "symbol": "xBTCs"
  },
  "roles": {
    "admin": "7APZtuCG...",
    "operator": "7APZtuCG...",
    "feeRecipient": "7APZtuCG..."
  }
}
```

---

## 🎯 Testing on Devnet First

**Always test on devnet before mainnet!**

```bash
# 1. Update config to devnet
{
  "network": "devnet",
  ...
}

# 2. Get devnet SOL
solana airdrop 5 YOUR_WALLET --url devnet

# 3. Deploy
npm run deploy:devnet -- --rewrite-declare-id

# 4. Verify on explorer
https://explorer.solana.com/address/YOUR_PROGRAM_ID?cluster=devnet
```

---

## 🔒 Security Best Practices

### Before Deployment
- ✅ Test thoroughly on devnet
- ✅ Verify all configuration addresses
- ✅ Double-check deposit mint address
- ✅ Use premium RPC for mainnet (Helius/QuickNode)

### After Deployment
- ✅ Verify program on Solana Explorer
- ✅ Test deposit/withdraw functionality
- ✅ **Transfer upgrade authority to multisig**
- ✅ Backup program keypair securely
- ✅ Save deployment record

### Transfer Upgrade Authority

**Use Squads for safe transfer:**
```bash
# See main README for detailed instructions
```

**Never:**
- ❌ Commit private keys to git
- ❌ Use same keys for devnet and mainnet
- ❌ Deploy to mainnet without devnet testing
- ❌ Leave upgrade authority with deployer wallet (production)

---

## 🛠️ Advanced Usage

### Custom Config File

```bash
node deploy/new-vault.mjs --network mainnet --config ./my-config.json \
  --rewrite-declare-id
```

### Direct Script Usage

```bash
# With options. --rewrite-declare-id is required for any run that actually
# deploys: it opts in to this script replacing target/deploy/august_vault-keypair.json
# and rewriting declare_id! in lib.rs and [programs.mainnet] in Anchor.toml.
node deploy/new-vault.mjs --network devnet --rewrite-declare-id

# Show help (no opt-in needed; nothing is written)
node deploy/new-vault.mjs --help
```

---

## 📚 Other Scripts (Fallback/Advanced)

| Script | Purpose | Status |
|--------|---------|--------|
| `new-vault.mjs` | Deploy a new program + its first vault | ✅ The main path (needs `--rewrite-declare-id`) |
| `bootstrap-config.mjs` | Create the `ProgramConfig` for an already-deployed program | ✅ Required before that program can create vaults |
| `helpers/upgrade.mjs` | Upgrade program code | ⚠️ **devnet only**; needs `--network devnet --program-id <ID>`. Mainnet goes through [`docs/UPGRADE.md`](../docs/UPGRADE.md) |
| `helpers/metadata.mjs` | Create share-token metadata | ⚠️ Requires the vault **admin** key, and `--program-id` off mainnet |
| `helpers/initialize.mjs` | — | ❌ **Stale and non-functional.** Predates `vault_version`, `program_config`, `payer` and `share_offset`; it exits with a pointer to the current tools |

**For a new deployment, use `new-vault.mjs`.**

---

## ❓ FAQ

**Q: Can I deploy multiple vaults?**  
A: Yes! Each run creates a new program with unique Program ID.

**Q: What if deployment fails mid-way?**  
A: Script has comprehensive error handling. Check the error message and deployment record.

**Q: Can I change token decimals?**  
A: No — the share mint always inherits the deposit mint's decimals
(`mint::decimals = deposit_mint.decimals` in `initialize.rs`), so shares and
deposits are always denominated alike. Pick the deposit mint accordingly.

**Q: How do I update vault configuration after deployment?**  
A: Use admin instructions (set_operator, set_fee_recipient, etc.) via SDK.

**Q: What if I need to deploy to the same Program ID?**  
A: That is an *upgrade*, not a deployment, and this script cannot do it — it
always generates a new program keypair.

- **Mainnet:** follow [`docs/UPGRADE.md`](../docs/UPGRADE.md). The upgrade
  authority is a Fordefi MPC key, so the upgrade is a signing ceremony over a
  buffer, not a script.
- **Devnet:** `node deploy/helpers/upgrade.mjs --network devnet --program-id <ID>`.
- **Adding a vault to a program that is already deployed:** no script does this
  today. See the argument table in the [root README](../README.md), and remember
  the program's `ProgramConfig` must exist first
  (`deploy/bootstrap-config.mjs`).

---

## 🎓 Example: Complete Mainnet Deployment

```bash
# 1. Edit config
vim deploy/deploy.config.json
# Set network: "mainnet"
# Set depositMint: "YOUR_MAINNET_TOKEN"
# Set admin/operator/feeRecipient: "YOUR_WALLET"
# Set shareTokenName: "My Vault"
# Set shareTokenSymbol: "MYV"  # ≤10 chars

# 2. Verify balance
solana balance YOUR_WALLET --url mainnet
# Ensure 5-10 SOL available

# 3. Deploy!
npm run deploy:mainnet -- --rewrite-declare-id

# 4. Verify on explorer
# Check the URLs in the output

# 5. Save deployment record
# Located in: deployments/MYV-mainnet-*.json

# Done! 🎉
```

---

## 📞 Support

**Need help?**
- Check error messages in console output
- Review Solana Explorer for transaction details
- Test on devnet before mainnet
- Check deployment record in `deployments/` folder

**Resources:**
- [Solana Docs](https://docs.solana.com/)
- [Anchor Docs](https://www.anchor-lang.com/)
- Main README: `../README.md`

---

## ✅ Checklist

Before deployment:
- [ ] Config file updated with correct values
- [ ] Token symbol ≤ 10 characters
- [ ] Deposit mint exists on target network
- [ ] Deployer wallet has 5-10 SOL (mainnet)
- [ ] Tested on devnet first

After deployment:
- [ ] Verified program on Solana Explorer
- [ ] Saved deployment record
- [ ] Backed up program keypair
- [ ] Transferred upgrade authority (production)
- [ ] Updated frontend/SDK with new addresses

---

**Ready to deploy? Run:** `npm run deploy:mainnet -- --rewrite-declare-id` 🚀
