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
import { PublicKey, LAMPORTS_PER_SOL, Connection, Keypair } from "@solana/web3.js";
import { sha256 } from "js-sha256";
import * as token from "@solana/spl-token";
import { getOrCreateAssociatedTokenAccount } from "@solana/spl-token";
import * as assert from "assert";
import { protocolAuthority, ensureProgramConfig } from "./helper/program-config";

/**
 * Helper: Derive all vault PDAs from a deposit mint and version
 */
function deriveVaultPdas(depositMint: PublicKey, programId: PublicKey, vaultVersion: number = 0) {
    const [statePda] = PublicKey.findProgramAddressSync(
        [Buffer.from("VAULT_STATE"), depositMint.toBuffer(), Buffer.from([vaultVersion])],
        programId
    );
    const [shareMint] = PublicKey.findProgramAddressSync(
        [Buffer.from("mint"), depositMint.toBuffer(), Buffer.from([vaultVersion])],
        programId
    );
    const [tokenAta] = PublicKey.findProgramAddressSync(
        [Buffer.from("token_vault"), depositMint.toBuffer(), Buffer.from([vaultVersion])],
        programId
    );
    const [nominatedAdminPda] = PublicKey.findProgramAddressSync(
        [Buffer.from("nominated_admin"), depositMint.toBuffer(), Buffer.from([vaultVersion])],
        programId
    );
    return { statePda, shareMint, tokenAta, nominatedAdminPda };
}

/**
 * Helper: Create multiple ATAs for a given owner
 */
async function createAtasForOwner(
    connection: Connection,
    payer: Keypair,
    mints: PublicKey[],
    owner: PublicKey
): Promise<PublicKey[]> {
    const atas: PublicKey[] = [];
    for (const mint of mints) {
        const ata = await getOrCreateAssociatedTokenAccount(connection, payer, mint, owner);
        atas.push(ata.address);
    }
    return atas;
}

/**
 * Multi-Vault Test Suite
 *
 * Tests the ability to create multiple vaults per program deployment
 * using different deposit tokens. Each vault is uniquely identified by
 * its deposit_mint in the PDA seeds.
 */
