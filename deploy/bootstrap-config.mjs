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
 *     --unsigned out.json [--payer <ops-keypair.json>] \
 *     [--nonce-account <PUBKEY>] [--url <RPC>]
 *
 *   # --payer lets a funded OPS key cover the ProgramConfig rent (~0.0021 SOL)
 *   # and the fee, so the upgrade authority does not need to hold SOL. It
 *   # partially signs locally; the upgrade authority adds the second signature.
 *
 *   # --nonce-account uses a DURABLE NONCE so the exported transaction does not
 *   # expire while a Fordefi ceremony is in progress. Strongly recommended for
 *   # mainnet; the nonce authority must be the upgrade authority.
 *
 *   # Read back and ASSERT the stored authority (no signer needed). Exits
 *   # non-zero on a mismatch; --authority is required so it can actually fail.
 *   node deploy/bootstrap-config.mjs --program-id <ID> --authority <PUBKEY> \
 *     [--url <RPC>]
 *
 * Re-running the sign-and-send form once the config exists is a no-op that
 * exits 0: with --authority omitted the signer's own pubkey is the value
 * asserted against, so the check still fails loudly if something else is
 * stored. Only a bare read with neither --authority nor --keypair has nothing
 * to compare against, and that is the one case that errors.
 *
 * NOTE: --url defaults to DEVNET. Always pass it explicitly for mainnet.
 *
 * `--authority` is the key that will be allowed to create vaults; when
 * bootstrapping it defaults to the upgrade authority. It is rotatable afterwards
 * with `set_config_authority` (signed by the current config authority), and
 * resettable by the upgrade authority with `override_config_authority`.
 */

