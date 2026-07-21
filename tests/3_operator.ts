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
import {PublicKey} from "@solana/web3.js";
import BN from "bn.js";
import {expect} from "chai";

describe("august-vault-operator", () => {
    anchor.setProvider(anchor.AnchorProvider.env());
    let vaultContext: VaultContext
    let amount = 1 * 10 ** 6  // Must be >= min_deposit (1_000_000 for 9 decimals)
    let deployCapital = 80 * amount / 100;

    before(async () => {
        vaultContext = new VaultContext
        await vaultContext.init()

        await vaultContext.vaultProgram.methods
            .deposit(new BN(amount))
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
    });

    it("Operator should be able to withdraw funds from the vault", async () => {
        const operatorBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.operatorUsdgAta
        )).amount

        const vaultBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.vaultUsdgAta
        )).amount

        const vaultBefore = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);

        await vaultContext.vaultProgram.methods.operatorWithdraw(new BN(amount))
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

        const operatorBalanceAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.operatorUsdgAta
        )).amount

        const vaultBalanceAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.vaultUsdgAta
        )).amount

        assert.equal(Number(vaultBalanceBefore), Number(vaultBalanceAfter) + amount)
        assert.equal(Number(operatorBalanceBefore) + amount, Number(operatorBalanceAfter))

        const vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);

        assert.equal(vault.deployedAum.toNumber(), amount);
        assert.equal(vault.localAum.toNumber(), new BN(vaultBefore.localAum).sub(new BN(amount)));
    });

    it("Should revert if there's not enough liquidity to redeem", async () => {
        const vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        const localAum = vault.localAum;

        await assert.rejects(vaultContext.vaultProgram.methods
            .redeem(new BN(localAum).add(new BN(1)))
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
            .rpc());
        /Not Enough Liquidity/
    });

    it("Operator should be able to deposit funds back on the vault", async () => {
        const operatorBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.operatorUsdgAta
        )).amount

        const vaultBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.vaultUsdgAta
        )).amount

        const vaultBefore = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);

        await vaultContext.vaultProgram.methods.operatorDeposit(new BN(amount))
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

        const operatorBalanceAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.operatorUsdgAta
        )).amount

        const vaultBalanceAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.vaultUsdgAta
        )).amount

        assert.equal(Number(vaultBalanceBefore) + amount, Number(vaultBalanceAfter))
        assert.equal(Number(operatorBalanceBefore), Number(operatorBalanceAfter) + amount)

        const vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);

        assert.equal(vault.deployedAum.toNumber(), 0);
        assert.equal(vault.localAum.toNumber(), new BN(vaultBefore.localAum).add(new BN(amount)));
    });

    it('Should be possible for the operator to update the aum of the vault', async () => {
        await vaultContext.vaultProgram.methods.operatorWithdraw(new BN(deployCapital))
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

        let vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        const deployedAumBefore = vault.deployedAum;


        // Vault limit is 0.2% (20 basis points), so use 0.1% increase to stay within limits
        const updatedAum = Math.floor(1001 * deployedAumBefore.toNumber() / 1000);
        await vaultContext.vaultProgram.methods.operatorUpdateAum(new BN(updatedAum))
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                operator: vaultContext.operator.publicKey,
                depositMint: vaultContext.usdgTokenMint,
            })
            .signers([vaultContext.operator])
            .rpc();

        vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        const deployedAumAfter = vault.deployedAum;

        assert.equal(deployedAumBefore.toNumber(), deployCapital);
        assert.equal(deployedAumAfter.toNumber(), updatedAum);
    });

    it('User should benefit from aum increase', async () => {
        const operatorBalance = (await token.getAccount(
            vaultContext.connection,
            vaultContext.operatorUsdgAta
        )).amount

        await vaultContext.vaultProgram.methods.operatorDeposit(new BN(operatorBalance))
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

        const senderSharesBeforeAumIncrease = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount

        const senderBalanceBeforeAumIncrease = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderUsdgAta
        )).amount

        await vaultContext.vaultProgram.methods
            .redeem(new BN(senderSharesBeforeAumIncrease).div(new BN(2)))
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

        const senderBalanceAfterFirstRedeem = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderUsdgAta
        )).amount

        const delta = new BN(senderBalanceAfterFirstRedeem).sub(new BN(senderBalanceBeforeAumIncrease));

        await token.mintTo(
            vaultContext.connection,
            vaultContext.deployer,
            vaultContext.usdgTokenMint,
            vaultContext.operatorUsdgAta,
            vaultContext.deployer.publicKey,
            10 * amount
        )

        await vaultContext.vaultProgram.methods.operatorDeposit(new BN(10 * amount))
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

        //deposit back the delta
        await vaultContext.vaultProgram.methods
            .deposit(new BN(delta))
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

        const senderSharesAfterAumIncrease = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount

        await vaultContext.vaultProgram.methods
            .redeem(new BN(senderSharesAfterAumIncrease))
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

        const senderBalanceAfterAumIncrease = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderUsdgAta
        )).amount

        expect(Number(senderBalanceAfterAumIncrease)).to.be.greaterThan(Number(senderBalanceAfterFirstRedeem));
    });
});
