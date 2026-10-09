#!/usr/bin/env node
// docs/UPGRADE.md Step 6 on devnet: walks the withdrawal queue end to end on a
// fresh test vault (own mint, so nothing real is touched) and stops at the
// first step that fails.
//
// Usage: DEVNET_RPC_URL=<private devnet RPC> pnpm run walk:devnet
// Requires: target/idl from `anchor build`, and ~/.config/solana/devnet-test.json
// holding the devnet upgrade and ProgramConfig authority (APuzEr…), funded.
import anchorPkg from "@coral-xyz/anchor";
const { AnchorProvider, Program, Wallet, BN } = anchorPkg;
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import * as token from "@solana/spl-token";
import { readFileSync } from "fs";
import { dirname, join } from "path";
import { fileURLToPath } from "url";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..");
const connection = new Connection(process.env.DEVNET_RPC_URL || "https://api.devnet.solana.com", "confirmed");
const DEVNET_GENESIS = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG";
const VAULT_ID = new PublicKey("up12bytoZBmwofqsySf2uqKQ7zpfeKiAWwfvqzJjtRt");
const QUEUE_ID = new PublicKey("upQhC7mgYwmHLVaatRWoGAu394piLT9th9FiQZHnPrW");
const DEPOSIT = 10_000_000_000n; // 10 tokens at 9 decimals
const COOLDOWN = 60; // seconds; long enough to refuse an early finalize, short enough to wait out

const dev = Keypair.fromSecretKey(
  Uint8Array.from(JSON.parse(readFileSync(join(process.env.HOME, ".config/solana/devnet-test.json"), "utf8")))
);
const provider = new AnchorProvider(connection, new Wallet(dev), { commitment: "confirmed" });
const load = (name, id) => {
  const idl = JSON.parse(readFileSync(join(repo, `target/idl/${name}.json`), "utf8"));
  idl.address = id.toBase58();
  return new Program(idl, provider);
};
const vaultProgram = load("august_vault", VAULT_ID);
const queueProgram = load("august_withdrawal_queue", QUEUE_ID);

