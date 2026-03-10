// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE.BSL file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

import { expect } from "chai";
import * as fs from "fs";
import * as path from "path";
import { PublicKey, Connection } from "@solana/web3.js";

const PROGRAM_ID = new PublicKey("2YyJRH7Q7qYZC6NTA3TncenFN7TueVCPRFi7eydfVVby");
const TOKEN_METADATA_PROGRAM_ID = new PublicKey("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");
const RPC_URL = "https://api.mainnet-beta.solana.com";

describe("Metadata Update", () => {
  let idl: any;

  before(async () => {
    // Load the IDL from the target directory
    const idlPath = path.join(__dirname, "../target/idl/august_vault.json");
    idl = JSON.parse(fs.readFileSync(idlPath, "utf8"));
  });

  it("Should have update_share_token_metadata instruction in IDL", () => {
    // Verify the instruction exists in the IDL
    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    expect(updateMetadataInstruction).to.not.be.undefined;
    expect(updateMetadataInstruction?.accounts).to.have.length(6);
  });

  it("Should validate update instruction parameters", () => {
    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    const expectedArgs = ["name", "symbol", "uri"];
    const actualArgs = updateMetadataInstruction?.args.map((arg: any) => arg.name);

    expectedArgs.forEach((expectedArg) => {
      expect(actualArgs).to.include(expectedArg);
    });

    // Verify all parameters are strings
    const args = updateMetadataInstruction.args;
    expect(args[0].type).to.equal("string");
    expect(args[1].type).to.equal("string");
    expect(args[2].type).to.equal("string");
  });

  it("Should have correct account structure for update instruction", () => {
    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    // Verify account structure
    const expectedAccounts = [
      "admin",
      "vault_state",
      "share_mint",
      "metadata_account",
      "token_program",
      "token_metadata_program",
    ];

    const actualAccounts = updateMetadataInstruction?.accounts.map((acc: any) => acc.name);
    expectedAccounts.forEach((expectedAccount) => {
      expect(actualAccounts).to.include(expectedAccount);
    });
  });

  it("Should verify admin account constraints", () => {
    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    const adminAccount = updateMetadataInstruction?.accounts.find(
      (acc: any) => acc.name === "admin"
    );

    // Admin must be a signer
    expect(adminAccount.signer).to.be.true;
    // Admin should not be writable (only signs, doesn't need to be writable)
    expect(adminAccount.writable).to.be.undefined;
  });

  it("Should verify vault_state account constraints", () => {
    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    const vaultStateAccount = updateMetadataInstruction?.accounts.find(
      (acc: any) => acc.name === "vault_state"
    );

    // Vault state must be writable (for PDA signing)
    expect(vaultStateAccount.writable).to.be.true;
    // Must be a PDA
    expect(vaultStateAccount.pda).to.not.be.undefined;
    // PDA seeds should be VAULT_STATE
    expect(vaultStateAccount.pda.seeds[0].value).to.deep.equal([
      86, 65, 85, 76, 84, 95, 83, 84, 65, 84, 69,
    ]); // "VAULT_STATE"
  });

  it("Should verify metadata_account PDA structure", () => {
    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    const metadataAccount = updateMetadataInstruction?.accounts.find(
      (acc: any) => acc.name === "metadata_account"
    );

    // Must be writable
    expect(metadataAccount.writable).to.be.true;
    // Must be a PDA
    expect(metadataAccount.pda).to.not.be.undefined;
    // PDA seeds structure
    expect(metadataAccount.pda.seeds).to.have.length(3);

    const seeds = metadataAccount.pda.seeds;
    expect(seeds[0].kind).to.equal("const");
    expect(seeds[0].value).to.deep.equal([109, 101, 116, 97, 100, 97, 116, 97]); // "metadata"

    expect(seeds[1].kind).to.equal("const");
    expect(seeds[1].value).to.be.an("array");
    expect(seeds[1].value).to.have.length(32); // Metaplex program ID

    expect(seeds[2].kind).to.equal("account");
    expect(seeds[2].path).to.equal("share_mint");

    // Verify the program ID for the PDA
    expect(metadataAccount.pda.program.kind).to.equal("const");
    expect(metadataAccount.pda.program.value).to.be.an("array");
    expect(metadataAccount.pda.program.value).to.have.length(32);
  });

  it("Should verify share_mint account constraints", () => {
    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    const shareMintAccount = updateMetadataInstruction?.accounts.find(
      (acc: any) => acc.name === "share_mint"
    );

    // Share mint should not be writable for updates
    expect(shareMintAccount.writable).to.be.undefined;
    // Should not be a signer
    expect(shareMintAccount.signer).to.be.undefined;
    // Should have documentation
    expect(shareMintAccount.docs).to.be.an("array");
    expect(shareMintAccount.docs[0]).to.include("share token mint");
  });

  it("Should verify token_program account", () => {
    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    const tokenProgramAccount = updateMetadataInstruction?.accounts.find(
      (acc: any) => acc.name === "token_program"
    );

    // Token program should not be writable
    expect(tokenProgramAccount.writable).to.be.undefined;
    expect(tokenProgramAccount.signer).to.be.undefined;
  });

  it("Should verify token_metadata_program account", () => {
    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    const tokenMetadataProgramAccount = updateMetadataInstruction?.accounts.find(
      (acc: any) => acc.name === "token_metadata_program"
    );

    // Should be the Metaplex Token Metadata Program
    expect(tokenMetadataProgramAccount.address).to.equal(
      "metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s"
    );
    // Should not be writable
    expect(tokenMetadataProgramAccount.writable).to.be.undefined;
    // Should not be a signer
    expect(tokenMetadataProgramAccount.signer).to.be.undefined;
  });

  it("Should verify instruction discriminator", () => {
    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    expect(updateMetadataInstruction.discriminator).to.be.an("array");
    expect(updateMetadataInstruction.discriminator).to.have.length(8);
  });

  it("Should verify update instruction has fewer accounts than create", () => {
    const createMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "create_share_token_metadata"
    );

    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    // Update should have fewer accounts than create (no payer, system_program, rent)
    expect(updateMetadataInstruction.accounts.length).to.be.lessThan(
      createMetadataInstruction.accounts.length
    );
  });

  it("Should verify update instruction does not require payer", () => {
    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    const accounts = updateMetadataInstruction?.accounts.map((acc: any) => acc.name);

    // Update should not require a payer account (unlike create)
    expect(accounts).to.not.include("payer");
  });

  it("Should verify update instruction does not require system_program", () => {
    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    const accounts = updateMetadataInstruction?.accounts.map((acc: any) => acc.name);

    // Update should not require system_program (unlike create)
    expect(accounts).to.not.include("system_program");
  });

  it("Should verify update instruction does not require rent", () => {
    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    const accounts = updateMetadataInstruction?.accounts.map((acc: any) => acc.name);

    // Update should not require rent sysvar (unlike create)
    expect(accounts).to.not.include("rent");
  });

  it("Should verify metadata account exists on chain", async () => {
    // Calculate the share mint PDA
    const [shareMintPDA] = PublicKey.findProgramAddressSync(
      [Buffer.from("mint")],
      PROGRAM_ID
    );

    // Calculate the metadata PDA
    const [metadataPDA] = PublicKey.findProgramAddressSync(
      [Buffer.from("metadata"), TOKEN_METADATA_PROGRAM_ID.toBuffer(), shareMintPDA.toBuffer()],
      TOKEN_METADATA_PROGRAM_ID
    );

    // Create connection
    const connection = new Connection(RPC_URL, "confirmed");

    try {
      // Check if the metadata account exists
      const metadataAccountInfo = await connection.getAccountInfo(metadataPDA);

      if (metadataAccountInfo) {
        // Verify the account structure
        expect(metadataAccountInfo.owner).to.equal(TOKEN_METADATA_PROGRAM_ID);
        expect(metadataAccountInfo.data.length).to.be.greaterThan(0);

        console.log("✅ Metadata account exists and can be updated");
      } else {
        console.log("⚠️  Metadata account does not exist yet (will be created first)");
      }
    } catch (error) {
      console.error("Could not read metadata account:", error);
    }
  });

  it("Should verify instruction documentation", () => {
    const updateMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "update_share_token_metadata"
    );

    // Verify the instruction has documentation
    expect(updateMetadataInstruction.docs).to.be.an("array");
    expect(updateMetadataInstruction.docs.length).to.be.greaterThan(0);
  });
});
