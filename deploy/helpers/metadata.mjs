#!/usr/bin/env node
// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.
import { Connection, Keypair, PublicKey, SystemProgram, SYSVAR_RENT_PUBKEY } from '@solana/web3.js';
import { AnchorProvider, Program, Wallet } from '@coral-xyz/anchor';
import { TOKEN_PROGRAM_ID } from '@solana/spl-token';
import bs58 from 'bs58';
import { readFileSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));

const c = {
  reset: '[0m',
  red: '[31m',
  green: '[32m',
  cyan: '[36m',
};

const log = (msg, color = 'reset') => console.log(`${c[color]}${msg}${c.reset}`);
const logSuccess = (msg) => log(`✅ ${msg}`, 'green');
const logError = (msg) => log(`❌ ${msg}`, 'red');
const logInfo = (msg) => log(`ℹ️  ${msg}`, 'cyan');

// Parse args. `--network` is honoured, not silently discarded: this previously
// accepted the flag and ignored it while defaulting to the LIVE MAINNET program
// and taking the RPC from deploy.config.json — so `meta:devnet` on a
// mainnet-configured machine sent a mainnet transaction while the operator
// believed otherwise.
const args = process.argv.slice(2).reduce((acc, arg, i, arr) => {
  // Reject a flag with no value rather than recording `undefined`. A trailing
  // `--network` (truncated paste, empty shell variable, npm script appending
  // the flag last) would otherwise leave `args.network` falsy, and the
  // cross-check below short-circuits on the falsy operand — silently restoring
  // the accept-and-ignore behaviour this parser exists to prevent, right before
  // a live transaction. Same for `--program-id`, whose default is the LIVE
  // MAINNET program.
  const value = (name) => {
    const v = arr[i + 1];
    if (v === undefined || v.startsWith('-')) {
      logError(`${name} requires a value`);
      process.exit(1);
    }
    return v;
  };
  if (arg === '--program-id') acc.programId = value(arg);
  if (arg === '--network') acc.network = value(arg);
  return acc;
}, { programId: 'up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt' });
const programId = new PublicKey(args.programId);