describe("multi-vault", () => {
    anchor.setProvider(anchor.AnchorProvider.env());
    const connection = anchor.getProvider().connection;
    const vaultProgram = anchor.workspace.augustVault as Program<AugustVault>;

    // Shared wallets for all vaults
    let deployer: Keypair;
    let admin: Keypair;
    let operator: Keypair;
    let feeRecipient: Keypair;

    // Vault 1: USDC-like token
    let usdcMint: PublicKey;
    let usdcMintKeypair: Keypair;
    let vault1StatePda: PublicKey;
    let vault1ShareMint: PublicKey;
    let vault1TokenAta: PublicKey;
    let vault1NominatedAdminPda: PublicKey;

    // Vault 2: USDT-like token
    let usdtMint: PublicKey;
    let usdtMintKeypair: Keypair;
    let vault2StatePda: PublicKey;
    let vault2ShareMint: PublicKey;
    let vault2TokenAta: PublicKey;
    let vault2NominatedAdminPda: PublicKey;

    // Vault 3: SOL-wrapped token (9 decimals)
    let wsolMint: PublicKey;
    let wsolMintKeypair: Keypair;
    let vault3StatePda: PublicKey;
    let vault3ShareMint: PublicKey;
    let vault3TokenAta: PublicKey;

    before(async () => {
        // Vault creation is gated on the ProgramConfig authority; whichever
        // suite runs first bootstraps it for the shared validator.
        await ensureProgramConfig(vaultProgram);
        // Initialize deployer and other wallets
        deployer = Keypair.fromSeed(Uint8Array.from(sha256.digest("multiVaultDeployer")));
        admin = Keypair.fromSeed(Uint8Array.from(sha256.digest("multiVaultAdmin")));
        operator = Keypair.fromSeed(Uint8Array.from(sha256.digest("multiVaultOperator")));
        feeRecipient = Keypair.fromSeed(Uint8Array.from(sha256.digest("multiVaultFeeRecipient")));

        // Initialize mint keypairs (deterministic for consistent addresses)
        usdcMintKeypair = Keypair.fromSeed(Uint8Array.from(sha256.digest("usdcMint")));
        usdtMintKeypair = Keypair.fromSeed(Uint8Array.from(sha256.digest("usdtMint")));
        wsolMintKeypair = Keypair.fromSeed(Uint8Array.from(sha256.digest("wsolMint")));

        // Derive PDAs for all three vaults using helper function
        const vault1Pdas = deriveVaultPdas(usdcMintKeypair.publicKey, vaultProgram.programId);
        vault1StatePda = vault1Pdas.statePda;
        vault1ShareMint = vault1Pdas.shareMint;
        vault1TokenAta = vault1Pdas.tokenAta;
        vault1NominatedAdminPda = vault1Pdas.nominatedAdminPda;

        const vault2Pdas = deriveVaultPdas(usdtMintKeypair.publicKey, vaultProgram.programId);
        vault2StatePda = vault2Pdas.statePda;
        vault2ShareMint = vault2Pdas.shareMint;
        vault2TokenAta = vault2Pdas.tokenAta;
        vault2NominatedAdminPda = vault2Pdas.nominatedAdminPda;

        const vault3Pdas = deriveVaultPdas(wsolMintKeypair.publicKey, vaultProgram.programId);
        vault3StatePda = vault3Pdas.statePda;
        vault3ShareMint = vault3Pdas.shareMint;
        vault3TokenAta = vault3Pdas.tokenAta;

        // Airdrop SOL to deployer and operator
        const sig0 = await connection.requestAirdrop(deployer.publicKey, 100 * LAMPORTS_PER_SOL);
        const sig1 = await connection.requestAirdrop(operator.publicKey, 100 * LAMPORTS_PER_SOL);
        const latestBlockHash = await connection.getLatestBlockhash();
        await connection.confirmTransaction({
            blockhash: latestBlockHash.blockhash,
            lastValidBlockHeight: latestBlockHash.lastValidBlockHeight,
            signature: sig0,
        });
        await connection.confirmTransaction({
            blockhash: latestBlockHash.blockhash,
            lastValidBlockHeight: latestBlockHash.lastValidBlockHeight,
            signature: sig1,
        });

        // Create the three different token mints
        usdcMint = await token.createMint(
            connection,
            deployer,
            deployer.publicKey,
            deployer.publicKey,
            6, // USDC has 6 decimals
            usdcMintKeypair
        );

        usdtMint = await token.createMint(
            connection,
            deployer,
            deployer.publicKey,
            deployer.publicKey,
            6, // USDT has 6 decimals
            usdtMintKeypair
        );

        wsolMint = await token.createMint(
            connection,
            deployer,
            deployer.publicKey,
            deployer.publicKey,
            9, // WSOL has 9 decimals
            wsolMintKeypair
        );
    });

    describe("Vault Creation", () => {
        it("Can initialize Vault 1 (USDC)", async () => {
            const vaultVersion = 0;
            await vaultProgram.methods
                .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey, vaultVersion)
                .accounts({
                    vaultState: vault1StatePda,
                    shareMint: vault1ShareMint,
                    vaultTokenAta: vault1TokenAta,
                    depositMint: usdcMint,
                    signer: protocolAuthority.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([protocolAuthority])
                .rpc();

            const vault = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            assert.deepEqual(vault.depositMint, usdcMint);
            assert.deepEqual(vault.shareMint, vault1ShareMint);
            assert.deepEqual(vault.admin, admin.publicKey);
            assert.deepEqual(vault.operator, operator.publicKey);
            assert.equal(vault.paused, false);
        });

        it("Can initialize Vault 2 (USDT) - same program, different deposit token", async () => {
            const vaultVersion = 0;
            await vaultProgram.methods
                .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey, vaultVersion)
                .accounts({
                    vaultState: vault2StatePda,
                    shareMint: vault2ShareMint,
                    vaultTokenAta: vault2TokenAta,
                    depositMint: usdtMint,
                    signer: protocolAuthority.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([protocolAuthority])
                .rpc();

            const vault = await vaultProgram.account.vaultState.fetch(vault2StatePda);
            assert.deepEqual(vault.depositMint, usdtMint);
            assert.deepEqual(vault.shareMint, vault2ShareMint);
            assert.deepEqual(vault.admin, admin.publicKey);
            assert.deepEqual(vault.operator, operator.publicKey);
        });

        it("Can initialize Vault 3 (WSOL) - third vault with 9 decimals", async () => {
            const vaultVersion = 0;
            await vaultProgram.methods
                .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey, vaultVersion)
                .accounts({
                    vaultState: vault3StatePda,
                    shareMint: vault3ShareMint,
                    vaultTokenAta: vault3TokenAta,
                    depositMint: wsolMint,
                    signer: protocolAuthority.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([protocolAuthority])
                .rpc();

            const vault = await vaultProgram.account.vaultState.fetch(vault3StatePda);
            assert.deepEqual(vault.depositMint, wsolMint);
            assert.deepEqual(vault.shareMint, vault3ShareMint);
        });

        it("All three vaults have different PDAs", async () => {
            assert.notDeepEqual(vault1StatePda, vault2StatePda);
            assert.notDeepEqual(vault2StatePda, vault3StatePda);
            assert.notDeepEqual(vault1StatePda, vault3StatePda);

            // Share mints are also different
            assert.notDeepEqual(vault1ShareMint, vault2ShareMint);
            assert.notDeepEqual(vault2ShareMint, vault3ShareMint);
        });

        it("Cannot initialize the same vault twice (same deposit token and version)", async () => {
            const vaultVersion = 0;
            await assert.rejects(
                vaultProgram.methods
                    .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey, vaultVersion)
                    .accounts({
                        vaultState: vault1StatePda,
                        shareMint: vault1ShareMint,
                        vaultTokenAta: vault1TokenAta,
                        depositMint: usdcMint,
                        signer: protocolAuthority.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([protocolAuthority])
                    .rpc(),
                /0x0/ // already initialized
            );
        });
    });

    describe("Independent Vault Operations", () => {
        let deployerUsdcAta: PublicKey;
        let deployerUsdtAta: PublicKey;
        let deployerWsolAta: PublicKey;
        let deployerVault1ShareAta: PublicKey;
        let deployerVault2ShareAta: PublicKey;
        let deployerVault3ShareAta: PublicKey;
        let feeRecipientUsdcAta: PublicKey;
        let feeRecipientUsdtAta: PublicKey;
        let feeRecipientWsolAta: PublicKey;

        before(async () => {
            // Create ATAs for depositing (deposit mints for deployer)
            [deployerUsdcAta, deployerUsdtAta, deployerWsolAta] = await createAtasForOwner(
                connection, deployer, [usdcMint, usdtMint, wsolMint], deployer.publicKey
            );

            // Create ATAs for share tokens (share mints for deployer)
            [deployerVault1ShareAta, deployerVault2ShareAta, deployerVault3ShareAta] = await createAtasForOwner(
                connection, deployer, [vault1ShareMint, vault2ShareMint, vault3ShareMint], deployer.publicKey
            );

            // Create fee recipient ATAs (deposit mints for fee recipient)
            [feeRecipientUsdcAta, feeRecipientUsdtAta, feeRecipientWsolAta] = await createAtasForOwner(
                connection, deployer, [usdcMint, usdtMint, wsolMint], feeRecipient.publicKey
            );

            // Mint tokens to deployer for deposits
            await token.mintTo(
                connection, deployer, usdcMint, deployerUsdcAta, deployer, 10_000_000_000 // 10,000 USDC
            );
            await token.mintTo(
                connection, deployer, usdtMint, deployerUsdtAta, deployer, 10_000_000_000 // 10,000 USDT
            );
            await token.mintTo(
                connection, deployer, wsolMint, deployerWsolAta, deployer, 10_000_000_000_000 // 10,000 WSOL
            );
        });

        it("Can deposit to Vault 1 (USDC)", async () => {
            const depositAmount = 1_000_000; // 1 USDC (6 decimals)

            await vaultProgram.methods
                .deposit(new anchor.BN(depositAmount))
                .accounts({
                    vaultState: vault1StatePda,
                    vaultTokenAta: vault1TokenAta,
                    senderTokenAccount: deployerUsdcAta,
                    senderShareAccount: deployerVault1ShareAta,
                    shareMint: vault1ShareMint,
                    depositMint: usdcMint,
                    signer: deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([deployer])
                .rpc();

            const vault = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            assert.equal(vault.localAum.toNumber(), depositAmount);
        });

        it("Can deposit to Vault 2 (USDT) - independent from Vault 1", async () => {
            const depositAmount = 2_000_000; // 2 USDT (6 decimals)

            await vaultProgram.methods
                .deposit(new anchor.BN(depositAmount))
                .accounts({
                    vaultState: vault2StatePda,
                    vaultTokenAta: vault2TokenAta,
                    senderTokenAccount: deployerUsdtAta,
                    senderShareAccount: deployerVault2ShareAta,
                    shareMint: vault2ShareMint,
                    depositMint: usdtMint,
                    signer: deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([deployer])
                .rpc();

            const vault1 = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            const vault2 = await vaultProgram.account.vaultState.fetch(vault2StatePda);

            // Vault 1 AUM unchanged
            assert.equal(vault1.localAum.toNumber(), 1_000_000);
            // Vault 2 has its own AUM
            assert.equal(vault2.localAum.toNumber(), 2_000_000);
        });

        it("Can deposit to Vault 3 (WSOL) - different decimals", async () => {
            const depositAmount = 1_000_000_000; // 1 WSOL (9 decimals)

            await vaultProgram.methods
                .deposit(new anchor.BN(depositAmount))
                .accounts({
                    vaultState: vault3StatePda,
                    vaultTokenAta: vault3TokenAta,
                    senderTokenAccount: deployerWsolAta,
                    senderShareAccount: deployerVault3ShareAta,
                    shareMint: vault3ShareMint,
                    depositMint: wsolMint,
                    signer: deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([deployer])
                .rpc();

            const vault3 = await vaultProgram.account.vaultState.fetch(vault3StatePda);
            assert.equal(vault3.localAum.toNumber(), depositAmount);
        });

        it("Vaults maintain independent state", async () => {
            const vault1 = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            const vault2 = await vaultProgram.account.vaultState.fetch(vault2StatePda);
            const vault3 = await vaultProgram.account.vaultState.fetch(vault3StatePda);

            // Each vault has its own deposit mint
            assert.deepEqual(vault1.depositMint, usdcMint);
            assert.deepEqual(vault2.depositMint, usdtMint);
            assert.deepEqual(vault3.depositMint, wsolMint);

            // Each vault has its own share mint
            assert.deepEqual(vault1.shareMint, vault1ShareMint);
            assert.deepEqual(vault2.shareMint, vault2ShareMint);
            assert.deepEqual(vault3.shareMint, vault3ShareMint);

            // Each vault has independent AUM
            assert.equal(vault1.localAum.toNumber(), 1_000_000);
            assert.equal(vault2.localAum.toNumber(), 2_000_000);
            assert.equal(vault3.localAum.toNumber(), 1_000_000_000);
        });

        it("Can pause Vault 1 without affecting Vault 2 or Vault 3", async () => {
            await vaultProgram.methods
                .pause()
                .accounts({
                    vaultState: vault1StatePda,
                    depositMint: usdcMint,
                    admin: admin.publicKey,
                })
                .signers([admin])
                .rpc();

            const vault1 = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            const vault2 = await vaultProgram.account.vaultState.fetch(vault2StatePda);
            const vault3 = await vaultProgram.account.vaultState.fetch(vault3StatePda);

            assert.equal(vault1.paused, true);
            assert.equal(vault2.paused, false);
            assert.equal(vault3.paused, false);
        });

        it("Cannot deposit to paused Vault 1, but can still deposit to Vault 2", async () => {
            // Try to deposit to paused Vault 1 - should fail
            await assert.rejects(
                vaultProgram.methods
                    .deposit(new anchor.BN(1_000_000))
                    .accounts({
                        vaultState: vault1StatePda,
                        vaultTokenAta: vault1TokenAta,
                        senderTokenAccount: deployerUsdcAta,
                        senderShareAccount: deployerVault1ShareAta,
                        shareMint: vault1ShareMint,
                        depositMint: usdcMint,
                        signer: deployer.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([deployer])
                    .rpc(),
                /VaultPaused/
            );

            // Can still deposit to Vault 2
            const vault2Before = await vaultProgram.account.vaultState.fetch(vault2StatePda);
            await vaultProgram.methods
                .deposit(new anchor.BN(1_000_000))
                .accounts({
                    vaultState: vault2StatePda,
                    vaultTokenAta: vault2TokenAta,
                    senderTokenAccount: deployerUsdtAta,
                    senderShareAccount: deployerVault2ShareAta,
                    shareMint: vault2ShareMint,
                    depositMint: usdtMint,
                    signer: deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([deployer])
                .rpc();

            const vault2After = await vaultProgram.account.vaultState.fetch(vault2StatePda);
            assert.equal(vault2After.localAum.toNumber(), vault2Before.localAum.toNumber() + 1_000_000);
        });

        it("Can unpause Vault 1", async () => {
            await vaultProgram.methods
                .unpause()
                .accounts({
                    vaultState: vault1StatePda,
                    depositMint: usdcMint,
                    admin: admin.publicKey,
                })
                .signers([admin])
                .rpc();

            const vault1 = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            assert.equal(vault1.paused, false);
        });

        it("Can redeem from Vault 2", async () => {
            const shareBalance = await token.getAccount(connection, deployerVault2ShareAta);
            const redeemShares = shareBalance.amount / 2n;

            await vaultProgram.methods
                .redeem(new anchor.BN(redeemShares.toString()))
                .accounts({
                    vaultState: vault2StatePda,
                    vaultDepositAta: vault2TokenAta,
                    senderTokenAccount: deployerUsdtAta,
                    senderShareAccount: deployerVault2ShareAta,
                    feeRecipientAccount: feeRecipientUsdtAta,
                    shareMint: vault2ShareMint,
                    depositMint: usdtMint,
                    signer: deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([deployer])
                .rpc();

            // Verify redemption occurred
            const newShareBalance = await token.getAccount(connection, deployerVault2ShareAta);
            assert.ok(newShareBalance.amount < shareBalance.amount);
        });
    });

    describe("Admin Nomination Per Vault", () => {
        let newAdmin: Keypair;

        before(async () => {
            newAdmin = Keypair.fromSeed(Uint8Array.from(sha256.digest("newMultiVaultAdmin")));
            const sig = await connection.requestAirdrop(newAdmin.publicKey, 10 * LAMPORTS_PER_SOL);
            const latestBlockHash = await connection.getLatestBlockhash();
            await connection.confirmTransaction({
                blockhash: latestBlockHash.blockhash,
                lastValidBlockHeight: latestBlockHash.lastValidBlockHeight,
                signature: sig,
            });
        });

        it("Can nominate admin for Vault 1 without affecting Vault 2", async () => {
            await vaultProgram.methods
                .nominateAdmin(newAdmin.publicKey)
                .accounts({
                    vaultState: vault1StatePda,
                    depositMint: usdcMint,
                    nominatedAdminPda: vault1NominatedAdminPda,
                    admin: admin.publicKey,
                    payer: deployer.publicKey,
                })
                .signers([admin, deployer])
                .rpc();

            // Vault 1 has a nomination
            const vault1NomAdmin = await vaultProgram.account.nominatedAdmin.fetch(vault1NominatedAdminPda);
            assert.deepEqual(vault1NomAdmin.nominatedAdmin, newAdmin.publicKey);

            // Vault 2 nomination PDA does not exist
            const vault2NomAdmin = await vaultProgram.account.nominatedAdmin.fetchNullable(vault2NominatedAdminPda);
            assert.equal(vault2NomAdmin, null);
        });
    });

    describe("Cross-Vault Security", () => {
        let deployerUsdcAta: PublicKey;
        let deployerUsdtAta: PublicKey;
        let deployerVault1ShareAta: PublicKey;
        let deployerVault2ShareAta: PublicKey;
        let feeRecipientUsdtAta: PublicKey;
        let operatorUsdcAta: PublicKey;
        let operatorUsdtAta: PublicKey;

        before(async () => {
            // Deployer ATAs for deposit mints and share mints
            [deployerUsdcAta, deployerUsdtAta] = await createAtasForOwner(
                connection, deployer, [usdcMint, usdtMint], deployer.publicKey
            );
            [deployerVault1ShareAta, deployerVault2ShareAta] = await createAtasForOwner(
                connection, deployer, [vault1ShareMint, vault2ShareMint], deployer.publicKey
            );

            // Fee recipient and operator ATAs
            [feeRecipientUsdtAta] = await createAtasForOwner(
                connection, deployer, [usdtMint], feeRecipient.publicKey
            );
            [operatorUsdcAta, operatorUsdtAta] = await createAtasForOwner(
                connection, deployer, [usdcMint, usdtMint], operator.publicKey
            );
        });

        it("Cannot deposit with wrong deposit_mint (mismatched accounts rejected)", async () => {
            // Try to use Vault 1's state PDA with Vault 2's deposit mint - should fail
            await assert.rejects(
                vaultProgram.methods
                    .deposit(new anchor.BN(1_000_000))
                    .accounts({
                        vaultState: vault1StatePda, // Vault 1's PDA
                        vaultTokenAta: vault1TokenAta,
                        senderTokenAccount: deployerUsdtAta, // USDT token (Vault 2's token)
                        senderShareAccount: deployerVault1ShareAta,
                        shareMint: vault1ShareMint,
                        depositMint: usdtMint, // Wrong mint for Vault 1
                        signer: deployer.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([deployer])
                    .rpc(),
                // Should fail due to seed mismatch (ConstraintSeeds) or WrongMint constraint
                /ConstraintSeeds|WrongMint|A seeds constraint was violated/
            );
        });

        it("Cannot redeem with wrong deposit_mint", async () => {
            // Try to redeem from Vault 1 using Vault 2's deposit mint
            await assert.rejects(
                vaultProgram.methods
                    .redeem(new anchor.BN(1_000))
                    .accounts({
                        vaultState: vault1StatePda, // Vault 1's PDA
                        vaultDepositAta: vault1TokenAta,
                        senderTokenAccount: deployerUsdtAta, // Wrong token
                        senderShareAccount: deployerVault1ShareAta,
                        feeRecipientAccount: feeRecipientUsdtAta,
                        shareMint: vault1ShareMint,
                        depositMint: usdtMint, // Wrong mint for Vault 1
                        signer: deployer.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([deployer])
                    .rpc(),
                /ConstraintSeeds|WrongMint|A seeds constraint was violated/
            );
        });

        it("Cannot pause Vault 1 with wrong deposit_mint", async () => {
            await assert.rejects(
                vaultProgram.methods
                    .pause()
                    .accounts({
                        vaultState: vault1StatePda,
                        depositMint: usdtMint, // Wrong mint for Vault 1
                        admin: admin.publicKey,
                    })
                    .signers([admin])
                    .rpc(),
                /ConstraintSeeds|WrongMint|A seeds constraint was violated/
            );
        });

        it("Cannot unpause Vault 1 with wrong deposit_mint", async () => {
            // First pause Vault 1 correctly
            const vault1State = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            if (!vault1State.paused) {
                await vaultProgram.methods
                    .pause()
                    .accounts({
                        vaultState: vault1StatePda,
                        depositMint: usdcMint,
                        admin: admin.publicKey,
                    })
                    .signers([admin])
                    .rpc();
            }

            // Try to unpause with wrong deposit mint
            await assert.rejects(
                vaultProgram.methods
                    .unpause()
                    .accounts({
                        vaultState: vault1StatePda,
                        depositMint: usdtMint, // Wrong mint
                        admin: admin.publicKey,
                    })
                    .signers([admin])
                    .rpc(),
                /ConstraintSeeds|WrongMint|A seeds constraint was violated/
            );

            // Unpause correctly for subsequent tests
            await vaultProgram.methods
                .unpause()
                .accounts({
                    vaultState: vault1StatePda,
                    depositMint: usdcMint,
                    admin: admin.publicKey,
                })
                .signers([admin])
                .rpc();
        });

        it("Cannot setOperator on Vault 1 with wrong deposit_mint", async () => {
            const randomOperator = Keypair.generate();
            await assert.rejects(
                vaultProgram.methods
                    .setOperator(randomOperator.publicKey)
                    .accounts({
                        vaultState: vault1StatePda,
                        depositMint: usdtMint, // Wrong mint for Vault 1
                        admin: admin.publicKey,
                    })
                    .signers([admin])
                    .rpc(),
                /ConstraintSeeds|WrongMint|A seeds constraint was violated/
            );
        });

        it("Cannot setFeeRecipient on Vault 1 with wrong deposit_mint", async () => {
            const randomRecipient = Keypair.generate();
            await assert.rejects(
                vaultProgram.methods
                    .setFeeRecipient(randomRecipient.publicKey)
                    .accounts({
                        vaultState: vault1StatePda,
                        depositMint: usdtMint, // Wrong mint for Vault 1
                        admin: admin.publicKey,
                    })
                    .signers([admin])
                    .rpc(),
                /ConstraintSeeds|WrongMint|A seeds constraint was violated/
            );
        });

        it("Cannot setWithdrawalFee on Vault 1 with wrong deposit_mint", async () => {
            await assert.rejects(
                vaultProgram.methods
                    .setWithdrawalFee(100) // 1% fee
                    .accounts({
                        vaultState: vault1StatePda,
                        depositMint: usdtMint, // Wrong mint for Vault 1
                        admin: admin.publicKey,
                    })
                    .signers([admin])
                    .rpc(),
                /ConstraintSeeds|WrongMint|A seeds constraint was violated/
            );
        });

        it("Cannot setAumLimits on Vault 1 with wrong deposit_mint", async () => {
            await assert.rejects(
                vaultProgram.methods
                    .setAumLimits(1000, 1000) // 10% limits
                    .accounts({
                        vaultState: vault1StatePda,
                        depositMint: usdtMint, // Wrong mint for Vault 1
                        admin: admin.publicKey,
                    })
                    .signers([admin])
                    .rpc(),
                /ConstraintSeeds|WrongMint|A seeds constraint was violated/
            );
        });

        it("Cannot operatorUpdateAum on Vault 1 with wrong deposit_mint", async () => {
            const vault1 = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            await assert.rejects(
                vaultProgram.methods
                    .operatorUpdateAum(vault1.localAum) // Same AUM, just testing access
                    .accounts({
                        vaultState: vault1StatePda,
                        depositMint: usdtMint, // Wrong mint for Vault 1
                        operator: operator.publicKey,
                    })
                    .signers([operator])
                    .rpc(),
                /ConstraintSeeds|WrongMint|A seeds constraint was violated/
            );
        });

        it("Cannot operatorWithdraw from Vault 1 with wrong deposit_mint", async () => {
            await assert.rejects(
                vaultProgram.methods
                    .operatorWithdraw(new anchor.BN(1_000))
                    .accounts({
                        vaultState: vault1StatePda,
                        vaultDepositAta: vault1TokenAta,
                        operatorTokenAccount: operatorUsdtAta, // Wrong token
                        depositMint: usdtMint, // Wrong mint for Vault 1
                        operator: operator.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([operator])
                    .rpc(),
                /ConstraintSeeds|WrongMint|A seeds constraint was violated/
            );
        });

        it("Cannot operatorDeposit to Vault 1 with wrong deposit_mint", async () => {
            // Mint some tokens to operator for this test
            await token.mintTo(
                connection, deployer, usdtMint, operatorUsdtAta, deployer, 1_000_000
            );

            await assert.rejects(
                vaultProgram.methods
                    .operatorDeposit(new anchor.BN(1_000))
                    .accounts({
                        vaultState: vault1StatePda,
                        vaultDepositAta: vault1TokenAta,
                        operatorTokenAccount: operatorUsdtAta, // Wrong token
                        depositMint: usdtMint, // Wrong mint for Vault 1
                        operator: operator.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([operator])
                    .rpc(),
                /ConstraintSeeds|WrongMint|A seeds constraint was violated/
            );
        });

        it("Cannot nominateAdmin on Vault 1 with wrong deposit_mint", async () => {
            const randomAdmin = Keypair.generate();
            await assert.rejects(
                vaultProgram.methods
                    .nominateAdmin(randomAdmin.publicKey)
                    .accounts({
                        vaultState: vault1StatePda,
                        depositMint: usdtMint, // Wrong mint for Vault 1
                        nominatedAdminPda: vault1NominatedAdminPda,
                        admin: admin.publicKey,
                        payer: deployer.publicKey,
                    })
                    .signers([admin, deployer])
                    .rpc(),
                /ConstraintSeeds|WrongMint|A seeds constraint was violated/
            );
        });

        it("Cannot acceptAdminNomination on Vault 1 with wrong deposit_mint", async () => {
            // Use the existing nomination from previous test
            const nomination = await vaultProgram.account.nominatedAdmin.fetchNullable(vault1NominatedAdminPda);
            if (!nomination) {
                // Skip if no nomination exists
                console.log("Skipping: No nomination exists for Vault 1");
                return;
            }

            // We need the nominated admin's keypair to sign
            const nominatedAdminKeypair = Keypair.fromSeed(Uint8Array.from(sha256.digest("newMultiVaultAdmin")));

            await assert.rejects(
                vaultProgram.methods
                    .acceptAdminNomination()
                    .accounts({
                        vaultState: vault1StatePda,
                        depositMint: usdtMint, // Wrong mint for Vault 1
                        nominatedAdminPda: vault1NominatedAdminPda,
                        newAdmin: nominatedAdminKeypair.publicKey,
                        receiver: deployer.publicKey,
                    })
                    .signers([nominatedAdminKeypair])
                    .rpc(),
                /ConstraintSeeds|WrongMint|A seeds constraint was violated/
            );
        });
    });

    describe("Operator Operations Per Vault", () => {
        let operatorUsdcAta: PublicKey;
        let operatorUsdtAta: PublicKey;

        before(async () => {
            operatorUsdcAta = (await getOrCreateAssociatedTokenAccount(
                connection, deployer, usdcMint, operator.publicKey
            )).address;
            operatorUsdtAta = (await getOrCreateAssociatedTokenAccount(
                connection, deployer, usdtMint, operator.publicKey
            )).address;

            // Mint tokens to operator for deposit tests
            await token.mintTo(
                connection, deployer, usdcMint, operatorUsdcAta, deployer, 10_000_000
            );
            await token.mintTo(
                connection, deployer, usdtMint, operatorUsdtAta, deployer, 10_000_000
            );
        });

        it("Can operatorWithdraw from Vault 1", async () => {
            const vault1Before = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            const withdrawAmount = Math.min(vault1Before.localAum.toNumber(), 100_000);

            if (withdrawAmount > 0) {
                await vaultProgram.methods
                    .operatorWithdraw(new anchor.BN(withdrawAmount))
                    .accounts({
                        vaultState: vault1StatePda,
                        vaultDepositAta: vault1TokenAta,
                        operatorTokenAccount: operatorUsdcAta,
                        depositMint: usdcMint,
                        operator: operator.publicKey,
                        tokenProgram: token.TOKEN_PROGRAM_ID,
                    })
                    .signers([operator])
                    .rpc();

                const vault1After = await vaultProgram.account.vaultState.fetch(vault1StatePda);
                // Verify local AUM decreased and deployed AUM increased
                assert.ok(vault1After.localAum.toNumber() < vault1Before.localAum.toNumber());
                assert.ok(vault1After.deployedAum.toNumber() > vault1Before.deployedAum.toNumber());
            }
        });

        it("Can operatorDeposit to Vault 1", async () => {
            const vault1Before = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            const depositAmount = 50_000;

            await vaultProgram.methods
                .operatorDeposit(new anchor.BN(depositAmount))
                .accounts({
                    vaultState: vault1StatePda,
                    vaultDepositAta: vault1TokenAta,
                    operatorTokenAccount: operatorUsdcAta,
                    depositMint: usdcMint,
                    operator: operator.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([operator])
                .rpc();

            const vault1After = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            assert.equal(
                vault1After.localAum.toNumber(),
                vault1Before.localAum.toNumber() + depositAmount
            );
        });

        it("Can operatorUpdateAum on Vault 1 without affecting Vault 2", async () => {
            const vault1Before = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            const vault2Before = await vaultProgram.account.vaultState.fetch(vault2StatePda);

            // Update Vault 1's AUM (within limits)
            const newAum = vault1Before.deployedAum;

            await vaultProgram.methods
                .operatorUpdateAum(newAum)
                .accounts({
                    vaultState: vault1StatePda,
                    depositMint: usdcMint,
                    operator: operator.publicKey,
                })
                .signers([operator])
                .rpc();

            const vault2After = await vaultProgram.account.vaultState.fetch(vault2StatePda);
            // Vault 2's AUM should be unchanged
            assert.equal(vault2After.localAum.toNumber(), vault2Before.localAum.toNumber());
            assert.equal(vault2After.deployedAum.toNumber(), vault2Before.deployedAum.toNumber());
        });

        it("Vault 1 operator operations don't affect Vault 2", async () => {
            const vault2Before = await vaultProgram.account.vaultState.fetch(vault2StatePda);

            // Do operator withdraw on Vault 1
            await vaultProgram.methods
                .operatorWithdraw(new anchor.BN(10_000))
                .accounts({
                    vaultState: vault1StatePda,
                    vaultDepositAta: vault1TokenAta,
                    operatorTokenAccount: operatorUsdcAta,
                    depositMint: usdcMint,
                    operator: operator.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID,
                })
                .signers([operator])
                .rpc();

            // Verify Vault 2 is unaffected
            const vault2After = await vaultProgram.account.vaultState.fetch(vault2StatePda);
            assert.equal(vault2After.localAum.toNumber(), vault2Before.localAum.toNumber());
            assert.equal(vault2After.deployedAum.toNumber(), vault2Before.deployedAum.toNumber());
        });
    });

    describe("Complete Admin Nomination Acceptance Flow", () => {
        let newAdmin: Keypair;

        before(async () => {
            newAdmin = Keypair.fromSeed(Uint8Array.from(sha256.digest("completeFlowNewAdmin")));
            const sig = await connection.requestAirdrop(newAdmin.publicKey, 10 * LAMPORTS_PER_SOL);
            const latestBlockHash = await connection.getLatestBlockhash();
            await connection.confirmTransaction({
                blockhash: latestBlockHash.blockhash,
                lastValidBlockHeight: latestBlockHash.lastValidBlockHeight,
                signature: sig,
            });
        });

        it("Complete admin transfer flow for Vault 2", async () => {
            // Step 1: Nominate new admin for Vault 2
            await vaultProgram.methods
                .nominateAdmin(newAdmin.publicKey)
                .accounts({
                    vaultState: vault2StatePda,
                    depositMint: usdtMint,
                    nominatedAdminPda: vault2NominatedAdminPda,
                    admin: admin.publicKey,
                    payer: deployer.publicKey,
                })
                .signers([admin, deployer])
                .rpc();

            // Verify nomination exists
            const nomination = await vaultProgram.account.nominatedAdmin.fetch(vault2NominatedAdminPda);
            assert.deepEqual(nomination.nominatedAdmin, newAdmin.publicKey);

            // Step 2: Accept the nomination
            await vaultProgram.methods
                .acceptAdminNomination()
                .accounts({
                    vaultState: vault2StatePda,
                    depositMint: usdtMint,
                    nominatedAdminPda: vault2NominatedAdminPda,
                    newAdmin: newAdmin.publicKey,
                    receiver: deployer.publicKey,
                })
                .signers([newAdmin])
                .rpc();

            // Step 3: Verify new admin is set
            const vault2 = await vaultProgram.account.vaultState.fetch(vault2StatePda);
            assert.deepEqual(vault2.admin, newAdmin.publicKey);

            // Step 4: Verify old admin can no longer perform admin actions on Vault 2
            await assert.rejects(
                vaultProgram.methods
                    .pause()
                    .accounts({
                        vaultState: vault2StatePda,
                        depositMint: usdtMint,
                        admin: admin.publicKey, // Old admin
                    })
                    .signers([admin])
                    .rpc(),
                /NotAdmin|Signer must be the admin/
            );

            // Step 5: Verify new admin CAN perform admin actions on Vault 2
            await vaultProgram.methods
                .pause()
                .accounts({
                    vaultState: vault2StatePda,
                    depositMint: usdtMint,
                    admin: newAdmin.publicKey, // New admin
                })
                .signers([newAdmin])
                .rpc();

            const vault2AfterPause = await vaultProgram.account.vaultState.fetch(vault2StatePda);
            assert.equal(vault2AfterPause.paused, true);

            // Step 6: Verify new admin CANNOT perform admin actions on Vault 1
            await assert.rejects(
                vaultProgram.methods
                    .pause()
                    .accounts({
                        vaultState: vault1StatePda,
                        depositMint: usdcMint,
                        admin: newAdmin.publicKey, // New admin of Vault 2, not Vault 1
                    })
                    .signers([newAdmin])
                    .rpc(),
                /NotAdmin|Signer must be the admin/
            );

            // Unpause Vault 2 for subsequent tests
            await vaultProgram.methods
                .unpause()
                .accounts({
                    vaultState: vault2StatePda,
                    depositMint: usdtMint,
                    admin: newAdmin.publicKey,
                })
                .signers([newAdmin])
                .rpc();
        });

        it("Vault 1 admin nomination is independent from Vault 2", async () => {
            // Vault 1 should still have original admin
            const vault1 = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            assert.deepEqual(vault1.admin, admin.publicKey);

            // Original admin can still operate Vault 1
            await vaultProgram.methods
                .pause()
                .accounts({
                    vaultState: vault1StatePda,
                    depositMint: usdcMint,
                    admin: admin.publicKey,
                })
                .signers([admin])
                .rpc();

            const vault1AfterPause = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            assert.equal(vault1AfterPause.paused, true);

            // Unpause for cleanup
            await vaultProgram.methods
                .unpause()
                .accounts({
                    vaultState: vault1StatePda,
                    depositMint: usdcMint,
                    admin: admin.publicKey,
                })
                .signers([admin])
                .rpc();
        });
    });

    describe("Different Operators Per Vault", () => {
        let vault2Operator: Keypair;
        let vault2OperatorUsdtAta: PublicKey;
        let newAdminForVault2: Keypair;

        before(async () => {
            vault2Operator = Keypair.fromSeed(Uint8Array.from(sha256.digest("vault2Operator")));
            newAdminForVault2 = Keypair.fromSeed(Uint8Array.from(sha256.digest("completeFlowNewAdmin"))); // Same as used above

            const sig = await connection.requestAirdrop(vault2Operator.publicKey, 10 * LAMPORTS_PER_SOL);
            const latestBlockHash = await connection.getLatestBlockhash();
            await connection.confirmTransaction({
                blockhash: latestBlockHash.blockhash,
                lastValidBlockHeight: latestBlockHash.lastValidBlockHeight,
                signature: sig,
            });

            vault2OperatorUsdtAta = (await getOrCreateAssociatedTokenAccount(
                connection, deployer, usdtMint, vault2Operator.publicKey
            )).address;
        });

        it("Can set different operator for Vault 2", async () => {
            // Use the new admin that was set in the previous test
            await vaultProgram.methods
                .setOperator(vault2Operator.publicKey)
                .accounts({
                    vaultState: vault2StatePda,
                    depositMint: usdtMint,
                    admin: newAdminForVault2.publicKey,
                })
                .signers([newAdminForVault2])
                .rpc();

            const vault2 = await vaultProgram.account.vaultState.fetch(vault2StatePda);
            assert.deepEqual(vault2.operator, vault2Operator.publicKey);

            // Vault 1 still has original operator
            const vault1 = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            assert.deepEqual(vault1.operator, operator.publicKey);
        });

        it("Original operator cannot operate on Vault 2 after change", async () => {
            const vault2 = await vaultProgram.account.vaultState.fetch(vault2StatePda);

            await assert.rejects(
                vaultProgram.methods
                    .operatorUpdateAum(vault2.deployedAum)
                    .accounts({
                        vaultState: vault2StatePda,
                        depositMint: usdtMint,
                        operator: operator.publicKey, // Original operator
                    })
                    .signers([operator])
                    .rpc(),
                /NotOperator|Signer must be the operator/
            );
        });

        it("New Vault 2 operator can operate on Vault 2", async () => {
            const vault2Before = await vaultProgram.account.vaultState.fetch(vault2StatePda);

            await vaultProgram.methods
                .operatorUpdateAum(vault2Before.deployedAum)
                .accounts({
                    vaultState: vault2StatePda,
                    depositMint: usdtMint,
                    operator: vault2Operator.publicKey,
                })
                .signers([vault2Operator])
                .rpc();

            // Should succeed without error
        });

        it("New Vault 2 operator cannot operate on Vault 1", async () => {
            const vault1 = await vaultProgram.account.vaultState.fetch(vault1StatePda);

            await assert.rejects(
                vaultProgram.methods
                    .operatorUpdateAum(vault1.deployedAum)
                    .accounts({
                        vaultState: vault1StatePda,
                        depositMint: usdcMint,
                        operator: vault2Operator.publicKey, // Vault 2's operator
                    })
                    .signers([vault2Operator])
                    .rpc(),
                /NotOperator|Signer must be the operator/
            );
        });

        it("Original operator can still operate on Vault 1", async () => {
            const vault1 = await vaultProgram.account.vaultState.fetch(vault1StatePda);

            await vaultProgram.methods
                .operatorUpdateAum(vault1.deployedAum)
                .accounts({
                    vaultState: vault1StatePda,
                    depositMint: usdcMint,
                    operator: operator.publicKey,
                })
                .signers([operator])
                .rpc();

            // Should succeed without error
        });
    });

    describe("Per-Vault Configuration Independence", () => {
        let newAdminForVault2: Keypair;

        before(async () => {
            newAdminForVault2 = Keypair.fromSeed(Uint8Array.from(sha256.digest("completeFlowNewAdmin")));
        });

        it("Can set different withdrawal fees per vault", async () => {
            // Set 1% fee on Vault 1
            await vaultProgram.methods
                .setWithdrawalFee(100) // 1% = 100 basis points
                .accounts({
                    vaultState: vault1StatePda,
                    depositMint: usdcMint,
                    admin: admin.publicKey,
                })
                .signers([admin])
                .rpc();

            // Set 2% fee on Vault 2
            await vaultProgram.methods
                .setWithdrawalFee(200) // 2% = 200 basis points
                .accounts({
                    vaultState: vault2StatePda,
                    depositMint: usdtMint,
                    admin: newAdminForVault2.publicKey,
                })
                .signers([newAdminForVault2])
                .rpc();

            const vault1 = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            const vault2 = await vaultProgram.account.vaultState.fetch(vault2StatePda);

            assert.equal(vault1.withdrawalFee, 100);
            assert.equal(vault2.withdrawalFee, 200);
        });

        it("Can set different AUM limits per vault", async () => {
            // Set 10% limits on Vault 1
            await vaultProgram.methods
                .setAumLimits(1000, 1000) // 10% increase/decrease limits
                .accounts({
                    vaultState: vault1StatePda,
                    depositMint: usdcMint,
                    admin: admin.publicKey,
                })
                .signers([admin])
                .rpc();

            // Set 20% limits on Vault 2
            await vaultProgram.methods
                .setAumLimits(2000, 2000) // 20% increase/decrease limits
                .accounts({
                    vaultState: vault2StatePda,
                    depositMint: usdtMint,
                    admin: newAdminForVault2.publicKey,
                })
                .signers([newAdminForVault2])
                .rpc();

            const vault1 = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            const vault2 = await vaultProgram.account.vaultState.fetch(vault2StatePda);

            assert.equal(vault1.aumIncreaseLimit, 1000);
            assert.equal(vault1.aumDecreaseLimit, 1000);
            assert.equal(vault2.aumIncreaseLimit, 2000);
            assert.equal(vault2.aumDecreaseLimit, 2000);
        });

        it("Can set different fee recipients per vault", async () => {
            const vault1FeeRecipient = Keypair.generate();
            const vault2FeeRecipient = Keypair.generate();

            await vaultProgram.methods
                .setFeeRecipient(vault1FeeRecipient.publicKey)
                .accounts({
                    vaultState: vault1StatePda,
                    depositMint: usdcMint,
                    admin: admin.publicKey,
                })
                .signers([admin])
                .rpc();

            await vaultProgram.methods
                .setFeeRecipient(vault2FeeRecipient.publicKey)
                .accounts({
                    vaultState: vault2StatePda,
                    depositMint: usdtMint,
                    admin: newAdminForVault2.publicKey,
                })
                .signers([newAdminForVault2])
                .rpc();

            const vault1 = await vaultProgram.account.vaultState.fetch(vault1StatePda);
            const vault2 = await vaultProgram.account.vaultState.fetch(vault2StatePda);

            assert.deepEqual(vault1.feeRecipient, vault1FeeRecipient.publicKey);
            assert.deepEqual(vault2.feeRecipient, vault2FeeRecipient.publicKey);
        });
    });
});
