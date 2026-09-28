// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

// The withdrawal queue end to end on a real validator, with a zero cooldown so
// no clock has to move (the LiteSVM suite covers waiting out a cooldown). Two
// vaults are set up identically, one gated by a queue and one not, so a
// finalize can be checked against what a direct redeem pays at the same state.

import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AugustVault } from "../target/types/august_vault";
import { AugustWithdrawalQueue } from "../target/types/august_withdrawal_queue";
import { PublicKey, Keypair, LAMPORTS_PER_SOL } from "@solana/web3.js";
import * as token from "@solana/spl-token";
import * as assert from "assert";
import { sha256 } from "js-sha256";
import bs58 from "bs58";
import BN from "bn.js";
import { DEFAULT_SHARE_OFFSET } from "./helper/config";
import { protocolAuthority, ensureProgramConfig } from "./helper/program-config";

const DEPOSIT = 10_000_000_000; // 10 tokens at 9 decimals
const WITHDRAWAL_FEE = 1_000; // 0.1%, parts per million
/// The prefix Anchor puts on every `emit_cpi!` self-invocation.
const EVENT_IX_TAG = Buffer.from("e445a52e51cb9a1d", "hex");

describe("august-withdrawal-queue", () => {
    anchor.setProvider(anchor.AnchorProvider.env());

    const connection = anchor.getProvider().connection;
    const vaultProgram = anchor.workspace.AugustVault as Program<AugustVault>;
    const queueProgram = anchor.workspace.AugustWithdrawalQueue as Program<AugustWithdrawalQueue>;

    const seed = (s: string) => Keypair.fromSeed(Uint8Array.from(sha256.digest(`withdrawalQueue${s}`)));
    const deployer = seed("Deployer");
    const operator = seed("Operator");
    const admin = seed("Admin");
    const feeRecipient = seed("FeeRecipient");
    const user = seed("User");
    const keeper = seed("Keeper");
    const stranger = seed("Stranger");

    /// Everything one vault needs; the suite builds two.
    interface Vault {
        depositMint: PublicKey;
        vaultState: PublicKey;
        shareMint: PublicKey;
        vaultTokenAta: PublicKey;
        userDepositAta: PublicKey;
        userShareAta: PublicKey;
        operatorAta: PublicKey;
        feeRecipientAta: PublicKey;
    }

    let queued: Vault;
    let plain: Vault;
    let queue: PublicKey;
    let escrowShares: PublicKey;
    let escrowAssets: PublicKey;

    const balance = async (account: PublicKey): Promise<bigint> =>
        (await token.getAccount(connection, account)).amount;

    const requestPda = (owner: PublicKey, id: number): PublicKey =>
        PublicKey.findProgramAddressSync(
            [
                Buffer.from("withdrawal_request"),
                queue.toBuffer(),
                owner.toBuffer(),
                new BN(id).toArrayLike(Buffer, "le", 8),
            ],
            queueProgram.programId
        )[0];

    /// Asserts `promise` fails with the named program error, whichever program
    /// raised it: a vault error surfacing through the queue's CPI still counts.
    async function expectError(promise: Promise<unknown>, name: string): Promise<void> {
        try {
            await promise;
        } catch (err: any) {
            const code = err?.error?.errorCode?.code;
            const logs: string[] = err?.logs ?? err?.transactionLogs ?? [];
            if (code === name || logs.some((l) => l.includes(`Error Code: ${name}`))) return;
            throw new Error(`expected ${name}, got ${code ?? err}\n${logs.join("\n")}`);
        }
        assert.fail(`expected ${name}, but the transaction succeeded`);
    }

    /// The queue's self-CPI events of type `name` in a confirmed transaction.
    async function queueEvents(signature: string, name: string): Promise<any[]> {
        const tx = await connection.getTransaction(signature, {
            commitment: "confirmed",
            maxSupportedTransactionVersion: 0,
        });
        const keys = tx!.transaction.message.getAccountKeys();
        const events: any[] = [];
        for (const inner of tx!.meta!.innerInstructions ?? []) {
            for (const ix of inner.instructions) {
                if (!keys.get(ix.programIdIndex)!.equals(queueProgram.programId)) continue;
                const data = Buffer.from(bs58.decode(ix.data));
                if (!data.subarray(0, 8).equals(EVENT_IX_TAG)) continue;
                const event = queueProgram.coder.events.decode(data.subarray(8).toString("base64"));
                if (event?.name === name) events.push(event.data);
            }
        }
        return events;
    }

    async function createVault(): Promise<Vault> {
        const depositMint = await token.createMint(connection, deployer, deployer.publicKey, null, 9);
        const pda = (label: string) =>
            PublicKey.findProgramAddressSync(
                [Buffer.from(label), depositMint.toBuffer(), Buffer.from([0])],
                vaultProgram.programId
            )[0];
        const vault: Vault = {
            depositMint,
            vaultState: pda("VAULT_STATE"),
            shareMint: pda("mint"),
            vaultTokenAta: pda("token_vault"),
        } as Vault;

        await vaultProgram.methods
            .initialize(admin.publicKey, operator.publicKey, feeRecipient.publicKey, 0, DEFAULT_SHARE_OFFSET)
            .accounts({
                vaultState: vault.vaultState,
                shareMint: vault.shareMint,
                vaultTokenAta: vault.vaultTokenAta,
                depositMint,
                signer: protocolAuthority.publicKey,
                tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([protocolAuthority])
            .rpc();

        const ata = async (mint: PublicKey, owner: PublicKey) =>
            (await token.getOrCreateAssociatedTokenAccount(connection, deployer, mint, owner)).address;
        vault.userDepositAta = await ata(depositMint, user.publicKey);
        vault.userShareAta = await ata(vault.shareMint, user.publicKey);
        vault.operatorAta = await ata(depositMint, operator.publicKey);
        vault.feeRecipientAta = await ata(depositMint, feeRecipient.publicKey);

        await token.mintTo(connection, deployer, depositMint, vault.userDepositAta, deployer, 2 * DEPOSIT);
        await vaultProgram.methods
            .deposit(new BN(DEPOSIT))
            .accounts({
                vaultState: vault.vaultState,
                vaultTokenAta: vault.vaultTokenAta,
                senderTokenAccount: vault.userDepositAta,
                senderShareAccount: vault.userShareAta,
                shareMint: vault.shareMint,
                depositMint,
                signer: user.publicKey,
                tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([user])
            .rpc();
        await vaultProgram.methods
            .setWithdrawalFee(WITHDRAWAL_FEE)
            .accounts({ vaultState: vault.vaultState, depositMint, admin: admin.publicKey })
            .signers([admin])
            .rpc();
        return vault;
    }

    const directRedeem = (vault: Vault, shares: number) =>
        vaultProgram.methods
            .redeem(new BN(shares))
            .accounts({
                vaultState: vault.vaultState,
                vaultDepositAta: vault.vaultTokenAta,
                senderTokenAccount: vault.userDepositAta,
                senderShareAccount: vault.userShareAta,
                feeRecipientAccount: vault.feeRecipientAta,
                shareMint: vault.shareMint,
                depositMint: vault.depositMint,
                signer: user.publicKey,
                tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([user])
            .rpc();

    const request = (id: number, shares: number, finalizer: PublicKey = PublicKey.default) =>
        queueProgram.methods
            .requestWithdrawal(new BN(id), new BN(shares), finalizer)
            .accountsPartial({
                queue,
                vaultState: queued.vaultState,
                owner: user.publicKey,
                ownerShareAccount: queued.userShareAta,
                escrowShares,
                shareMint: queued.shareMint,
                recipientTokenAccount: queued.userDepositAta,
                request: requestPda(user.publicKey, id),
                tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([user])
            .rpc({ commitment: "confirmed" });

    const finalize = (id: number, sequence: BN, finalizer: Keypair = user) =>
        queueProgram.methods
            .finalizeWithdrawal(sequence)
            .accountsPartial({
                queue,
                vaultState: queued.vaultState,
                vaultDepositAta: queued.vaultTokenAta,
                feeRecipientAccount: queued.feeRecipientAta,
                escrowShares,
                escrowAssets,
                shareMint: queued.shareMint,
                depositMint: queued.depositMint,
                finalizer: finalizer.publicKey,
                request: requestPda(user.publicKey, id),
                owner: user.publicKey,
                recipientTokenAccount: queued.userDepositAta,
                vaultProgram: vaultProgram.programId,
                tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([finalizer])
            .rpc({ commitment: "confirmed" });

    const cancel = (id: number, sequence: BN) =>
        queueProgram.methods
            .cancelWithdrawal(sequence)
            .accountsPartial({
                queue,
                owner: user.publicKey,
                request: requestPda(user.publicKey, id),
                escrowShares,
                shareMint: queued.shareMint,
                destinationShareAccount: queued.userShareAta,
                tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([user])
            .rpc();

    const operatorMove = (method: "operatorWithdraw" | "operatorDeposit", amount: BN) =>
        vaultProgram.methods[method](amount)
            .accountsPartial({
                vaultState: queued.vaultState,
                vaultDepositAta: queued.vaultTokenAta,
                operatorTokenAccount: queued.operatorAta,
                subaccount: null,
                depositMint: queued.depositMint,
                operator: operator.publicKey,
                tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([operator])
            .rpc();

    const fetchRequest = (id: number) =>
        queueProgram.account.withdrawalRequest.fetch(requestPda(user.publicKey, id));
    const fetchQueue = () => queueProgram.account.withdrawalQueue.fetch(queue);

    before(async () => {
        await ensureProgramConfig(vaultProgram);
        await Promise.all(
            [deployer, operator, admin, user, keeper, stranger].map(async (kp) => {
                const sig = await connection.requestAirdrop(kp.publicKey, 10 * LAMPORTS_PER_SOL);
                const latest = await connection.getLatestBlockhash();
                await connection.confirmTransaction({ signature: sig, ...latest });
            })
        );

        queued = await createVault();
        plain = await createVault();

        queue = PublicKey.findProgramAddressSync(
            [Buffer.from("withdrawal_queue"), queued.vaultState.toBuffer()],
            queueProgram.programId
        )[0];
        escrowShares = token.getAssociatedTokenAddressSync(queued.shareMint, queue, true);
        escrowAssets = token.getAssociatedTokenAddressSync(queued.depositMint, queue, true);

        await queueProgram.methods
            .initializeQueue(new BN(0))
            .accountsPartial({
                vaultState: queued.vaultState,
                admin: admin.publicKey,
                payer: deployer.publicKey,
                depositMint: queued.depositMint,
                shareMint: queued.shareMint,
                queue,
                escrowShares,
                escrowAssets,
                tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([admin, deployer])
            .rpc();
        await vaultProgram.methods
            .attachWithdrawalQueue()
            .accounts({
                vaultState: queued.vaultState,
                depositMint: queued.depositMint,
                admin: admin.publicKey,
                queue,
            })
            .signers([admin])
            .rpc();
    });

    it("gates the vault on the queue: a direct redeem is refused", async () => {
        const vault = await vaultProgram.account.vaultState.fetch(queued.vaultState);
        assert.ok(vault.withdrawalQueueAuthority.equals(queue));
        await expectError(directRedeem(queued, 1_000_000), "WithdrawalQueueRequired");
    });

    it("request escrows the shares and is eligible at once under a zero cooldown", async () => {
        const shares = Number(await balance(queued.userShareAta)) / 4;
        await request(0, shares);

        const r = await fetchRequest(0);
        assert.equal(r.shares.toNumber(), shares);
        assert.equal(r.sequence.toNumber(), 1);
        assert.equal(r.eligibleAt.toNumber(), r.requestedAt.toNumber());
        assert.equal(r.expiresAt.toNumber(), 0, "no fulfillment window is set");

        const q = await fetchQueue();
        assert.equal(q.pendingRequests.toNumber(), 1);
        assert.equal(q.pendingShares.toNumber(), shares);
        assert.equal(await balance(escrowShares), BigInt(shares));
    });

    it("finalize pays what a direct redeem pays on an identical ungated vault", async () => {
        const r = await fetchRequest(0);
        const shares = r.shares.toNumber();
        const before = {
            queuedUser: await balance(queued.userDepositAta),
            queuedFee: await balance(queued.feeRecipientAta),
            queuedVault: await balance(queued.vaultTokenAta),
            plainUser: await balance(plain.userDepositAta),
            plainFee: await balance(plain.feeRecipientAta),
        };

        const signature = await finalize(0, r.sequence);
        await directRedeem(plain, shares);

        const queuedReceived = (await balance(queued.userDepositAta)) - before.queuedUser;
        const queuedFee = (await balance(queued.feeRecipientAta)) - before.queuedFee;
        const plainReceived = (await balance(plain.userDepositAta)) - before.plainUser;
        const plainFee = (await balance(plain.feeRecipientAta)) - before.plainFee;
        assert.equal(queuedReceived, plainReceived, "queue payout differs from a direct redeem");
        assert.equal(queuedFee, plainFee, "queue fee differs from a direct redeem");
        assert.ok(queuedFee > BigInt(0), "the vault's fee was charged");
        assert.equal(
            before.queuedVault - (await balance(queued.vaultTokenAta)),
            queuedReceived + queuedFee,
            "vault outflow is not the payout plus the fee"
        );

        const [event] = await queueEvents(signature, "withdrawalFinalized");
        assert.equal(event.shares.toNumber(), shares);
        assert.equal(BigInt(event.assets.toString()), queuedReceived);

        assert.equal(await connection.getAccountInfo(requestPda(user.publicKey, 0)), null, "request closed");
        const q = await fetchQueue();
        assert.equal(q.pendingRequests.toNumber(), 0);
        assert.equal(q.pendingShares.toNumber(), 0);
        assert.equal(await balance(escrowShares), BigInt(0));
        assert.equal(await balance(escrowAssets), BigInt(0), "nothing left behind in the asset escrow");
    });

    it("a request naming a finalizer refuses a stranger and pays when the keeper finalizes", async () => {
        await request(1, 1_000_000_000, keeper.publicKey);
        const r = await fetchRequest(1);

        await expectError(finalize(1, r.sequence, stranger), "FinalizerNotAllowed");
        const before = await balance(queued.userDepositAta);
        await finalize(1, r.sequence, keeper);
        assert.ok((await balance(queued.userDepositAta)) > before, "the owner's recipient was paid");
    });

    it("cancel returns the shares, and a recreated id refuses the old stamp", async () => {
        const shares = 1_000_000_000;
        const before = await balance(queued.userShareAta);
        await request(2, shares);
        const stale = (await fetchRequest(2)).sequence;
        await cancel(2, stale);
        assert.equal(await balance(queued.userShareAta), before, "cancel returned every share");

        await request(2, shares);
        const fresh = (await fetchRequest(2)).sequence;
        assert.ok(fresh.gt(stale));
        await expectError(finalize(2, stale), "StaleRequestSequence");
        await finalize(2, fresh);
    });

    it("finalize without liquidity leaves the request pending until the operator returns funds", async () => {
        await request(3, 1_000_000_000);
        const r = await fetchRequest(3);
        const local = (await vaultProgram.account.vaultState.fetch(queued.vaultState)).localAum;

        await operatorMove("operatorWithdraw", local);
        await expectError(finalize(3, r.sequence), "NotEnoughLiquidity");
        assert.equal((await fetchRequest(3)).shares.toNumber(), r.shares.toNumber(), "request still pending");

        await operatorMove("operatorDeposit", local);
        await finalize(3, r.sequence);
    });

    it("sweep returns stray shares and leaves the escrow holding exactly the pending shares", async () => {
        await request(4, 1_000_000_000);
        const stray = BigInt(123_456_789);
        await token.transfer(connection, user, queued.userShareAta, escrowShares, user, stray);
        const pending = (await fetchQueue()).pendingShares;
        assert.equal(await balance(escrowShares), BigInt(pending.toString()) + stray);

        const before = await balance(queued.userShareAta);
        await queueProgram.methods
            .sweepEscrowShares()
            .accountsPartial({
                queue,
                vaultState: queued.vaultState,
                admin: admin.publicKey,
                escrowShares,
                shareMint: queued.shareMint,
                destination: queued.userShareAta,
                tokenProgram: token.TOKEN_PROGRAM_ID,
            })
            .signers([admin])
            .rpc();

        assert.equal((await balance(queued.userShareAta)) - before, stray);
        assert.equal(await balance(escrowShares), BigInt(pending.toString()));
    });

    it("release_vault reopens instant redemption and a pending request still finalizes", async () => {
        await queueProgram.methods
            .releaseVault()
            .accountsPartial({
                queue,
                vaultState: queued.vaultState,
                depositMint: queued.depositMint,
                admin: admin.publicKey,
                vaultProgram: vaultProgram.programId,
            })
            .signers([admin])
            .rpc();

        const vault = await vaultProgram.account.vaultState.fetch(queued.vaultState);
        assert.ok(vault.withdrawalQueueAuthority.equals(PublicKey.default));
        await directRedeem(queued, 1_000_000);

        const r = await fetchRequest(4);
        await finalize(4, r.sequence);
        const q = await fetchQueue();
        assert.equal(q.pendingRequests.toNumber(), 0);
        assert.equal(await balance(escrowShares), BigInt(0));
    });
});
