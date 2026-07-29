#!/usr/bin/env node
// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.
import { Keypair, PublicKey } from '@solana/web3.js';
import bs58 from 'bs58';
import { writeFileSync, readFileSync, existsSync, unlinkSync } from 'fs';
import { execFileSync } from 'child_process';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));

// Take the cluster and program from argv instead of hardcoding them.
//
// This previously ignored argv entirely and hardcoded `--provider.cluster devnet`
// with a program ID that is not the live mainnet program, so `upgrade:mainnet`
// performed a DEVNET upgrade of an unrelated program and then printed a success
// banner. Accepting a flag and ignoring it is worse than not accepting it.
const argv = process.argv.slice(2);
const flag = (name) => {
  const i = argv.indexOf(name);
  if (i === -1) return undefined;
  const v = argv[i + 1];
  if (v === undefined || v.startsWith('--')) {
    throw new Error(`${name} requires a value`);
  }
  return v;
};

const network = flag('--network');
const programId = flag('--program-id');

if (network === 'mainnet' || network === 'mainnet-beta') {
  console.error(
    '❌ Refusing to upgrade mainnet from this script.\n\n' +
    '   The mainnet upgrade authority is a Fordefi MPC key, so the upgrade\n' +
    '   cannot be signed by a local keypair the way `anchor upgrade` requires.\n' +
    '   Mainnet upgrades go through the buffer + Fordefi ceremony documented in\n' +
    '   docs/UPGRADE.md. Follow that runbook instead.'
  );
  process.exit(1);
}
if (network !== 'devnet') {
  console.error(
    '❌ Pass --network devnet explicitly.\n\n' +
    '   Usage: node deploy/helpers/upgrade.mjs --network devnet \\\n' +
    '            --program-id <PROGRAM_ID>\n\n' +
    '   This script drives `anchor upgrade` with a locally held keypair, which\n' +
    '   only applies to devnet. For mainnet see docs/UPGRADE.md.'
  );
  process.exit(1);
}
if (!programId) {
  console.error(
    '❌ Pass --program-id <PROGRAM_ID>. It is not inferred, so an upgrade can\n' +
    '   never target a program you did not name.\n\n' +
    '   Devnet: C8B1EpsSGVWK2vMrk3aDT3kL7RCE77otokUh4EC35kK7'
  );
  process.exit(1);
}
// Parse before use: this value reaches a subprocess argument list, and parsing
// rejects anything that is not a real base58 pubkey.
let programKey;
try {
  programKey = new PublicKey(programId);
} catch {
  console.error(`❌ --program-id ${programId} is not a valid public key.`);
  process.exit(1);
}

// `deploy/deploy.config.json`, not `deploy/helpers/` — this was one level short,
// so every run died with an unhandled ENOENT stack trace before reaching the
// upgrade. `metadata.mjs` resolves it the same way.
const configPath = join(__dirname, '..', 'deploy.config.json');
if (!existsSync(configPath)) {
  console.error(
    `❌ No deploy config at ${configPath}.\n\n` +
    '   Create it from the template:\n' +
    '     cp deploy/deploy.config.example.json deploy/deploy.config.json\n' +
    '   then fill in deployerPrivateKey and rpcEndpoint.'
  );
  process.exit(1);
}
const config = JSON.parse(readFileSync(configPath, 'utf-8'));
if (!config.deployerPrivateKey) {
  console.error(`❌ ${configPath} has no "deployerPrivateKey".`);
  process.exit(1);
}
const deployer = Keypair.fromSecretKey(bs58.decode(config.deployerPrivateKey));

// Create temp keypair
const tmpPath = '/tmp/upgrade_keypair.json';
writeFileSync(tmpPath, JSON.stringify(Array.from(deployer.secretKey)));

try {
  console.log(`Upgrading ${programKey.toBase58()} on ${network}...`);
  // execFileSync with an argument array — no shell, so nothing here can be
  // interpreted as a shell metacharacter even if an argument were attacker-shaped.
  execFileSync(
    'anchor',
    [
      'upgrade', 'target/deploy/august_vault.so',
      '--program-id', programKey.toBase58(),
      '--provider.cluster', network,
      '--provider.wallet', tmpPath,
    ],
    { cwd: join(__dirname, '..'), stdio: 'inherit' }
  );
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
