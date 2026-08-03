// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AugustVault } from "../target/types/august_vault";
import { PublicKey, Keypair, LAMPORTS_PER_SOL } from "@solana/web3.js";
import * as token from "@solana/spl-token";
import * as assert from "assert";
import { sha256 } from "js-sha256";
import BN from "bn.js";
import { protocolAuthority, ensureProgramConfig } from "./helper/program-config";

/**
 * Version Security Test Suite
 *
 * Tests the security properties of versioned PDAs including:
 * 1. Wrong version access attempts (cross-version isolation)
 * 2. Concurrent multi-version vaults (same deposit mint, different versions active simultaneously)
 * 3. Version boundary tests (0 and 255)
 * 4. closeVault with wrong version PDAs
 */
describe("version-security", () => {
    anchor.setProvider(anchor.AnchorProvider.env());

    const connection = anchor.getProvider().connection;
    const vaultProgram = anchor.workspace.AugustVault as Program<AugustVault>;

    // Unique seeds for this test suite
    const deployer = Keypair.fromSeed(Uint8Array.from(sha256.digest("versionSecurityDeployer")));
    const operator = Keypair.fromSeed(Uint8Array.from(sha256.digest("versionSecurityOperator")));
    const admin = Keypair.fromSeed(Uint8Array.from(sha256.digest("versionSecurityAdmin")));
    const feeRecipient = Keypair.fromSeed(Uint8Array.from(sha256.digest("versionSecurityFeeRecipient")));

    /**
     * Helper: Derive all vault PDAs from a deposit mint and version
     */
    function deriveVaultPdas(depositMint: PublicKey, vaultVersion: number) {
        const [statePda] = PublicKey.findProgramAddressSync(
            [Buffer.from("VAULT_STATE"), depositMint.toBuffer(), Buffer.from([vaultVersion])],
            vaultProgram.programId
        );
        const [shareMint] = PublicKey.findProgramAddressSync(
            [Buffer.from("mint"), depositMint.toBuffer(), Buffer.from([vaultVersion])],
            vaultProgram.programId
        );
        const [tokenAta] = PublicKey.findProgramAddressSync(
            [Buffer.from("token_vault"), depositMint.toBuffer(), Buffer.from([vaultVersion])],
            vaultProgram.programId
        );
        const [nominatedAdminPda] = PublicKey.findProgramAddressSync(
            [Buffer.from("nominated_admin"), depositMint.toBuffer(), Buffer.from([vaultVersion])],
            vaultProgram.programId
        );
        return { statePda, shareMint, tokenAta, nominatedAdminPda };
    }

    before(async () => {
        // Vault creation is gated on the ProgramConfig authority; whichever
        // suite runs first bootstraps it for the shared validator.
        await ensureProgramConfig(vaultProgram);
        // Airdrop to all accounts
        const airdropPromises = [deployer, operator, admin].map(async (kp) => {
            const sig = await connection.requestAirdrop(kp.publicKey, 100 * LAMPORTS_PER_SOL);
            const latestBlockHash = await connection.getLatestBlockhash();
            await connection.confirmTransaction({
                blockhash: latestBlockHash.blockhash,
                lastValidBlockHeight: latestBlockHash.lastValidBlockHeight,
                signature: sig,
            });
        });
        await Promise.all(airdropPromises);
    });

    describe("Concurrent Multi-Version Vaults", () => {
        let depositMint: PublicKey;
        let depositMintKeypair: Keypair;

        // Version 0 vault
        let v0StatePda: PublicKey;
        let v0ShareMint: PublicKey;
        let v0TokenAta: PublicKey;

        // Version 1 vault
        let v1StatePda: PublicKey;
        let v1ShareMint: PublicKey;
        let v1TokenAta: PublicKey;

        // User ATAs
        let userDepositAta: PublicKey;
        let userV0ShareAta: PublicKey;
        let userV1ShareAta: PublicKey;
        let feeRecipientDepositAta: PublicKey;
        let operatorDepositAta: PublicKey;

        before(async () => {
            // Create a deposit mint
            depositMintKeypair = Keypair.fromSeed(Uint8Array.from(sha256.digest("concurrentVersionMint")));
            depositMint = await token.createMint(
                connection,
                deployer,
                deployer.publicKey,
                deployer.publicKey,
                6,
                depositMintKeypair
            );

            // Derive PDAs for both versions
            const v0Pdas = deriveVaultPdas(depositMint, 0);
            v0StatePda = v0Pdas.statePda;
            v0ShareMint = v0Pdas.shareMint;
            v0TokenAta = v0Pdas.tokenAta;

            const v1Pdas = deriveVaultPdas(depositMint, 1);
            v1StatePda = v1Pdas.statePda;
            v1ShareMint = v1Pdas.shareMint;
            v1TokenAta = v1Pdas.tokenAta;

            // Create user deposit ATA
            userDepositAta = await token.createAssociatedTokenAccount(
                connection,
                deployer,
                depositMint,
                deployer.publicKey
            );

            // Mint tokens to user
            await token.mintTo(
                connection,
                deployer,
                depositMint,
                userDepositAta,
                deployer,
                100_000_000 // 100 tokens
            );

            // Create fee recipient ATA
            feeRecipientDepositAta = await token.createAssociatedTokenAccount(
                connection,
                deployer,
                depositMint,
                feeRecipient.publicKey
            );

            // Create operator ATA
            operatorDepositAta = await token.createAssociatedTokenAccount(
                connection,
                deployer,
                depositMint,
                operator.publicKey
            );
        });

        it("Can initialize version 0 vault", async () => {
            await vaultProgram.methods
                .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey, 0)
                .accounts({
                    vaultState: v0StatePda,
                    shareMint: v0ShareMint,
                    vaultTokenAta: v0TokenAta,
                    depositMint: depositMint,
                    signer: protocolAuthority.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([protocolAuthority])
                .rpc();

            const vault = await vaultProgram.account.vaultState.fetch(v0StatePda);
            assert.deepEqual(vault.vaultVersion, [0]);
        });

        it("Can initialize version 1 vault with same deposit mint (both active)", async () => {
            await vaultProgram.methods
                .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey, 1)
                .accounts({
                    vaultState: v1StatePda,
                    shareMint: v1ShareMint,
                    vaultTokenAta: v1TokenAta,
                    depositMint: depositMint,
                    signer: protocolAuthority.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([protocolAuthority])
                .rpc();

            const vault = await vaultProgram.account.vaultState.fetch(v1StatePda);
            assert.deepEqual(vault.vaultVersion, [1]);

            // Verify both vaults exist simultaneously
            const v0Vault = await vaultProgram.account.vaultState.fetchNullable(v0StatePda);
            const v1Vault = await vaultProgram.account.vaultState.fetchNullable(v1StatePda);
            assert.notEqual(v0Vault, null, "Version 0 vault should exist");
            assert.notEqual(v1Vault, null, "Version 1 vault should exist");
        });

        it("Both vaults have different PDAs despite same deposit mint", async () => {
            assert.notDeepEqual(v0StatePda, v1StatePda, "State PDAs should differ");
            assert.notDeepEqual(v0ShareMint, v1ShareMint, "Share mints should differ");
            assert.notDeepEqual(v0TokenAta, v1TokenAta, "Token ATAs should differ");
        });

        it("Can deposit to version 0 vault", async () => {
            // Create share ATA for version 0
            userV0ShareAta = await token.createAssociatedTokenAccount(
                connection,
                deployer,
                v0ShareMint,
                deployer.publicKey
            );

            const depositAmount = 10_000_000; // 10 tokens
            await vaultProgram.methods
                .deposit(new BN(depositAmount))
                .accounts({
                    vaultState: v0StatePda,
                    vaultTokenAta: v0TokenAta,
                    senderTokenAccount: userDepositAta,
                    senderShareAccount: userV0ShareAta,
                    shareMint: v0ShareMint,
                    depositMint: depositMint,
                    signer: deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([deployer])
                .rpc();

            const v0Vault = await vaultProgram.account.vaultState.fetch(v0StatePda);
            assert.equal(v0Vault.localAum.toNumber(), depositAmount);
        });

        it("Can deposit to version 1 vault independently", async () => {
            // Create share ATA for version 1
            userV1ShareAta = await token.createAssociatedTokenAccount(
                connection,
                deployer,
                v1ShareMint,
                deployer.publicKey
            );

            const depositAmount = 20_000_000; // 20 tokens
            await vaultProgram.methods
                .deposit(new BN(depositAmount))
                .accounts({
                    vaultState: v1StatePda,
                    vaultTokenAta: v1TokenAta,
                    senderTokenAccount: userDepositAta,
                    senderShareAccount: userV1ShareAta,
                    shareMint: v1ShareMint,
                    depositMint: depositMint,
                    signer: deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([deployer])
                .rpc();

            const v1Vault = await vaultProgram.account.vaultState.fetch(v1StatePda);
            assert.equal(v1Vault.localAum.toNumber(), depositAmount);
        });

        it("Vaults maintain completely independent state", async () => {
            const v0Vault = await vaultProgram.account.vaultState.fetch(v0StatePda);
            const v1Vault = await vaultProgram.account.vaultState.fetch(v1StatePda);

            // Independent AUM
            assert.equal(v0Vault.localAum.toNumber(), 10_000_000);
            assert.equal(v1Vault.localAum.toNumber(), 20_000_000);

            // Same deposit mint
            assert.deepEqual(v0Vault.depositMint, depositMint);
            assert.deepEqual(v1Vault.depositMint, depositMint);

            // Different share mints
            assert.notDeepEqual(v0Vault.shareMint, v1Vault.shareMint);

            // Same admin/operator (but could be different)
            assert.deepEqual(v0Vault.admin, v1Vault.admin);
            assert.deepEqual(v0Vault.operator, v1Vault.operator);
        });

        it("Pausing version 0 does not affect version 1", async () => {
            await vaultProgram.methods
                .pause()
                .accounts({
                    vaultState: v0StatePda,
                    depositMint: depositMint,
                    admin: admin.publicKey,
                })
                .signers([admin])
                .rpc();

            const v0Vault = await vaultProgram.account.vaultState.fetch(v0StatePda);
            const v1Vault = await vaultProgram.account.vaultState.fetch(v1StatePda);

            assert.equal(v0Vault.paused, true, "Version 0 should be paused");
            assert.equal(v1Vault.paused, false, "Version 1 should not be paused");

            // Deposit to version 1 should still work
            await vaultProgram.methods
                .deposit(new BN(1_000_000))
                .accounts({
                    vaultState: v1StatePda,
                    vaultTokenAta: v1TokenAta,
                    senderTokenAccount: userDepositAta,
                    senderShareAccount: userV1ShareAta,
                    shareMint: v1ShareMint,
                    depositMint: depositMint,
                    signer: deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([deployer])
                .rpc();

            // Unpause version 0
            await vaultProgram.methods
                .unpause()
                .accounts({
                    vaultState: v0StatePda,
                    depositMint: depositMint,
                    admin: admin.publicKey,
                })
                .signers([admin])
                .rpc();
        });

        it("Can redeem from version 0 without affecting version 1", async () => {
            const v1VaultBefore = await vaultProgram.account.vaultState.fetch(v1StatePda);
            const shareBalance = (await token.getAccount(connection, userV0ShareAta)).amount;
            const redeemAmount = shareBalance / 2n;

            await vaultProgram.methods
                .redeem(new BN(redeemAmount.toString()))
                .accounts({
                    vaultState: v0StatePda,
                    vaultDepositAta: v0TokenAta,
                    senderTokenAccount: userDepositAta,
                    senderShareAccount: userV0ShareAta,
                    feeRecipientAccount: feeRecipientDepositAta,
                    shareMint: v0ShareMint,
                    depositMint: depositMint,
                    signer: deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([deployer])
                .rpc();

            const v1VaultAfter = await vaultProgram.account.vaultState.fetch(v1StatePda);
            assert.equal(
                v1VaultBefore.localAum.toNumber(),
                v1VaultAfter.localAum.toNumber(),
                "Version 1 AUM should be unchanged"
            );
        });

        it("Closing version 0 does not affect version 1", async () => {
            // Redeem remaining shares from version 0
            const remainingShares = (await token.getAccount(connection, userV0ShareAta)).amount;
            if (remainingShares > 0n) {
                await vaultProgram.methods
                    .redeem(new BN(remainingShares.toString()))
                    .accounts({
                        vaultState: v0StatePda,
                        vaultDepositAta: v0TokenAta,
                        senderTokenAccount: userDepositAta,
                        senderShareAccount: userV0ShareAta,
                        feeRecipientAccount: feeRecipientDepositAta,
                        shareMint: v0ShareMint,
                        depositMint: depositMint,
                        signer: deployer.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([deployer])
                    .rpc();
            }

            // Close version 0
            await vaultProgram.methods
                .closeVault()
                .accounts({
                    vaultState: v0StatePda,
                    shareMint: v0ShareMint,
                    vaultTokenAta: v0TokenAta,
                    depositMint: depositMint,
                    admin: admin.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([admin])
                .rpc();

            // Verify version 0 is closed
            const v0Vault = await vaultProgram.account.vaultState.fetchNullable(v0StatePda);
            assert.equal(v0Vault, null, "Version 0 should be closed");

            // Verify version 1 still exists and is operational
            const v1Vault = await vaultProgram.account.vaultState.fetchNullable(v1StatePda);
            assert.notEqual(v1Vault, null, "Version 1 should still exist");

            // Can still deposit to version 1
            await vaultProgram.methods
                .deposit(new BN(1_000_000))
                .accounts({
                    vaultState: v1StatePda,
                    vaultTokenAta: v1TokenAta,
                    senderTokenAccount: userDepositAta,
                    senderShareAccount: userV1ShareAta,
                    shareMint: v1ShareMint,
                    depositMint: depositMint,
                    signer: deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([deployer])
                .rpc();
        });
    });

    describe("Wrong Version Access Attempts", () => {
        let depositMint: PublicKey;
        let depositMintKeypair: Keypair;

        // Version 0 vault
        let v0StatePda: PublicKey;
        let v0ShareMint: PublicKey;
        let v0TokenAta: PublicKey;
        let v0NominatedAdminPda: PublicKey;

        // Version 1 vault (will use these PDAs to attempt cross-version access)
        let v1StatePda: PublicKey;
        let v1ShareMint: PublicKey;
        let v1TokenAta: PublicKey;
        let v1NominatedAdminPda: PublicKey;

        // User ATAs
        let userDepositAta: PublicKey;
        let userV0ShareAta: PublicKey;
        let feeRecipientDepositAta: PublicKey;
        let operatorDepositAta: PublicKey;

        before(async () => {
            // Create a unique deposit mint for this test
            depositMintKeypair = Keypair.fromSeed(Uint8Array.from(sha256.digest("wrongVersionMint")));
            depositMint = await token.createMint(
                connection,
                deployer,
                deployer.publicKey,
                deployer.publicKey,
                6,
                depositMintKeypair
            );

            // Derive PDAs for version 0
            const v0Pdas = deriveVaultPdas(depositMint, 0);
            v0StatePda = v0Pdas.statePda;
            v0ShareMint = v0Pdas.shareMint;
            v0TokenAta = v0Pdas.tokenAta;
            v0NominatedAdminPda = v0Pdas.nominatedAdminPda;

            // Derive PDAs for version 1 (non-existent vault)
            const v1Pdas = deriveVaultPdas(depositMint, 1);
            v1StatePda = v1Pdas.statePda;
            v1ShareMint = v1Pdas.shareMint;
            v1TokenAta = v1Pdas.tokenAta;
            v1NominatedAdminPda = v1Pdas.nominatedAdminPda;

            // Create user deposit ATA
            userDepositAta = await token.createAssociatedTokenAccount(
                connection,
                deployer,
                depositMint,
                deployer.publicKey
            );

            // Mint tokens to user
            await token.mintTo(
                connection,
                deployer,
                depositMint,
                userDepositAta,
                deployer,
                100_000_000
            );

            // Create fee recipient ATA
            feeRecipientDepositAta = await token.createAssociatedTokenAccount(
                connection,
                deployer,
                depositMint,
                feeRecipient.publicKey
            );

            // Create operator ATA
            operatorDepositAta = await token.createAssociatedTokenAccount(
                connection,
                deployer,
                depositMint,
                operator.publicKey
            );

            // Initialize version 0 vault
            await vaultProgram.methods
                .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey, 0)
                .accounts({
                    vaultState: v0StatePda,
                    shareMint: v0ShareMint,
                    vaultTokenAta: v0TokenAta,
                    depositMint: depositMint,
                    signer: protocolAuthority.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([protocolAuthority])
                .rpc();

            // Create user share ATA for version 0
            userV0ShareAta = await token.createAssociatedTokenAccount(
                connection,
                deployer,
                v0ShareMint,
                deployer.publicKey
            );

            // Deposit to version 0 vault
            await vaultProgram.methods
                .deposit(new BN(10_000_000))
                .accounts({
                    vaultState: v0StatePda,
                    vaultTokenAta: v0TokenAta,
                    senderTokenAccount: userDepositAta,
                    senderShareAccount: userV0ShareAta,
                    shareMint: v0ShareMint,
                    depositMint: depositMint,
                    signer: deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([deployer])
                .rpc();
        });

        it("Cannot deposit to version 0 using version 1 derived PDAs", async () => {
            await assert.rejects(
                vaultProgram.methods
                    .deposit(new BN(1_000_000))
                    .accounts({
                        vaultState: v1StatePda, // Wrong version PDA
                        vaultTokenAta: v1TokenAta, // Wrong version
                        senderTokenAccount: userDepositAta,
                        senderShareAccount: userV0ShareAta,
                        shareMint: v1ShareMint, // Wrong version
                        depositMint: depositMint,
                        signer: deployer.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([deployer])
                    .rpc(),
                /AccountNotInitialized|A seeds constraint was violated|AnchorError/
            );
        });

        it("Cannot redeem from version 0 using version 1 derived PDAs", async () => {
            await assert.rejects(
                vaultProgram.methods
                    .redeem(new BN(1_000))
                    .accounts({
                        vaultState: v1StatePda, // Wrong version PDA
                        vaultDepositAta: v1TokenAta, // Wrong version
                        senderTokenAccount: userDepositAta,
                        senderShareAccount: userV0ShareAta,
                        feeRecipientAccount: feeRecipientDepositAta,
                        shareMint: v1ShareMint, // Wrong version
                        depositMint: depositMint,
                        signer: deployer.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([deployer])
                    .rpc(),
                /AccountNotInitialized|A seeds constraint was violated|AnchorError/
            );
        });

        it("Cannot pause version 0 using version 1 state PDA", async () => {
            await assert.rejects(
                vaultProgram.methods
                    .pause()
                    .accounts({
                        vaultState: v1StatePda, // Wrong version PDA
                        depositMint: depositMint,
                        admin: admin.publicKey,
                    })
                    .signers([admin])
                    .rpc(),
                /AccountNotInitialized|A seeds constraint was violated|AnchorError/
            );
        });

        it("Cannot unpause version 0 using version 1 state PDA", async () => {
            // First pause version 0 correctly
            await vaultProgram.methods
                .pause()
                .accounts({
                    vaultState: v0StatePda,
                    depositMint: depositMint,
                    admin: admin.publicKey,
                })
                .signers([admin])
                .rpc();

            await assert.rejects(
                vaultProgram.methods
                    .unpause()
                    .accounts({
                        vaultState: v1StatePda, // Wrong version PDA
                        depositMint: depositMint,
                        admin: admin.publicKey,
                    })
                    .signers([admin])
                    .rpc(),
                /AccountNotInitialized|A seeds constraint was violated|AnchorError/
            );

            // Unpause for subsequent tests
            await vaultProgram.methods
                .unpause()
                .accounts({
                    vaultState: v0StatePda,
                    depositMint: depositMint,
                    admin: admin.publicKey,
                })
                .signers([admin])
                .rpc();
        });

        it("Cannot setOperator on version 0 using version 1 state PDA", async () => {
            const newOperator = Keypair.generate();
            await assert.rejects(
                vaultProgram.methods
                    .setOperator(newOperator.publicKey)
                    .accounts({
                        vaultState: v1StatePda, // Wrong version PDA
                        depositMint: depositMint,
                        admin: admin.publicKey,
                    })
                    .signers([admin])
                    .rpc(),
                /AccountNotInitialized|A seeds constraint was violated|AnchorError/
            );
        });

        it("Cannot setFeeRecipient on version 0 using version 1 state PDA", async () => {
            const newFeeRecipient = Keypair.generate();
            await assert.rejects(
                vaultProgram.methods
                    .setFeeRecipient(newFeeRecipient.publicKey)
                    .accounts({
                        vaultState: v1StatePda, // Wrong version PDA
                        depositMint: depositMint,
                        admin: admin.publicKey,
                    })
                    .signers([admin])
                    .rpc(),
                /AccountNotInitialized|A seeds constraint was violated|AnchorError/
            );
        });

        it("Cannot setWithdrawalFee on version 0 using version 1 state PDA", async () => {
            await assert.rejects(
                vaultProgram.methods
                    .setWithdrawalFee(100)
                    .accounts({
                        vaultState: v1StatePda, // Wrong version PDA
                        depositMint: depositMint,
                        admin: admin.publicKey,
                    })
                    .signers([admin])
                    .rpc(),
                /AccountNotInitialized|A seeds constraint was violated|AnchorError/
            );
        });

        it("Cannot setAumLimits on version 0 using version 1 state PDA", async () => {
            await assert.rejects(
                vaultProgram.methods
                    .setAumLimits(1000, 1000)
                    .accounts({
                        vaultState: v1StatePda, // Wrong version PDA
                        depositMint: depositMint,
                        admin: admin.publicKey,
                    })
                    .signers([admin])
                    .rpc(),
                /AccountNotInitialized|A seeds constraint was violated|AnchorError/
            );
        });

        it("Cannot operatorWithdraw from version 0 using version 1 state PDA", async () => {
            await assert.rejects(
                vaultProgram.methods
                    .operatorWithdraw(new BN(1_000))
                    .accounts({
                        vaultState: v1StatePda, // Wrong version PDA
                        vaultDepositAta: v1TokenAta, // Wrong version
                        operatorTokenAccount: operatorDepositAta,
                        depositMint: depositMint,
                        operator: operator.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([operator])
                    .rpc(),
                /AccountNotInitialized|A seeds constraint was violated|AnchorError/
            );
        });

        it("Cannot operatorDeposit to version 0 using version 1 state PDA", async () => {
            // Mint tokens to operator
            await token.mintTo(
                connection,
                deployer,
                depositMint,
                operatorDepositAta,
                deployer,
                1_000_000
            );

            await assert.rejects(
                vaultProgram.methods
                    .operatorDeposit(new BN(1_000))
                    .accounts({
                        vaultState: v1StatePda, // Wrong version PDA
                        vaultDepositAta: v1TokenAta, // Wrong version
                        operatorTokenAccount: operatorDepositAta,
                        depositMint: depositMint,
                        operator: operator.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([operator])
                    .rpc(),
                /AccountNotInitialized|A seeds constraint was violated|AnchorError/
            );
        });

        it("Cannot operatorUpdateAum on version 0 using version 1 state PDA", async () => {
            await assert.rejects(
                vaultProgram.methods
                    .operatorUpdateAum(new BN(100))
                    .accounts({
                        vaultState: v1StatePda, // Wrong version PDA
                        depositMint: depositMint,
                        operator: operator.publicKey,
                    })
                    .signers([operator])
                    .rpc(),
                /AccountNotInitialized|A seeds constraint was violated|AnchorError/
            );
        });

        it("Cannot nominateAdmin on version 0 using version 1 state PDA", async () => {
            const newAdmin = Keypair.generate();
            await assert.rejects(
                vaultProgram.methods
                    .nominateAdmin(newAdmin.publicKey)
                    .accounts({
                        vaultState: v1StatePda, // Wrong version PDA
                        depositMint: depositMint,
                        nominatedAdminPda: v1NominatedAdminPda, // Wrong version
                        admin: admin.publicKey,
                        payer: deployer.publicKey,
                    })
                    .signers([admin, deployer])
                    .rpc(),
                /AccountNotInitialized|A seeds constraint was violated|AnchorError/
            );
        });
    });

    describe("closeVault with Wrong Version", () => {
        let depositMint: PublicKey;
        let depositMintKeypair: Keypair;

        // Version 0 vault
        let v0StatePda: PublicKey;
        let v0ShareMint: PublicKey;
        let v0TokenAta: PublicKey;

        // Version 1 PDAs (non-existent vault)
        let v1StatePda: PublicKey;
        let v1ShareMint: PublicKey;
        let v1TokenAta: PublicKey;

        before(async () => {
            // Create a unique deposit mint for this test
            depositMintKeypair = Keypair.fromSeed(Uint8Array.from(sha256.digest("closeWrongVersionMint")));
            depositMint = await token.createMint(
                connection,
                deployer,
                deployer.publicKey,
                deployer.publicKey,
                6,
                depositMintKeypair
            );

            // Derive PDAs for version 0
            const v0Pdas = deriveVaultPdas(depositMint, 0);
            v0StatePda = v0Pdas.statePda;
            v0ShareMint = v0Pdas.shareMint;
            v0TokenAta = v0Pdas.tokenAta;

            // Derive PDAs for version 1
            const v1Pdas = deriveVaultPdas(depositMint, 1);
            v1StatePda = v1Pdas.statePda;
            v1ShareMint = v1Pdas.shareMint;
            v1TokenAta = v1Pdas.tokenAta;

            // Initialize version 0 vault
            await vaultProgram.methods
                .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey, 0)
                .accounts({
                    vaultState: v0StatePda,
                    shareMint: v0ShareMint,
                    vaultTokenAta: v0TokenAta,
                    depositMint: depositMint,
                    signer: protocolAuthority.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([protocolAuthority])
                .rpc();
        });

        it("Cannot close vault using wrong version PDAs", async () => {
            await assert.rejects(
                vaultProgram.methods
                    .closeVault()
                    .accounts({
                        vaultState: v1StatePda, // Wrong version PDA
                        shareMint: v1ShareMint, // Wrong version
                        vaultTokenAta: v1TokenAta, // Wrong version
                        depositMint: depositMint,
                        admin: admin.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([admin])
                    .rpc(),
                /AccountNotInitialized|A seeds constraint was violated|AnchorError/
            );

            // Verify version 0 vault still exists
            const v0Vault = await vaultProgram.account.vaultState.fetchNullable(v0StatePda);
            assert.notEqual(v0Vault, null, "Version 0 vault should still exist");
        });

        it("Cannot close version 0 using mixed version accounts", async () => {
            // Try using v0 state but v1 share mint
            await assert.rejects(
                vaultProgram.methods
                    .closeVault()
                    .accounts({
                        vaultState: v0StatePda, // Correct version
                        shareMint: v1ShareMint, // Wrong version!
                        vaultTokenAta: v0TokenAta, // Correct version
                        depositMint: depositMint,
                        admin: admin.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([admin])
                    .rpc(),
                /A seeds constraint was violated|ConstraintSeeds|AnchorError/
            );

            // Verify version 0 vault still exists
            const v0Vault = await vaultProgram.account.vaultState.fetchNullable(v0StatePda);
            assert.notEqual(v0Vault, null, "Version 0 vault should still exist");
        });

        it("Can close vault using correct version PDAs", async () => {
            await vaultProgram.methods
                .closeVault()
                .accounts({
                    vaultState: v0StatePda,
                    shareMint: v0ShareMint,
                    vaultTokenAta: v0TokenAta,
                    depositMint: depositMint,
                    admin: admin.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([admin])
                .rpc();

            // Verify vault is closed
            const v0Vault = await vaultProgram.account.vaultState.fetchNullable(v0StatePda);
            assert.equal(v0Vault, null, "Version 0 vault should be closed");
        });
    });

    describe("Version Boundary Tests", () => {
        describe("Version 0 (minimum boundary)", () => {
            let depositMint: PublicKey;
            let depositMintKeypair: Keypair;
            let v0StatePda: PublicKey;
            let v0ShareMint: PublicKey;
            let v0TokenAta: PublicKey;

            before(async () => {
                depositMintKeypair = Keypair.fromSeed(Uint8Array.from(sha256.digest("version0BoundaryMint")));
                depositMint = await token.createMint(
                    connection,
                    deployer,
                    deployer.publicKey,
                    deployer.publicKey,
                    6,
                    depositMintKeypair
                );

                const pdas = deriveVaultPdas(depositMint, 0);
                v0StatePda = pdas.statePda;
                v0ShareMint = pdas.shareMint;
                v0TokenAta = pdas.tokenAta;
            });

            it("Can initialize vault with version 0", async () => {
                await vaultProgram.methods
                    .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey, 0)
                    .accounts({
                        vaultState: v0StatePda,
                        shareMint: v0ShareMint,
                        vaultTokenAta: v0TokenAta,
                        depositMint: depositMint,
                        signer: protocolAuthority.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([protocolAuthority])
                    .rpc();

                const vault = await vaultProgram.account.vaultState.fetch(v0StatePda);
                assert.deepEqual(vault.vaultVersion, [0], "Vault version should be [0]");
            });

            it("Version 0 is stored correctly in state", async () => {
                const vault = await vaultProgram.account.vaultState.fetch(v0StatePda);

                // Verify stored version matches PDA derivation version
                const [rederived] = PublicKey.findProgramAddressSync(
                    [Buffer.from("VAULT_STATE"), depositMint.toBuffer(), Buffer.from(vault.vaultVersion)],
                    vaultProgram.programId
                );
                assert.deepEqual(rederived, v0StatePda, "Re-derived PDA should match");
            });

            after(async () => {
                // Clean up
                await vaultProgram.methods
                    .closeVault()
                    .accounts({
                        vaultState: v0StatePda,
                        shareMint: v0ShareMint,
                        vaultTokenAta: v0TokenAta,
                        depositMint: depositMint,
                        admin: admin.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([admin])
                    .rpc();
            });
        });

        describe("Version 255 (maximum boundary)", () => {
            let depositMint: PublicKey;
            let depositMintKeypair: Keypair;
            let v255StatePda: PublicKey;
            let v255ShareMint: PublicKey;
            let v255TokenAta: PublicKey;
            let userDepositAta: PublicKey;
            let userShareAta: PublicKey;
            let feeRecipientAta: PublicKey;

            before(async () => {
                depositMintKeypair = Keypair.fromSeed(Uint8Array.from(sha256.digest("version255BoundaryMint")));
                depositMint = await token.createMint(
                    connection,
                    deployer,
                    deployer.publicKey,
                    deployer.publicKey,
                    6,
                    depositMintKeypair
                );

                const pdas = deriveVaultPdas(depositMint, 255);
                v255StatePda = pdas.statePda;
                v255ShareMint = pdas.shareMint;
                v255TokenAta = pdas.tokenAta;

                // Create user deposit ATA
                userDepositAta = await token.createAssociatedTokenAccount(
                    connection,
                    deployer,
                    depositMint,
                    deployer.publicKey
                );

                // Mint tokens
                await token.mintTo(
                    connection,
                    deployer,
                    depositMint,
                    userDepositAta,
                    deployer,
                    10_000_000
                );

                // Create fee recipient ATA
                feeRecipientAta = await token.createAssociatedTokenAccount(
                    connection,
                    deployer,
                    depositMint,
                    feeRecipient.publicKey
                );
            });

            it("Can initialize vault with version 255", async () => {
                await vaultProgram.methods
                    .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey, 255)
                    .accounts({
                        vaultState: v255StatePda,
                        shareMint: v255ShareMint,
                        vaultTokenAta: v255TokenAta,
                        depositMint: depositMint,
                        signer: protocolAuthority.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([protocolAuthority])
                    .rpc();

                const vault = await vaultProgram.account.vaultState.fetch(v255StatePda);
                assert.deepEqual(vault.vaultVersion, [255], "Vault version should be [255]");
            });

            it("Version 255 is stored correctly in state", async () => {
                const vault = await vaultProgram.account.vaultState.fetch(v255StatePda);

                // Verify stored version matches PDA derivation version
                const [rederived] = PublicKey.findProgramAddressSync(
                    [Buffer.from("VAULT_STATE"), depositMint.toBuffer(), Buffer.from(vault.vaultVersion)],
                    vaultProgram.programId
                );
                assert.deepEqual(rederived, v255StatePda, "Re-derived PDA should match");
            });

            it("Can deposit to version 255 vault", async () => {
                userShareAta = await token.createAssociatedTokenAccount(
                    connection,
                    deployer,
                    v255ShareMint,
                    deployer.publicKey
                );

                const depositAmount = 1_000_000;
                await vaultProgram.methods
                    .deposit(new BN(depositAmount))
                    .accounts({
                        vaultState: v255StatePda,
                        vaultTokenAta: v255TokenAta,
                        senderTokenAccount: userDepositAta,
                        senderShareAccount: userShareAta,
                        shareMint: v255ShareMint,
                        depositMint: depositMint,
                        signer: deployer.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([deployer])
                    .rpc();

                const vault = await vaultProgram.account.vaultState.fetch(v255StatePda);
                assert.equal(vault.localAum.toNumber(), depositAmount);
            });

            it("Can redeem from version 255 vault", async () => {
                const shareBalance = (await token.getAccount(connection, userShareAta)).amount;

                await vaultProgram.methods
                    .redeem(new BN(shareBalance.toString()))
                    .accounts({
                        vaultState: v255StatePda,
                        vaultDepositAta: v255TokenAta,
                        senderTokenAccount: userDepositAta,
                        senderShareAccount: userShareAta,
                        feeRecipientAccount: feeRecipientAta,
                        shareMint: v255ShareMint,
                        depositMint: depositMint,
                        signer: deployer.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([deployer])
                    .rpc();

                const newShareBalance = (await token.getAccount(connection, userShareAta)).amount;
                assert.equal(Number(newShareBalance), 0, "All shares should be redeemed");
            });

            it("Can close version 255 vault", async () => {
                await vaultProgram.methods
                    .closeVault()
                    .accounts({
                        vaultState: v255StatePda,
                        shareMint: v255ShareMint,
                        vaultTokenAta: v255TokenAta,
                        depositMint: depositMint,
                        admin: admin.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([admin])
                    .rpc();

                const vault = await vaultProgram.account.vaultState.fetchNullable(v255StatePda);
                assert.equal(vault, null, "Version 255 vault should be closed");
            });
        });

        describe("Version consistency across operations", () => {
            let depositMint: PublicKey;
            let depositMintKeypair: Keypair;
            let v5StatePda: PublicKey;
            let v5ShareMint: PublicKey;
            let v5TokenAta: PublicKey;

            before(async () => {
                depositMintKeypair = Keypair.fromSeed(Uint8Array.from(sha256.digest("version5ConsistencyMint")));
                depositMint = await token.createMint(
                    connection,
                    deployer,
                    deployer.publicKey,
                    deployer.publicKey,
                    6,
                    depositMintKeypair
                );

                const pdas = deriveVaultPdas(depositMint, 5);
                v5StatePda = pdas.statePda;
                v5ShareMint = pdas.shareMint;
                v5TokenAta = pdas.tokenAta;

                await vaultProgram.methods
                    .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey, 5)
                    .accounts({
                        vaultState: v5StatePda,
                        shareMint: v5ShareMint,
                        vaultTokenAta: v5TokenAta,
                        depositMint: depositMint,
                        signer: protocolAuthority.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([protocolAuthority])
                    .rpc();
            });

            it("Vault stores the exact version passed during initialization", async () => {
                const vault = await vaultProgram.account.vaultState.fetch(v5StatePda);
                assert.deepEqual(vault.vaultVersion, [5], "Vault version should be exactly [5]");
            });

            it("PDA can be re-derived from stored version", async () => {
                const vault = await vaultProgram.account.vaultState.fetch(v5StatePda);

                // All PDAs should be re-derivable using the stored version
                const [stateRederived] = PublicKey.findProgramAddressSync(
                    [Buffer.from("VAULT_STATE"), vault.depositMint.toBuffer(), Buffer.from(vault.vaultVersion)],
                    vaultProgram.programId
                );
                const [shareRederived] = PublicKey.findProgramAddressSync(
                    [Buffer.from("mint"), vault.depositMint.toBuffer(), Buffer.from(vault.vaultVersion)],
                    vaultProgram.programId
                );

                assert.deepEqual(stateRederived, v5StatePda, "State PDA should match");
                assert.deepEqual(shareRederived, v5ShareMint, "Share mint PDA should match");
            });

            after(async () => {
                await vaultProgram.methods
                    .closeVault()
                    .accounts({
                        vaultState: v5StatePda,
                        shareMint: v5ShareMint,
                        vaultTokenAta: v5TokenAta,
                        depositMint: depositMint,
                        admin: admin.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([admin])
                    .rpc();
            });
        });
    });
});