import {
  Connection,
  Keypair,
  NONCE_ACCOUNT_LENGTH,
  NonceAccount,
  PublicKey,
  SystemProgram,
  Transaction,
} from '@solana/web3.js';
import { AnchorProvider, Program, Wallet } from '@coral-xyz/anchor';
import { existsSync, readFileSync, writeFileSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import {
  ensureProgramConfig,
  explainUpgradeAuthority,
  fetchUpgradeAuthorityState,
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
      case '--nonce-account': out.nonceAccount = next(); break;
      case '--payer': out.payer = next(); break;
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
    // Extract the header block by delimiter rather than by line index: a
    // hardcoded slice silently truncates or garbles the help text the moment
    // anyone edits the comment above.
    const src = readFileSync(new URL(import.meta.url)).toString();
    const doc = src.slice(src.indexOf('/**') + 3, src.indexOf('*/'));
    console.log(doc.replace(/^\s*\* ?/gm, '').trim());
    process.exit(args.help ? 0 : 1);
  }
  // No keypair and no --unsigned is still valid: it means "just tell me what is
  // stored". That only fails below if the config does not exist yet, where there
  // is genuinely nothing to report and nothing to sign with.

  const programId = new PublicKey(args.programId);
  const connection = new Connection(args.url || 'https://api.devnet.solana.com', 'confirmed');

  // The Anchor IDL is required by every branch below. `solana-verify build`
  // (the only build step in the upgrade runbook) produces target/deploy/*.so and
  // NO IDL, so a clean checkout following that runbook would otherwise die here
  // with a bare ENOENT on the first and last command of the ceremony.
  const idlPath = join(__dirname, '..', 'target', 'idl', 'august_vault.json');
  if (!existsSync(idlPath)) {
    throw new Error(
      `no Anchor IDL at ${idlPath}.\n` +
      `   Run \`anchor build\` first — \`solana-verify build\` alone does not ` +
      `emit one. The IDL is only used to encode the instruction locally; it does ` +
      `not have to come from the verifiable build.`
    );
  }
  const idl = JSON.parse(readFileSync(idlPath, 'utf-8'));
  idl.address = programId.toBase58();

  const configPda = programConfigPda(programId);
  const authState = await fetchUpgradeAuthorityState(connection, programId);
  const upgradeAuthority = authState.ok ? authState.authority : null;

  console.log(`Program            : ${programId.toBase58()}`);
  console.log(`ProgramData        : ${programDataPda(programId).toBase58()}`);
  console.log(
    `Upgrade authority  : ${
      upgradeAuthority ? upgradeAuthority.toBase58() : `none (${authState.reason})`
    }`
  );
  console.log(`Config PDA         : ${configPda.toBase58()}`);

  // READ-ONLY PATH FIRST, before requiring either a signer or an upgrade
  // authority. Two reasons: the documented mainnet follow-up re-runs this to read
  // back what an externally held key stored, and a program that has since been
  // made immutable has no upgrade authority at all — but its config is still
  // perfectly valid and still worth querying.
  const existing = await connection.getAccountInfo(configPda);
  if (existing !== null && existing.data.length > 0 && existing.owner.equals(programId)) {
    const readOnly = new AnchorProvider(
      connection,
      new Wallet(Keypair.generate()), // never signs
      { commitment: 'confirmed' }
    );
    const cfg = await new Program(idl, readOnly).account.programConfig.fetch(configPda);
    console.log(`\nStored vault-creation authority: ${cfg.authority.toBase58()}`);

    // Verification must be able to FAIL, or it is not verification. But the
    // expected value does not have to come from `--authority`: the documented
    // sign-and-send invocation is `--program-id <ID> --keypair <auth.json>`
    // with `--authority` omitted, which means "install the signer itself", so
    // on a re-run the signer's own pubkey IS the expected value. Deriving it
    // keeps the assertion sharp while making a second run of the documented
    // command idempotent — matching `ensureProgramConfig`, which callers such
    // as `new-vault.mjs` already invoke for the same program.
    //
    // Only a bare read with nothing supplied has genuinely nothing to compare
    // against, and that still fails rather than printing a pubkey and exiting 0.
    const expected = args.authority
      ? new PublicKey(args.authority)
      : args.keypair
        ? loadKeypair(args.keypair).publicKey
        : null;

    if (expected === null) {
      throw new Error(
        `the config already exists, so pass --authority <PUBKEY> to assert which ` +
        `key you expect to be stored. Re-run with ` +
        `--authority ${cfg.authority.toBase58()} to confirm the value above is ` +
        `the intended one.`
      );
    }
    if (!cfg.authority.equals(expected)) {
      throw new Error(
        `MISMATCH: the stored authority is ${cfg.authority.toBase58()}, not the ` +
        `${expected.toBase58()} you ${args.authority ? 'passed' : 'signed with'}. ` +
        `Vault creation is gated behind the stored key. Rotate with ` +
        `set_config_authority (needs a signature from the stored key), or reset ` +
        `with override_config_authority (needs the upgrade authority).`
      );
    }
    console.log(
      `✅ Matches the expected authority. Nothing to do — the config is ` +
      `create-once, so this is a no-op.`
    );
    return;
  }

  // Everything below bootstraps, which is the only part that needs an upgrade
  // authority to exist.
  if (upgradeAuthority === null) {
    // Only a genuinely immutable program is unrecoverable; the other three
    // reasons are almost always a wrong --program-id or --url. Do not tell an
    // operator their program is permanently broken because of a typo.
    const detail = explainUpgradeAuthority(authState.reason, programId);
    throw new Error(
      authState.reason === 'immutable'
        ? `${detail} Its config can never be bootstrapped, and vault creation ` +
          `requires the config, so this program can no longer create vaults.`
        : `${detail} Cannot bootstrap the config.`
    );
  }

  const authority = args.authority ? new PublicKey(args.authority) : upgradeAuthority;
  console.log(`Vault-creation auth: ${authority.toBase58()}`);

  if (args.unsigned) {
    // WHO PAYS. `initialize_config` takes `payer` as a Signer separate from
    // `upgrade_authority`, so the account rent and the transaction fee do NOT
    // have to come from the Fordefi MPC key — and the runbook's Prerequisites
    // provision a funded ops fee-payer that is explicitly NOT the upgrade
    // authority. Default to that ops payer when one is supplied: it partially
    // signs here and the external authority adds the second signature.
    //
    // Falling back to the upgrade authority is still supported (one signature,
    // simpler ceremony), but then it must hold SOL, which is checked below
    // rather than assumed.
    const opsPayer = args.payer ? loadKeypair(args.payer) : null;
    const payerPubkey = opsPayer ? opsPayer.publicKey : upgradeAuthority;

    const rent = await connection.getMinimumBalanceForRentExemption(169);
    const needed = rent + 10_000; // rent + generous fee headroom
    const payerBalance = await connection.getBalance(payerPubkey);
    console.log(
      `Fee/rent payer    : ${payerPubkey.toBase58()}` +
      `${opsPayer ? ' (ops payer)' : ' (upgrade authority)'}`
    );
    console.log(
      `                    balance ${payerBalance / 1e9} SOL, needs ~${needed / 1e9} SOL`
    );
    if (payerBalance < needed) {
      throw new Error(
        `${payerPubkey.toBase58()} holds ${payerBalance / 1e9} SOL but needs about ` +
        `${needed / 1e9} SOL (ProgramConfig rent ${rent / 1e9} + fee). ` +
        (opsPayer
          ? 'Fund the ops payer.'
          : 'Fund it, or pass --payer <ops-keypair.json> so a funded ops key ' +
            'covers the cost instead of the upgrade authority.') +
        ' Otherwise the ceremony completes and the submission then fails.'
      );
    }

    // Build the instruction only; the external signer supplies its signature.
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
        payer: payerPubkey,
        programData: programDataPda(programId),
      })
      .instruction();

    const tx = new Transaction();

    // A normal recentBlockhash expires after ~150 blocks (roughly 60-90s), which
    // a Fordefi review-and-approve ceremony will almost always outlast — the
    // signed transaction would then be rejected as "Blockhash not found" after
    // the approvals were already collected. A DURABLE NONCE has no expiry: the
    // nonce value stays valid until the nonce account is advanced, which happens
    // only when this very transaction lands.
    let nonceInfo = null;
    if (args.nonceAccount) {
      const noncePubkey = new PublicKey(args.nonceAccount);
      const info = await connection.getAccountInfo(noncePubkey);
      if (!info) {
        throw new Error(`nonce account ${noncePubkey.toBase58()} does not exist`);
      }
      if (!info.owner.equals(SystemProgram.programId) ||
          info.data.length !== NONCE_ACCOUNT_LENGTH) {
        throw new Error(
          `${noncePubkey.toBase58()} is not a durable nonce account ` +
          `(owner ${info.owner.toBase58()}, ${info.data.length} bytes)`
        );
      }
      nonceInfo = NonceAccount.fromAccountData(info.data);
      if (!nonceInfo.authorizedPubkey.equals(upgradeAuthority)) {
        throw new Error(
          `the nonce authority is ${nonceInfo.authorizedPubkey.toBase58()} but the ` +
          `upgrade authority is ${upgradeAuthority.toBase58()}. AdvanceNonceAccount ` +
          `must be signed by the nonce authority, so they must match here.`
        );
      }
      // The advance instruction MUST be first; the nonce doubles as the blockhash.
      tx.add(SystemProgram.nonceAdvance({
        noncePubkey,
        authorizedPubkey: upgradeAuthority,
      }));
      tx.recentBlockhash = nonceInfo.nonce;
    } else {
      tx.recentBlockhash = (await connection.getLatestBlockhash()).blockhash;
    }

    tx.add(ix);
    tx.feePayer = payerPubkey;
    // Attach the ops payer's signature now; the export then needs only the
    // upgrade authority's. Order matters: blockhash and feePayer must be set
    // before signing.
    if (opsPayer) tx.partialSign(opsPayer);
    const serialized = tx
      .serialize({ requireAllSignatures: false, verifySignatures: false })
      .toString('base64');
    writeFileSync(args.unsigned, JSON.stringify({
      transaction: serialized,
      durableNonce: args.nonceAccount ?? null,
      feePayer: payerPubkey.toBase58(),
      signedBy: opsPayer ? [opsPayer.publicKey.toBase58()] : [],
      awaitingSignatureFrom: upgradeAuthority.toBase58(),
    }, null, 2));
    console.log(`\n✅ Unsigned transaction written to ${args.unsigned}`);
    if (nonceInfo) {
      console.log(`   Durable nonce: ${args.nonceAccount} (no expiry).`);
    } else {
      console.log('');
      console.log('⚠️  This transaction carries an ordinary recent blockhash, which');
      console.log('   expires in roughly 60-90 seconds. A Fordefi approval ceremony');
      console.log('   will very likely outlast it and the submission will fail with');
      console.log('   "Blockhash not found" AFTER the approvals were collected.');
      console.log('   Either regenerate this file immediately before signing, or');
      console.log('   re-run with --nonce-account <PUBKEY> to use a durable nonce');
      console.log('   whose authority is the upgrade authority.');
      console.log('');
    }
    console.log('   Have the upgrade authority sign and submit it, then re-run');
    console.log('   this script with --authority <PUBKEY> to verify what was stored.');
    return;
  }

  if (!args.keypair) {
    throw new Error(
      'the program config does not exist yet, so there is nothing to verify. ' +
      'Pass --keypair to sign locally, or --unsigned <path> to emit a ' +
      'transaction for an externally held upgrade authority.'
    );
  }
  if (args.nonceAccount) {
    throw new Error(
      '--nonce-account applies only to --unsigned. A durable nonce exists so an ' +
      'exported transaction does not expire during an external signing ceremony; ' +
      'this path signs and sends immediately.'
    );
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
  // Honour --payer here too, rather than accepting it and silently charging the
  // upgrade authority — the accept-and-ignore defect this PR removed elsewhere.
  const localPayer = args.payer ? loadKeypair(args.payer) : null;
  const payerPubkey = (localPayer ?? signer).publicKey;
  const rent = await connection.getMinimumBalanceForRentExemption(169);
  const needed = rent + 10_000;
  const payerBalance = await connection.getBalance(payerPubkey);
  if (payerBalance < needed) {
    throw new Error(
      `${payerPubkey.toBase58()} holds ${payerBalance / 1e9} SOL but needs about ` +
      `${needed / 1e9} SOL (ProgramConfig rent ${rent / 1e9} + fee).`
    );
  }
  const stored = await ensureProgramConfig({
    program,
    signer,
    desiredAuthority: authority,
    payer: localPayer,
  });
  console.log(`\n✅ Stored vault-creation authority: ${stored.toBase58()}`);
}

main().catch((e) => {
  console.error(`❌ ${e.message}`);
  process.exit(1);
});
