#!/usr/bin/env node
// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

/**
 * STALE — superseded, and non-functional. Do not use.
 *
 * This script predates several changes and cannot succeed as written:
 *   - it calls `initialize` with the OLD 3-argument signature (the instruction
 *     now also takes `vault_version`), and builds PDAs without the
 *     `vault_version` seed, so the accounts would be rejected on-chain;
 *   - it does not pass `program_config` or the separate `payer` that
 *     `initialize` now requires, so Anchor aborts before any RPC;
 *   - it reads the IDL from `deploy/target/idl/` (one `..` short), which does
 *     not exist, and that throw is outside the try/catch and unhandled.
 *
 * It fails closed — there is no path by which it creates a mis-seeded vault.
 * The `initialize`, `initialize:devnet` and `initialize:mainnet` npm scripts
 * that used to point here have been removed, so nothing invokes it any more.
 *
 * **This file is a candidate for deletion**: it has no entry point and cannot
 * work. It is kept only so the guard below explains where the functionality
 * went, for anyone who finds a stale reference to the old scripts. Use instead:
 *   - `deploy/bootstrap-config.mjs` to create the ProgramConfig, then
 *   - `deploy/new-vault.mjs` (or the runbook in `docs/UPGRADE.md`) to create a
 *     vault.
 */
console.error(
  '❌ deploy/helpers/initialize.mjs is stale and non-functional.\n\n' +
  '   `initialize` now requires `vault_version`, `program_config` and a separate\n' +
  '   `payer`, none of which this script supplies.\n\n' +
  '   Use:  node deploy/bootstrap-config.mjs --program-id <ID> ...   (create the config)\n' +
  '     then node deploy/new-vault.mjs                                (create a vault)\n' +
  '   For the mainnet path see docs/UPGRADE.md.'
);
process.exit(1);

// Everything below is the original body, kept for reference when this script is
// either rewritten against the current instruction set or deleted. It is
// unreachable: ES module imports are evaluated first, then the guard above
// exits. Original usage was: node initialize.mjs <programId>

import { Connection, Keypair, PublicKey, SystemProgram, SYSVAR_RENT_PUBKEY } from '@solana/web3.js';
import { AnchorProvider, Program, Wallet } from '@coral-xyz/anchor';
import { TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID } from '@solana/spl-token';
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

const programId = process.argv[2] || '6fLGeBBKZrLVhvnbdeu9v8xo5HTrBCxHebi2247cBprD';

async function main() {
  log('\n🚀 Vault Initialization', 'green');
  log(`Program ID: ${programId}
`, 'cyan');

  // Load config
  const configPath = join(__dirname, '..', 'deploy.config.json');
  const config = JSON.parse(readFileSync(configPath, 'utf-8'));
  
  const deployer = Keypair.fromSecretKey(bs58.decode(config.deployerPrivateKey));
  logSuccess(`Deployer: ${deployer.publicKey.toBase58()}`);

  // Setup connection
  const connection = new Connection(config.rpcEndpoint || 'https://api.devnet.solana.com', 'confirmed');
  const balance = await connection.getBalance(deployer.publicKey);
  log(`Balance: ${(balance / 1e9).toFixed(4)} SOL
`, 'cyan');

  if (balance < 0.1e9) {
    logError('Insufficient balance. Need at least 0.1 SOL');
    process.exit(1);
  }

  // Load program
  const idlPath = join(__dirname, '..', 'target', 'idl', 'august_vault.json');
  const idl = JSON.parse(readFileSync(idlPath, 'utf-8'));
  
  const provider = new AnchorProvider(connection, new Wallet(deployer), { commitment: 'confirmed' });
  
  // Update IDL with correct program ID
  idl.address = programId;
  const program = new Program(idl, provider);

  // Initialize vault
  logInfo('Initializing vault...');
  
  try {
    const depositMint = new PublicKey(config.vaultConfig.depositMint);
    const admin = new PublicKey(config.vaultConfig.admin);
    const operator = new PublicKey(config.vaultConfig.operator);
    const feeRecipient = new PublicKey(config.vaultConfig.feeRecipient);

    const [vaultState] = PublicKey.findProgramAddressSync(
      [Buffer.from('VAULT_STATE'), depositMint.toBuffer()],
      program.programId
    );

    const [shareMint] = PublicKey.findProgramAddressSync(
      [Buffer.from('mint'), depositMint.toBuffer()],
      program.programId
    );

    const [vaultTokenAta] = PublicKey.findProgramAddressSync(
      [Buffer.from('token_vault'), depositMint.toBuffer()],
      program.programId
    );

    // Determine token program (check if deposit mint is Token-2022)
    const mintInfo = await connection.getAccountInfo(depositMint);
    const isToken2022 = mintInfo?.owner.equals(TOKEN_2022_PROGRAM_ID);
    const tokenProgram = isToken2022 ? TOKEN_2022_PROGRAM_ID : TOKEN_PROGRAM_ID;
    
    logInfo(`Using ${isToken2022 ? 'Token-2022' : 'Token'} program`);

    const tx = await program.methods
      .initialize(admin, operator, feeRecipient)
      .accounts({
        vaultState,
        shareMint,
        vaultTokenAta,
        depositMint,
        signer: deployer.publicKey,
        systemProgram: SystemProgram.programId,
        tokenProgram,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    logSuccess(`Vault initialized!`);
    log(`Transaction: ${tx}`, 'cyan');
    log(`Vault State: ${vaultState.toBase58()}`, 'cyan');
    log(`Share Mint: ${shareMint.toBase58()}`, 'cyan');

    // Create metadata
    logInfo('\nCreating share token metadata...');
    
    const TOKEN_METADATA_PROGRAM_ID = new PublicKey('metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s');
    const [metadataAccount] = PublicKey.findProgramAddressSync(
      [
        Buffer.from('metadata'),
        TOKEN_METADATA_PROGRAM_ID.toBuffer(),
        shareMint.toBuffer(),
      ],
      TOKEN_METADATA_PROGRAM_ID
    );

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

    log('✅ Vault fully initialized and ready to use! 🎉\n', 'green');
    
    const explorerBase = 'https://explorer.solana.com';
    const cluster = config.network === 'mainnet' ? '' : `?cluster=${config.network}`;
    log(`View on explorer:`, 'cyan');
    log(`  Program: ${explorerBase}/address/${programId}${cluster}`, 'cyan');
    log(`  Vault: ${explorerBase}/address/${vaultState.toBase58()}${cluster}`, 'cyan');
    log(`  Token: ${explorerBase}/address/${shareMint.toBase58()}${cluster}
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

main();
