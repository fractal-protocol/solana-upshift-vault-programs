#!/usr/bin/env node
// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

/**
 * Complete Vault Deployment Script
 * 
 * This script handles the entire deployment process:
 * 1. Generates a new program keypair
 * 2. Updates program ID in source code
 * 3. Builds the program
 * 4. Deploys to mainnet
 * 5. Initializes the vault
 * 6. Creates metadata
 * 7. Saves deployment record
 * 
 * Usage: node new-vault.mjs [--network mainnet|devnet]
 */

import { redactEndpoint } from './helpers/redact.mjs';

/// Mirror of the program's share-offset rule (`VaultState::is_valid_share_offset`).
/// Kept in step with `MIN_SHARE_OFFSET` / `MAX_SHARE_OFFSET` in
/// `programs/august-vault/src/state/vault.rs`.
const MIN_SHARE_OFFSET = 1_000;
const MAX_SHARE_OFFSET = 1_000_000;
const DEFAULT_SHARE_OFFSET = 1_000_000;
/// Mirror of `MIN_SUPPLY_MULTIPLE`. Named rather than inlined into the message
/// below so `scripts/check-references.mjs` can cross-check it: it is what the
/// pre-flight quotes as the minimum first deposit, right before an operator
/// commits several SOL.
const MIN_SUPPLY_MULTIPLE = 100;
function isValidShareOffset(v) {
  if (!Number.isInteger(v) || v < MIN_SHARE_OFFSET || v > MAX_SHARE_OFFSET) return false;
  let p = 1;
  while (p < v) p *= 10;
  return p === v;
}
import { Connection, Keypair, PublicKey, SystemProgram, SYSVAR_RENT_PUBKEY } from '@solana/web3.js';
import { AnchorProvider, Program, Wallet } from '@coral-xyz/anchor';
// `BN` is not a named ESM export of the anchor package (it is CommonJS), and
// bn.js is only a transitive dependency, so reach it through anchor's default
// export. `node --check` does NOT catch a bad named import — only actually
// loading the module does.
import anchorPkg from '@coral-xyz/anchor';
const { BN } = anchorPkg;
import { TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, getMint } from '@solana/spl-token';
import bs58 from 'bs58';
import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'fs';
import { execSync } from 'child_process';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { ensureProgramConfig } from './helpers/program-config.mjs';

const __dirname = dirname(fileURLToPath(import.meta.url));
const projectRoot = join(__dirname, '..');

// Colors
const c = {
  reset: '[0m',
  red: '[31m',
  green: '[32m',
  yellow: '[33m',
  cyan: '[36m',
  bold: '[1m',
};

const log = (msg, color = 'reset') => console.log(`${c[color]}${msg}${c.reset}`);
const logSuccess = (msg) => log(`✅ ${msg}`, 'green');
const logError = (msg) => log(`❌ ${msg}`, 'red');
const logInfo = (msg) => log(`ℹ️  ${msg}`, 'cyan');
const logWarning = (msg) => log(`⚠️  ${msg}`, 'yellow');
const logStep = (step, msg) => log(`
${c.bold}[STEP ${step}]${c.reset} ${msg}`, 'cyan');

