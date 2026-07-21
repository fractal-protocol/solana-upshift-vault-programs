// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

import * as anchor from "@coral-xyz/anchor";
import {Program} from "@coral-xyz/anchor";
import {AugustVault} from "../../target/types/august_vault";
import {PublicKey, LAMPORTS_PER_SOL, Connection, Keypair, SystemProgram} from "@solana/web3.js";
import {sha256} from "js-sha256"
import * as token from "@solana/spl-token"
import {getOrCreateAssociatedTokenAccount} from "@solana/spl-token";

export class VaultContext {
    public connection: Connection
    public vaultProgram: anchor.Program<AugustVault>
    public vaultStatePda: PublicKey
    public nominatedAdminPda: PublicKey

    public deployer: Keypair
    public operator: Keypair
    public admin: Keypair
    public feeRecipient: Keypair
    public usdgMintKeyPair: Keypair

    public shareMint: PublicKey
    public usdgTokenMint: PublicKey

    public vaultUsdgAta: PublicKey
    public senderShareAta: PublicKey
    public senderUsdgAta: PublicKey
    public operatorUsdgAta: PublicKey
    public feeRecipientUsdgAta: PublicKey

    async init() {
        anchor.setProvider(anchor.AnchorProvider.env())
        this.connection = anchor.getProvider().connection

        await this.init_wallet_context()

        // Initialize program reference first
        this.vaultProgram = anchor.workspace.augustVault as Program<AugustVault>;

        // We need to know the deposit mint address to derive vault PDA
        // So we use the deterministic keypair
        const depositMintAddress = this.usdgMintKeyPair.publicKey;

        // Derive vault PDA with deposit_mint and version
        const vaultVersion = 0; // Default version for existing tests
        [this.vaultStatePda] = anchor.web3.PublicKey.findProgramAddressSync(
            [Buffer.from("VAULT_STATE"), depositMintAddress.toBuffer(), Buffer.from([vaultVersion])],
            this.vaultProgram.programId
        );

        // Derive nominated admin PDA with deposit_mint and version
        [this.nominatedAdminPda] = anchor.web3.PublicKey.findProgramAddressSync(
            [Buffer.from("nominated_admin"), depositMintAddress.toBuffer(), Buffer.from([vaultVersion])],
            this.vaultProgram.programId
        );

        if ((await this.vaultProgram.account.vaultState.fetchNullable(this.vaultStatePda)) == null)
            await this.airdrop()

        await this.init_deposit_mint()

        // Derive PDAs before initializing vault
        await this.init_token_mint_context()
        await this.init_vault_usdg_ata()

        if ((await this.vaultProgram.account.vaultState.fetchNullable(this.vaultStatePda)) == null)
            await this.init_vault()

        await this.init_sender_ata()
    }

    async init_wallet_context() {
        this.deployer = anchor.web3.Keypair.fromSeed(Uint8Array.from(sha256.digest("deployer42")));
        this.operator = anchor.web3.Keypair.fromSeed(Uint8Array.from(sha256.digest("operator")));
        this.usdgMintKeyPair = anchor.web3.Keypair.fromSeed(Uint8Array.from(sha256.digest("mintUsdg")));
        this.admin = anchor.web3.Keypair.fromSeed(Uint8Array.from(sha256.digest("admin")));
        this.feeRecipient = anchor.web3.Keypair.fromSeed(Uint8Array.from(sha256.digest("feeRecipient")));
    }

    async init_deposit_mint() {
        if ((await this.vaultProgram.account.vaultState.fetchNullable(this.vaultStatePda)) == null) {
            this.usdgTokenMint = await token.createMint(
                this.connection,
                this.deployer,
                this.deployer.publicKey,
                this.deployer.publicKey,
                9,
                this.usdgMintKeyPair
            )
        } else {
            this.usdgTokenMint = (await token.getMint(this.connection, this.usdgMintKeyPair.publicKey)).address
        }
    }

    async airdrop() {
        const sig0 = await this.connection.requestAirdrop(this.deployer.publicKey, 4200 * LAMPORTS_PER_SOL)
        const sig1 = await this.connection.requestAirdrop(this.operator.publicKey, 4200 * LAMPORTS_PER_SOL)

        const latestBlockHash = await this.connection.getLatestBlockhash();

        await this.connection.confirmTransaction({
            blockhash: latestBlockHash.blockhash,
            lastValidBlockHeight: latestBlockHash.lastValidBlockHeight,
            signature: sig0,
        });

        await this.connection.confirmTransaction({
            blockhash: latestBlockHash.blockhash,
            lastValidBlockHeight: latestBlockHash.lastValidBlockHeight,
            signature: sig1,
        });
    }

    async init_token_mint_context() {
        const vaultVersion = 0;
        [this.shareMint] = anchor.web3.PublicKey.findProgramAddressSync(
            [Buffer.from("mint"), this.usdgTokenMint.toBuffer(), Buffer.from([vaultVersion])],
            this.vaultProgram.programId,
        );
    }

    async init_vault_usdg_ata() {
        const vaultVersion = 0;
        [this.vaultUsdgAta] = anchor.web3.PublicKey.findProgramAddressSync(
            [
                Buffer.from("token_vault"),
                this.usdgTokenMint.toBuffer(),
                Buffer.from([vaultVersion]),
            ],
            this.vaultProgram.programId
        );
    }

    // Vault context is now initialized in init() method with deposit_mint-based PDA derivation

    async init_sender_ata() {
        this.senderUsdgAta = (await getOrCreateAssociatedTokenAccount(
            this.connection,
            this.deployer,
            this.usdgTokenMint,
            this.deployer.publicKey
        )).address

        this.senderShareAta = (await getOrCreateAssociatedTokenAccount(
            this.connection,
            this.deployer,
            this.shareMint,
            this.deployer.publicKey
        )).address

        this.operatorUsdgAta = (await getOrCreateAssociatedTokenAccount(
            this.connection,
            this.operator,
            this.usdgTokenMint,
            this.operator.publicKey
        )).address

        this.feeRecipientUsdgAta = (await getOrCreateAssociatedTokenAccount(
            this.connection,
            this.deployer,
            this.usdgTokenMint,
            this.feeRecipient.publicKey
        )).address
    }

    async init_vault() {
        const vaultVersion = 0;
        await this.vaultProgram.methods.initialize(this.admin.publicKey, this.operator.publicKey, this.feeRecipient.publicKey, vaultVersion)
            .accounts({
                vaultState: this.vaultStatePda,
                shareMint: this.shareMint,
                vaultTokenAta: this.vaultUsdgAta,
                depositMint: this.usdgTokenMint,
                signer: this.deployer.publicKey,
                tokenProgram: token.TOKEN_PROGRAM_ID
            })
            .signers([this.deployer])
            .rpc()
    }
}
