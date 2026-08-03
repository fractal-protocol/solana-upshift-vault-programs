#!/usr/bin/env node
// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

/**
 * Bootstrap the ProgramConfig of an ALREADY-DEPLOYED program.
 *
 * `initialize` is gated on the ProgramConfig authority, and the config is created
 * once by the program's upgrade authority. `new-vault.mjs` cannot do this for an
 * existing program — it generates a fresh keypair and deploys a new one — so this
 * is the tool for the post-upgrade step.
 *
 * Usage:
 *   # Sign and send locally (devnet, where the team holds the upgrade authority):
 *   node deploy/bootstrap-config.mjs --program-id <ID> --keypair <path> \
 *     [--authority <PUBKEY>] [--url <RPC>]
 *
 *   # Emit an unsigned transaction for an externally held authority (Fordefi):
 *   node deploy/bootstrap-config.mjs --program-id <ID> --authority <PUBKEY> \
 *     --unsigned out.json [--url <RPC>]
 *
 * `--authority` is the key that will be allowed to create vaults; it defaults to
 * the upgrade authority. It is rotatable afterwards with `set_config_authority`,
 * and resettable by the upgrade authority with `override_config_authority`.
 */

import { Connection, Keypair, PublicKey, Transaction } from '@solana/web3.js';
import { AnchorProvider, Program, Wallet } from '@coral-xyz/anchor';
import { readFileSync, writeFileSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import {
  ensureProgramConfig,
  fetchUpgradeAuthority,
  programConfigPda,
  programDataPda,
} from './helpers/program-config.mjs';

const __dirname = dirname(fileURLToPath(import.meta.url));

function parseArgs() {
  const argv = process.argv.slice(2);
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const next = () => {
      const v = argv[i + 1];
      if (v === undefined || v.startsWith('--')) {
        throw new Error(`${argv[i]} requires a value`);
      }
      i++;
      return v;
    };
    switch (argv[i]) {
      case '--program-id': out.programId = next(); break;
      case '--authority': out.authority = next(); break;
      case '--keypair': out.keypair = next(); break;
      case '--unsigned': out.unsigned = next(); break;
      case '--url': out.url = next(); break;
      case '--help': case '-h': out.help = true; break;
      default: throw new Error(`unknown argument: ${argv[i]}`);
    }
  }
  return out;
}

function loadKeypair(path) {
  return Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(readFileSync(path, 'utf-8')))
  );
}

async function main() {
  const args = parseArgs();
  if (args.help || !args.programId) {
    console.log(readFileSync(new URL(import.meta.url)).toString()
      .split('\n').slice(9, 31).join('\n'));
    process.exit(args.help ? 0 : 1);
  }
  if (!args.keypair && !args.unsigned) {
    throw new Error('pass --keypair to sign locally, or --unsigned <path>');
  }

  const programId = new PublicKey(args.programId);
  const connection = new Connection(args.url || 'https://api.devnet.solana.com', 'confirmed');

  const idl = JSON.parse(
    readFileSync(join(__dirname, '..', 'target', 'idl', 'august_vault.json'), 'utf-8')
  );
  idl.address = programId.toBase58();

  const upgradeAuthority = await fetchUpgradeAuthority(connection, programId);
  if (upgradeAuthority === null) {
    throw new Error(
      `${programId.toBase58()} has no readable upgrade authority — cannot bootstrap.`
    );
  }
  console.log(`Program            : ${programId.toBase58()}`);
  console.log(`ProgramData        : ${programDataPda(programId).toBase58()}`);
  console.log(`Upgrade authority  : ${upgradeAuthority.toBase58()}`);
  console.log(`Config PDA         : ${programConfigPda(programId).toBase58()}`);

  const authority = args.authority ? new PublicKey(args.authority) : upgradeAuthority;
  console.log(`Vault-creation auth: ${authority.toBase58()}`);

  if (args.unsigned) {
    // Build the instruction only; the external signer supplies the signature.
    const provider = new AnchorProvider(
      connection,
      new Wallet(Keypair.generate()), // never signs; required by the constructor
      { commitment: 'confirmed' }
    );
    const program = new Program(idl, provider);
    const ix = await program.methods
      .initializeConfig(authority)
      .accounts({
        upgradeAuthority,
        payer: upgradeAuthority,
        programData: programDataPda(programId),
      })
      .instruction();

    const tx = new Transaction().add(ix);
    tx.feePayer = upgradeAuthority;
    tx.recentBlockhash = (await connection.getLatestBlockhash()).blockhash;
    const serialized = tx
      .serialize({ requireAllSignatures: false, verifySignatures: false })
      .toString('base64');
    writeFileSync(args.unsigned, JSON.stringify({ transaction: serialized }, null, 2));
    console.log(`\n✅ Unsigned transaction written to ${args.unsigned}`);
    console.log('   Have the upgrade authority sign and submit it, then re-run');
    console.log('   this script without --unsigned to verify the stored authority.');
    return;
  }

  const signer = loadKeypair(args.keypair);
  if (!signer.publicKey.equals(upgradeAuthority)) {
    throw new Error(
      `--keypair is ${signer.publicKey.toBase58()} but the upgrade authority is ` +
      `${upgradeAuthority.toBase58()}. Use --unsigned for an externally held key.`
    );
  }
  const provider = new AnchorProvider(connection, new Wallet(signer), {
    commitment: 'confirmed',
  });
  const program = new Program(idl, provider);
  const stored = await ensureProgramConfig({
    program,
    signer,
    desiredAuthority: authority,
  });
  console.log(`\n✅ Stored vault-creation authority: ${stored.toBase58()}`);
}

main().catch((e) => {
  console.error(`❌ ${e.message}`);
  process.exit(1);
});
