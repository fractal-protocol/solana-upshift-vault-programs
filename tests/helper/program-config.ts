// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

import * as anchor from "@coral-xyz/anchor";
import {AugustVault} from "../../target/types/august_vault";
import {Keypair, LAMPORTS_PER_SOL, PublicKey} from "@solana/web3.js";
import {sha256} from "js-sha256"

/// The single key permitted to call `initialize` (see `ProgramConfig`).
///
/// Vault creation is gated program-wide, so every suite has to create its vaults
/// with the same key rather than its own `deployer`. Suites keep their own
/// deployer for airdrops, mints and ATAs; only `initialize` needs this one.
export const protocolAuthority = Keypair.fromSeed(
    Uint8Array.from(sha256.digest("protocolAuthority"))
);

const BPF_LOADER_UPGRADEABLE = new PublicKey(
    "BPFLoaderUpgradeab1e11111111111111111111111"
);

export function programConfigPda(programId: PublicKey): PublicKey {
    return PublicKey.findProgramAddressSync(
        [Buffer.from("program_config")],
        programId
    )[0];
}

export function programDataPda(programId: PublicKey): PublicKey {
    return PublicKey.findProgramAddressSync(
        [programId.toBuffer()],
        BPF_LOADER_UPGRADEABLE
    )[0];
}

/// Bootstrap the program config, naming `protocolAuthority` as the vault
/// creator, and fund that key.
///
/// Idempotent: suites share one validator, so whichever runs first creates the
/// config and the rest reuse it. Bootstrapping is authenticated against the
/// loader's `ProgramData`, so it must be signed by the program's upgrade
/// authority — on localnet that is the provider wallet that deployed it.
export async function ensureProgramConfig(
    program: anchor.Program<AugustVault>
): Promise<void> {
    const provider = program.provider as anchor.AnchorProvider;
    const connection = provider.connection;

    // Refuse to run anywhere but a local validator. `protocolAuthority`'s seed is
    // committed in this file, so its private key is public; installing it as the
    // config authority of a shared cluster would hand vault creation to anyone.
    const endpoint = connection.rpcEndpoint;
    const isLocal = /^https?:\/\/(localhost|127\.0\.0\.1|0\.0\.0\.0)(:|$)/.test(endpoint);
    if (!isLocal) {
        throw new Error(
            `ensureProgramConfig refuses to run against ${endpoint}. These suites ` +
            `bootstrap the program config with a keypair whose seed is committed ` +
            `in tests/helper/program-config.ts, so it must only ever be used on a ` +
            `local validator. Point ANCHOR_PROVIDER_URL at localhost.`
        );
    }

    const balance = await connection.getBalance(protocolAuthority.publicKey);
    if (balance < 100 * LAMPORTS_PER_SOL) {
        const sig = await connection.requestAirdrop(
            protocolAuthority.publicKey,
            200 * LAMPORTS_PER_SOL
        );
        const latest = await connection.getLatestBlockhash();
        await connection.confirmTransaction({signature: sig, ...latest});
    }

    const configPda = programConfigPda(program.programId);
    const existing = await connection.getAccountInfo(configPda);
    // Only a program-owned account with data is a real config; a lamport-only
    // account parked at this deterministic PDA is not, and bootstrapping over it
    // still works.
    const isRealConfig =
        existing !== null &&
        existing.data.length > 0 &&
        existing.owner.equals(program.programId);

    if (isRealConfig) {
        // Already bootstrapped by an earlier suite. Assert the invariant the
        // rest of the tests depend on rather than silently proceeding.
        const cfg = await program.account.programConfig.fetch(configPda);
        if (!cfg.authority.equals(protocolAuthority.publicKey)) {
            throw new Error(
                `program config authority is ${cfg.authority.toBase58()}, ` +
                `expected the shared protocolAuthority ` +
                `${protocolAuthority.publicKey.toBase58()} — a suite rotated it`
            );
        }
        return;
    }

    // `program_data` is passed explicitly: the IDL bakes the build-time program
    // ID into its seed, so Anchor's resolution would target the committed
    // program rather than whichever one these tests are running against.
    await program.methods
        .initializeConfig(protocolAuthority.publicKey)
        .accounts({
            upgradeAuthority: provider.wallet.publicKey,
            payer: provider.wallet.publicKey,
            programData: programDataPda(program.programId),
        })
        .rpc();
}
