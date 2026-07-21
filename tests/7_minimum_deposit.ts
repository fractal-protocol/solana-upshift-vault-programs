// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

import * as anchor from "@coral-xyz/anchor";
import * as token from "@solana/spl-token"
import * as assert from "assert";
import {VaultContext} from "./helper/context";
import BN from "bn.js";
import {expect} from "chai";

describe("august-vault-minimum-deposit", () => {
    anchor.setProvider(anchor.AnchorProvider.env());
    let vaultContext: VaultContext

    before(async () => {
        vaultContext = new VaultContext
        await vaultContext.init()
    });

    after(async () => {
        // Clean up any remaining shares
        try {
            const senderSharesBefore = (await token.getAccount(
                vaultContext.connection,
                vaultContext.senderShareAta
            )).amount

            if(senderSharesBefore > 0) {
                await vaultContext.vaultProgram.methods
                    .redeem(new BN(senderSharesBefore))
                    .accounts({
                        senderTokenAccount: vaultContext.senderUsdgAta,
                        senderShareAccount: vaultContext.senderShareAta,
                        feeRecipientAccount: vaultContext.feeRecipientUsdgAta,
                        depositMint: vaultContext.usdgTokenMint,
                        signer: vaultContext.deployer.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID
                    })
                    .signers([vaultContext.deployer])
                    .rpc();
            }
        } catch (e) {
            // Ignore cleanup errors
        }
    })

    it("Should reject first deposit below minimum (8 decimals)", async () => {
        // Get the deposit mint info to check decimals
        const mintInfo = await token.getMint(vaultContext.connection, vaultContext.usdgTokenMint);
        console.log(`Testing with ${mintInfo.decimals} decimals`);

        // For 8 decimals, minimum should be 100,000 units (0.001 tokens)
        const belowMinimum = 50_000; // 0.0005 tokens

        try {
            await vaultContext.vaultProgram.methods
                .deposit(new BN(belowMinimum))
                .accounts({
                    senderTokenAccount: vaultContext.senderUsdgAta,
                    senderShareAccount: vaultContext.senderShareAta,
                    depositMint: vaultContext.usdgTokenMint,
                    signer: vaultContext.deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([vaultContext.deployer])
                .rpc();

            assert.fail("Should have rejected deposit below minimum");
        } catch (error) {
            expect(error.message).to.include("Insufficient amount for initial deposit");
        }
    });

    it("Should accept first deposit at minimum (8 decimals)", async () => {
        // For 8 decimals, minimum should be 100,000 units (0.001 tokens)
        const minimumDeposit = 100_000;

        await vaultContext.vaultProgram.methods
            .deposit(new BN(minimumDeposit))
            .accounts({
                senderTokenAccount: vaultContext.senderUsdgAta,
                senderShareAccount: vaultContext.senderShareAta,
                depositMint: vaultContext.usdgTokenMint,
                signer: vaultContext.deployer.publicKey,
                tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([vaultContext.deployer])
            .rpc();

        // Verify shares were minted 1:1
        const senderShares = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount;

        assert.equal(senderShares.toString(), minimumDeposit.toString());
    });

    it("Should accept subsequent deposits below minimum (no minimum after first deposit)", async () => {
        // After first deposit, any amount should be accepted
        const smallDeposit = 1; // 1 unit (0.00000001 tokens with 8 decimals)

        await vaultContext.vaultProgram.methods
            .deposit(new BN(smallDeposit))
            .accounts({
                senderTokenAccount: vaultContext.senderUsdgAta,
                senderShareAccount: vaultContext.senderShareAta,
                depositMint: vaultContext.usdgTokenMint,
                signer: vaultContext.deployer.publicKey,
                tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([vaultContext.deployer])
            .rpc();

        // Verify shares were minted 1:1
        const senderShares = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount;

        const expectedShares = 100_000 + 1; // Previous deposit + new deposit
        assert.equal(senderShares.toString(), expectedShares.toString());
    });

    it("Should maintain 1:1 exchange ratio", async () => {
        const depositAmount = 50_000;

        const sharesBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount;

        await vaultContext.vaultProgram.methods
            .deposit(new BN(depositAmount))
            .accounts({
                senderTokenAccount: vaultContext.senderUsdgAta,
                senderShareAccount: vaultContext.senderShareAta,
                depositMint: vaultContext.usdgTokenMint,
                signer: vaultContext.deployer.publicKey,
                tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([vaultContext.deployer])
            .rpc();

        const sharesAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount;

        const sharesMinted = new BN(sharesAfter).sub(new BN(sharesBefore));
        assert.equal(sharesMinted.toString(), depositAmount.toString());
    });
});

describe("august-vault-minimum-deposit-different-decimals", () => {
    anchor.setProvider(anchor.AnchorProvider.env());
    
    // Test different decimal configurations
    const testCases = [
        { decimals: 0, minUnits: 1, description: "0 decimals" },
        { decimals: 1, minUnits: 1, description: "1 decimal" },
        { decimals: 2, minUnits: 1, description: "2 decimals" },
        { decimals: 3, minUnits: 1, description: "3 decimals" },
        { decimals: 4, minUnits: 10, description: "4 decimals" },
        { decimals: 5, minUnits: 100, description: "5 decimals" },
        { decimals: 6, minUnits: 1000, description: "6 decimals" },
        { decimals: 7, minUnits: 10000, description: "7 decimals" },
        { decimals: 8, minUnits: 100000, description: "8 decimals" },
        { decimals: 9, minUnits: 1000000, description: "9 decimals" },
    ];

    testCases.forEach(({ decimals, minUnits, description }) => {
        it(`Should calculate correct minimum for ${description}`, async () => {
            // Create a new vault context for each test
            const testVaultContext = new VaultContext();
            await testVaultContext.init();

            // Create a new token mint with specific decimals
            const testMint = await token.createMint(
                testVaultContext.connection,
                testVaultContext.deployer,
                testVaultContext.deployer.publicKey,
                null,
                decimals
            );

            // Create associated token accounts
            const testSenderAta = await token.createAssociatedTokenAccount(
                testVaultContext.connection,
                testVaultContext.deployer,
                testMint,
                testVaultContext.deployer.publicKey
            );

            const testShareAta = await token.createAssociatedTokenAccount(
                testVaultContext.connection,
                testVaultContext.deployer,
                testVaultContext.shareMint,
                testVaultContext.deployer.publicKey
            );

            // Mint some tokens to the sender
            await token.mintTo(
                testVaultContext.connection,
                testVaultContext.deployer,
                testMint,
                testSenderAta,
                testVaultContext.deployer.publicKey,
                minUnits * 10 // Mint enough for testing
            );

            // Test that deposit below minimum is rejected
            try {
                await testVaultContext.vaultProgram.methods
                    .deposit(new BN(minUnits - 1))
                    .accounts({
                        senderTokenAccount: testSenderAta,
                        senderShareAccount: testShareAta,
                        depositMint: testMint,
                        signer: testVaultContext.deployer.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([testVaultContext.deployer])
                    .rpc();

                assert.fail(`Should have rejected deposit below minimum for ${description}`);
            } catch (error) {
                expect(error.message).to.include("Insufficient amount for initial deposit");
            }

            // Test that deposit at minimum is accepted
            await testVaultContext.vaultProgram.methods
                .deposit(new BN(minUnits))
                .accounts({
                    senderTokenAccount: testSenderAta,
                    senderShareAccount: testShareAta,
                    depositMint: testMint,
                    signer: testVaultContext.deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([testVaultContext.deployer])
                .rpc();

            // Verify shares were minted 1:1
            const senderShares = (await token.getAccount(
                testVaultContext.connection,
                testShareAta
            )).amount;

            assert.equal(senderShares.toString(), minUnits.toString());
        });
    });
});