// Parse args
const args = process.argv.slice(2).reduce((acc, arg, i, arr) => {
  // Reject a flag with no value rather than recording `undefined`: a trailing
  // `--network` would otherwise silently disable the cluster cross-check that
  // exists to stop a devnet-labelled run deploying to mainnet.
  const value = (name) => {
    const v = arr[i + 1];
    if (v === undefined || v.startsWith('-')) {
      console.error(`❌ ${name} requires a value`);
      process.exit(1);
    }
    return v;
  };
  if (arg === '--network' || arg === '-n') acc.network = value(arg);
  if (arg === '--config' || arg === '-c') acc.configPath = value(arg);
  if (arg === '--rewrite-declare-id') acc.rewriteDeclareId = true;
  if (arg === '--help' || arg === '-h') {
    console.log(`
Usage: node new-vault.mjs [options]

Options:
  -n, --network <name>    Network: devnet|mainnet (default: from config)
  -c, --config <path>     Config file path (default: ./deploy.config.json)
      --rewrite-declare-id  REQUIRED — nothing runs without it. Rewrites
                            declare_id! in lib.rs and Anchor.toml in your
                            working tree so they name the newly generated
                            program. Revert both files afterwards.
  -h, --help             Show this help

This script DEPLOYS A NEW PROGRAM. It is not the tool for adding a vault to an
already-deployed program — see the README.

This script will:
  1. Generate a new program keypair
  2. Update program ID in source files
  3. Build the Anchor program
  4. Deploy to Solana
  5. Initialize the vault
  6. Create token metadata
  7. Save deployment record
    `);
    process.exit(0);
  }
  return acc;
}, { configPath: join(__dirname, 'deploy.config.json'), network: null });

// Load config
function loadConfig() {
  if (!existsSync(args.configPath)) {
    logError(`Config not found: ${args.configPath}`);
    log('Create deploy.config.json from deploy.config.example.json', 'yellow');
    process.exit(1);
  }

  const config = JSON.parse(readFileSync(args.configPath, 'utf-8'));
  
  if (!config.deployerPrivateKey || config.deployerPrivateKey === 'YOUR_BASE58_PRIVATE_KEY_HERE') {
    logError('Set deployerPrivateKey in config file');
    process.exit(1);
  }

  // Validate symbol length (max 10 chars for Metaplex)
  if (config.vaultConfig.shareTokenSymbol && config.vaultConfig.shareTokenSymbol.length > 10) {
    logError(`Token symbol "${config.vaultConfig.shareTokenSymbol}" is too long (max 10 characters)`);
    logInfo('Please update shareTokenSymbol in your config file');
    process.exit(1);
  }

  // Validate vanity prefix if provided
  if (config.vanityPrefix) {
    if (config.vanityPrefix.length > 8) {
      logWarning('Vanity prefix is long (>8 chars). This may take a very long time!');
    }
  }

  // Check the destructive opt-in FIRST, before anything mutates the worktree.
  //
  // Step 1 (`generateProgramKeypair`) already backs up and REPLACES
  // target/deploy/august_vault-keypair.json, so guarding only at step 2 still
  // rotated a deployment keypair on a supposedly-safe run — and a second run
  // overwrote the backup with the first run's throwaway, so the original was
  // recoverable only from elsewhere. Nothing may be written before this check.
  if (!args.rewriteDeclareId) {
    logError('This script DEPLOYS A NEW PROGRAM and rewrites your worktree.');
    logError('   It replaces target/deploy/august_vault-keypair.json, and rewrites');
    logError('   declare_id! in lib.rs and [programs.mainnet] in Anchor.toml, so');
    logError('   every later build targets a different program.');
    logError('   Re-run with --rewrite-declare-id if that is genuinely what you want.');
    logError('   To add a vault to an EXISTING program, see the README instead.');
    process.exit(1);
  }

  // Validate the share offset HERE, before any deployment work. The program
  // rejects a bad value at `initialize` — which is step 5, after the keypair has
  // been generated, the program rebuilt and deployed at a cost of 2-5 SOL. An
  // invalid offset would therefore burn a real deployment before predictably
  // failing with InvalidShareOffset, and the offset is permanent once set.
  const shareOffset = config.vaultConfig?.shareOffset ?? DEFAULT_SHARE_OFFSET;
  if (!isValidShareOffset(shareOffset)) {
    logError(
      `vaultConfig.shareOffset (${shareOffset}) is not valid. It must be a power ` +
      `of ten from ${MIN_SHARE_OFFSET.toLocaleString()} to ` +
      `${MAX_SHARE_OFFSET.toLocaleString()} inclusive.`
    );
    logError(
      '   The offset is fixed for the vault\'s life and sets both the pricing and ' +
      'the minimum first deposit, so choose it for what a base unit of the ' +
      'deposit mint is worth.'
    );
    process.exit(1);
  }
  config.vaultConfig = { ...config.vaultConfig, shareOffset };

  // `--network` must not merely relabel the config. The cluster actually used is
  // decided by `config.rpcEndpoint` (see getRpcEndpoint and deployProgram), so
  // accepting `--network devnet` against a mainnet rpcEndpoint would deploy to
  // MAINNET — spending real SOL — while printing "devnet". Cross-check instead.
  if (args.network) {
    if (config.network && config.network !== args.network) {
      logError(
        `--network ${args.network} contradicts deploy.config.json ` +
        `("network": "${config.network}").`
      );
      logError(
        `   The cluster is decided by rpcEndpoint (${config.rpcEndpoint ? redactEndpoint(config.rpcEndpoint) : 'unset'}), ` +
        `so this would have targeted ${config.network}.`
      );
      logError('   Fix the config or drop the flag.');
      process.exit(1);
    }
    if (!config.network && config.rpcEndpoint) {
      logError(
        `--network ${args.network} was passed, but the config has no "network" ` +
        `field to check it against and rpcEndpoint (${redactEndpoint(config.rpcEndpoint)}) is ` +
        `what decides the cluster.`
      );
      logError('   Add "network" to deploy.config.json so the two can be cross-checked.');
      process.exit(1);
    }
    config.network = args.network;
  }
  return config;
}

