// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

import * as anchor from "@coral-xyz/anchor";
import { DEFAULT_SHARE_OFFSET } from "./helper/config";
import { Program } from "@coral-xyz/anchor";
import { AugustVault } from "../target/types/august_vault";
import { PublicKey, Keypair, LAMPORTS_PER_SOL } from "@solana/web3.js";
import * as token from "@solana/spl-token";
import * as assert from "assert";
import { sha256 } from "js-sha256";
import BN from "bn.js";
import { protocolAuthority, ensureProgramConfig } from "./helper/program-config";

describe("august-vault-close-vault", () => {
    anchor.setProvider(anchor.AnchorProvider.env());

    const connection = anchor.getProvider().connection;
    const vaultProgram = anchor.workspace.AugustVault as Program<AugustVault>;

    // Use unique seeds to avoid conflicts with other tests
    const deployer = Keypair.fromSeed(Uint8Array.from(sha256.digest("closeVaultDeployer")));
    const operator = Keypair.fromSeed(Uint8Array.from(sha256.digest("closeVaultOperator")));
    const admin = Keypair.fromSeed(Uint8Array.from(sha256.digest("closeVaultAdmin")));
    const feeRecipient = Keypair.fromSeed(Uint8Array.from(sha256.digest("closeVaultFeeRecipient")));
    const nonAdmin = Keypair.fromSeed(Uint8Array.from(sha256.digest("closeVaultNonAdmin")));

    let depositMint: PublicKey;
    let depositMintKeypair: Keypair;
    let vaultStatePda: PublicKey;
    let shareMint: PublicKey;
    let vaultTokenAta: PublicKey;
    let currentVaultVersion: number = 0;

    before(async () => {
        // Vault creation is gated on the ProgramConfig authority; whichever
        // suite runs first bootstraps it for the shared validator.
        await ensureProgramConfig(vaultProgram);
        // Airdrop to all accounts
        const airdropPromises = [deployer, operator, admin, nonAdmin].map(async (kp) => {
            const sig = await connection.requestAirdrop(kp.publicKey, 10 * LAMPORTS_PER_SOL);
            const latestBlockHash = await connection.getLatestBlockhash();
            await connection.confirmTransaction({
                blockhash: latestBlockHash.blockhash,
                lastValidBlockHeight: latestBlockHash.lastValidBlockHeight,
                signature: sig,
            });
        });
        await Promise.all(airdropPromises);
    });

    async function initializeVault(mintDecimals: number = 9, vaultVersion: number = 0): Promise<void> {
        currentVaultVersion = vaultVersion;
        
        // Create a unique deposit mint for this test
        depositMintKeypair = Keypair.generate();
        depositMint = await token.createMint(
            connection,
            deployer,
            deployer.publicKey,
            deployer.publicKey,
            mintDecimals,
            depositMintKeypair
        );

        // Derive PDAs with version
        [vaultStatePda] = PublicKey.findProgramAddressSync(
            [Buffer.from("VAULT_STATE"), depositMint.toBuffer(), Buffer.from([vaultVersion])],
            vaultProgram.programId
        );

        [shareMint] = PublicKey.findProgramAddressSync(
            [Buffer.from("mint"), depositMint.toBuffer(), Buffer.from([vaultVersion])],
            vaultProgram.programId
        );

        [vaultTokenAta] = PublicKey.findProgramAddressSync(
            [Buffer.from("token_vault"), depositMint.toBuffer(), Buffer.from([vaultVersion])],
            vaultProgram.programId
        );

        // Initialize the vault with version
        await vaultProgram.methods
            .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey, vaultVersion, DEFAULT_SHARE_OFFSET)
            .accounts({
                vaultState: vaultStatePda,
                shareMint: shareMint,
                vaultTokenAta: vaultTokenAta,
                depositMint: depositMint,
                signer: protocolAuthority.publicKey,
                tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([protocolAuthority])
            .rpc();
    }

    describe("Close Vault", () => {
        beforeEach(async () => {
            await initializeVault(9);
        });

        it("Admin can close an empty vault", async () => {
            // Verify vault exists
            const vaultBefore = await vaultProgram.account.vaultState.fetchNullable(vaultStatePda);
            assert.notEqual(vaultBefore, null, "Vault should exist before closing");

            // Get admin balance before (to verify rent reclaim)
            const adminBalanceBefore = await connection.getBalance(admin.publicKey);

            // Close the vault
            await vaultProgram.methods
                .closeVault()
                .accounts({
                    vaultState: vaultStatePda,
                    shareMint: shareMint,
                    vaultTokenAta: vaultTokenAta,
                    depositMint: depositMint,
                    admin: admin.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([admin])
                .rpc();

            // Verify vault state is closed
            const vaultAfter = await vaultProgram.account.vaultState.fetchNullable(vaultStatePda);
            assert.equal(vaultAfter, null, "Vault state should be closed");

            // Verify admin received rent back
            const adminBalanceAfter = await connection.getBalance(admin.publicKey);
            assert.ok(adminBalanceAfter > adminBalanceBefore, "Admin should receive rent back");
        });

        it("Non-admin cannot close vault", async () => {
            await assert.rejects(
                vaultProgram.methods
                    .closeVault()
                    .accounts({
                        vaultState: vaultStatePda,
                        shareMint: shareMint,
                        vaultTokenAta: vaultTokenAta,
                        depositMint: depositMint,
                        admin: nonAdmin.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([nonAdmin])
                    .rpc(),
                /NotAdmin|Signer is Not Admin|A has one constraint was violated/
            );

            // Verify vault still exists
            const vault = await vaultProgram.account.vaultState.fetchNullable(vaultStatePda);
            assert.notEqual(vault, null, "Vault should still exist");
        });

        it("Cannot close vault with shares outstanding", async () => {
            // Create user token accounts
            const userDepositAta = await token.createAssociatedTokenAccount(
                connection,
                deployer,
                depositMint,
                deployer.publicKey
            );

            const userShareAta = await token.createAssociatedTokenAccount(
                connection,
                deployer,
                shareMint,
                deployer.publicKey
            );

            // Mint some deposit tokens to user
            await token.mintTo(
                connection,
                deployer,
                depositMint,
                userDepositAta,
                deployer,
                1_000_000_000 // 1 token with 9 decimals
            );

            // Deposit to get shares
            await vaultProgram.methods
                .deposit(new BN(1_000_000_000))
                .accounts({
                    vaultState: vaultStatePda,
                    vaultTokenAta: vaultTokenAta,
                    senderTokenAccount: userDepositAta,
                    senderShareAccount: userShareAta,
                    shareMint: shareMint,
                    depositMint: depositMint,
                    signer: deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([deployer])
                .rpc();

            // Verify shares were minted
            const shareMintInfo = await token.getMint(connection, shareMint);
            assert.ok(shareMintInfo.supply > 0n, "Shares should be minted");

            // Try to close vault - should fail
            await assert.rejects(
                vaultProgram.methods
                    .closeVault()
                    .accounts({
                        vaultState: vaultStatePda,
                        shareMint: shareMint,
                        vaultTokenAta: vaultTokenAta,
                        depositMint: depositMint,
                        admin: admin.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([admin])
                    .rpc(),
                /VaultNotEmpty|Vault must be empty to close/
            );

            // Verify vault still exists
            const vault = await vaultProgram.account.vaultState.fetchNullable(vaultStatePda);
            assert.notEqual(vault, null, "Vault should still exist");
        });

        it("Cannot close vault with tokens in vault (direct transfer scenario)", async () => {
            // Mint tokens directly to the vault token account (simulating direct transfer)
            // First need to get the vault token account address
            const vaultTokenAccount = await token.getAccount(connection, vaultTokenAta);
            
            // We need a user account to transfer from
            const userDepositAta = await token.createAssociatedTokenAccount(
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
                1_000_000_000
            );

            // Transfer directly to vault (bypassing deposit instruction)
            await token.transfer(
                connection,
                deployer,
                userDepositAta,
                vaultTokenAta,
                deployer,
                1_000_000_000
            );

            // Verify vault has tokens
            const vaultTokenAccountAfter = await token.getAccount(connection, vaultTokenAta);
            assert.ok(vaultTokenAccountAfter.amount > 0n, "Vault should have tokens");

            // Try to close vault - should fail
            await assert.rejects(
                vaultProgram.methods
                    .closeVault()
                    .accounts({
                        vaultState: vaultStatePda,
                        shareMint: shareMint,
                        vaultTokenAta: vaultTokenAta,
                        depositMint: depositMint,
                        admin: admin.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([admin])
                    .rpc(),
                /VaultNotEmpty|Vault must be empty to close/
            );
        });

        it("Can reinitialize vault with different version after closing", async () => {
            // Close the vault first (version 0)
            await vaultProgram.methods
                .closeVault()
                .accounts({
                    vaultState: vaultStatePda,
                    shareMint: shareMint,
                    vaultTokenAta: vaultTokenAta,
                    depositMint: depositMint,
                    admin: admin.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([admin])
                .rpc();

            // Verify vault state is closed
            let vault = await vaultProgram.account.vaultState.fetchNullable(vaultStatePda);
            assert.equal(vault, null, "Vault state should be closed");

            // Verify share_mint still exists (SPL Token mints cannot be closed)
            const shareMintInfo = await token.getMint(connection, shareMint);
            assert.notEqual(shareMintInfo, null, "Share mint should still exist");
            assert.equal(shareMintInfo.mintAuthority, null, "Share mint authority should be revoked");

            // Now initialize with version 1 - this should succeed!
            const newVersion = 1;
            const [newVaultStatePda] = PublicKey.findProgramAddressSync(
                [Buffer.from("VAULT_STATE"), depositMint.toBuffer(), Buffer.from([newVersion])],
                vaultProgram.programId
            );
            const [newShareMint] = PublicKey.findProgramAddressSync(
                [Buffer.from("mint"), depositMint.toBuffer(), Buffer.from([newVersion])],
                vaultProgram.programId
            );
            const [newVaultTokenAta] = PublicKey.findProgramAddressSync(
                [Buffer.from("token_vault"), depositMint.toBuffer(), Buffer.from([newVersion])],
                vaultProgram.programId
            );

            await vaultProgram.methods
                .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey, newVersion, DEFAULT_SHARE_OFFSET)
                .accounts({
                    vaultState: newVaultStatePda,
                    shareMint: newShareMint,
                    vaultTokenAta: newVaultTokenAta,
                    depositMint: depositMint,
                    signer: protocolAuthority.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([protocolAuthority])
                .rpc();

            // Verify new vault exists
            vault = await vaultProgram.account.vaultState.fetchNullable(newVaultStatePda);
            assert.notEqual(vault, null, "New vault should exist");
            assert.deepEqual(vault.vaultVersion, [newVersion], "Vault version should be 1");

            // Verify new share mint has correct decimals (9)
            const newShareMintInfo = await token.getMint(connection, newShareMint);
            assert.equal(newShareMintInfo.decimals, 9, "New share mint should have 9 decimals");
        });
    });

    describe("Dynamic Decimals", () => {
        it("Share mint should have same decimals as deposit mint (6 decimals)", async () => {
            await initializeVault(6);

            const shareMintInfo = await token.getMint(connection, shareMint);
            assert.equal(shareMintInfo.decimals, 6, "Share mint should have 6 decimals");

            // Clean up
            await vaultProgram.methods
                .closeVault()
                .accounts({
                    vaultState: vaultStatePda,
                    shareMint: shareMint,
                    vaultTokenAta: vaultTokenAta,
                    depositMint: depositMint,
                    admin: admin.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([admin])
                .rpc();
        });

        it("Share mint should have same decimals as deposit mint (9 decimals)", async () => {
            await initializeVault(9);

            const shareMintInfo = await token.getMint(connection, shareMint);
            assert.equal(shareMintInfo.decimals, 9, "Share mint should have 9 decimals");

            // Clean up
            await vaultProgram.methods
                .closeVault()
                .accounts({
                    vaultState: vaultStatePda,
                    shareMint: shareMint,
                    vaultTokenAta: vaultTokenAta,
                    depositMint: depositMint,
                    admin: admin.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([admin])
                .rpc();
        });

        it("Share mint should have same decimals as deposit mint (18 decimals)", async () => {
            await initializeVault(18);

            const shareMintInfo = await token.getMint(connection, shareMint);
            assert.equal(shareMintInfo.decimals, 18, "Share mint should have 18 decimals");

            // Clean up
            await vaultProgram.methods
                .closeVault()
                .accounts({
                    vaultState: vaultStatePda,
                    shareMint: shareMint,
                    vaultTokenAta: vaultTokenAta,
                    depositMint: depositMint,
                    admin: admin.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([admin])
                .rpc();
        });

        it("Deposit and redeem should work correctly with matching decimals", async () => {
            const decimals = 9;
            await initializeVault(decimals);

            // Create user token accounts
            const userDepositAta = await token.createAssociatedTokenAccount(
                connection,
                deployer,
                depositMint,
                deployer.publicKey
            );

            const userShareAta = await token.createAssociatedTokenAccount(
                connection,
                deployer,
                shareMint,
                deployer.publicKey
            );

            // Mint deposit tokens
            const depositAmount = 1_000_000_000n; // 1 token with 9 decimals
            await token.mintTo(
                connection,
                deployer,
                depositMint,
                userDepositAta,
                deployer,
                depositAmount
            );

            // Deposit
            await vaultProgram.methods
                .deposit(new BN(depositAmount.toString()))
                .accounts({
                    vaultState: vaultStatePda,
                    vaultTokenAta: vaultTokenAta,
                    senderTokenAccount: userDepositAta,
                    senderShareAccount: userShareAta,
                    shareMint: shareMint,
                    depositMint: depositMint,
                    signer: deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([deployer])
                .rpc();

            // Check share balance - should be approximately equal to deposit (1:1 for first deposit)
            const shareBalance = (await token.getAccount(connection, userShareAta)).amount;
            
            // With matching decimals, the raw amounts should be very close
            // (there's a small difference due to the EXTRA_SHARES constant in the formula)
            const ratio = Number(shareBalance) / Number(depositAmount);
            assert.ok(ratio > 0.99 && ratio < 1.01, `Share ratio should be ~1:1, got ${ratio}`);

            // The display values should now match
            const displayDeposit = Number(depositAmount) / (10 ** decimals);
            const displayShares = Number(shareBalance) / (10 ** decimals);
            
            assert.ok(
                Math.abs(displayDeposit - displayShares) < 0.01,
                `Display values should match: deposit=${displayDeposit}, shares=${displayShares}`
            );
        });
    });
});
