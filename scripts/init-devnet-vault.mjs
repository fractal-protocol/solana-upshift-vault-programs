#!/usr/bin/env node
// Initialize the devnet jitoSOL vault (version 0) and create its share-token
// metadata. Idempotent: skips initialization/metadata if they already exist.
//
// Usage: node scripts/init-devnet-vault.mjs   (or: pnpm run init:devnet-vault)
// Requires: a funded devnet keypair at ~/.config/solana/devnet-test.json,
// and (optionally) DEVNET_RPC_URL for a custom RPC endpoint.

import { Connection, Keypair, PublicKey, SystemProgram, SYSVAR_RENT_PUBKEY } from "@solana/web3.js";
import { AnchorProvider, Program, Wallet } from "@coral-xyz/anchor";
import { TOKEN_PROGRAM_ID } from "@solana/spl-token";
import { readFileSync } from "fs";
import { dirname, join } from "path";
import { fileURLToPath } from "url";
import { ensureProgramConfig } from "../deploy/helpers/program-config.mjs";

const __dirname = dirname(fileURLToPath(import.meta.url));

const PROGRAM_ID = new PublicKey("C8B1EpsSGVWK2vMrk3aDT3kL7RCE77otokUh4EC35kK7");
const JITOSOL_MINT = new PublicKey("J1tos8mqbhdGcF3pgj4PCKyVjzWSURcpLZU7pPGHxSYi");
const VAULT_VERSION = 0;

const TOKEN_NAME = "Sentora JitoSOL Vault";
const TOKEN_SYMBOL = "sentJitoSOL";
const TOKEN_URI = "";

const TOKEN_METADATA_PROGRAM_ID = new PublicKey("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");

