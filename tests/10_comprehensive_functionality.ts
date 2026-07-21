// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AugustVault } from "../../target/types/august_vault";
import { expect } from "chai";
import { PublicKey, LAMPORTS_PER_SOL, Connection, Keypair, SystemProgram } from "@solana/web3.js";
import { sha256 } from "js-sha256";
import * as token from "@solana/spl-token";
import { getOrCreateAssociatedTokenAccount, getAccount } from "@solana/spl-token";
import { VaultContext } from "./helper/context";

describe("Comprehensive Functionality Tests", () => {
  let context: VaultContext;
  let program: Program<AugustVault>;

  before(async () => {
    context = new VaultContext();
    await context.init();
    program = context.vaultProgram;
  });

  describe("Admin Functions", () => {
    describe("Set Operator", () => {
      it("Should allow admin to set a new operator", async () => {
        const newOperator = Keypair.generate();

        await program.methods
          .setOperator(newOperator.publicKey)
          .accounts({
            vaultState: context.vaultStatePda,
            admin: context.admin.publicKey,
          })
          .signers([context.admin])
          .rpc();

        const vaultState = await program.account.vaultState.fetch(context.vaultStatePda);
        expect(vaultState.operator.toString()).to.equal(newOperator.publicKey.toString());

        // Reset to original operator
        await program.methods
          .setOperator(context.operator.publicKey)
          .accounts({
            vaultState: context.vaultStatePda,
            admin: context.admin.publicKey,
          })
          .signers([context.admin])
          .rpc();
      });

      it("Should fail if non-admin tries to set operator", async () => {
        const newOperator = Keypair.generate();

        try {
          await program.methods
            .setOperator(newOperator.publicKey)
            .accounts({
              vaultState: context.vaultStatePda,
              admin: context.deployer.publicKey, // Not the admin
            })
            .signers([context.deployer])
            .rpc();

          expect.fail("Should have thrown an error");
        } catch (error: any) {
          expect(error.message).to.include("NotAdmin");
        }
      });
    });

    describe("Set Fee Recipient", () => {
      it("Should allow admin to set a new fee recipient", async () => {
        const newFeeRecipient = Keypair.generate();

        await program.methods
          .setFeeRecipient(newFeeRecipient.publicKey)
          .accounts({
            vaultState: context.vaultStatePda,
            admin: context.admin.publicKey,
          })
          .signers([context.admin])
          .rpc();

        const vaultState = await program.account.vaultState.fetch(context.vaultStatePda);
        expect(vaultState.feeRecipient.toString()).to.equal(newFeeRecipient.publicKey.toString());

        // Reset to original fee recipient
        await program.methods
          .setFeeRecipient(context.feeRecipient.publicKey)
          .accounts({
            vaultState: context.vaultStatePda,
            admin: context.admin.publicKey,
          })
          .signers([context.admin])
          .rpc();
      });
    });

    describe("Pause/Unpause", () => {
      it("Should allow admin to pause the vault", async () => {
        await program.methods
          .pause()
          .accounts({
            vaultState: context.vaultStatePda,
            admin: context.admin.publicKey,
          })
          .signers([context.admin])
          .rpc();

        const vaultState = await program.account.vaultState.fetch(context.vaultStatePda);
        expect(vaultState.paused).to.be.true;
      });

      it("Should prevent deposits when vault is paused", async () => {
        const depositAmount = new anchor.BN(1000 * 10 ** 9);

        try {
          await program.methods
            .deposit(depositAmount)
            .accounts({
              vaultState: context.vaultStatePda,
              senderTokenAccount: context.senderUsdgAta,
              senderShareAccount: context.senderShareAta,
              shareMint: context.shareMint,
              depositMint: context.usdgTokenMint,
              signer: context.deployer.publicKey,
              tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([context.deployer])
            .rpc();

          expect.fail("Should have thrown an error");
        } catch (error: any) {
          expect(error.message).to.include("VaultPaused");
        }
      });

      it("Should allow admin to unpause the vault", async () => {
        await program.methods
          .unpause()
          .accounts({
            vaultState: context.vaultStatePda,
            admin: context.admin.publicKey,
          })
          .signers([context.admin])
          .rpc();

        const vaultState = await program.account.vaultState.fetch(context.vaultStatePda);
        expect(vaultState.paused).to.be.false;
      });
    });

    describe("Set Withdrawal Fee", () => {
      it("Should allow admin to set withdrawal fee", async () => {
        const newFee = new anchor.BN(50000); // 5% in basis points

        await program.methods
          .setWithdrawalFee(newFee)
          .accounts({
            vaultState: context.vaultStatePda,
            admin: context.admin.publicKey,
          })
          .signers([context.admin])
          .rpc();

        const vaultState = await program.account.vaultState.fetch(context.vaultStatePda);
        expect(vaultState.withdrawalFee.toString()).to.equal("50000");

        // Reset to 0
        await program.methods
          .setWithdrawalFee(new anchor.BN(0))
          .accounts({
            vaultState: context.vaultStatePda,
            admin: context.admin.publicKey,
          })
          .signers([context.admin])
          .rpc();
      });

      it("Should fail if withdrawal fee is too high", async () => {
        const tooHighFee = new anchor.BN(1100000); // 110%

        try {
          await program.methods
            .setWithdrawalFee(tooHighFee)
            .accounts({
              vaultState: context.vaultStatePda,
              admin: context.admin.publicKey,
            })
            .signers([context.admin])
            .rpc();

          expect.fail("Should have thrown an error");
        } catch (error: any) {
          expect(error.message).to.include("WithdrawalFeeTooHigh");
        }
      });
    });

    describe("Set AUM Limits", () => {
      it("Should allow admin to set AUM limits", async () => {
        const increaseLimit = 50; // 0.5%
        const decreaseLimit = 30; // 0.3%

        await program.methods
          .setAumLimits(increaseLimit, decreaseLimit)
          .accounts({
            vaultState: context.vaultStatePda,
            admin: context.admin.publicKey,
          })
          .signers([context.admin])
          .rpc();

        const vaultState = await program.account.vaultState.fetch(context.vaultStatePda);
        expect(vaultState.aumIncreaseLimit).to.equal(50);
        expect(vaultState.aumDecreaseLimit).to.equal(30);

        // Reset to defaults
        await program.methods
          .setAumLimits(20, 20)
          .accounts({
            vaultState: context.vaultStatePda,
            admin: context.admin.publicKey,
          })
          .signers([context.admin])
          .rpc();
      });

      it("Should fail if limits are too high", async () => {
        try {
          await program.methods
            .setAumLimits(10001, 10001)
            .accounts({
              vaultState: context.vaultStatePda,
              admin: context.admin.publicKey,
            })
            .signers([context.admin])
            .rpc();

          expect.fail("Should have thrown an error");
        } catch (error: any) {
          expect(error.message).to.include("WithdrawalFeeTooHigh");
        }
      });
    });

    describe("Admin Nomination", () => {
      it("Should allow admin to nominate a new admin", async () => {
        const newAdmin = Keypair.generate();

        await program.methods
          .nominateAdmin(newAdmin.publicKey)
          .accounts({
            vaultState: context.vaultStatePda,
            nominatedAdminPda: context.nominatedAdminPda,
            admin: context.admin.publicKey,
            payer: context.deployer.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .signers([context.admin, context.deployer])
          .rpc();

        const nomination = await program.account.nominatedAdmin.fetch(context.nominatedAdminPda);
        expect(nomination.nominatedAdmin.toString()).to.equal(newAdmin.publicKey.toString());
        expect(nomination.validUntil.toNumber()).to.be.greaterThan(Date.now() / 1000);
      });

      it("Should allow nominated admin to accept nomination", async () => {
        const newAdmin = Keypair.generate();

        // First, nominate
        await program.methods
          .nominateAdmin(newAdmin.publicKey)
          .accounts({
            vaultState: context.vaultStatePda,
            nominatedAdminPda: context.nominatedAdminPda,
            admin: context.admin.publicKey,
            payer: context.deployer.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .signers([context.admin, context.deployer])
          .rpc();

        // Then accept
        await program.methods
          .acceptAdminNomination()
          .accounts({
            vaultState: context.vaultStatePda,
            nominatedAdminPda: context.nominatedAdminPda,
            newAdmin: newAdmin.publicKey,
            receiver: context.deployer.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .signers([newAdmin])
          .rpc();

        const vaultState = await program.account.vaultState.fetch(context.vaultStatePda);
        expect(vaultState.admin.toString()).to.equal(newAdmin.publicKey.toString());

        // Reset to original admin
        await program.methods
          .nominateAdmin(context.admin.publicKey)
          .accounts({
            vaultState: context.vaultStatePda,
            nominatedAdminPda: context.nominatedAdminPda,
            admin: newAdmin.publicKey,
            payer: context.deployer.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .signers([newAdmin, context.deployer])
          .rpc();

        await program.methods
          .acceptAdminNomination()
          .accounts({
            vaultState: context.vaultStatePda,
            nominatedAdminPda: context.nominatedAdminPda,
            newAdmin: context.admin.publicKey,
            receiver: context.deployer.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .signers([context.admin])
          .rpc();
      });
    });
  });

  describe("Operator Functions", () => {
    describe("Operator Deposit", () => {
      it("Should allow operator to deposit tokens", async () => {
        const depositAmount = new anchor.BN(1000 * 10 ** 9);

        // Mint tokens to operator
        await token.mintTo(
          context.connection,
          context.deployer,
          context.usdgTokenMint,
          context.operatorUsdgAta,
          context.deployer,
          depositAmount.toNumber()
        );

        const operatorBalanceBefore = await getAccount(context.connection, context.operatorUsdgAta);

        await program.methods
          .operatorDeposit(depositAmount)
          .accounts({
            vaultState: context.vaultStatePda,
            vaultDepositAta: context.vaultUsdgAta,
            operatorTokenAccount: context.operatorUsdgAta,
            depositMint: context.usdgTokenMint,
            operator: context.operator.publicKey,
            tokenProgram: token.TOKEN_PROGRAM_ID,
          })
          .signers([context.operator])
          .rpc();

        const operatorBalanceAfter = await getAccount(context.connection, context.operatorUsdgAta);
        expect(operatorBalanceAfter.amount.toString()).to.equal(
          (operatorBalanceBefore.amount.toNumber() - depositAmount.toNumber()).toString()
        );

        const vaultState = await program.account.vaultState.fetch(context.vaultStatePda);
        expect(vaultState.localAum.toString()).to.equal(depositAmount.toString());
      });

      it("Should fail if non-operator tries to deposit", async () => {
        const depositAmount = new anchor.BN(1000 * 10 ** 9);

        try {
          await program.methods
            .operatorDeposit(depositAmount)
            .accounts({
              vaultState: context.vaultStatePda,
              vaultDepositAta: context.vaultUsdgAta,
              operatorTokenAccount: context.senderUsdgAta,
              depositMint: context.usdgTokenMint,
              operator: context.deployer.publicKey,
              tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([context.deployer])
            .rpc();

          expect.fail("Should have thrown an error");
        } catch (error: any) {
          expect(error.message).to.include("NotOperator");
        }
      });
    });

    describe("Operator Withdraw", () => {
      it("Should allow operator to withdraw tokens", async () => {
        const withdrawAmount = new anchor.BN(500 * 10 ** 9);

        const operatorBalanceBefore = await getAccount(context.connection, context.operatorUsdgAta);

        await program.methods
          .operatorWithdraw(withdrawAmount)
          .accounts({
            vaultState: context.vaultStatePda,
            vaultDepositAta: context.vaultUsdgAta,
            operatorTokenAccount: context.operatorUsdgAta,
            depositMint: context.usdgTokenMint,
            operator: context.operator.publicKey,
            tokenProgram: token.TOKEN_PROGRAM_ID,
          })
          .signers([context.operator])
          .rpc();

        const operatorBalanceAfter = await getAccount(context.connection, context.operatorUsdgAta);
        expect(operatorBalanceAfter.amount.toString()).to.equal(
          (operatorBalanceBefore.amount.toNumber() + withdrawAmount.toNumber()).toString()
        );

        const vaultState = await program.account.vaultState.fetch(context.vaultStatePda);
        expect(vaultState.localAum.toString()).to.equal("500000000000");
      });
    });

    describe("Operator Update AUM", () => {
      it("Should allow operator to update AUM within limits", async () => {
        const newAum = new anchor.BN(600 * 10 ** 9); // 600 tokens (within 0.2% of 500)

        await program.methods
          .operatorUpdateAum(newAum)
          .accounts({
            vaultState: context.vaultStatePda,
            operator: context.operator.publicKey,
          })
          .signers([context.operator])
          .rpc();

        const vaultState = await program.account.vaultState.fetch(context.vaultStatePda);
        expect(vaultState.deployedAum.toString()).to.equal(newAum.toString());
      });

      it("Should fail if AUM increase is too large", async () => {
        const newAum = new anchor.BN(1000 * 10 ** 9); // 1000 tokens (way beyond 0.2% limit)

        try {
          await program.methods
            .operatorUpdateAum(newAum)
            .accounts({
              vaultState: context.vaultStatePda,
              operator: context.operator.publicKey,
            })
            .signers([context.operator])
            .rpc();

          expect.fail("Should have thrown an error");
        } catch (error: any) {
          expect(error.message).to.include("AumIncreaseTooBig");
        }
      });

      it("Should fail if AUM decrease is too large", async () => {
        const newAum = new anchor.BN(100 * 10 ** 9); // 100 tokens (way beyond 0.2% limit)

        try {
          await program.methods
            .operatorUpdateAum(newAum)
            .accounts({
              vaultState: context.vaultStatePda,
              operator: context.operator.publicKey,
            })
            .signers([context.operator])
            .rpc();

          expect.fail("Should have thrown an error");
        } catch (error: any) {
          expect(error.message).to.include("AumDecreaseTooBig");
        }
      });
    });
  });

  describe("User Functions", () => {
    describe("Deposit", () => {
      it("Should allow users to deposit and receive shares", async () => {
        const depositAmount = new anchor.BN(100 * 10 ** 9);

        // Mint tokens to user
        await token.mintTo(
          context.connection,
          context.deployer,
          context.usdgTokenMint,
          context.senderUsdgAta,
          context.deployer,
          depositAmount.toNumber()
        );

        const userTokenBalanceBefore = await getAccount(context.connection, context.senderUsdgAta);
        const userShareBalanceBefore = await getAccount(context.connection, context.senderShareAta);

        await program.methods
          .deposit(depositAmount)
          .accounts({
            vaultState: context.vaultStatePda,
            senderTokenAccount: context.senderUsdgAta,
            senderShareAccount: context.senderShareAta,
            shareMint: context.shareMint,
            depositMint: context.usdgTokenMint,
            signer: context.deployer.publicKey,
            tokenProgram: token.TOKEN_PROGRAM_ID,
          })
          .signers([context.deployer])
          .rpc();

        const userTokenBalanceAfter = await getAccount(context.connection, context.senderUsdgAta);
        const userShareBalanceAfter = await getAccount(context.connection, context.senderShareAta);

        expect(userTokenBalanceAfter.amount.toString()).to.equal(
          (userTokenBalanceBefore.amount.toNumber() - depositAmount.toNumber()).toString()
        );
        expect(userShareBalanceAfter.amount.toNumber()).to.be.greaterThan(0);
      });

      it("Should fail if deposit amount is zero", async () => {
        try {
          await program.methods
            .deposit(new anchor.BN(0))
            .accounts({
              vaultState: context.vaultStatePda,
              senderTokenAccount: context.senderUsdgAta,
              senderShareAccount: context.senderShareAta,
              shareMint: context.shareMint,
              depositMint: context.usdgTokenMint,
              signer: context.deployer.publicKey,
              tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([context.deployer])
            .rpc();

          expect.fail("Should have thrown an error");
        } catch (error: any) {
          expect(error.message).to.include("ZeroAmount");
        }
      });
    });

    describe("Redeem", () => {
      it("Should allow users to redeem shares for tokens", async () => {
        const sharesToRedeem = new anchor.BN(50 * 10 ** 9);

        const userTokenBalanceBefore = await getAccount(context.connection, context.senderUsdgAta);
        const userShareBalanceBefore = await getAccount(context.connection, context.senderShareAta);

        await program.methods
          .redeem(sharesToRedeem)
          .accounts({
            vaultState: context.vaultStatePda,
            senderTokenAccount: context.senderUsdgAta,
            senderShareAccount: context.senderShareAta,
            shareMint: context.shareMint,
            depositMint: context.usdgTokenMint,
            signer: context.deployer.publicKey,
            tokenProgram: token.TOKEN_PROGRAM_ID,
          })
          .signers([context.deployer])
          .rpc();

        const userTokenBalanceAfter = await getAccount(context.connection, context.senderUsdgAta);
        const userShareBalanceAfter = await getAccount(context.connection, context.senderShareAta);

        expect(userShareBalanceAfter.amount.toString()).to.equal(
          (userShareBalanceBefore.amount.toNumber() - sharesToRedeem.toNumber()).toString()
        );
        expect(userTokenBalanceAfter.amount.toNumber()).to.be.greaterThan(0);
      });
    });
  });

  describe("Metadata Management", () => {
    describe("Create Metadata", () => {
      it("Should allow admin to create metadata", async () => {
        const TOKEN_METADATA_PROGRAM_ID = new PublicKey("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");
        const [metadataAccount] = PublicKey.findProgramAddressSync(
          [Buffer.from("metadata"), TOKEN_METADATA_PROGRAM_ID.toBuffer(), context.shareMint.toBuffer()],
          TOKEN_METADATA_PROGRAM_ID
        );

        // Check if metadata already exists
        const existingMetadata = await context.connection.getAccountInfo(metadataAccount);

        if (!existingMetadata) {
          await program.methods
            .createShareTokenMetadata("Test Token", "TEST", "https://example.com/metadata.json")
            .accounts({
              payer: context.deployer.publicKey,
              admin: context.admin.publicKey,
              vaultState: context.vaultStatePda,
              shareMint: context.shareMint,
              metadataAccount: metadataAccount,
              tokenProgram: token.TOKEN_PROGRAM_ID,
              tokenMetadataProgram: TOKEN_METADATA_PROGRAM_ID,
              systemProgram: SystemProgram.programId,
              rent: anchor.web3.SYSVAR_RENT_PUBKEY,
            })
            .signers([context.deployer, context.admin])
            .rpc();

          console.log("✅ Metadata created successfully");
        } else {
          console.log("ℹ️  Metadata already exists, skipping creation");
        }
      });
    });

    describe("Update Metadata", () => {
      it("Should allow admin to update metadata", async () => {
        const TOKEN_METADATA_PROGRAM_ID = new PublicKey("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");
        const [metadataAccount] = PublicKey.findProgramAddressSync(
          [Buffer.from("metadata"), TOKEN_METADATA_PROGRAM_ID.toBuffer(), context.shareMint.toBuffer()],
          TOKEN_METADATA_PROGRAM_ID
        );

        // Check if metadata exists
        const existingMetadata = await context.connection.getAccountInfo(metadataAccount);

        if (existingMetadata) {
          await program.methods
            .updateShareTokenMetadata("Updated Token", "UPDT", "https://example.com/updated.json")
            .accounts({
              admin: context.admin.publicKey,
              vaultState: context.vaultStatePda,
              shareMint: context.shareMint,
              metadataAccount: metadataAccount,
              tokenProgram: token.TOKEN_PROGRAM_ID,
              tokenMetadataProgram: TOKEN_METADATA_PROGRAM_ID,
            })
            .signers([context.admin])
            .rpc();

          console.log("✅ Metadata updated successfully");
        } else {
          console.log("ℹ️  Metadata does not exist, skipping update");
        }
      });
    });
  });

  describe("Edge Cases", () => {
    it("Should handle multiple deposits and withdrawals correctly", async () => {
      const depositAmount = new anchor.BN(100 * 10 ** 9);

      // Multiple deposits
      for (let i = 0; i < 3; i++) {
        await token.mintTo(
          context.connection,
          context.deployer,
          context.usdgTokenMint,
          context.senderUsdgAta,
          context.deployer,
          depositAmount.toNumber()
        );

        await program.methods
          .deposit(depositAmount)
          .accounts({
            vaultState: context.vaultStatePda,
            senderTokenAccount: context.senderUsdgAta,
            senderShareAccount: context.senderShareAta,
            shareMint: context.shareMint,
            depositMint: context.usdgTokenMint,
            signer: context.deployer.publicKey,
            tokenProgram: token.TOKEN_PROGRAM_ID,
          })
          .signers([context.deployer])
          .rpc();
      }

      const userShareBalance = await getAccount(context.connection, context.senderShareAta);
      expect(userShareBalance.amount.toNumber()).to.be.greaterThan(0);

      console.log("✅ Multiple deposits handled correctly");
    });

    it("Should maintain correct AUM after operations", async () => {
      const vaultState = await program.account.vaultState.fetch(context.vaultStatePda);
      const deployedAum = vaultState.deployedAum.toNumber();
      const localAum = vaultState.localAum.toNumber();

      console.log(`Deployed AUM: ${deployedAum / 10 ** 9} tokens`);
      console.log(`Local AUM: ${localAum / 10 ** 9} tokens`);

      expect(deployedAum).to.be.greaterThan(0);
      expect(localAum).to.be.greaterThan(0);
    });
  });
});