// Generate new program keypair with optional vanity prefix
function generateProgramKeypair(vanityPrefix = null) {
  logStep(1, 'Generating new program keypair');
  
  const newKeypairPath = join(projectRoot, 'target', 'deploy', 'august_vault-keypair-new.json');
  const currentKeypairPath = join(projectRoot, 'target', 'deploy', 'august_vault-keypair.json');
  const backupKeypairPath = join(projectRoot, 'target', 'deploy', 'august_vault-keypair-backup.json');
  
  // Backup current keypair if it exists
  if (existsSync(currentKeypairPath)) {
    logInfo('Backing up existing program keypair...');
    execSync(`cp "${currentKeypairPath}" "${backupKeypairPath}"`, { cwd: projectRoot });
    logSuccess('Existing keypair backed up');
  }
  
  let programId;
  
  if (vanityPrefix) {
    // Generate vanity address
    logInfo(`Generating vanity address starting with "${vanityPrefix}"...`);
    logWarning('This may take a while (30 seconds to 5 minutes depending on prefix length)');

    // solana-keygen grind outputs to current directory, so we run it in target/deploy
    const targetDeployDir = join(projectRoot, 'target', 'deploy');
    if (!existsSync(targetDeployDir)) {
      mkdirSync(targetDeployDir, { recursive: true });
    }

    const vanityCmd = `solana-keygen grind --starts-with ${vanityPrefix}:1`;
    const output = execSync(vanityCmd, { encoding: 'utf-8', cwd: targetDeployDir });

    // Extract keypair filename from output (format: "Wrote keypair to <PREFIX>....json")
    const fileMatch = output.match(/Wrote keypair to ([^\s]+\.json)/);
    if (!fileMatch) {
      logError('Failed to find generated keypair file from vanity generation');
      logError(`Output was: ${output}`);
      process.exit(1);
    }

    const generatedFile = join(targetDeployDir, fileMatch[1]);

    // Move generated file to new keypair path
    execSync(`mv "${generatedFile}" "${newKeypairPath}"`, { cwd: projectRoot });

    // Get the program ID from the keypair
    const keypairData = JSON.parse(readFileSync(newKeypairPath, 'utf-8'));
    const keypair = Keypair.fromSecretKey(Uint8Array.from(keypairData));
    programId = keypair.publicKey.toBase58();

    logSuccess(`✨ Vanity Program ID: ${programId}`);
  } else {
    // Generate random keypair
    logInfo('Generating new keypair...');
    const output = execSync(
      `solana-keygen new --no-bip39-passphrase -o "${newKeypairPath}" --force`,
      { encoding: 'utf-8', cwd: projectRoot }
    );
    
    // Extract program ID from output
    const match = output.match(/pubkey: ([A-Za-z0-9]+)/);
    if (!match) {
      logError('Failed to extract program ID from keygen output');
      process.exit(1);
    }
    
    programId = match[1];
    logSuccess(`New Program ID: ${programId}`);
  }
  
  // Replace current keypair with new one
  execSync(`cp "${newKeypairPath}" "${currentKeypairPath}"`, { cwd: projectRoot });
  
  return programId;
}

