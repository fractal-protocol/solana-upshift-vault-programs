// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

#!/usr/bin/env node
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

// Parse programId from CLI args or use default
const args = process.argv.slice(2).reduce((acc, arg, i, arr) => {
  if (arg === '--program-id') acc.programId = arr[i + 1];
  return acc;
}, { programId: 'up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt' });
const programId = new PublicKey(args.programId);

async function main() {
  log('
🎨 Creating Vault Token Metadata', 'green');

  // Load config
  const configPath = join(__dirname, '..', 'deploy.config.json');
  const config = JSON.parse(readFileSync(configPath, 'utf-8'));
  
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

  // Derive PDAs (include depositMint in seeds for multi-vault support)
  const depositMint = new PublicKey(config.vaultConfig.depositMint);

  const [vaultState] = PublicKey.findProgramAddressSync(
    [Buffer.from('VAULT_STATE'), depositMint.toBuffer()],
    programId
  );

  const [shareMint] = PublicKey.findProgramAddressSync(
    [Buffer.from('mint'), depositMint.toBuffer()],
    programId
  );

  log(`Deposit Mint: ${depositMint.toBase58()}`, 'cyan');
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
      console.log('
Program logs:');
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
