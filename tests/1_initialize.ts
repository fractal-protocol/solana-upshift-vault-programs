// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE.BSL file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

import * as anchor from "@coral-xyz/anchor";
import * as assert from "assert";
import {VaultContext} from "./helper/context";
import * as token from "@solana/spl-token";

describe("august-vault-initialize", () => {
    anchor.setProvider(anchor.AnchorProvider.env());
    let vaultContext: VaultContext

    before(async () => {
        vaultContext = new VaultContext
    });

    it("It should be possible to initialize the vault", async () => {
        await vaultContext.init()
        const vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        assert.deepEqual(vault.shareMint, vaultContext.shareMint);
        assert.deepEqual(vault.operator, vaultContext.operator.publicKey);
        assert.deepEqual(vault.admin, vaultContext.admin.publicKey);
        assert.deepEqual(vault.deployedAum.toNumber(), 0);
        // Get expected bump dynamically based on program ID with deposit_mint and version
        const vaultVersion = 0;
        const [, expectedBump] = anchor.web3.PublicKey.findProgramAddressSync(
            [Buffer.from("VAULT_STATE"), vaultContext.usdgTokenMint.toBuffer(), Buffer.from([vaultVersion])],
            vaultContext.vaultProgram.programId
        );
        assert.deepEqual(vault.pdaBump, [expectedBump]);
        assert.deepEqual(vault.vaultVersion, [vaultVersion]);
    });

    it("Cannot be reinitialized", async () => {
        const vaultVersion = 0;
        await assert.rejects(
            vaultContext.vaultProgram.methods
                .initialize(vaultContext.admin.publicKey, vaultContext.operator.publicKey, vaultContext.feeRecipient.publicKey, vaultVersion)
                .accounts({
                    vaultState: vaultContext.vaultStatePda,
                    shareMint: vaultContext.shareMint,
                    vaultTokenAta: vaultContext.vaultUsdgAta,
                    depositMint: vaultContext.usdgTokenMint,
                    signer: vaultContext.deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID
                })
                .signers([vaultContext.deployer])
                .rpc(),
            /0x0/) // already initialized
    });
});
