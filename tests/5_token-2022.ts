// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

import * as anchor from "@coral-xyz/anchor";
import * as token from "@solana/spl-token";
import * as assert from "assert";
import { VaultContext } from "./helper/context";
import { PublicKey, Keypair } from "@solana/web3.js";
import BN from "bn.js";
import { AugustVault } from "../target/types/august_vault";
import { LAMPORTS_PER_SOL } from "@solana/web3.js";
import {expect} from "chai";

describe("august-vault-token2022", () => {
    anchor.setProvider(anchor.AnchorProvider.env());
    let vaultProgram: anchor.Program<AugustVault>;
    let token2022Mint: PublicKey;
    let token2022MintKeypair: Keypair;
    let vaultToken2022Ata: PublicKey;
    let senderToken2022Ata: PublicKey;
    let senderShareAta: PublicKey;
    let feeRecipientToken2022Ata: PublicKey;
    let vaultStatePda: PublicKey;
    let shareMint: PublicKey;
    let deployer: Keypair;
    let operator: Keypair;
    let admin: Keypair;
    let feeRecipient: Keypair;

    const amount = 1 * 10 ** 6;  // Must be >= min_deposit (1_000_000 for 9 decimals)

    before(async () => {
        // Initialize keypairs
        deployer = anchor.web3.Keypair.generate();
        operator = anchor.web3.Keypair.generate();
        admin = anchor.web3.Keypair.generate();
        feeRecipient = anchor.web3.Keypair.generate();
        token2022MintKeypair = anchor.web3.Keypair.generate();

        vaultProgram = anchor.workspace.augustVault as anchor.Program<AugustVault>;
        [vaultStatePda] = anchor.web3.PublicKey.findProgramAddressSync(
            [Buffer.from("VAULT_STATE")],
            vaultProgram.programId
        );

        [shareMint] = anchor.web3.PublicKey.findProgramAddressSync(
            [Buffer.from("mint")],
            vaultProgram.programId
        );

        const connection = anchor.getProvider().connection;
        const sig = await connection.requestAirdrop(deployer.publicKey, 4200 * LAMPORTS_PER_SOL);

        const latestBlockHash = await connection.getLatestBlockhash("confirmed");

        await connection.confirmTransaction({
            blockhash: latestBlockHash.blockhash,
            lastValidBlockHeight: latestBlockHash.lastValidBlockHeight,
            signature: sig,
        }, "confirmed");

        token2022Mint = await token.createMint(
            connection,
            deployer,
            deployer.publicKey,
            deployer.publicKey,
            9,
            token2022MintKeypair,
            {
                commitment: "confirmed",
                preflightCommitment: undefined,
            },
            token.TOKEN_2022_PROGRAM_ID
        );

        [vaultToken2022Ata] = anchor.web3.PublicKey.findProgramAddressSync(
            [
                Buffer.from("token_vault"),
                token2022Mint.toBuffer(),
            ],
            vaultProgram.programId
        );

        senderToken2022Ata = (await token.getOrCreateAssociatedTokenAccount(
            connection,
            deployer,
            token2022Mint,
            deployer.publicKey,
            false,
            undefined,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).address;

        feeRecipientToken2022Ata = (await token.getOrCreateAssociatedTokenAccount(
            connection,
            deployer,
            token2022Mint,
            feeRecipient.publicKey,
            false,
            undefined,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).address;

        await token.mintTo(
            connection,
            deployer,
            token2022Mint,
            senderToken2022Ata,
            deployer.publicKey,
            10 * amount,
            [],
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        );
    });


    it("Should initialize vault with Token2022 mint", async () => {
        await vaultProgram.methods
            .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey)
            .accounts({
                depositMint: token2022Mint,
                signer: deployer.publicKey,
                tokenProgram: token.TOKEN_2022_PROGRAM_ID
            })
            .signers([deployer])
            .rpc();

        // Verify vault state
        const vault = await vaultProgram.account.vaultState.fetch(vaultStatePda) as any;
        assert.deepEqual(vault.shareMint, shareMint);
        assert.deepEqual(vault.depositMint, token2022Mint);
        assert.deepEqual(vault.operator, operator.publicKey);
        assert.deepEqual(vault.admin, admin.publicKey);
        assert.deepEqual(vault.deployedAum.toNumber(), 0);


        //ShareMint created at Init
        senderShareAta = (await token.getOrCreateAssociatedTokenAccount(
            anchor.getProvider().connection,
            deployer,
            shareMint,
            deployer.publicKey,
            false,
            undefined,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).address;
    });

    it("Should deposit Token2022 tokens", async () => {
        const senderBalanceBefore = (await token.getAccount(
            anchor.getProvider().connection,
            senderToken2022Ata,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).amount;

        const vaultBalanceBefore = (await token.getAccount(
            anchor.getProvider().connection,
            vaultToken2022Ata,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).amount;

        await vaultProgram.methods
            .deposit(new BN(amount))
            .accounts({
                senderTokenAccount: senderToken2022Ata,
                senderShareAccount: senderShareAta,
                depositMint: token2022Mint,
                signer: deployer.publicKey,
                tokenProgram: token.TOKEN_2022_PROGRAM_ID,
            })
            .signers([deployer])
            .rpc();

        const senderBalanceAfter = (await token.getAccount(
            anchor.getProvider().connection,
            senderToken2022Ata,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).amount;

        const vaultBalanceAfter = (await token.getAccount(
            anchor.getProvider().connection,
            vaultToken2022Ata,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).amount;

        assert.equal(Number(senderBalanceBefore), Number(senderBalanceAfter) + amount);
        assert.equal(Number(vaultBalanceBefore) + amount, Number(vaultBalanceAfter));
    });

    it("Should mint shares on Token2022 deposits", async () => {
        const senderSharesBefore = (await token.getAccount(
            anchor.getProvider().connection,
            senderShareAta,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).amount;

        await vaultProgram.methods
            .deposit(new BN(amount))
            .accounts({
                senderTokenAccount: senderToken2022Ata,
                senderShareAccount: senderShareAta,
                depositMint: token2022Mint,
                signer: deployer.publicKey,
                tokenProgram: token.TOKEN_2022_PROGRAM_ID
            })
            .signers([deployer])
            .rpc();

        const senderSharesAfter = (await token.getAccount(
            anchor.getProvider().connection,
            senderShareAta,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).amount;

        assert.equal(new BN(senderSharesAfter).toNumber(), new BN(senderSharesBefore).mul(new BN(2)).toNumber())
    });

    it("Should redeem Token2022 tokens", async () => {
        const senderBalanceBefore = (await token.getAccount(
            anchor.getProvider().connection,
            senderToken2022Ata,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).amount;

        const senderSharesBefore = (await token.getAccount(
            anchor.getProvider().connection,
            senderShareAta,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).amount;

        const vaultBalanceBefore = (await token.getAccount(
            anchor.getProvider().connection,
            vaultToken2022Ata,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).amount;

        await vaultProgram.methods
            .redeem(new BN(senderSharesBefore).div(new BN(2)))
            .accounts({
                senderTokenAccount: senderToken2022Ata,
                senderShareAccount: senderShareAta,
                feeRecipientAccount: feeRecipientToken2022Ata,
                depositMint: token2022Mint,
                signer: deployer.publicKey,
                tokenProgram: token.TOKEN_2022_PROGRAM_ID,
            })
            .signers([deployer])
            .rpc();

        const senderBalanceAfter = (await token.getAccount(
            anchor.getProvider().connection,
            senderToken2022Ata,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).amount;

        const vaultBalanceAfter = (await token.getAccount(
            anchor.getProvider().connection,
            vaultToken2022Ata,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).amount;

        expect(Number(senderBalanceAfter)).to.be.greaterThan(Number(senderBalanceBefore));
        expect(Number(vaultBalanceBefore)).to.be.greaterThan(Number(vaultBalanceAfter));
    });

    it("Should burn shares on Token2022 redeem", async () => {
        const senderSharesBefore = (await token.getAccount(
            anchor.getProvider().connection,
            senderShareAta,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).amount;

        await vaultProgram.methods
            .redeem(new BN(senderSharesBefore))
            .accounts({
                senderTokenAccount: senderToken2022Ata,
                senderShareAccount: senderShareAta,
                feeRecipientAccount: feeRecipientToken2022Ata,
                depositMint: token2022Mint,
                signer: deployer.publicKey,
                tokenProgram: token.TOKEN_2022_PROGRAM_ID,
            })
            .signers([deployer])
            .rpc();

        const senderSharesAfter = (await token.getAccount(
            anchor.getProvider().connection,
            senderShareAta,
            undefined,
            token.TOKEN_2022_PROGRAM_ID
        )).amount;

        expect(Number(senderSharesBefore)).to.be.greaterThan(Number(senderSharesAfter));
    });
});
