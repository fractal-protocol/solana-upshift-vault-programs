#!/usr/bin/env node
// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.
import { Keypair } from '@solana/web3.js';
import bs58 from 'bs58';
import { writeFileSync, readFileSync, existsSync, unlinkSync } from 'fs';
import { execSync } from 'child_process';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));

const configPath = join(__dirname, 'deploy.config.json');
const config = JSON.parse(readFileSync(configPath, 'utf-8'));
const deployer = Keypair.fromSecretKey(bs58.decode(config.deployerPrivateKey));

// Create temp keypair
const tmpPath = '/tmp/upgrade_keypair.json';
writeFileSync(tmpPath, JSON.stringify(Array.from(deployer.secretKey)));

try {
  console.log('Upgrading program...');
//   Edit below command to match your program id (target/deploy/august_vault.so)
  const cmd = `anchor upgrade target/deploy/august_vault.so --program-id 7B8n9vL51b6ibqRAd1adZsi3x3kxtq5NZaondh22Vkyq --provider.cluster devnet --provider.wallet ${tmpPath}`;
  execSync(cmd, { cwd: join(__dirname, '..'), stdio: 'inherit' });
  console.log('✅ Upgrade complete!');
  console.log('');
  console.log('⚠️  NEXT STEP — bootstrap the program config.');
  console.log('   Vault creation is gated on a ProgramConfig authority, and the');
  console.log('   config does not exist until initialize_config has been run by');
  console.log('   this program\'s upgrade authority. Until then `initialize` fails');
  console.log('   closed and no new vault can be created (existing vaults are');
  console.log('   unaffected).');
  console.log('');
  console.log('   Run: node deploy/bootstrap-config.mjs --program-id <ID> \\');
  console.log('          [--authority <PUBKEY>] [--unsigned <OUT.json>]');
  console.log('');
  console.log('   Do NOT use deploy/new-vault.mjs for this — it generates a fresh');
  console.log('   program keypair and deploys a NEW program, so it would bootstrap');
  console.log('   that program\'s config and leave this one still gated.');
  console.log('   Where the upgrade authority is held externally (Fordefi on');
  console.log('   mainnet), use --unsigned and have the authority sign it.');
} finally {
  if (existsSync(tmpPath)) unlinkSync(tmpPath);
}