// Update program ID in source files
//
// DESTRUCTIVE: rewrites `declare_id!` in lib.rs and the mainnet entry in
// Anchor.toml, in the working tree. Anyone who runs this script to see what it
// does — or who gets as far as step 2 before aborting — is left with a mutated
// checkout that builds a DIFFERENT program, and the next `anchor build` silently
// produces bytecode for it. That has to be opted into.
function updateProgramId(programId) {
  // Belt and braces: loadConfig() already refused without the opt-in, so this
  // can only fire if a future caller reaches step 2 by another path.
  if (!args.rewriteDeclareId) {
    throw new Error('updateProgramId reached without --rewrite-declare-id');
  }
  logStep(2, 'Updating program ID in source files');
  
  // Update lib.rs
  const libRsPath = join(projectRoot, 'programs', 'august-vault', 'src', 'lib.rs');
  let libRs = readFileSync(libRsPath, 'utf-8');
  const oldDeclareId = libRs.match(/declare_id!\("([^"]+)"\);/);
  
  if (oldDeclareId) {
    libRs = libRs.replace(/declare_id!\("([^"]+)"\);/, `declare_id!("${programId}");`);
    writeFileSync(libRsPath, libRs);
    logSuccess(`Updated lib.rs (old: ${oldDeclareId[1]})`);
  } else {
    logError('Could not find declare_id! in lib.rs');
    process.exit(1);
  }
  
  // Update Anchor.toml
  const anchorTomlPath = join(projectRoot, 'Anchor.toml');
  let anchorToml = readFileSync(anchorTomlPath, 'utf-8');
  const oldMainnetId = anchorToml.match(/\[programs\.mainnet\]\s*august_vault = "([^"]+)"/);
  
  if (oldMainnetId) {
    anchorToml = anchorToml.replace(
      /(\[programs\.mainnet\]\s*august_vault = )"([^"]+)"/,
      `$1"${programId}"`
    );
    writeFileSync(anchorTomlPath, anchorToml);
    logSuccess(`Updated Anchor.toml (old: ${oldMainnetId[1]})`);
  } else {
    logWarning('Could not update Anchor.toml mainnet section');
  }
  
  return programId;
}

// Build program
function buildProgram() {
  logStep(3, 'Building Anchor program');
  
  try {
    logInfo('Running: anchor build');
    execSync('anchor build', { 
      cwd: projectRoot,
      stdio: ['inherit', 'pipe', 'pipe'],
      encoding: 'utf-8'
    });
    logSuccess('Build complete');
    return true;
  } catch (error) {
    logError('Build failed');
    console.error(error.stdout || error.message);
    return false;
  }
}

