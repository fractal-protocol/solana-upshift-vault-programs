// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

import * as anchor from "@coral-xyz/anchor";
import * as token from "@solana/spl-token"
import * as assert from "assert";
import {VaultContext} from "./helper/context";
import { DEFAULT_SHARE_OFFSET, MIN_FIRST_DEPOSIT, MIN_SUPPLY_MULTIPLE } from "./helper/config";
import BN from "bn.js";
import {expect} from "chai";

/**
 * Tests for redeem CEI (Check-Effects-Interactions) pattern compliance.
 *
 * These tests verify that when a redeem operation fails due to insufficient
 * liquidity, no state changes occur:
 * - Shares are NOT burned
 * - No fees are transferred
 * - local_aum is NOT modified
 *
 * This addresses the security finding C-01 from the security audit.
 */
describe("august-vault-redeem-cei-pattern", () => {
    anchor.setProvider(anchor.AnchorProvider.env());
    let vaultContext: VaultContext;

    // Derived in `before`, because this suite shares a vault with the earlier
    // ones and what they leave behind changes what it takes to open it again.
    let depositAmount: number;
    let operatorWithdrawAmount: number;

    before(async () => {
        vaultContext = new VaultContext;
        await vaultContext.init();

        // The program floors the OPENING SHARE SUPPLY at
        // `MIN_SUPPLY_MULTIPLE * share_offset`, checked on the shares minted
        // rather than the amount deposited. Those coincide only for a vault
        // holding nothing: with assets `A` still recorded at zero supply — which
        // is where the preceding suites leave this shared vault — a deposit of
        // `X` mints `X * offset / (A + offset)`, so clearing the floor takes
        // `X >= MIN_SUPPLY_MULTIPLE * (A + offset)`. Derive it from the live
        // state rather than hardcoding, so this suite does not silently depend
        // on how much residue the suites before it happened to leave.
        const state = await vaultContext.vaultProgram.account.vaultState.fetch(
            vaultContext.vaultStatePda
        );
        const totalAssets = state.localAum.toNumber() + state.deployedAum.toNumber();
        const offset = DEFAULT_SHARE_OFFSET.toNumber();
        const requiredToOpen = MIN_SUPPLY_MULTIPLE * (totalAssets + offset);
        depositAmount = Math.max(10 * MIN_FIRST_DEPOSIT, requiredToOpen);
        operatorWithdrawAmount = Math.floor(depositAmount * 9 / 10);  // reduce local_aum

        // Mint tokens to depositor
        await token.mintTo(
            vaultContext.connection,
            vaultContext.deployer,
            vaultContext.usdgTokenMint,
            vaultContext.senderUsdgAta,
            vaultContext.deployer.publicKey,
            depositAmount
        );

        // Make initial deposit to get shares
        await vaultContext.vaultProgram.methods
            .deposit(new BN(depositAmount))
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                vaultTokenAta: vaultContext.vaultUsdgAta,
                senderTokenAccount: vaultContext.senderUsdgAta,
                senderShareAccount: vaultContext.senderShareAta,
                shareMint: vaultContext.shareMint,
                depositMint: vaultContext.usdgTokenMint,
                signer: vaultContext.deployer.publicKey,
                tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([vaultContext.deployer])
            .rpc();

        // Operator withdraws most of the funds to reduce local_aum
        // This simulates the scenario where local liquidity is low
        await vaultContext.vaultProgram.methods
            .operatorWithdraw(new BN(operatorWithdrawAmount))
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                vaultDepositAta: vaultContext.vaultUsdgAta,
                operatorTokenAccount: vaultContext.operatorUsdgAta,
                depositMint: vaultContext.usdgTokenMint,
                operator: vaultContext.operator.publicKey,
                tokenProgram: token.TOKEN_PROGRAM_ID
            })
            .signers([vaultContext.operator])
            .rpc();
    });

    after(async () => {
        // Cleanup: operator deposits back funds and user redeems
        const operatorBalance = (await token.getAccount(
            vaultContext.connection,
            vaultContext.operatorUsdgAta
        )).amount;

        if (operatorBalance > 0) {
            await vaultContext.vaultProgram.methods
                .operatorDeposit(new BN(operatorBalance))
                .accounts({
                    vaultState: vaultContext.vaultStatePda,
                    vaultDepositAta: vaultContext.vaultUsdgAta,
                    operatorTokenAccount: vaultContext.operatorUsdgAta,
                    depositMint: vaultContext.usdgTokenMint,
                    operator: vaultContext.operator.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID
                })
                .signers([vaultContext.operator])
                .rpc();
        }

        const senderShares = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount;

        if (senderShares > 0) {
            await vaultContext.vaultProgram.methods
                .redeem(new BN(senderShares))
                .accounts({
                    vaultState: vaultContext.vaultStatePda,
                    vaultDepositAta: vaultContext.vaultUsdgAta,
                    senderTokenAccount: vaultContext.senderUsdgAta,
                    senderShareAccount: vaultContext.senderShareAta,
                    feeRecipientAccount: vaultContext.feeRecipientUsdgAta,
                    shareMint: vaultContext.shareMint,
                    depositMint: vaultContext.usdgTokenMint,
                    signer: vaultContext.deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID
                })
                .signers([vaultContext.deployer])
                .rpc();
        }
    });

    it("Should fail with NotEnoughLiquidity when redeeming more than local_aum", async () => {
        const vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        const senderShares = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount;

        // Verify setup: local_aum should be low (only ~10% of deposit)
        expect(vault.localAum.toNumber()).to.be.lessThan(depositAmount);
        expect(Number(senderShares)).to.be.greaterThan(0);

        // Try to redeem all shares - this should fail because local_aum is insufficient
        await assert.rejects(
            vaultContext.vaultProgram.methods
                .redeem(new BN(senderShares))
                .accounts({
                    vaultState: vaultContext.vaultStatePda,
                    vaultDepositAta: vaultContext.vaultUsdgAta,
                    senderTokenAccount: vaultContext.senderUsdgAta,
                    senderShareAccount: vaultContext.senderShareAta,
                    feeRecipientAccount: vaultContext.feeRecipientUsdgAta,
                    shareMint: vaultContext.shareMint,
                    depositMint: vaultContext.usdgTokenMint,
                    signer: vaultContext.deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID
                })
                .signers([vaultContext.deployer])
                .rpc(),
            /NotEnoughLiquidity|Not Enough Liquidity/
        );
    });

    it("CEI: Shares should NOT be burned when redeem fails due to insufficient liquidity", async () => {
        // Get share balance BEFORE the failed redeem attempt
        const sharesBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount;

        // Try to redeem all shares (should fail)
        try {
            await vaultContext.vaultProgram.methods
                .redeem(new BN(sharesBefore))
                .accounts({
                    vaultState: vaultContext.vaultStatePda,
                    vaultDepositAta: vaultContext.vaultUsdgAta,
                    senderTokenAccount: vaultContext.senderUsdgAta,
                    senderShareAccount: vaultContext.senderShareAta,
                    feeRecipientAccount: vaultContext.feeRecipientUsdgAta,
                    shareMint: vaultContext.shareMint,
                    depositMint: vaultContext.usdgTokenMint,
                    signer: vaultContext.deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID
                })
                .signers([vaultContext.deployer])
                .rpc();

            assert.fail("Expected redeem to fail with NotEnoughLiquidity");
        } catch (error: any) {
            // Verify it failed with the expected error
            expect(error.toString()).to.match(/NotEnoughLiquidity|Not Enough Liquidity/);
        }

        // Get share balance AFTER the failed redeem attempt
        const sharesAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount;

        // CEI compliance: shares should be UNCHANGED (not burned)
        assert.equal(
            Number(sharesAfter),
            Number(sharesBefore),
            "Shares should NOT be burned when redeem fails - CEI pattern violated!"
        );
    });

    it("CEI: Fee recipient balance should NOT change when redeem fails", async () => {
        const senderShares = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount;

        // Get fee recipient balance BEFORE the failed redeem attempt
        const feeRecipientBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.feeRecipientUsdgAta
        )).amount;

        // Try to redeem all shares (should fail)
        try {
            await vaultContext.vaultProgram.methods
                .redeem(new BN(senderShares))
                .accounts({
                    vaultState: vaultContext.vaultStatePda,
                    vaultDepositAta: vaultContext.vaultUsdgAta,
                    senderTokenAccount: vaultContext.senderUsdgAta,
                    senderShareAccount: vaultContext.senderShareAta,
                    feeRecipientAccount: vaultContext.feeRecipientUsdgAta,
                    shareMint: vaultContext.shareMint,
                    depositMint: vaultContext.usdgTokenMint,
                    signer: vaultContext.deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID
                })
                .signers([vaultContext.deployer])
                .rpc();

            assert.fail("Expected redeem to fail with NotEnoughLiquidity");
        } catch (error: any) {
            expect(error.toString()).to.match(/NotEnoughLiquidity|Not Enough Liquidity/);
        }

        // Get fee recipient balance AFTER the failed redeem attempt
        const feeRecipientBalanceAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.feeRecipientUsdgAta
        )).amount;

        // CEI compliance: fee recipient balance should be UNCHANGED
        assert.equal(
            Number(feeRecipientBalanceAfter),
            Number(feeRecipientBalanceBefore),
            "Fee recipient balance should NOT change when redeem fails - CEI pattern violated!"
        );
    });

    it("CEI: local_aum should NOT change when redeem fails", async () => {
        const senderShares = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount;

        // Get vault state BEFORE the failed redeem attempt
        const vaultBefore = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);

        // Try to redeem all shares (should fail)
        try {
            await vaultContext.vaultProgram.methods
                .redeem(new BN(senderShares))
                .accounts({
                    vaultState: vaultContext.vaultStatePda,
                    vaultDepositAta: vaultContext.vaultUsdgAta,
                    senderTokenAccount: vaultContext.senderUsdgAta,
                    senderShareAccount: vaultContext.senderShareAta,
                    feeRecipientAccount: vaultContext.feeRecipientUsdgAta,
                    shareMint: vaultContext.shareMint,
                    depositMint: vaultContext.usdgTokenMint,
                    signer: vaultContext.deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID
                })
                .signers([vaultContext.deployer])
                .rpc();

            assert.fail("Expected redeem to fail with NotEnoughLiquidity");
        } catch (error: any) {
            expect(error.toString()).to.match(/NotEnoughLiquidity|Not Enough Liquidity/);
        }

        // Get vault state AFTER the failed redeem attempt
        const vaultAfter = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);

        // CEI compliance: local_aum should be UNCHANGED
        assert.equal(
            vaultAfter.localAum.toNumber(),
            vaultBefore.localAum.toNumber(),
            "local_aum should NOT change when redeem fails - CEI pattern violated!"
        );
    });

    it("CEI: Sender token account balance should NOT change when redeem fails", async () => {
        const senderShares = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount;

        // Get sender token balance BEFORE the failed redeem attempt
        const senderBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderUsdgAta
        )).amount;

        // Try to redeem all shares (should fail)
        try {
            await vaultContext.vaultProgram.methods
                .redeem(new BN(senderShares))
                .accounts({
                    vaultState: vaultContext.vaultStatePda,
                    vaultDepositAta: vaultContext.vaultUsdgAta,
                    senderTokenAccount: vaultContext.senderUsdgAta,
                    senderShareAccount: vaultContext.senderShareAta,
                    feeRecipientAccount: vaultContext.feeRecipientUsdgAta,
                    shareMint: vaultContext.shareMint,
                    depositMint: vaultContext.usdgTokenMint,
                    signer: vaultContext.deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID
                })
                .signers([vaultContext.deployer])
                .rpc();

            assert.fail("Expected redeem to fail with NotEnoughLiquidity");
        } catch (error: any) {
            expect(error.toString()).to.match(/NotEnoughLiquidity|Not Enough Liquidity/);
        }

        // Get sender token balance AFTER the failed redeem attempt
        const senderBalanceAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderUsdgAta
        )).amount;

        // CEI compliance: sender token balance should be UNCHANGED
        assert.equal(
            Number(senderBalanceAfter),
            Number(senderBalanceBefore),
            "Sender token balance should NOT change when redeem fails - CEI pattern violated!"
        );
    });

    it("CEI: Vault token account balance should NOT change when redeem fails", async () => {
        const senderShares = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount;

        // Get vault token balance BEFORE the failed redeem attempt
        const vaultBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.vaultUsdgAta
        )).amount;

        // Try to redeem all shares (should fail)
        try {
            await vaultContext.vaultProgram.methods
                .redeem(new BN(senderShares))
                .accounts({
                    vaultState: vaultContext.vaultStatePda,
                    vaultDepositAta: vaultContext.vaultUsdgAta,
                    senderTokenAccount: vaultContext.senderUsdgAta,
                    senderShareAccount: vaultContext.senderShareAta,
                    feeRecipientAccount: vaultContext.feeRecipientUsdgAta,
                    shareMint: vaultContext.shareMint,
                    depositMint: vaultContext.usdgTokenMint,
                    signer: vaultContext.deployer.publicKey,
                    tokenProgram: token.TOKEN_PROGRAM_ID
                })
                .signers([vaultContext.deployer])
                .rpc();

            assert.fail("Expected redeem to fail with NotEnoughLiquidity");
        } catch (error: any) {
            expect(error.toString()).to.match(/NotEnoughLiquidity|Not Enough Liquidity/);
        }

        // Get vault token balance AFTER the failed redeem attempt
        const vaultBalanceAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.vaultUsdgAta
        )).amount;

        // CEI compliance: vault token balance should be UNCHANGED
        assert.equal(
            Number(vaultBalanceAfter),
            Number(vaultBalanceBefore),
            "Vault token balance should NOT change when redeem fails - CEI pattern violated!"
        );
    });

    it("Should allow partial redeem within available liquidity", async () => {
        // This test verifies that valid redeems (within liquidity) still work after the CEI fix.
        // It may be skipped when run after other test suites that change the vault state.

        const vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        const localAum = vault.localAum.toNumber();
        const totalAssets = localAum + vault.deployedAum.toNumber();
        const supply = Number((await token.getMint(vaultContext.connection, vaultContext.shareMint)).supply);

        const senderShares = Number((await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount);

        // Calculate how many shares we can safely redeem (assets must be <= localAum)
        // Using conservative estimate: shares * totalAssets / supply <= localAum * 0.5
        // So: shares <= localAum * 0.5 * supply / totalAssets
        const maxSafeShares = supply > 0 && totalAssets > 0
            ? Math.floor((localAum * 0.5 * supply) / totalAssets)
            : 0;

        const safeRedeemShares = Math.min(maxSafeShares, senderShares, 50000);

        // Skip if we can't do a meaningful redeem
        if (safeRedeemShares < 100 || localAum < 100) {
            console.log(`  (Skipping partial redeem test - conditions not met for safe redeem)`);
            console.log(`    localAum=${localAum}, totalAssets=${totalAssets}, supply=${supply}`);
            console.log(`    senderShares=${senderShares}, maxSafeShares=${maxSafeShares}, safeRedeemShares=${safeRedeemShares}`);
            return;
        }

        const sharesBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount;

        const vaultBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.vaultUsdgAta
        )).amount;

        // This should succeed
        await vaultContext.vaultProgram.methods
            .redeem(new BN(safeRedeemShares))
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                vaultDepositAta: vaultContext.vaultUsdgAta,
                senderTokenAccount: vaultContext.senderUsdgAta,
                senderShareAccount: vaultContext.senderShareAta,
                feeRecipientAccount: vaultContext.feeRecipientUsdgAta,
                shareMint: vaultContext.shareMint,
                depositMint: vaultContext.usdgTokenMint,
                signer: vaultContext.deployer.publicKey,
                tokenProgram: token.TOKEN_PROGRAM_ID
            })
            .signers([vaultContext.deployer])
            .rpc();

        const sharesAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount;

        const vaultBalanceAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.vaultUsdgAta
        )).amount;

        // Verify shares were burned
        expect(Number(sharesAfter)).to.be.lessThan(Number(sharesBefore));
        // Verify vault balance decreased
        expect(Number(vaultBalanceAfter)).to.be.lessThan(Number(vaultBalanceBefore));
    });
});
