#!/bin/bash

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo -e "${BLUE}╔════════════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║     Solana Vaults - Test Runner                      ║${NC}"
echo -e "${BLUE}╚════════════════════════════════════════════════════════╝${NC}"
echo ""

RPC="http://127.0.0.1:8899"
WALLET="/tmp/deployer_keypair.json" # must match [provider].wallet in Anchor.toml
LIB_RS="programs/august-vault/src/lib.rs"
PROGRAM_KP="target/deploy/august_vault-keypair.json"

# The maintained TypeScript suites — kept in sync with Anchor.toml [scripts].test
# (the set CI runs and keeps green). Suites 4-10 (admin, token-2022, metadata,
# minimum-deposit, update-metadata, set-aum-limits, comprehensive) predate the
# multi-vault / versioned-PDA / 4-arg-initialize refactors and currently fail
# against the program; they are intentionally not offered here until updated.
MAINTAINED_SUITES="tests/1_*.ts tests/2_*.ts tests/3_*.ts tests/11_*.ts tests/12_*.ts tests/13_*.ts"

# Backups are set when a step first mutates a file and restored (verified) by the
# EXIT trap, so a dev machine is never left with a mutated declare_id!, test
# command, or a destroyed program keypair. BUILT marks that we produced build
# artifacts (which may carry a non-committed program id) for cleanup.
LIB_BAK=""
ANCHOR_BAK=""
PROGRAM_KP_BAK="" # "__none__" => no keypair existed; delete the generated one
BUILT=""

# shellcheck disable=SC2329  # invoked indirectly via `trap cleanup EXIT`
cleanup() {
    # Verified restores: keep the backup and warn if a restore can't be confirmed.
    if [ -n "$LIB_BAK" ] && [ -f "$LIB_BAK" ]; then
        if cp "$LIB_BAK" "$LIB_RS" 2>/dev/null && cmp -s "$LIB_BAK" "$LIB_RS"; then rm -f "$LIB_BAK"
        else echo -e "${RED}⚠ failed to restore $LIB_RS — backup kept at $LIB_BAK${NC}"; fi
    fi
    if [ -n "$ANCHOR_BAK" ] && [ -f "$ANCHOR_BAK" ]; then
        if cp "$ANCHOR_BAK" Anchor.toml 2>/dev/null && cmp -s "$ANCHOR_BAK" Anchor.toml; then rm -f "$ANCHOR_BAK"
        else echo -e "${RED}⚠ failed to restore Anchor.toml — backup kept at $ANCHOR_BAK${NC}"; fi
    fi
    if [ "$PROGRAM_KP_BAK" = "__none__" ]; then
        rm -f "$PROGRAM_KP"
    elif [ -n "$PROGRAM_KP_BAK" ] && [ -f "$PROGRAM_KP_BAK" ]; then
        if cp "$PROGRAM_KP_BAK" "$PROGRAM_KP" 2>/dev/null && cmp -s "$PROGRAM_KP_BAK" "$PROGRAM_KP"; then rm -f "$PROGRAM_KP_BAK"
        else echo -e "${RED}⚠ failed to restore the program keypair — backup kept at $PROGRAM_KP_BAK${NC}"; fi
    fi
    # Remove build artifacts we generated — they may carry a non-committed program
    # id and must not be deployed/published accidentally (target/ is rebuildable).
    if [ -n "$BUILT" ]; then
        rm -f target/deploy/august_vault.so target/idl/august_vault.json target/types/august_vault.ts
    fi
    if [ -n "$VALIDATOR_PID" ]; then
        echo ""
        echo -e "${YELLOW}Cleaning up validator (PID: $VALIDATOR_PID)...${NC}"
        kill "$VALIDATOR_PID" 2>/dev/null
        echo -e "${GREEN}✓ Validator stopped${NC}"
    fi
}
trap cleanup EXIT

