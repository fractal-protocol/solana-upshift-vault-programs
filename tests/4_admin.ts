// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE.BSL file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

import * as anchor from "@coral-xyz/anchor";
import * as token from "@solana/spl-token"
import * as assert from "assert";
import { VaultContext } from "./helper/context";
import BN from "bn.js";

describe("august-vault-admin", () => {
    anchor.setProvider(anchor.AnchorProvider.env());
    let vaultContext: VaultContext
    let amount = 1 * 10 ** 6
    const FEE_RATE_DENOMINATOR_VALUE = 1000000;
    const threePercentFee = 3 * FEE_RATE_DENOMINATOR_VALUE / 100;

    before(async () => {
        vaultContext = new VaultContext
        await vaultContext.init()
    });

    it("Admin can update Withdrawal Fee", async () => {
        await token.mintTo(
            vaultContext.connection,
            vaultContext.deployer,
            vaultContext.usdgTokenMint,
            vaultContext.senderUsdgAta,
            vaultContext.deployer.publicKey,
            2 * amount
        )

        let vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        assert.equal(vault.withdrawalFee, 0);

        const senderBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderUsdgAta
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
                tokenProgram: token.TOKEN_PROGRAM_ID
            })
            .signers([vaultContext.deployer])
            .rpc();

        const senderShareBalance = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount

        await vaultContext.vaultProgram.methods.setWithdrawalFee(threePercentFee)
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                depositMint: vaultContext.usdgTokenMint,
                admin: vaultContext.admin.publicKey,
            })
            .signers([vaultContext.admin])
            .rpc();

        vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        assert.equal(vault.withdrawalFee, threePercentFee);

        await vaultContext.vaultProgram.methods
            .redeem(new BN(senderShareBalance))
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

        const fees = amount * threePercentFee / FEE_RATE_DENOMINATOR_VALUE;
        assert.notEqual(fees, 0);
        assert.equal(senderBalanceBefore - senderBalanceAfter, fees)
    });

    it('Fee recipient should collect the fees', async () => {
        const feeRecipientBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.feeRecipientUsdgAta
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

        const userShareBalanceBefore = (await token.getAccount(
            vaultContext.connection,
            vaultContext.senderShareAta
        )).amount

        await vaultContext.vaultProgram.methods
            .redeem(new BN(userShareBalanceBefore))
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

        const feeRecipientBalanceAfter = (await token.getAccount(
            vaultContext.connection,
            vaultContext.feeRecipientUsdgAta
        )).amount

        const fees = amount * threePercentFee / FEE_RATE_DENOMINATOR_VALUE;
        assert.notEqual(fees, 0);
        assert.equal(feeRecipientBalanceAfter - feeRecipientBalanceBefore, fees)
    });

    it('Only admin can update Withdrawal fee', async () => {
        let vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        const withdrawalFeeBefore = vault.withdrawalFee;

        await assert.rejects(vaultContext.vaultProgram.methods.setWithdrawalFee(withdrawalFeeBefore * 2)
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                depositMint: vaultContext.usdgTokenMint,
                admin: vaultContext.deployer.publicKey,
            })
            .signers([vaultContext.deployer])
            .rpc());
        /Signer is Not Admin/
        vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        assert.equal(withdrawalFeeBefore, vault.withdrawalFee);
    });

    it('Only admin can nominate new admin', async () => {
        await assert.rejects(vaultContext.vaultProgram.methods.nominateAdmin(vaultContext.deployer.publicKey)
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                nominatedAdminPda: vaultContext.nominatedAdminPda,
                admin: vaultContext.deployer.publicKey,
                payer: vaultContext.deployer.publicKey,
                systemProgram: anchor.web3.SystemProgram.programId,
            })
            .signers([vaultContext.deployer, vaultContext.admin])
            .rpc());
        // Signer is Not Admin

        let vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        assert.deepEqual(vault.admin, vaultContext.admin.publicKey);

        await vaultContext.vaultProgram.methods.nominateAdmin(vaultContext.deployer.publicKey)
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                nominatedAdminPda: vaultContext.nominatedAdminPda,
                admin: vaultContext.admin.publicKey,
                payer: vaultContext.deployer.publicKey,
                systemProgram: anchor.web3.SystemProgram.programId,
            })
            .signers([vaultContext.deployer, vaultContext.admin])
            .rpc();

        // Now the nominated admin can accept the nomination
        await vaultContext.vaultProgram.methods.acceptAdminNomination()
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                nominatedAdminPda: vaultContext.nominatedAdminPda,
                newAdmin: vaultContext.deployer.publicKey,
                receiver: vaultContext.deployer.publicKey,
                systemProgram: anchor.web3.SystemProgram.programId,
            })
            .signers([vaultContext.deployer])
            .rpc();

        vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        assert.deepEqual(vault.admin, vaultContext.deployer.publicKey);

        // Reset to admin as admin of Vault using the same two-step process
        await vaultContext.vaultProgram.methods.nominateAdmin(vaultContext.admin.publicKey)
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                nominatedAdminPda: vaultContext.nominatedAdminPda,
                admin: vaultContext.deployer.publicKey,
                payer: vaultContext.deployer.publicKey,
                systemProgram: anchor.web3.SystemProgram.programId,
            })
            .signers([vaultContext.deployer])
            .rpc();

        await vaultContext.vaultProgram.methods.acceptAdminNomination()
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                nominatedAdminPda: vaultContext.nominatedAdminPda,
                newAdmin: vaultContext.admin.publicKey,
                receiver: vaultContext.admin.publicKey,
                systemProgram: anchor.web3.SystemProgram.programId,
            })
            .signers([vaultContext.admin])
            .rpc();
    });

    it('Only admin can set Operator', async () => {
        await assert.rejects(vaultContext.vaultProgram.methods.setOperator(vaultContext.deployer.publicKey)
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                depositMint: vaultContext.usdgTokenMint,
                admin: vaultContext.deployer.publicKey,
            })
            .signers([vaultContext.deployer])
            .rpc());
        /Signer is Not Admin/

        let vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        assert.deepEqual(vault.operator, vaultContext.operator.publicKey);

        await vaultContext.vaultProgram.methods.setOperator(vaultContext.deployer.publicKey)
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                depositMint: vaultContext.usdgTokenMint,
                admin: vaultContext.admin.publicKey,
            })
            .signers([vaultContext.admin])
            .rpc();

        vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        assert.deepEqual(vault.operator, vaultContext.deployer.publicKey);

        //reset to Operator as operator of Vault
        await vaultContext.vaultProgram.methods.setOperator(vaultContext.operator.publicKey)
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                depositMint: vaultContext.usdgTokenMint,
                admin: vaultContext.admin.publicKey,
            })
            .signers([vaultContext.admin])
            .rpc();
    });

    it('Only Admin can update Fee recipient', async () => {
        await assert.rejects(vaultContext.vaultProgram.methods.setFeeRecipient(vaultContext.deployer.publicKey)
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                depositMint: vaultContext.usdgTokenMint,
                admin: vaultContext.deployer.publicKey,
            })
            .signers([vaultContext.deployer])
            .rpc());
        /Signer is Not Admin/

        let vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        assert.deepEqual(vault.feeRecipient, vaultContext.feeRecipient.publicKey);

        await vaultContext.vaultProgram.methods.setFeeRecipient(vaultContext.deployer.publicKey)
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                depositMint: vaultContext.usdgTokenMint,
                admin: vaultContext.admin.publicKey,
            })
            .signers([vaultContext.admin])
            .rpc();

        vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        assert.deepEqual(vault.feeRecipient, vaultContext.deployer.publicKey);

        //reset to fee recipient as fee recipient of Vault
        await vaultContext.vaultProgram.methods.setFeeRecipient(vaultContext.feeRecipient.publicKey)
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                depositMint: vaultContext.usdgTokenMint,
                admin: vaultContext.admin.publicKey,
            })
            .signers([vaultContext.admin])
            .rpc();
    });

    it('Only Admin can pause the vault', async () => {
        await assert.rejects(vaultContext.vaultProgram.methods.pause()
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                depositMint: vaultContext.usdgTokenMint,
                admin: vaultContext.deployer.publicKey,
            })
            .signers([vaultContext.deployer])
            .rpc());
        /Signer is Not Admin/

        let vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        assert.equal(vault.paused, false);

        await vaultContext.vaultProgram.methods.pause()
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                depositMint: vaultContext.usdgTokenMint,
                admin: vaultContext.admin.publicKey,
            })
            .signers([vaultContext.admin])
            .rpc();

        vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        assert.equal(vault.paused, true);

        await vaultContext.vaultProgram.methods.unpause()
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                depositMint: vaultContext.usdgTokenMint,
                admin: vaultContext.admin.publicKey,
            })
            .signers([vaultContext.admin])
            .rpc();

        vault = await vaultContext.vaultProgram.account.vaultState.fetch(vaultContext.vaultStatePda);
        assert.equal(vault.paused, false);

        //reset to paused as paused of Vault
        await vaultContext.vaultProgram.methods.unpause()
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                depositMint: vaultContext.usdgTokenMint,
                admin: vaultContext.admin.publicKey,
            })
            .signers([vaultContext.admin])
            .rpc();
    });

    it('Should not be possible to set a Withdrawal Fee > 10%', async () => {
        await assert.rejects(vaultContext.vaultProgram.methods.setWithdrawalFee(10 * FEE_RATE_DENOMINATOR_VALUE / 100)
            .accounts({
                vaultState: vaultContext.vaultStatePda,
                depositMint: vaultContext.usdgTokenMint,
                admin: vaultContext.admin.publicKey,
            })
            .signers([vaultContext.admin])
            .rpc());
    });
});
