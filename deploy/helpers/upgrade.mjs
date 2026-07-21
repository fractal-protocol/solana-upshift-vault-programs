// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

#!/usr/bin/env node
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
} finally {
  if (existsSync(tmpPath)) unlinkSync(tmpPath);
}