async function main() {
  const walletPath = join(process.env.HOME, ".config/solana/devnet-test.json");
  const walletKeypair = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(readFileSync(walletPath, "utf-8")))
  );

  console.log("Wallet address:", walletKeypair.publicKey.toBase58());

  const connection = new Connection(
    process.env.DEVNET_RPC_URL || "https://api.devnet.solana.com",
    "confirmed"
  );
  const wallet = new Wallet(walletKeypair);
  const provider = new AnchorProvider(connection, wallet, { commitment: "confirmed" });

  const idlPath = join(__dirname, "../target/idl/august_vault.json");
  const idl = JSON.parse(readFileSync(idlPath, "utf-8"));
  // A normal `anchor build` stamps the IDL address with declare_id! (mainnet
  // up12...); this devnet script targets PROGRAM_ID and derives every PDA
  // from it, so align the IDL address to avoid seed-validation failures.
  idl.address = PROGRAM_ID.toBase58();
  const program = new Program(idl, provider);

  const [vaultStatePda] = PublicKey.findProgramAddressSync(
    [Buffer.from("VAULT_STATE"), JITOSOL_MINT.toBuffer(), Buffer.from([VAULT_VERSION])],
    PROGRAM_ID
  );
  const [shareMintPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("mint"), JITOSOL_MINT.toBuffer(), Buffer.from([VAULT_VERSION])],
    PROGRAM_ID
  );
  const [vaultTokenAtaPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("token_vault"), JITOSOL_MINT.toBuffer(), Buffer.from([VAULT_VERSION])],
    PROGRAM_ID
  );

  console.log("\n=== Vault Configuration ===");
  console.log("Program ID:", PROGRAM_ID.toBase58());
  console.log("Deposit Mint (jitoSOL):", JITOSOL_MINT.toBase58());
  console.log("Vault Version:", VAULT_VERSION);
  console.log("\n=== Derived PDAs ===");
  console.log("Vault State PDA:", vaultStatePda.toBase58());
  console.log("Share Mint PDA:", shareMintPda.toBase58());
  console.log("Vault Token ATA PDA:", vaultTokenAtaPda.toBase58());

  const vaultAccount = await connection.getAccountInfo(vaultStatePda);
  if (vaultAccount) {
    console.log("\n⚠️  Vault already exists at this PDA! Skipping initialization...");
  } else {
    // Vault creation is gated on the ProgramConfig authority; bootstrap it (or
    // fail loudly) before attempting to initialize.
    console.log("\n=== Program Config ===");
    const configAuthority = await ensureProgramConfig({
      program,
      signer: walletKeypair,
      desiredAuthority: walletKeypair.publicKey,
    });
    if (!configAuthority.equals(walletKeypair.publicKey)) {
      throw new Error(
        `Vault creation is restricted to ${configAuthority.toBase58()}, but this ` +
        `script signs as ${walletKeypair.publicKey.toBase58()}.`
      );
    }

    console.log("\n=== Initializing Vault ===");
    console.log("Admin/Operator/Fee Recipient:", walletKeypair.publicKey.toBase58());

    try {
      const initTx = await program.methods
        .initialize(
          walletKeypair.publicKey, // admin
          walletKeypair.publicKey, // operator
          walletKeypair.publicKey, // fee_recipient
          VAULT_VERSION // vault_version
        )
        .accounts({
          vaultState: vaultStatePda,
          shareMint: shareMintPda,
          vaultTokenAta: vaultTokenAtaPda,
          depositMint: JITOSOL_MINT,
          signer: walletKeypair.publicKey,
          systemProgram: SystemProgram.programId,
          tokenProgram: TOKEN_PROGRAM_ID,
          rent: SYSVAR_RENT_PUBKEY,
        })
        .rpc();

      console.log("✅ Vault initialized! Transaction:", initTx);
    } catch (e) {
      if (e && e.logs) console.error("Logs:", e.logs);
      // Fail hard: never let automation treat a failed initialization as success.
      throw new Error(`Failed to initialize vault: ${(e && e.message) || e}`);
    }
  }

  // Give the transaction a moment to finalize before reading state back.
  await new Promise((resolve) => setTimeout(resolve, 2000));

  console.log("\n=== Verifying Share Mint ===");
  const shareMintInfo = await connection.getParsedAccountInfo(shareMintPda);
  if (shareMintInfo.value && "parsed" in shareMintInfo.value.data) {
    const mintData = shareMintInfo.value.data.parsed.info;
    console.log("Share Mint Decimals:", mintData.decimals);
    console.log("Share Mint Supply:", mintData.supply);
    console.log("Share Mint Authority:", mintData.mintAuthority);
    if (mintData.decimals !== 9) {
      throw new Error(`Share mint decimals mismatch! Expected 9, got ${mintData.decimals}`);
    }
    console.log("✅ Share mint has correct decimals (9)");
  }

  console.log("\n=== Creating Token Metadata ===");
  console.log("Name:", TOKEN_NAME, "| Symbol:", TOKEN_SYMBOL);

  const [metadataPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("metadata"), TOKEN_METADATA_PROGRAM_ID.toBuffer(), shareMintPda.toBuffer()],
    TOKEN_METADATA_PROGRAM_ID
  );
  console.log("Metadata PDA:", metadataPda.toBase58());

  const metadataAccount = await connection.getAccountInfo(metadataPda);
  if (metadataAccount) {
    console.log("⚠️  Metadata already exists, skipping...");
  } else {
    // Let any failure propagate to the top-level handler (non-zero exit) so a
    // failed metadata creation is never reported as success.
    const metadataTx = await program.methods
      .createShareTokenMetadata(TOKEN_NAME, TOKEN_SYMBOL, TOKEN_URI)
      .accounts({
        vaultState: vaultStatePda,
        shareMint: shareMintPda,
        depositMint: JITOSOL_MINT,
        metadata: metadataPda,
        admin: walletKeypair.publicKey,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        tokenMetadataProgram: TOKEN_METADATA_PROGRAM_ID,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .rpc();
    console.log("✅ Token metadata created! Transaction:", metadataTx);
  }

  console.log("\n=== Summary ===");
  console.log("Program ID:", PROGRAM_ID.toBase58());
  console.log("Vault State:", vaultStatePda.toBase58());
  console.log("Share Mint:", shareMintPda.toBase58());
  console.log("Vault Token ATA:", vaultTokenAtaPda.toBase58());
  console.log("Deposit Mint (jitoSOL):", JITOSOL_MINT.toBase58());
  console.log("Token:", TOKEN_NAME, `(${TOKEN_SYMBOL})`, "| Vault Version:", VAULT_VERSION);
}

main().catch((e) => {
  console.error("❌ Fatal error:", (e && e.message) || e);
  process.exitCode = 1;
});