// Deploy program
async function deployProgram(config, deployer, programId) {
  logStep(4, 'Deploying program to Solana');
  
  try {
    // Create temporary deployer keypair file
    const tmpKeypairPath = join(__dirname, '.tmp-deployer.json');
    const keypairArray = Array.from(deployer.secretKey);
    writeFileSync(tmpKeypairPath, JSON.stringify(keypairArray));
    
    logInfo(`Deploying to ${config.network}...`);
    logInfo(`Cluster actually used: ${redactEndpoint(config.rpcEndpoint) || config.network}`);
  // Echo the offset explicitly. It is permanent, unchangeable, and defaults
  // silently when the config key is absent or mistyped — so it is the one
  // parameter the operator most needs to see before spending SOL.
  logInfo(
    `Share offset: ${config.vaultConfig.shareOffset.toLocaleString()}` +
    `${config.vaultConfig.shareOffset === DEFAULT_SHARE_OFFSET ? ' (default)' : ''}` +
    ` -> minimum first deposit ${(
      MIN_SUPPLY_MULTIPLE * config.vaultConfig.shareOffset
    ).toLocaleString()} base units (or the mint's decimals floor, whichever is larger)`
  );
    logWarning('This may take several minutes and cost 2-5 SOL');
    
    // Use custom RPC endpoint if provided, otherwise use network name
    const clusterArg = config.rpcEndpoint || config.network;
    const deployCmd = `anchor deploy --provider.cluster ${clusterArg} --provider.wallet "${tmpKeypairPath}"`;
    const output = execSync(deployCmd, { 
      cwd: projectRoot,
      encoding: 'utf-8',
      stdio: ['inherit', 'pipe', 'pipe']
    });
    
    // Clean up temp keypair
    if (existsSync(tmpKeypairPath)) {
      execSync(`rm "${tmpKeypairPath}"`);
    }
    
    // Extract signature
    const sigMatch = output.match(/Signature: ([A-Za-z0-9]+)/);
    const signature = sigMatch ? sigMatch[1] : 'unknown';
    
    logSuccess(`Program deployed!`);
    log(`Program ID: ${programId}`, 'cyan');
    log(`Signature: ${signature}`, 'cyan');
    
    return { programId, signature };
  } catch (error) {
    // Clean up temp keypair on error
    const tmpKeypairPath = join(__dirname, '.tmp-deployer.json');
    if (existsSync(tmpKeypairPath)) {
      execSync(`rm "${tmpKeypairPath}"`);
    }
    
    logError(`Deployment failed: ${error.message}`);
    if (error.stdout) console.log(error.stdout);
    if (error.stderr) console.log(error.stderr);
    return null;
  }
}