async function main() {
  log('\n🎨 Creating Vault Token Metadata', 'green');

  // Load config
  const configPath = join(__dirname, '..', 'deploy.config.json');
  const config = JSON.parse(readFileSync(configPath, 'utf-8'));

  // The RPC is what actually decides the cluster, so `network` in the config is
  // only advisory — and an ABSENT `network` must not silently disable this
  // check, or `meta:devnet` against a mainnet rpcEndpoint proceeds exactly as
  // before. Require it whenever --network is passed.
  if (args.network) {
    if (!config.network) {
      logError(
        `--network ${args.network} was passed but deploy.config.json has no ` +
        `"network" field to check it against.\n` +
        `   The cluster is decided by rpcEndpoint (${config.rpcEndpoint}).\n` +
        `   Add "network" to the config so the two can be cross-checked.`
      );
      process.exit(1);
    }
    if (args.network !== config.network) {
      logError(
        `--network ${args.network} contradicts deploy.config.json ` +
        `("network": "${config.network}", rpcEndpoint drives the actual cluster).\n` +
        `   This script sends to the config's RPC, so it would have targeted ` +
        `${config.network}. Fix the config or drop the flag.`
      );
      process.exit(1);
    }
  }
  logInfo(`Cluster: ${config.network ?? 'unspecified'} (${config.rpcEndpoint})`);
  logInfo(`Program: ${programId.toBase58()}`);

  const deployer = Keypair.fromSecretKey(bs58.decode(config.deployerPrivateKey));
  logSuccess(`Deployer: ${deployer.publicKey.toBase58()}`);

  // Setup connection
  const connection = new Connection(config.rpcEndpoint, 'confirmed');
  const balance = await connection.getBalance(deployer.publicKey);
  log(`Balance: ${(balance / 1e9).toFixed(4)} SOL
`, 'cyan');

  // Load program
  const idlPath = join(__dirname, '..', '..', 'target', 'idl', 'august_vault.json');
  const idl = JSON.parse(readFileSync(idlPath, 'utf-8'));
  idl.address = programId.toBase58();
  
  const provider = new AnchorProvider(connection, new Wallet(deployer), { commitment: 'confirmed' });
  const program = new Program(idl, provider);

  // Derive PDAs. The seeds are (tag, deposit_mint, vault_version) — omitting
  // vault_version derives an address that does not exist, so every run failed
  // with AccountNotInitialized after paying a fee. Matches
  // `create_metadata.rs` and the derivation in `new-vault.mjs`.
  const depositMint = new PublicKey(config.vaultConfig.depositMint);
  // Validate before it reaches `Buffer.from([...])`, which coerces silently:
  // `Buffer.from(['v1'])` is `<Buffer 00>` and `Buffer.from([300])` is
  // `<Buffer 2c>`, so a bad config value derives a DIFFERENT live vault's PDAs
  // while the log below prints the value the operator wrote. Unlike
  // `new-vault.mjs` there is no `u8` instruction argument downstream to throw
  // on it, so this is the only place it can be caught.
  const vaultVersion = config.vaultConfig.vaultVersion ?? 0;
  if (!Number.isInteger(vaultVersion) || vaultVersion < 0 || vaultVersion > 255) {
    logError(
      `vaultConfig.vaultVersion must be an integer in 0..=255, got ` +
      `${JSON.stringify(config.vaultConfig.vaultVersion)}.`
    );
    process.exit(1);
  }

  const [vaultState] = PublicKey.findProgramAddressSync(
    [Buffer.from('VAULT_STATE'), depositMint.toBuffer(), Buffer.from([vaultVersion])],
    programId
  );

  const [shareMint] = PublicKey.findProgramAddressSync(
    [Buffer.from('mint'), depositMint.toBuffer(), Buffer.from([vaultVersion])],
    programId
  );

  log(`Deposit Mint: ${depositMint.toBase58()}`, 'cyan');
  log(`Vault Version: ${vaultVersion}`, 'cyan');
  log(`Vault State: ${vaultState.toBase58()}`, 'cyan');
  log(`Share Mint: ${shareMint.toBase58()}
`, 'cyan');

  // Create metadata
  try {
    logInfo('Creating share token metadata...');
    
    const admin = new PublicKey(config.vaultConfig.admin);
    const TOKEN_METADATA_PROGRAM_ID = new PublicKey('metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s');
    
    const [metadataAccount] = PublicKey.findProgramAddressSync(
      [
        Buffer.from('metadata'),
        TOKEN_METADATA_PROGRAM_ID.toBuffer(),
        shareMint.toBuffer(),
      ],
      TOKEN_METADATA_PROGRAM_ID
    );

    log(`Token Name: ${config.vaultConfig.shareTokenName}`, 'cyan');
    log(`Token Symbol: ${config.vaultConfig.shareTokenSymbol}`, 'cyan');
    log(`Token URI: ${config.vaultConfig.shareTokenUri || '(empty)'}
`, 'cyan');

    const metadataTx = await program.methods
      .createShareTokenMetadata(
        config.vaultConfig.shareTokenName,
        config.vaultConfig.shareTokenSymbol,
        config.vaultConfig.shareTokenUri
      )
      .accounts({
        payer: deployer.publicKey,
        admin: admin,
        vaultState,
        depositMint,
        shareMint,
        metadataAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
        tokenMetadataProgram: TOKEN_METADATA_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    logSuccess(`Metadata created!`);
    log(`Transaction: ${metadataTx}
`, 'cyan');

    const explorerBase = 'https://explorer.solana.com';
    log(`View on Solana Explorer:`, 'green');
    log(`  Vault: ${explorerBase}/address/${vaultState.toBase58()}`, 'cyan');
    log(`  Token: ${explorerBase}/address/${shareMint.toBase58()}`, 'cyan');
    log(`  Transaction: ${explorerBase}/tx/${metadataTx}
`, 'cyan');

  } catch (error) {
    logError(`Failed: ${error.message}`);
    if (error.logs) {
      console.log('\nProgram logs:');
      error.logs.forEach(log => console.log(log));
    }
    process.exit(1);
  }
}

main().catch(error => {
  logError(`Error: ${error.message}`);
  console.error(error);
  process.exit(1);
});
