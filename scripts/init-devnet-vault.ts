const anchor = require("@coral-xyz/anchor");
const { PublicKey, SystemProgram, SYSVAR_RENT_PUBKEY, Keypair } = require("@solana/web3.js");
const { TOKEN_PROGRAM_ID } = require("@solana/spl-token");
const fs = require("fs");
const path = require("path");

const PROGRAM_ID = new PublicKey("C8B1EpsSGVWK2vMrk3aDT3kL7RCE77otokUh4EC35kK7");
const JITOSOL_MINT = new PublicKey("J1tos8mqbhdGcF3pgj4PCKyVjzWSURcpLZU7pPGHxSYi");
const VAULT_VERSION = 0;

// Token metadata
const TOKEN_NAME = "Sentora JitoSOL Vault";
const TOKEN_SYMBOL = "sentJitoSOL";
const TOKEN_URI = "";

// Metaplex Token Metadata Program
const TOKEN_METADATA_PROGRAM_ID = new PublicKey("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");

async function main() {
  // Load wallet from devnet-test.json
  const walletPath = path.join(process.env.HOME, ".config/solana/devnet-test.json");
  const walletKeypair = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(walletPath, "utf-8")))
  );

  console.log("Wallet address:", walletKeypair.publicKey.toBase58());

  // Setup connection and provider
  const connection = new anchor.web3.Connection(process.env.DEVNET_RPC_URL || "https://api.devnet.solana.com", "confirmed");
  const wallet = new anchor.Wallet(walletKeypair);
  const provider = new anchor.AnchorProvider(connection, wallet, { commitment: "confirmed" });
  anchor.setProvider(provider);

  // Load IDL
  const idlPath = path.join(__dirname, "../target/idl/august_vault.json");
  const idl = JSON.parse(fs.readFileSync(idlPath, "utf-8"));
  // A normal `anchor build` stamps the IDL address with declare_id! (mainnet
  // up12...); this devnet script targets PROGRAM_ID and derives every PDA
  // from it, so align the IDL address to avoid seed-validation failures.
  idl.address = PROGRAM_ID.toBase58();
  const program = new anchor.Program(idl, provider);

  // Derive PDAs
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

  // Check if vault already exists
  const vaultAccount = await connection.getAccountInfo(vaultStatePda);
  if (vaultAccount) {
    console.log("\n⚠️  Vault already exists at this PDA!");
    console.log("Skipping initialization...");
  } else {
    console.log("\n=== Initializing Vault ===");
    console.log("Admin:", walletKeypair.publicKey.toBase58());
    console.log("Operator:", walletKeypair.publicKey.toBase58());
    console.log("Fee Recipient:", walletKeypair.publicKey.toBase58());

    try {
      const initTx = await program.methods
        .initialize(
          walletKeypair.publicKey, // admin
          walletKeypair.publicKey, // operator
          walletKeypair.publicKey, // fee_recipient
          VAULT_VERSION            // vault_version
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

      console.log("✅ Vault initialized!");
      console.log("Transaction:", initTx);
    } catch (e: any) {
      console.error("❌ Failed to initialize vault:", e.message || e);
      if (e.logs) {
        console.error("Logs:", e.logs);
      }
      return;
    }
  }

  // Wait a bit for the transaction to finalize
  await new Promise(resolve => setTimeout(resolve, 2000));

  // Verify share mint decimals
  console.log("\n=== Verifying Share Mint ===");
  const shareMintInfo = await connection.getParsedAccountInfo(shareMintPda);
  if (shareMintInfo.value && "parsed" in (shareMintInfo.value.data as any)) {
    const mintData = (shareMintInfo.value.data as any).parsed.info;
    console.log("Share Mint Decimals:", mintData.decimals);
    console.log("Share Mint Supply:", mintData.supply);
    console.log("Share Mint Authority:", mintData.mintAuthority);

    if (mintData.decimals !== 9) {
      console.error("❌ ERROR: Share mint decimals mismatch! Expected 9, got", mintData.decimals);
      return;
    }
    console.log("✅ Share mint has correct decimals (9)");
  }

  // Create token metadata
  console.log("\n=== Creating Token Metadata ===");
  console.log("Name:", TOKEN_NAME);
  console.log("Symbol:", TOKEN_SYMBOL);

  // Derive metadata PDA
  const [metadataPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("metadata"), TOKEN_METADATA_PROGRAM_ID.toBuffer(), shareMintPda.toBuffer()],
    TOKEN_METADATA_PROGRAM_ID
  );
  console.log("Metadata PDA:", metadataPda.toBase58());

  // Check if metadata already exists
  const metadataAccount = await connection.getAccountInfo(metadataPda);
  if (metadataAccount) {
    console.log("⚠️  Metadata already exists, skipping...");
  } else {
    try {
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

      console.log("✅ Token metadata created!");
      console.log("Transaction:", metadataTx);
    } catch (e: any) {
      console.error("❌ Failed to create metadata:", e.message || e);
      if (e.logs) {
        console.error("Logs:", e.logs);
      }
    }
  }

  console.log("\n=== Summary ===");
  console.log("Program ID:", PROGRAM_ID.toBase58());
  console.log("Vault State:", vaultStatePda.toBase58());
  console.log("Share Mint:", shareMintPda.toBase58());
  console.log("Vault Token ATA:", vaultTokenAtaPda.toBase58());
  console.log("Deposit Mint (jitoSOL):", JITOSOL_MINT.toBase58());
  console.log("Token Name:", TOKEN_NAME);
  console.log("Token Symbol:", TOKEN_SYMBOL);
  console.log("Vault Version:", VAULT_VERSION);
}

main().catch(console.error);