const results = [];
const step = async (label, fn) => {
  try {
    const note = await fn();
    results.push(["PASS", label, note ?? ""]);
    console.log(`PASS  ${label}${note ? `  (${note})` : ""}`);
  } catch (e) {
    results.push(["FAIL", label, String(e?.message ?? e).split("\n")[0]]);
    console.log(`FAIL  ${label}\n${e?.logs?.slice(-6).join("\n") ?? e}`);
    throw e;
  }
};
async function expectError(promise, name) {
  try {
    await promise;
  } catch (err) {
    const code = err?.error?.errorCode?.code;
    const logs = err?.logs ?? err?.transactionLogs ?? [];
    if (code === name || logs.some((l) => l.includes(`Error Code: ${name}`))) return name;
    throw new Error(`expected ${name}, got ${code ?? err?.message ?? err}`);
  }
  throw new Error(`expected ${name}, but the transaction succeeded`);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const balance = async (a) => (await token.getAccount(connection, a)).amount;

// Fresh role keys, funded from the devnet authority, which also acts as admin,
// ProgramConfig authority and mint authority.
const user = Keypair.generate();
const keeper = Keypair.generate();
const stranger = Keypair.generate();
const operator = Keypair.generate();
const feeRecipient = Keypair.generate();

let mint, vaultState, shareMint, vaultTokenAta, userDeposit, userShares, feeAta;
let queue, escrowShares, escrowAssets;
const requestPda = (id) =>
  PublicKey.findProgramAddressSync(
    [Buffer.from("withdrawal_request"), queue.toBuffer(), user.publicKey.toBuffer(), new BN(id).toArrayLike(Buffer, "le", 8)],
    QUEUE_ID
  )[0];

const directRedeem = (shares) =>
  vaultProgram.methods.redeem(new BN(shares.toString())).accountsPartial({
    vaultState, vaultDepositAta: vaultTokenAta, senderTokenAccount: userDeposit, senderShareAccount: userShares,
    feeRecipientAccount: feeAta, shareMint, depositMint: mint, signer: user.publicKey, tokenProgram: token.TOKEN_PROGRAM_ID,
  }).signers([user]).rpc();
const request = (id, shares, floor, finalizer = PublicKey.default) =>
  queueProgram.methods.requestWithdrawal(new BN(id), new BN(shares.toString()), new BN(floor.toString()), finalizer)
    .accountsPartial({
      queue, vaultState, owner: user.publicKey, ownerShareAccount: userShares, escrowShares, shareMint,
      recipientTokenAccount: userDeposit, request: requestPda(id), tokenProgram: token.TOKEN_PROGRAM_ID,
    }).signers([user]).rpc();
const finalize = (id, seq, signer) =>
  queueProgram.methods.finalizeWithdrawal(seq).accountsPartial({
    queue, vaultState, vaultDepositAta: vaultTokenAta, feeRecipientAccount: feeAta, escrowShares, escrowAssets,
    shareMint, depositMint: mint, finalizer: signer.publicKey, request: requestPda(id), owner: user.publicKey,
    recipientTokenAccount: userDeposit, vaultProgram: VAULT_ID, tokenProgram: token.TOKEN_PROGRAM_ID,
  }).signers([signer]).rpc();
const cancel = (id, seq) =>
  queueProgram.methods.cancelWithdrawal(seq).accountsPartial({
    queue, owner: user.publicKey, request: requestPda(id), escrowShares, shareMint,
    destinationShareAccount: userShares, tokenProgram: token.TOKEN_PROGRAM_ID,
  }).signers([user]).rpc();
const expedite = (id, seq) =>
  queueProgram.methods.expediteRequest(seq).accountsPartial({
    queue, vaultState, authority: dev.publicKey, request: requestPda(id),
  }).rpc();
const setWindow = (s) =>
  queueProgram.methods.setFulfillmentWindow(new BN(s)).accountsPartial({ queue, vaultState, admin: dev.publicKey }).rpc();
const attach = () =>
  vaultProgram.methods.attachWithdrawalQueue().accountsPartial({ vaultState, depositMint: mint, admin: dev.publicKey, queue }).rpc();
const fetchRequest = (id) => queueProgram.account.withdrawalRequest.fetch(requestPda(id));
const fetchQueue = () => queueProgram.account.withdrawalQueue.fetch(queue);

async function main() {
  // The program ids are the same on mainnet; refuse anything but devnet.
  const genesis = await connection.getGenesisHash();
  if (genesis !== DEVNET_GENESIS) throw new Error(`not devnet: genesis ${genesis}`);
  console.log("authority", dev.publicKey.toBase58(), "balance", (await connection.getBalance(dev.publicKey)) / LAMPORTS_PER_SOL);

  await step("fund role keys", async () => {
    const tx = new Transaction();
    for (const k of [user, keeper, stranger, operator]) {
      tx.add(SystemProgram.transfer({ fromPubkey: dev.publicKey, toPubkey: k.publicKey, lamports: 0.05 * LAMPORTS_PER_SOL }));
    }
    await provider.sendAndConfirm(tx);
  });

  await step("fresh test vault: mint, initialize, deposit, 0.1% fee", async () => {
    mint = await token.createMint(connection, dev, dev.publicKey, null, 9);
    const pda = (l) => PublicKey.findProgramAddressSync([Buffer.from(l), mint.toBuffer(), Buffer.from([0])], VAULT_ID)[0];
    vaultState = pda("VAULT_STATE"); shareMint = pda("mint"); vaultTokenAta = pda("token_vault");
    await vaultProgram.methods.initialize(dev.publicKey, operator.publicKey, feeRecipient.publicKey, 0, new BN(1_000_000))
      .accountsPartial({
        signer: dev.publicKey, payer: dev.publicKey, depositMint: mint, vaultState, shareMint, vaultTokenAta,
        tokenProgram: token.TOKEN_PROGRAM_ID,
      }).rpc();
    const ata = async (m, o) => (await token.getOrCreateAssociatedTokenAccount(connection, dev, m, o)).address;
    userDeposit = await ata(mint, user.publicKey);
    userShares = await ata(shareMint, user.publicKey);
    feeAta = await ata(mint, feeRecipient.publicKey);
    await token.mintTo(connection, dev, mint, userDeposit, dev, DEPOSIT);
    await vaultProgram.methods.deposit(new BN(DEPOSIT.toString())).accountsPartial({
      vaultState, vaultTokenAta, senderTokenAccount: userDeposit, senderShareAccount: userShares, shareMint,
      depositMint: mint, signer: user.publicKey, tokenProgram: token.TOKEN_PROGRAM_ID,
    }).signers([user]).rpc();
    await vaultProgram.methods.setWithdrawalFee(1_000).accountsPartial({ vaultState, depositMint: mint, admin: dev.publicKey }).rpc();
    return `vault ${vaultState.toBase58()}, mint ${mint.toBase58()}`;
  });

  await step(`initialize_queue (cooldown ${COOLDOWN}s) and attach`, async () => {
    queue = PublicKey.findProgramAddressSync([Buffer.from("withdrawal_queue"), vaultState.toBuffer()], QUEUE_ID)[0];
    escrowShares = token.getAssociatedTokenAddressSync(shareMint, queue, true);
    escrowAssets = token.getAssociatedTokenAddressSync(mint, queue, true);
    await queueProgram.methods.initializeQueue(new BN(COOLDOWN)).accountsPartial({
      vaultState, admin: dev.publicKey, payer: dev.publicKey, depositMint: mint, shareMint, queue, escrowShares,
      escrowAssets, tokenProgram: token.TOKEN_PROGRAM_ID,
    }).rpc();
    await attach();
    return `queue ${queue.toBase58()}`;
  });

  await step("direct redeem refused while attached", () => expectError(directRedeem(1_000_000n), "WithdrawalQueueRequired"));

  await step("window minimum: 3600s refused, 86399s refused", async () => {
    await expectError(setWindow(3600), "FulfillmentWindowOutOfBounds");
    await expectError(setWindow(86_399), "FulfillmentWindowOutOfBounds");
  });
  await step("window minimum: 86400s accepted", async () => {
    await setWindow(86_400);
    const q = await fetchQueue();
    if (q.fulfillmentWindowSeconds.toNumber() !== 86_400) throw new Error("window not stored");
  });

  const shares = await balance(userShares);
  const quarter = shares / 4n;

  await step("request 1: floor 1, finalizer = keeper; stamps floor and deadline", async () => {
    await request(1, quarter, 1n, keeper.publicKey);
    const r = await fetchRequest(1);
    if (r.minAssetsOut.toString() !== "1") throw new Error("floor not stored");
    if (r.expiresAt.toNumber() !== r.scheduledEligibleAt.toNumber() + 86_400) throw new Error("deadline wrong");
    return `seq ${r.sequence}, eligible +${r.eligibleAt.toNumber() - r.requestedAt.toNumber()}s`;
  });
  await step("finalize before eligibility refused", async () =>
    expectError(finalize(1, (await fetchRequest(1)).sequence, keeper), "CooldownNotElapsed"));
  await step("expedite by admin", async () => {
    await expedite(1, (await fetchRequest(1)).sequence);
  });
  await step("stranger refused by the named finalizer", async () =>
    expectError(finalize(1, (await fetchRequest(1)).sequence, stranger), "FinalizerNotAllowed"));
  await step("keeper finalizes, owner paid, fee charged, escrows empty", async () => {
    const before = { user: await balance(userDeposit), fee: await balance(feeAta) };
    await finalize(1, (await fetchRequest(1)).sequence, keeper);
    const paid = (await balance(userDeposit)) - before.user;
    const fee = (await balance(feeAta)) - before.fee;
    if (paid <= 0n || fee <= 0n) throw new Error(`paid ${paid}, fee ${fee}`);
    if ((await connection.getAccountInfo(requestPda(1))) !== null) throw new Error("request not closed");
    if ((await balance(escrowAssets)) !== 0n) throw new Error("asset escrow not empty");
    return `paid ${paid}, fee ${fee}`;
  });

  await step("request 2 with an unreachable floor: expedited finalize refused, request survives", async () => {
    await request(2, quarter, 10n ** 18n);
    const seq = (await fetchRequest(2)).sequence;
    await expedite(2, seq);
    await expectError(finalize(2, seq, user), "PayoutBelowFloor");
    if ((await fetchRequest(2)).shares.toString() !== quarter.toString()) throw new Error("request changed");
  });
  await step("cancel request 2 returns every share", async () => {
    const before = await balance(userShares);
    await cancel(2, (await fetchRequest(2)).sequence);
    if ((await balance(userShares)) - before !== quarter) throw new Error("shares not returned");
  });

  await step(`request 3, no floor: owner finalizes after the ${COOLDOWN}s cooldown runs out`, async () => {
    await request(3, quarter, 0n);
    const r = await fetchRequest(3);
    const wait = r.eligibleAt.toNumber() - Math.floor(Date.now() / 1000) + 10;
    if (wait > 0) await sleep(wait * 1000);
    const before = await balance(userDeposit);
    await finalize(3, r.sequence, user);
    return `paid ${(await balance(userDeposit)) - before}`;
  });

  await step("request 4 left pending across release_vault", async () => request(4, quarter / 2n, 0n));
  await step("release_vault, then a direct redeem works", async () => {
    await queueProgram.methods.releaseVault().accountsPartial({
      queue, vaultState, depositMint: mint, admin: dev.publicKey, vaultProgram: VAULT_ID,
    }).rpc();
    const v = await vaultProgram.account.vaultState.fetch(vaultState);
    if (!v.withdrawalQueueAuthority.equals(PublicKey.default)) throw new Error("gate still set");
    await directRedeem(1_000_000n);
  });
  await step("pending request 4 cancels after release", async () => cancel(4, (await fetchRequest(4)).sequence));
  await step("re-attach: gate restored, direct redeem refused again", async () => {
    await attach();
    await expectError(directRedeem(1_000_000n), "WithdrawalQueueRequired");
    const q = await fetchQueue();
    if (q.pendingRequests.toNumber() !== 0 || q.pendingShares.toNumber() !== 0) throw new Error("counters not zero");
    if ((await balance(escrowShares)) !== 0n) throw new Error("share escrow not empty");
  });

  console.log(`\n${results.length} steps passed. vault ${vaultState.toBase58()} queue ${queue.toBase58()}`);
  console.log("authority balance", (await connection.getBalance(dev.publicKey)) / LAMPORTS_PER_SOL);
}

main().catch((e) => {
  if (results.length === 0) console.log(e?.message ?? e);
  console.log(`\nSTOPPED after ${results.filter((r) => r[0] === "PASS").length} passing steps`);
  process.exit(1);
});