# Back up an existing program keypair (verified) BEFORE anything can overwrite it,
# or mark that none existed. Aborts if the backup can't be made/verified, so a
# dev's deployment key is never destroyed. Shared by the build paths below.
backup_program_keypair() {
    mkdir -p target/deploy
    if [ -f "$PROGRAM_KP" ]; then
        # Verify a LOCAL candidate first; publish it to the cleanup-visible global
        # only once cp+cmp confirm a good copy, so the EXIT trap can never restore
        # a partial/corrupt backup over the real keypair.
        local cand
        cand="$(mktemp)"
        if ! cp "$PROGRAM_KP" "$cand" || ! cmp -s "$PROGRAM_KP" "$cand"; then
            rm -f "$cand"
            echo -e "${RED}✗ Could not safely back up the existing program keypair — aborting so it isn't destroyed.${NC}"
            exit 1
        fi
        PROGRAM_KP_BAK="$cand"
    else
        PROGRAM_KP_BAK="__none__"
    fi
}

# Start a FRESH local validator that this script owns, and fund the provider
# wallet. The suites create deterministic mint/PDA accounts (and each run uses a
# fresh program id), so a ledger carrying leftovers from a previous run collides
# ("account already in use"). We therefore always start our own --reset validator
# and refuse to run against a pre-existing one whose state we can't trust, rather
# than silently reusing (and failing on) a dirty ledger. Only for validator-backed
# choices; the EXIT trap tears down the validator we start.
ensure_localnet() {
    if curl -s "$RPC/health" > /dev/null 2>&1; then
        local port="${RPC##*:}" # e.g. 8899 — only this port blocks us
        echo -e "${RED}✗ A validator is already running on $RPC.${NC}"
        echo -e "${YELLOW}  This runner needs a fresh (--reset) ledger for deterministic tests: a reused${NC}"
        echo -e "${YELLOW}  ledger collides on the suites' deterministic mint/PDA accounts. Free port${NC}"
        echo -e "${YELLOW}  $port and re-run (scoped to that port, not all validators):${NC}"
        echo -e "${YELLOW}      kill \"\$(lsof -ti tcp:$port)\"${NC}"
        exit 1
    fi
    echo -e "${YELLOW}Starting a fresh local validator (--reset)...${NC}"
    solana-test-validator --reset > /dev/null 2>&1 &
    VALIDATOR_PID=$!
    local up=0
    for _ in $(seq 1 30); do
        if curl -s "$RPC/health" > /dev/null 2>&1; then up=1; break; fi
        sleep 1
    done
    if [ "$up" -ne 1 ]; then
        echo -e "${RED}✗ Local validator did not become healthy at $RPC (it may have failed to start or bind).${NC}"
        exit 1
    fi
    echo -e "${GREEN}✓ Fresh validator started (PID: $VALIDATOR_PID)${NC}"
    SKIP_VALIDATOR="--skip-local-validator"

    [ -f "$WALLET" ] || solana-keygen new --no-bip39-passphrase -o "$WALLET" --force > /dev/null
    if ! solana airdrop 100 "$(solana-keygen pubkey "$WALLET")" -u "$RPC" > /dev/null 2>&1; then
        echo -e "${YELLOW}⚠ Could not airdrop the deployer wallet at $RPC — validator-backed tests may fail.${NC}"
    fi
}