// Initialize vault
async function initializeVault(program, config, deployer) {
  logStep(5, 'Initializing vault');

  try {
    // `initialize` is gated on the ProgramConfig authority, which must exist
    // before any vault can be created. Idempotent: a no-op after the first
    // deployment against this program.
    const configAuthority = await ensureProgramConfig({
      program,
      signer: deployer,
      desiredAuthority: deployer.publicKey,
      log: (msg) => log(msg, 'cyan'),
    });

    if (!configAuthority.equals(deployer.publicKey)) {
      throw new Error(
        `Vault creation is restricted to ${configAuthority.toBase58()}, but this ` +
        `script signs as ${deployer.publicKey.toBase58()}. Either run it with the ` +
        `configured authority, or rotate it with set_config_authority.`
      );
    }

    const depositMint = new PublicKey(config.vaultConfig.depositMint);
    const admin = new PublicKey(config.vaultConfig.admin);
    const operator = new PublicKey(config.vaultConfig.operator);
    const feeRecipient = new PublicKey(config.vaultConfig.feeRecipient);
    const vaultVersion = config.vaultConfig.vaultVersion ?? 0;
    // Already validated in loadConfig(), before any SOL was spent.
    const shareOffset = config.vaultConfig.shareOffset;

    // Derive PDAs - Multi-vault architecture: all PDAs include deposit_mint + vault_version
    const [vaultState] = PublicKey.findProgramAddressSync(
      [Buffer.from('VAULT_STATE'), depositMint.toBuffer(), Buffer.from([vaultVersion])],
      program.programId
    );

    const [shareMint] = PublicKey.findProgramAddressSync(
      [Buffer.from('mint'), depositMint.toBuffer(), Buffer.from([vaultVersion])],
      program.programId
    );

    const [vaultTokenAta] = PublicKey.findProgramAddressSync(
      [Buffer.from('token_vault'), depositMint.toBuffer(), Buffer.from([vaultVersion])],
      program.programId
    );

    logInfo('Deriving vault accounts (multi-vault architecture)...');
    log(`  Deposit Mint: ${depositMint.toBase58()}`, 'cyan');
    log(`  Vault State: ${vaultState.toBase58()}`, 'cyan');
    log(`  Share Mint: ${shareMint.toBase58()}`, 'cyan');

    // Determine token program
    const connection = program.provider.connection;
    const mintInfo = await connection.getAccountInfo(depositMint);
    
    if (!mintInfo) {
      throw new Error(
        `Deposit mint account not found: ${depositMint.toBase58()}. ` +
        `Verify the mint address is correct and exists on ${config.network}.`
      );
    }
    
    const isToken2022 = mintInfo.owner.equals(TOKEN_2022_PROGRAM_ID);
    const tokenProgram = isToken2022 ? TOKEN_2022_PROGRAM_ID : TOKEN_PROGRAM_ID;
    
    logInfo(`Token Program: ${isToken2022 ? 'Token-2022' : 'SPL Token'}`);
    logInfo('Sending initialize transaction...');

    const tx = await program.methods
      .initialize(admin, operator, feeRecipient, vaultVersion, new BN(shareOffset))
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

    logSuccess('Vault initialized!');
    log(`Transaction: ${tx}`, 'cyan');
    
    return { vaultState, shareMint, vaultTokenAta, tx };
  } catch (error) {
    logError(`Vault initialization failed: ${error.message}`);
    if (error.logs) {
      console.log('\nProgram logs:');
      error.logs.forEach(log => console.log(log));
    }
    return null;
  }
}

// Create metadata
async function createMetadata(program, config, deployer, shareMint) {
  logStep(6, 'Creating share token metadata');
  
  try {
    const admin = new PublicKey(config.vaultConfig.admin);
    const depositMint = new PublicKey(config.vaultConfig.depositMint);
    
    // Multi-vault architecture: PDA includes deposit_mint + vault_version
    const vaultVersion = config.vaultConfig.vaultVersion ?? 0;
    const [vaultState] = PublicKey.findProgramAddressSync(
      [Buffer.from('VAULT_STATE'), depositMint.toBuffer(), Buffer.from([vaultVersion])],
      program.programId
    );

    const TOKEN_METADATA_PROGRAM_ID = new PublicKey('metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s');
    const [metadataAccount] = PublicKey.findProgramAddressSync(
      [
        Buffer.from('metadata'),
        TOKEN_METADATA_PROGRAM_ID.toBuffer(),
        shareMint.toBuffer(),
      ],
      TOKEN_METADATA_PROGRAM_ID
    );

    logInfo('Metadata details:');
    log(`  Name: ${config.vaultConfig.shareTokenName}`, 'cyan');
    log(`  Symbol: ${config.vaultConfig.shareTokenSymbol}`, 'cyan');
    log(`  URI: ${config.vaultConfig.shareTokenUri || '(empty)'}`, 'cyan');
    
    logInfo('Sending metadata transaction...');

    const tx = await program.methods
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

    logSuccess('Metadata created!');
    log(`Transaction: ${tx}`, 'cyan');
    
    return { tx };
  } catch (error) {
    logError(`Metadata creation failed: ${error.message}`);
    if (error.logs) {
      console.log('\nProgram logs:');
      error.logs.forEach(log => console.log(log));
    }
    return null;
  }
}

