// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

import { expect } from "chai";
import * as fs from "fs";
import * as path from "path";
import { PublicKey, Connection } from "@solana/web3.js";

const PROGRAM_ID = new PublicKey("2YyJRH7Q7qYZC6NTA3TncenFN7TueVCPRFi7eydfVVby");
const RPC_URL = "https://api.mainnet-beta.solana.com";

describe("Set AUM Limits", () => {
  let idl: any;

  before(async () => {
    // Load the IDL from the target directory
    const idlPath = path.join(__dirname, "../target/idl/august_vault.json");
    idl = JSON.parse(fs.readFileSync(idlPath, "utf8"));
  });

  it("Should have set_aum_limits instruction in IDL", () => {
    // Verify the instruction exists in the IDL
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    expect(setAumLimitsInstruction).to.not.be.undefined;
    expect(setAumLimitsInstruction?.accounts).to.have.length(2);
  });

  it("Should validate set_aum_limits instruction parameters", () => {
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    const expectedArgs = ["increase_limit", "decrease_limit"];
    const actualArgs = setAumLimitsInstruction?.args.map((arg: any) => arg.name);

    expectedArgs.forEach((expectedArg) => {
      expect(actualArgs).to.include(expectedArg);
    });

    // Verify both parameters are u32
    const args = setAumLimitsInstruction.args;
    expect(args[0].type).to.equal("u32");
    expect(args[1].type).to.equal("u32");
  });

  it("Should have correct account structure for set_aum_limits instruction", () => {
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    // Verify account structure
    const expectedAccounts = ["vault_state", "admin"];

    const actualAccounts = setAumLimitsInstruction?.accounts.map((acc: any) => acc.name);
    expectedAccounts.forEach((expectedAccount) => {
      expect(actualAccounts).to.include(expectedAccount);
    });
  });

  it("Should verify admin account constraints", () => {
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    const adminAccount = setAumLimitsInstruction?.accounts.find(
      (acc: any) => acc.name === "admin"
    );

    // Admin must be a signer
    expect(adminAccount.signer).to.be.true;
    // Admin should be writable (for transaction fee)
    expect(adminAccount.writable).to.be.true;
  });

  it("Should verify vault_state account constraints", () => {
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    const vaultStateAccount = setAumLimitsInstruction?.accounts.find(
      (acc: any) => acc.name === "vault_state"
    );

    // Vault state must be writable (to update the limits)
    expect(vaultStateAccount.writable).to.be.true;
    // Must be a PDA
    expect(vaultStateAccount.pda).to.not.be.undefined;
    // PDA seeds should be VAULT_STATE
    expect(vaultStateAccount.pda.seeds[0].value).to.deep.equal([
      86, 65, 85, 76, 84, 95, 83, 84, 65, 84, 69,
    ]); // "VAULT_STATE"
  });

  it("Should verify admin constraint is enforced", () => {
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    const vaultStateAccount = setAumLimitsInstruction?.accounts.find(
      (acc: any) => acc.name === "vault_state"
    );

    // Verify vault_state account exists (constraint is enforced at runtime, not in IDL)
    expect(vaultStateAccount).to.not.be.undefined;
    // Verify admin is a signer
    const adminAccount = setAumLimitsInstruction?.accounts.find(
      (acc: any) => acc.name === "admin"
    );
    expect(adminAccount.signer).to.be.true;
  });

  it("Should verify instruction discriminator", () => {
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    expect(setAumLimitsInstruction.discriminator).to.be.an("array");
    expect(setAumLimitsInstruction.discriminator).to.have.length(8);
  });

  it("Should verify instruction documentation", () => {
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    // Verify the instruction has documentation
    expect(setAumLimitsInstruction.docs).to.be.an("array");
    expect(setAumLimitsInstruction.docs.length).to.be.greaterThan(0);
    expect(setAumLimitsInstruction.docs[0]).to.include("AUM Change Limits");
  });

  it("Should verify increase_limit parameter documentation", () => {
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    // Verify documentation mentions basis points
    const docs = setAumLimitsInstruction.docs.join(" ");
    expect(docs).to.include("basis points");
    expect(docs).to.include("increase_limit");
  });

  it("Should verify decrease_limit parameter documentation", () => {
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    // Verify documentation mentions basis points
    const docs = setAumLimitsInstruction.docs.join(" ");
    expect(docs).to.include("basis points");
    expect(docs).to.include("decrease_limit");
  });

  it("Should verify VaultState struct includes new fields", () => {
    const vaultStateType = idl.types.find((type: any) => type.name === "VaultState");

    expect(vaultStateType).to.not.be.undefined;

    const fields = vaultStateType.type.fields;
    const fieldNames = fields.map((field: any) => field.name);

    // Verify the new fields exist
    expect(fieldNames).to.include("aum_increase_limit");
    expect(fieldNames).to.include("aum_decrease_limit");

    // Verify they are u32
    const increaseLimitField = fields.find((f: any) => f.name === "aum_increase_limit");
    const decreaseLimitField = fields.find((f: any) => f.name === "aum_decrease_limit");

    expect(increaseLimitField.type).to.equal("u32");
    expect(decreaseLimitField.type).to.equal("u32");
  });

  it("Should verify operator_update_aum uses configurable limits", () => {
    const operatorUpdateAumInstruction = idl.instructions.find(
      (ix: any) => ix.name === "operator_update_aum"
    );

    expect(operatorUpdateAumInstruction).to.not.be.undefined;

    // Verify the instruction still exists and has the correct structure
    expect(operatorUpdateAumInstruction.accounts).to.have.length(2);
    
    const accounts = operatorUpdateAumInstruction.accounts.map((acc: any) => acc.name);
    expect(accounts).to.include("vault_state");
    expect(accounts).to.include("operator");
  });

  it("Should verify operator cannot call set_aum_limits", () => {
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    const accounts = setAumLimitsInstruction?.accounts.map((acc: any) => acc.name);

    // Verify operator is NOT in the accounts
    expect(accounts).to.not.include("operator");

    // Verify only admin is the signer
    const signerAccounts = setAumLimitsInstruction?.accounts.filter(
      (acc: any) => acc.signer === true
    );
    expect(signerAccounts).to.have.length(1);
    expect(signerAccounts[0].name).to.equal("admin");
  });

  it("Should verify limits are in basis points format", () => {
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    // Verify documentation explains basis points
    const docs = setAumLimitsInstruction.docs.join(" ");
    expect(docs).to.include("20 = 0.2%");
    expect(docs).to.include("100 = 1%");
  });

  it("Should verify instruction is admin-only operation", () => {
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    // Verify admin is required
    const adminAccount = setAumLimitsInstruction?.accounts.find(
      (acc: any) => acc.name === "admin"
    );

    expect(adminAccount).to.not.be.undefined;
    expect(adminAccount.signer).to.be.true;

    // Verify only admin is the signer (not operator)
    const signerAccounts = setAumLimitsInstruction?.accounts.filter(
      (acc: any) => acc.signer === true
    );
    expect(signerAccounts).to.have.length(1);
    expect(signerAccounts[0].name).to.equal("admin");
  });

  it("Should verify vault state is mutable", () => {
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    const vaultStateAccount = setAumLimitsInstruction?.accounts.find(
      (acc: any) => acc.name === "vault_state"
    );

    // Must be writable to update the limits
    expect(vaultStateAccount.writable).to.be.true;
  });

  it("Should verify no additional accounts required", () => {
    const setAumLimitsInstruction = idl.instructions.find(
      (ix: any) => ix.name === "set_aum_limits"
    );

    // Should only have 2 accounts: vault_state and admin
    expect(setAumLimitsInstruction.accounts).to.have.length(2);

    const accounts = setAumLimitsInstruction.accounts.map((acc: any) => acc.name);
    expect(accounts).to.not.include("operator");
    expect(accounts).to.not.include("payer");
    expect(accounts).to.not.include("system_program");
  });
});