# Generate an ephemeral program keypair and sync it into declare_id! + Anchor.toml
# so `anchor test` can deploy locally (the real mainnet program keypair isn't in
# the repo). Rewritten source/config + the keypair are restored on exit.
prepare_local_program() {
    backup_program_keypair
    # Same fail-closed pattern: verify a local candidate, then publish to the global.
    local libcand anchorcand
    libcand="$(mktemp)"
    if ! cp "$LIB_RS" "$libcand" || ! cmp -s "$LIB_RS" "$libcand"; then
        rm -f "$libcand"; echo -e "${RED}✗ failed to back up $LIB_RS${NC}"; exit 1
    fi
    LIB_BAK="$libcand"
    anchorcand="$(mktemp)"
    if ! cp Anchor.toml "$anchorcand" || ! cmp -s Anchor.toml "$anchorcand"; then
        rm -f "$anchorcand"; echo -e "${RED}✗ failed to back up Anchor.toml${NC}"; exit 1
    fi
    ANCHOR_BAK="$anchorcand"

    # A fresh program id each run is fine because ensure_localnet guarantees a
    # clean (--reset) ledger — nothing from a prior run's id/PDAs lingers.
    solana-keygen new --no-bip39-passphrase -o "$PROGRAM_KP" --force > /dev/null
    local pid
    pid=$(solana-keygen pubkey "$PROGRAM_KP")
    BUILT=1
    # Do NOT suppress failures: if declare_id! isn't synced, anchor deploys under
    # $pid while lib.rs still declares up12…, causing misleading failures.
    if ! anchor keys sync > /dev/null 2>&1; then
        echo -e "${RED}✗ 'anchor keys sync' failed — cannot align declare_id! for a local deploy.${NC}"; exit 1
    fi
    if ! grep -q "$pid" "$LIB_RS"; then
        echo -e "${RED}✗ declare_id! was not synced to $pid — aborting to avoid a program-ID mismatch.${NC}"; exit 1
    fi
    awk -v id="$pid" '/^august_vault = /{print "august_vault = \"" id "\""; next} {print}' \
        Anchor.toml > Anchor.toml.tmp && mv Anchor.toml.tmp Anchor.toml
    echo -e "${GREEN}✓ Local program id synced: $pid${NC}"
}

# Run `anchor test` against ONLY the given files by rewriting the [scripts].test
# line (passing files positionally APPENDS to the configured suite). Restored on exit.
run_focused() {
    local files="$1"
    awk -v f="$files" '/^test = /{print "test = \"yarn run ts-mocha -p ./tsconfig.json -t 1000000 " f "\""; next} {print}' \
        Anchor.toml > Anchor.toml.tmp && mv Anchor.toml.tmp Anchor.toml
    anchor test $SKIP_VALIDATOR
}

# Menu — read the choice FIRST, then spin up localnet. Every option runs a
# maintained (CI-green) suite; the stale suites 4-10 are deliberately not listed.
echo -e "${BLUE}Select test suite:${NC}"
echo "1) Run all maintained tests (CI suite: init, users, operator, multi-vault, close, version)"
echo "2) Run user function tests (2_users)"
echo "3) Run operator function tests (3_operator)"
echo "4) Run multi-vault + redeem-CEI tests (11_*)"
echo "5) Run close-vault tests (12_*)"
echo "6) Run version-security tests (13_*)"
echo "7) Exit"
echo ""

read -r -p "Enter choice [1-7]: " choice

# All maintained suites are validator-backed and share the same prep.
run_suite() {
    ensure_localnet
    prepare_local_program
    run_focused "$1"
}

case $choice in
    1)
        echo -e "${GREEN}Running all maintained tests...${NC}"
        run_suite "$MAINTAINED_SUITES"
        ;;
    2)
        echo -e "${GREEN}Running user function tests...${NC}"
        run_suite "tests/2_users.ts"
        ;;
    3)
        echo -e "${GREEN}Running operator function tests...${NC}"
        run_suite "tests/3_operator.ts"
        ;;
    4)
        echo -e "${GREEN}Running multi-vault + redeem-CEI tests...${NC}"
        run_suite "tests/11_*.ts"
        ;;
    5)
        echo -e "${GREEN}Running close-vault tests...${NC}"
        run_suite "tests/12_*.ts"
        ;;
    6)
        echo -e "${GREEN}Running version-security tests...${NC}"
        run_suite "tests/13_*.ts"
        ;;
    7)
        echo -e "${YELLOW}Exiting...${NC}"
        exit 0
        ;;
    *)
        echo -e "${RED}Invalid choice${NC}"
        exit 1
        ;;
esac

# Preserve the test command's exit status (cleanup runs via the EXIT trap).
status=$?

echo ""
if [ "$status" -eq 0 ]; then
    echo -e "${GREEN}✓ Tests completed!${NC}"
else
    echo -e "${RED}✗ Tests failed (exit $status)${NC}"
fi
exit $status