// Save deployment record
async function saveDeploymentRecord(config, results) {
  logStep(7, 'Saving deployment record');
  
  const deploymentDir = join(projectRoot, 'deployments');
  if (!existsSync(deploymentDir)) {
    mkdirSync(deploymentDir, { recursive: true });
  }

  const timestamp = new Date().toISOString();
  const filename = `${config.vaultConfig.shareTokenSymbol}-${config.network}-${Date.now()}.json`;
  
  // Get deposit mint info
  let depositMintInfo = null;
  try {
    const connection = new Connection(config.rpcEndpoint || 'https://api.mainnet-beta.solana.com');
    const mint = await getMint(connection, new PublicKey(config.vaultConfig.depositMint));
    depositMintInfo = {
      decimals: mint.decimals,
      supply: mint.supply.toString()
    };
  } catch (error) {
    logWarning('Could not fetch deposit mint info');
  }

  const record = {
    deployment: {
      timestamp,
      network: config.network,
    },
    program: {
      programId: results.programId,
      deployerWallet: results.deployerWallet,
      deployTx: results.deployTx,
    },
    vault: {
      vaultState: results.vaultState?.toString(),
      shareMint: results.shareMint?.toString(),
      depositMint: config.vaultConfig.depositMint,
      depositMintDecimals: depositMintInfo?.decimals,
      vaultVersion: config.vaultConfig.vaultVersion ?? 0,
      // Permanent and unchangeable: record it so the vault's pricing and its
      // minimum first deposit can be reconstructed from this file alone.
      shareOffset: config.vaultConfig.shareOffset,
      name: config.vaultConfig.shareTokenName,
      symbol: config.vaultConfig.shareTokenSymbol,
      // The share mint inherits the deposit mint's decimals; nothing is fixed at 8.
      decimals: depositMintInfo?.decimals,
      uri: config.vaultConfig.shareTokenUri || '',
      initTx: results.initTx,
      metadataTx: results.metadataTx,
    },
    roles: {
      admin: config.vaultConfig.admin,
      operator: config.vaultConfig.operator,
      feeRecipient: config.vaultConfig.feeRecipient,
    },
  };

  const filepath = join(deploymentDir, filename);
  writeFileSync(filepath, JSON.stringify(record, null, 2));
  logSuccess(`Deployment record saved: ${filename}`);
  
  return record;
}

// Print summary
function printSummary(record) {
  const explorerBase = record.deployment.network === 'mainnet' 
    ? 'https://explorer.solana.com' 
    : `https://explorer.solana.com?cluster=${record.deployment.network}`;
  
  console.log('\n' + '='.repeat(70));
  log(`${c.bold}${c.green}Vault Deployment Completed${c.reset}`, 'green');
  console.log('='.repeat(70));
  
  log('\nPROGRAM:', 'green');
  log(`   ID: ${record.program.programId}`, 'cyan');
  log(`   Explorer: ${explorerBase}/address/${record.program.programId}`, 'cyan');
  
  log('\nVAULT:', 'green');
  log(`   State: ${record.vault.vaultState}`, 'cyan');
  log(`   Explorer: ${explorerBase}/address/${record.vault.vaultState}`, 'cyan');
  
  log('\nSHARE TOKEN:', 'green');
  log(`   Name: ${record.vault.name}`, 'cyan');
  log(`   Symbol: ${record.vault.symbol}`, 'cyan');
  log(`   Decimals: ${record.vault.decimals}`, 'cyan');
  log(`   Mint: ${record.vault.shareMint}`, 'cyan');
  log(`   Explorer: ${explorerBase}/address/${record.vault.shareMint}`, 'cyan');
  
  log('\nDEPOSIT TOKEN:', 'green');
  log(`   Mint: ${record.vault.depositMint}`, 'cyan');
  log(`   Decimals: ${record.vault.depositMintDecimals || 'N/A'}`, 'cyan');
  
  log('\nROLES:', 'green');
  log(`   Admin: ${record.roles.admin}`, 'cyan');
  log(`   Operator: ${record.roles.operator}`, 'cyan');
  log(`   Fee Recipient: ${record.roles.feeRecipient}`, 'cyan');
  
  log('\nTRANSACTIONS:', 'green');
  log(`   Deploy: ${explorerBase}/tx/${record.program.deployTx}`, 'cyan');
  log(`   Initialize: ${explorerBase}/tx/${record.vault.initTx}`, 'cyan');
  log(`   Metadata: ${explorerBase}/tx/${record.vault.metadataTx}`, 'cyan');
  
  console.log('\n' + '='.repeat(70));
  logSuccess('All steps completed successfully!');
  console.log('='.repeat(70) + '\n');
}

