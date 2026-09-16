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
# Every program in the workspace. `anchor keys sync` rewrites declare_id! in ALL
# of them, so all of them must be backed up and restored — backing up only one
# would leave the others pointing at an ephemeral localnet key.
#
# Derived from programs/ rather than listed: a hardcoded list silently omits a new
# program, which is exactly the failure the paragraph above warns about. Crate
# directories use dashes, artifacts and Anchor.toml keys use underscores.
PROGRAMS=""
for _dir in programs/*/; do
    _prog="$(basename "$_dir" | tr '-' '_')"
    if [ ! -f "programs/$(basename "$_dir")/src/lib.rs" ]; then
        echo -e "${RED}✗ programs/$(basename "$_dir") has no src/lib.rs — cannot back up its declare_id!.${NC}"; exit 1
    fi
    PROGRAMS="$PROGRAMS $_prog"
done
if [ -z "$PROGRAMS" ]; then
    echo -e "${RED}✗ No programs found under programs/ — refusing to run.${NC}"; exit 1
fi
LIB_RS_FOR() { echo "programs/$(echo "$1" | tr '_' '-')/src/lib.rs"; }
PROGRAM_KP_FOR() { echo "target/deploy/${1}-keypair.json"; }

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
# Backups live as files under one temp dir, named by program, so the EXIT trap
# can restore each without needing per-program shell variables.
BAK_DIR=""
ANCHOR_BAK=""
BUILT=""

# shellcheck disable=SC2329  # invoked indirectly via `trap cleanup EXIT`
cleanup() {
    # Verified restores: keep the backup and warn if a restore can't be confirmed.
    if [ -n "$BAK_DIR" ] && [ -d "$BAK_DIR" ]; then
        for prog in $PROGRAMS; do
            libbak="$BAK_DIR/${prog}.lib.rs"
            librs="$(LIB_RS_FOR "$prog")"
            if [ -f "$libbak" ]; then
                if cp "$libbak" "$librs" 2>/dev/null && cmp -s "$libbak" "$librs"; then rm -f "$libbak"
                else echo -e "${RED}⚠ failed to restore $librs — backup kept at $libbak${NC}"; fi
            fi
        done
    fi
    if [ -n "$ANCHOR_BAK" ] && [ -f "$ANCHOR_BAK" ]; then
        if cp "$ANCHOR_BAK" Anchor.toml 2>/dev/null && cmp -s "$ANCHOR_BAK" Anchor.toml; then rm -f "$ANCHOR_BAK"
        else echo -e "${RED}⚠ failed to restore Anchor.toml — backup kept at $ANCHOR_BAK${NC}"; fi
    fi
    if [ -n "$BAK_DIR" ] && [ -d "$BAK_DIR" ]; then
        for prog in $PROGRAMS; do
            kp="$(PROGRAM_KP_FOR "$prog")"
            kpbak="$BAK_DIR/${prog}.keypair.json"
            if [ -f "$BAK_DIR/${prog}.keypair.none" ]; then
                rm -f "$kp" "$BAK_DIR/${prog}.keypair.none"
            elif [ -f "$kpbak" ]; then
                if cp "$kpbak" "$kp" 2>/dev/null && cmp -s "$kpbak" "$kp"; then rm -f "$kpbak"
                else echo -e "${RED}⚠ failed to restore $kp — backup kept at $kpbak${NC}"; fi
            elif [ -n "$BUILT" ]; then
                # We replaced this keypair but hold no record of what was there.
                # Silence would read as "nothing to restore"; say it out loud.
                echo -e "${RED}⚠ no backup record for $prog — $kp holds a run-local key${NC}"
            fi
        done
        rmdir "$BAK_DIR" 2>/dev/null || true
    fi
    # Remove build artifacts we generated — they may carry a non-committed program
    # id and must not be deployed/published accidentally (target/ is rebuildable).
    if [ -n "$BUILT" ]; then
        for prog in $PROGRAMS; do
            rm -f "target/deploy/${prog}.so" "target/idl/${prog}.json" "target/types/${prog}.ts"
        done
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
    if [ -z "$BAK_DIR" ] && ! BAK_DIR="$(mktemp -d)"; then
        echo -e "${RED}✗ Could not create a backup directory — aborting before anything is overwritten.${NC}"
        exit 1
    fi
    for prog in $PROGRAMS; do
        local kp cand
        kp="$(PROGRAM_KP_FOR "$prog")"
        if [ -f "$kp" ]; then
            # Verify a LOCAL candidate first; publish it to the cleanup-visible
            # location only once cp+cmp confirm a good copy, so the EXIT trap can
            # never restore a partial/corrupt backup over the real keypair.
            cand="$(mktemp)"
            if ! cp "$kp" "$cand" || ! cmp -s "$kp" "$cand"; then
                rm -f "$cand"
                echo -e "${RED}✗ Could not safely back up $kp — aborting so it isn't destroyed.${NC}"
                exit 1
            fi
            # Check the publish too. A failure here leaves neither a backup nor a
            # marker, so cleanup() finds nothing and says nothing — while the
            # `solana-keygen --force` below has already destroyed the real key.
            if ! mv "$cand" "$BAK_DIR/${prog}.keypair.json"; then
                rm -f "$cand"
                echo -e "${RED}✗ Could not store the backup of $kp — aborting so it isn't destroyed.${NC}"
                exit 1
            fi
        else
            if ! : > "$BAK_DIR/${prog}.keypair.none"; then
                echo -e "${RED}✗ Could not record that $kp is absent — aborting rather than leave cleanup guessing.${NC}"
                exit 1
            fi
        fi
    done
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
    # Same fail-closed pattern: verify a local candidate, then publish it into
    # $BAK_DIR, checking the publish itself — see backup_program_keypair.
    local libcand anchorcand
    for prog in $PROGRAMS; do
        librs="$(LIB_RS_FOR "$prog")"
        libcand="$(mktemp)"
        if ! cp "$librs" "$libcand" || ! cmp -s "$librs" "$libcand"; then
            rm -f "$libcand"; echo -e "${RED}✗ failed to back up $librs${NC}"; exit 1
        fi
        if ! mv "$libcand" "$BAK_DIR/${prog}.lib.rs"; then
            rm -f "$libcand"; echo -e "${RED}✗ failed to store the backup of $librs${NC}"; exit 1
        fi
    done
    anchorcand="$(mktemp)"
    if ! cp Anchor.toml "$anchorcand" || ! cmp -s Anchor.toml "$anchorcand"; then
        rm -f "$anchorcand"; echo -e "${RED}✗ failed to back up Anchor.toml${NC}"; exit 1
    fi
    ANCHOR_BAK="$anchorcand"

    # A fresh program id each run is fine because ensure_localnet guarantees a
    # clean (--reset) ledger — nothing from a prior run's id/PDAs lingers.
    for prog in $PROGRAMS; do
        solana-keygen new --no-bip39-passphrase -o "$(PROGRAM_KP_FOR "$prog")" --force > /dev/null
    done
    BUILT=1
    # Do NOT suppress failures: if declare_id! isn't synced, anchor deploys under
    # the fresh id while lib.rs still declares the committed one, causing
    # misleading failures.
    if ! anchor keys sync > /dev/null 2>&1; then
        echo -e "${RED}✗ 'anchor keys sync' failed — cannot align declare_id! for a local deploy.${NC}"; exit 1
    fi
    # Verify and rewrite per program. Checking only one would let a second
    # program deploy under an id its source does not declare, which surfaces as
    # an unrelated instruction failure much later.
    local pid librs
    for prog in $PROGRAMS; do
        pid=$(solana-keygen pubkey "$(PROGRAM_KP_FOR "$prog")")
        librs="$(LIB_RS_FOR "$prog")"
        if ! grep -q "$pid" "$librs"; then
            echo -e "${RED}✗ declare_id! in $librs was not synced to $pid — aborting to avoid a program-ID mismatch.${NC}"; exit 1
        fi
        if ! awk -v id="$pid" -v prog="$prog" \
            '$0 ~ "^" prog " = " {print prog " = \"" id "\""; next} {print}' \
            Anchor.toml > Anchor.toml.tmp || ! mv Anchor.toml.tmp Anchor.toml; then
            rm -f Anchor.toml.tmp
            echo -e "${RED}✗ failed to rewrite Anchor.toml for $prog${NC}"; exit 1
        fi
        # awk exits 0 having matched nothing, so confirm the id actually landed —
        # a renamed [programs.*] entry would otherwise deploy under a stale id.
        if ! grep -q "^${prog} = \"${pid}\"" Anchor.toml; then
            echo -e "${RED}✗ Anchor.toml has no '${prog} = ...' entry — aborting to avoid a program-ID mismatch.${NC}"; exit 1
        fi
        echo -e "${GREEN}✓ Local program id synced for $prog: $pid${NC}"
    done
}

# Run `anchor test` against ONLY the given files by rewriting the [scripts].test
# line (passing files positionally APPENDS to the configured suite). Restored on exit.
run_focused() {
    local files="$1"
    awk -v f="$files" '/^test = /{print "test = \"pnpm exec ts-mocha -p ./tsconfig.json -t 1000000 " f "\""; next} {print}' \
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
