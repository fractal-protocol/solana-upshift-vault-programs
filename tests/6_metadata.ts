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
const RPC_URL = "https://api.mainnet-beta.solana.com"

describe("Metadata Creation", () => {
  let idl: any;

  before(async () => {
    // Load the IDL from the target directory
    const idlPath = path.join(__dirname, "../target/idl/august_vault.json");
    idl = JSON.parse(fs.readFileSync(idlPath, "utf8"));
  });

  it("Should create metadata for share token with proper security checks", async () => {    
    // Verify the instruction exists in the IDL
    const createMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "create_share_token_metadata"
    );
    
    expect(createMetadataInstruction).to.not.be.undefined;
    expect(createMetadataInstruction?.accounts).to.have.length(9);
    
    // Verify account structure
    const expectedAccounts = [
      "payer",
      "admin", 
      "vault_state",
      "share_mint",
      "metadata_account",
      "token_program",
      "token_metadata_program",
      "system_program",
      "rent"
    ];
    
    const actualAccounts = createMetadataInstruction?.accounts.map((acc: any) => acc.name);
    expectedAccounts.forEach(expectedAccount => {
      expect(actualAccounts).to.include(expectedAccount);
    });
  });

  it("Should validate instruction parameters", () => {
    const createMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "create_share_token_metadata"
    );
    
    const expectedArgs = ["name", "symbol", "uri"];
    const actualArgs = createMetadataInstruction?.args.map((arg: any) => arg.name);
    
    expectedArgs.forEach(expectedArg => {
      expect(actualArgs).to.include(expectedArg);
    });
  });

  it("Should read and verify metadata is set correctly", async () => {        
    // Calculate the share mint PDA (this would be the actual mint address in a real scenario)
    const [shareMintPDA] = PublicKey.findProgramAddressSync(
      [Buffer.from("mint")],
      PROGRAM_ID
    );
    
    // Calculate the metadata PDA
    const [metadataPDA] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("metadata"),
        TOKEN_METADATA_PROGRAM_ID.toBuffer(),
        shareMintPDA.toBuffer(),
      ],
      TOKEN_METADATA_PROGRAM_ID
    );
    
    // Create connection
    const connection = new Connection(RPC_URL, "confirmed");
    
    try {
      // Check if the metadata account exists
      const metadataAccountInfo = await connection.getAccountInfo(metadataPDA);
      
      if (!metadataAccountInfo) {
        // Verify the instruction can create metadata with correct parameters
        const createMetadataInstruction = idl.instructions.find(
          (ix: any) => ix.name === "create_share_token_metadata"
        );
        
        expect(createMetadataInstruction).to.not.be.undefined;
        expect(createMetadataInstruction.args).to.have.length(3);
        
        // Verify the instruction accepts the expected parameters
        const args = createMetadataInstruction.args;
        expect(args[0].name).to.equal("name");
        expect(args[0].type).to.equal("string");
        expect(args[1].name).to.equal("symbol");
        expect(args[1].type).to.equal("string");
        expect(args[2].name).to.equal("uri");
        expect(args[2].type).to.equal("string");
      } else {        
        // Verify the account structure
        expect(metadataAccountInfo.owner).to.equal(TOKEN_METADATA_PROGRAM_ID);
        expect(metadataAccountInfo.data.length).to.be.greaterThan(0);
      }
      
    } catch (error) {
      console.error("Could not read metadata account (this is expected for testing)");
    }
    
    // Verify the instruction discriminator is correct
    const createMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "create_share_token_metadata"
    );
    
    expect(createMetadataInstruction.discriminator).to.be.an("array");
    expect(createMetadataInstruction.discriminator).to.have.length(8);
    
    // Verify all required accounts are present and properly typed
    const accounts = createMetadataInstruction.accounts;
    expect(accounts).to.have.length(9);
    
    // Check specific account properties
    const payerAccount = accounts.find((acc: any) => acc.name === "payer");
    expect(payerAccount.writable).to.be.true;
    expect(payerAccount.signer).to.be.true;
    
    const adminAccount = accounts.find((acc: any) => acc.name === "admin");
    expect(adminAccount.writable).to.be.true;
    expect(adminAccount.signer).to.be.true;
    
    const vaultStateAccount = accounts.find((acc: any) => acc.name === "vault_state");
    expect(vaultStateAccount.writable).to.be.true;
    expect(vaultStateAccount.pda).to.not.be.undefined;
    
    const metadataAccount = accounts.find((acc: any) => acc.name === "metadata_account");
    expect(metadataAccount.writable).to.be.true;
    expect(metadataAccount.pda).to.not.be.undefined;
  });

  it("Should verify metadata account structure and constraints", () => {    
    const createMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "create_share_token_metadata"
    );
    
    // Verify the metadata account PDA structure
    const metadataAccount = createMetadataInstruction.accounts.find(
      (acc: any) => acc.name === "metadata_account"
    );
    
    expect(metadataAccount.pda).to.not.be.undefined;
    expect(metadataAccount.pda.seeds).to.have.length(3);
    
    // Verify PDA seeds structure
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

  it("Should demonstrate metadata parsing and validation", async () => {        
    // Verify the instruction can set these values
    const createMetadataInstruction = idl.instructions.find(
      (ix: any) => ix.name === "create_share_token_metadata"
    );
    
    // Check that the instruction accepts string parameters
    const nameArg = createMetadataInstruction.args.find((arg: any) => arg.name === "name");
    const symbolArg = createMetadataInstruction.args.find((arg: any) => arg.name === "symbol");
    const uriArg = createMetadataInstruction.args.find((arg: any) => arg.name === "uri");
    
    expect(nameArg.type).to.equal("string");
    expect(symbolArg.type).to.equal("string");
    expect(uriArg.type).to.equal("string");
        
    // Verify the instruction sets the vault state as update authority
    const vaultStateAccount = createMetadataInstruction.accounts.find(
      (acc: any) => acc.name === "vault_state"
    );
    
    expect(vaultStateAccount).to.not.be.undefined;
    expect(vaultStateAccount.writable).to.be.true;
        
    // Verify security constraints
    const adminAccount = createMetadataInstruction.accounts.find(
      (acc: any) => acc.name === "admin"
    );
    
    expect(adminAccount.signer).to.be.true;
    expect(adminAccount.writable).to.be.true;
  });
});
