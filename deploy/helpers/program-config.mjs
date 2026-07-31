// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

/**
 * Program-config bootstrap, shared by the deployment scripts.
 *
 * `initialize` is gated on the `ProgramConfig` authority, so a freshly deployed
 * (or freshly upgraded) program cannot create vaults until the config exists.
 * Bootstrapping is authenticated against the loader's `ProgramData`, so only the
 * program's current upgrade authority can do it.
 */

import { PublicKey } from '@solana/web3.js';

const BPF_LOADER_UPGRADEABLE = new PublicKey(
  'BPFLoaderUpgradeab1e11111111111111111111111'
);

export function programConfigPda(programId) {
  return PublicKey.findProgramAddressSync(
    [Buffer.from('program_config')],
    programId
  )[0];
}

export function programDataPda(programId) {
  return PublicKey.findProgramAddressSync(
    [programId.toBuffer()],
    BPF_LOADER_UPGRADEABLE
  )[0];
}

/**
 * Read the program's on-chain upgrade authority state.
 *
 * Returns a discriminated result rather than a bare `null`, because the reasons
 * for "no authority" are operationally very different and must not be reported
 * with the same message. "Immutable" is a permanent, unrecoverable property of a
 * real program; "not-found" usually just means a typo in `--program-id` or the
 * wrong `--url`. Collapsing them tells an operator their program is permanently
 * broken when they actually mistyped an address.
 *
 * @returns {{ok: true, authority: PublicKey}
 *          | {ok: false, reason: 'not-found'|'wrong-owner'|'malformed'|'immutable'}}
 */
export async function fetchUpgradeAuthorityState(connection, programId) {
  const info = await connection.getAccountInfo(programDataPda(programId));
  if (!info) return { ok: false, reason: 'not-found' };
  if (!info.owner.equals(BPF_LOADER_UPGRADEABLE)) {
    return { ok: false, reason: 'wrong-owner' };
  }
  // bincode UpgradeableLoaderState::ProgramData — 4-byte variant (3), 8-byte
  // slot, then Option<Pubkey> as a 1-byte tag plus the key.
  if (info.data.length < 45 || info.data.readUInt32LE(0) !== 3) {
    return { ok: false, reason: 'malformed' };
  }
  // Only an explicit `Option::None` tag (0) means genuinely immutable. Any
  // other tag is not a valid bincode `Option`, so it is a parse failure, not a
  // revoked authority — treating it as immutable would tell an operator their
  // program can never create vaults again on the strength of a corrupt or
  // unrecognised byte.
  const tag = info.data[12];
  if (tag === 0) return { ok: false, reason: 'immutable' };
  if (tag !== 1) return { ok: false, reason: 'malformed' };
  return { ok: true, authority: new PublicKey(info.data.subarray(13, 45)) };
}

/** Human-readable explanation for a non-`ok` {@link fetchUpgradeAuthorityState}. */
export function explainUpgradeAuthority(reason, programId) {
  const id = programId.toBase58();
  switch (reason) {
    case 'not-found':
      return `no ProgramData account exists for ${id}. The program is not deployed ` +
        `on this cluster, or --program-id / --url is wrong.`;
    case 'wrong-owner':
      return `the ProgramData account for ${id} is not owned by the upgradeable ` +
        `loader, so ${id} is not an upgradeable program.`;
    case 'malformed':
      return `the ProgramData account for ${id} did not parse as ` +
        `UpgradeableLoaderState::ProgramData.`;
    case 'immutable':
      return `${id} is immutable — its upgrade authority has been revoked.`;
    default:
      return `unknown ProgramData state for ${id}.`;
  }
}

/**
 * Ensure the program config exists and return the authority permitted to create
 * vaults.
 *
 * Idempotent: if the config already exists its stored authority is returned
 * untouched. Otherwise it is created, naming `desiredAuthority` — which requires
 * `signer` to be the program's current upgrade authority.
 *
 * Throws rather than warning when the config is missing and `signer` cannot
 * create it: proceeding would fail at `initialize` with a much less obvious
 * error.
 */
export async function ensureProgramConfig({
  program,
  signer,
  desiredAuthority,
  log = console.log,
}) {
  const connection = program.provider.connection;
  const programId = program.programId;
  const configPda = programConfigPda(programId);

  // Existence alone is not enough: the PDA is deterministic and derivable from
  // the public repo, so anyone can park the rent-exempt minimum there. Such an
  // account is system-owned with no data, and `initialize_config` still succeeds
  // against it (Anchor's `init` transfers, allocates and assigns), so treat only
  // a program-owned account with data as a real config.
  const existing = await connection.getAccountInfo(configPda);
  const isRealConfig =
    existing !== null &&
    existing.data.length > 0 &&
    existing.owner.equals(programId);

  if (existing !== null && !isRealConfig) {
    log(
      `⚠️  ${configPda.toBase58()} holds ${existing.lamports} lamports but no config ` +
      `data (owner ${existing.owner.toBase58()}) — treating it as unclaimed and ` +
      `proceeding with the bootstrap.`
    );
  }

  if (isRealConfig) {
    const cfg = await program.account.programConfig.fetch(configPda);
    log(`ℹ️  Program config already exists at ${configPda.toBase58()}`);
    log(`   Vault-creation authority: ${cfg.authority.toBase58()}`);
    return cfg.authority;
  }

  const authority = desiredAuthority ?? signer.publicKey;
  const authState = await fetchUpgradeAuthorityState(connection, programId);

  if (!authState.ok) {
    // Report WHICH of the four states this is. `new-vault.mjs` and
    // `init-devnet-vault.mjs` both reach here, and a wrong --url or an
    // undeployed program must not be reported as permanent immutability.
    throw new Error(
      `${explainUpgradeAuthority(authState.reason, programId)} The program config ` +
      `cannot be bootstrapped, and it is mandatory before any vault can be ` +
      `initialized.`
    );
  }
  const onChainUpgradeAuthority = authState.authority;

  if (!onChainUpgradeAuthority.equals(signer.publicKey)) {
    throw new Error(
      `The program config does not exist yet and must be created by the ` +
      `program's upgrade authority.\n` +
      `  upgrade authority : ${onChainUpgradeAuthority.toBase58()}\n` +
      `  available signer  : ${signer.publicKey.toBase58()}\n` +
      `Run initialize_config with the upgrade authority first (on mainnet that ` +
      `is a Fordefi-signed transaction), naming the key that should be allowed ` +
      `to create vaults. Vault initialization cannot proceed until then.`
    );
  }

  log(`ℹ️  Bootstrapping program config at ${configPda.toBase58()}`);
  log(`   Vault-creation authority will be: ${authority.toBase58()}`);
  const tx = await program.methods
    .initializeConfig(authority)
    .accounts({
      upgradeAuthority: signer.publicKey,
      payer: signer.publicKey,
      // Passed explicitly rather than left to Anchor's PDA resolution: the IDL
      // bakes the build-time `declare_id!` into this account's seed, so
      // resolution would target the committed program's ProgramData even when
      // the caller has retargeted `idl.address` to another cluster's program.
      programData: programDataPda(programId),
    })
    .signers([signer])
    .rpc();
  log(`✅ Program config created. Transaction: ${tx}`);
  log(
    `⚠️  ${authority.toBase58()} is now the ONLY key that can create vaults on ` +
    `this program. If that is a hot deployer key, rotate it to your intended ` +
    `long-term key with set_config_authority before going live — the upgrade ` +
    `authority can also reset it later via override_config_authority.`
  );
  return authority;
}
