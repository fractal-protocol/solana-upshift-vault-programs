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

describe("august-vault-users", () => {
    anchor.setProvider(anchor.AnchorProvider.env());
    let vaultContext: VaultContext

    let amount = 1 * 10 ** 6  // Must be >= min_deposit (1_000_000 for 9 decimals)

    before(async () => {
        vaultContext = new VaultContext
        await vaultContext.init()

        await token.mintTo(
            vaultContext.connection,
            vaultContext.deployer,
            vaultContext.usdgTokenMint,
            vaultContext.senderUsdgAta,
            vaultContext.deployer.publicKey,
            2 * amount
        )
    });

    after(async () => {
        //Burn all shares and get users funds back
        const senderSharesBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount

        if(senderSharesBefore > 0) {
            await vaultContext.vaultProgram.methods
                .redeem(new BN(senderSharesBefore))
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
    })

    it("It should be possible to deposit tokens in the vault", async () => {
        const senderBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderUsdgAta
        )).amount

        const vaultBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.vaultUsdgAta
        )).amount

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

        const senderBalanceAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderUsdgAta
        )).amount

        const vaultBalanceAfter =  (await token.getAccount(
            vaultContext.connection,
            vaultContext.vaultUsdgAta
        )).amount

        assert.equal(Number(senderBalanceBefore), Number(senderBalanceAfter) + amount)
        assert.equal(Number(vaultBalanceBefore) + amount, Number(vaultBalanceAfter))
    });

    it("It should increase local_aum on deposits", async () => {
        const vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        assert.equal(vault.localAum, amount);
    });

    it("It should mint shares on deposits", async () => {
        const senderSharesBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount

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

        const senderSharesAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount

        //Same amount of token deposited
        assert.equal(new BN(senderSharesAfter).toNumber(), new BN(senderSharesBefore).mul(new BN(2)).toNumber())
    })

    it("It should be possible to redeem tokens from the vault", async () => {
        const senderSharesBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount

        const senderBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderUsdgAta
        )).amount

        const vaultBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.vaultUsdgAta
        )).amount


        await vaultContext.vaultProgram.methods
            .redeem(new BN(senderSharesBefore).div(new BN(2)))
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

        const senderBalanceAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderUsdgAta
        )).amount

        const vaultBalanceAfter =  (await token.getAccount(
            vaultContext.connection,
            vaultContext.vaultUsdgAta
        )).amount

        expect(Number(senderBalanceAfter)).to.be.greaterThan(Number(senderBalanceBefore));
        expect(Number(vaultBalanceBefore)).to.be.greaterThan(Number(vaultBalanceAfter));
    });

    it("It should decrease local_aum on deposits", async () => {
        const vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        //we had 2 deposits of amount and 1 withdrawal
        assert.equal(vault.localAum, 2 * amount - amount);
    });


    it("It should burn shares on redeem", async () => {
        const senderSharesBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount

        await vaultContext.vaultProgram.methods
            .redeem(new BN(senderSharesBefore))
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

        const senderSharesAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount

        assert.equal(Number(senderSharesAfter), 0)
    })

    it("It should not be possible to deposit when the vault is paused", async () => {
        await vaultContext.vaultProgram.methods.pause()
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                admin: vaultContext.admin.publicKey,
                depositMint: vaultContext.usdgTokenMint,
            })
            .signers([vaultContext.admin])
            .rpc();

        await assert.rejects(vaultContext.vaultProgram.methods.deposit(new BN(amount))
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
            .rpc());

        //reset to unpause
        await vaultContext.vaultProgram.methods.unpause()
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                admin: vaultContext.admin.publicKey,
                depositMint: vaultContext.usdgTokenMint,
            })
            .signers([vaultContext.admin])
            .rpc();
    });
});