// Get RPC endpoint
function getRpcEndpoint(config) {
  if (config.rpcEndpoint) return config.rpcEndpoint;
  const endpoints = {
    devnet: 'https://api.devnet.solana.com',
    mainnet: 'https://api.mainnet-beta.solana.com',
  };
  return endpoints[config.network] || endpoints.mainnet;
}

// Main execution
async function main() {
  console.log('\n' + '='.repeat(70));
  log(`${c.bold}${c.green}Create NewSolana Vault${c.reset}`, 'green');
  console.log('='.repeat(70) + '\n');

  // Load config
  const config = loadConfig();
  log(`Network: ${config.network}`, 'cyan');
  log(`Deposit Mint: ${config.vaultConfig.depositMint}`, 'cyan');
  log(`Share Token: ${config.vaultConfig.shareTokenName} (${config.vaultConfig.shareTokenSymbol})`, 'cyan');
  
  // Load deployer
  const deployer = Keypair.fromSecretKey(bs58.decode(config.deployerPrivateKey));
  logSuccess(`Deployer: ${deployer.publicKey.toBase58()}`);

  // Check balance
  const connection = new Connection(getRpcEndpoint(config), 'confirmed');
  const balance = await connection.getBalance(deployer.publicKey);
  log(`Balance: ${(balance / 1e9).toFixed(4)} SOL`, 'cyan');

  if (config.network === 'mainnet' && balance < 5e9) {
    logWarning('Balance is low. Recommended: 5-10 SOL for mainnet deployment');
    logWarning('Continuing anyway...');
  }

  // Step 1: Generate new program keypair
  const programId = generateProgramKeypair(config.vanityPrefix);

  // Step 2: Update program ID in source
  updateProgramId(programId);

  // Step 3: Build
  if (!buildProgram()) {
    logError('Build failed. Aborting deployment.');
    process.exit(1);
  }

  // Step 4: Deploy
  const deployResult = await deployProgram(config, deployer, programId);
  if (!deployResult) {
    logError('Deployment failed. Aborting.');
    process.exit(1);
  }

  // Load program for initialization
  const idlPath = join(projectRoot, 'target', 'idl', 'august_vault.json');
  const idl = JSON.parse(readFileSync(idlPath, 'utf-8'));
  idl.address = programId;
  
  const provider = new AnchorProvider(
    connection,
    new Wallet(deployer),
    { commitment: 'confirmed' }
  );
  
  const program = new Program(idl, provider);

  // Step 5: Initialize vault
  const initResult = await initializeVault(program, config, deployer);
  if (!initResult) {
    logError('Vault initialization failed. Aborting.');
    process.exit(1);
  }

  // Step 6: Create metadata
  const metadataResult = await createMetadata(program, config, deployer, initResult.shareMint);
  if (!metadataResult) {
    logWarning('Metadata creation failed, but vault is functional');
  }

  // Step 7: Save record
  const record = await saveDeploymentRecord(config, {
    programId: deployResult.programId,
    deployerWallet: deployer.publicKey.toBase58(),
    deployTx: deployResult.signature,
    vaultState: initResult.vaultState,
    shareMint: initResult.shareMint,
    initTx: initResult.tx,
    metadataTx: metadataResult?.tx,
  });

  // Print summary
  printSummary(record);
}

// Run with error handling
main().catch(error => {
  logError(`Fatal error: ${error.message}`);
  console.error(error);
  process.exit(1);
});
