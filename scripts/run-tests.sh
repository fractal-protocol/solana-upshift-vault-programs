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

# Check if local validator is running
echo -e "${YELLOW}Checking if local validator is running...${NC}"
if curl -s http://127.0.0.1:8899/health > /dev/null 2>&1; then
    echo -e "${GREEN}✓ Local validator is running${NC}"
    SKIP_VALIDATOR="--skip-local-validator"
else
    echo -e "${YELLOW}⚠ Local validator not detected${NC}"
    echo -e "${YELLOW}Starting local validator in background...${NC}"
    solana-test-validator --reset > /dev/null 2>&1 &
    VALIDATOR_PID=$!
    sleep 3
    echo -e "${GREEN}✓ Local validator started (PID: $VALIDATOR_PID)${NC}"
    # We started our own validator, so tell anchor not to start a second one
    # (a second validator on :8899 would fail with a port conflict).
    SKIP_VALIDATOR="--skip-local-validator"
fi
echo ""

# Stop our background validator (if any) on any exit path.
cleanup() {
    if [ -n "$VALIDATOR_PID" ]; then
        echo ""
        echo -e "${YELLOW}Cleaning up validator (PID: $VALIDATOR_PID)...${NC}"
        kill "$VALIDATOR_PID" 2>/dev/null
        echo -e "${GREEN}✓ Validator stopped${NC}"
    fi
}
trap cleanup EXIT

# Menu
echo -e "${BLUE}Select test suite:${NC}"
echo "1) Run all tests (comprehensive)"
echo "2) Run admin function tests"
echo "3) Run operator function tests"
echo "4) Run user function tests"
echo "5) Run metadata tests (IDL validation)"
echo "6) Run AUM limits tests (IDL validation)"
echo "7) Run comprehensive functionality test"
echo "8) Run all IDL validation tests (no validator needed)"
echo "9) Exit"
echo ""

read -p "Enter choice [1-9]: " choice

case $choice in
    1)
        echo -e "${GREEN}Running all tests...${NC}"
        anchor test $SKIP_VALIDATOR
        ;;
    2)
        echo -e "${GREEN}Running admin function tests...${NC}"
        anchor test $SKIP_VALIDATOR tests/4_admin.ts
        ;;
    3)
        echo -e "${GREEN}Running operator function tests...${NC}"
        anchor test $SKIP_VALIDATOR tests/3_operator.ts
        ;;
    4)
        echo -e "${GREEN}Running user function tests...${NC}"
        anchor test $SKIP_VALIDATOR tests/2_users.ts
        ;;
    5)
        echo -e "${GREEN}Running metadata tests (IDL validation)...${NC}"
        npx ts-mocha -p ./tsconfig.json -t 1000000 tests/6_metadata.ts tests/8_update_metadata.ts
        ;;
    6)
        echo -e "${GREEN}Running AUM limits tests (IDL validation)...${NC}"
        npx ts-mocha -p ./tsconfig.json -t 1000000 tests/9_set_aum_limits.ts
        ;;
    7)
        echo -e "${GREEN}Running comprehensive functionality test...${NC}"
        anchor test $SKIP_VALIDATOR tests/10_comprehensive_functionality.ts
        ;;
    8)
        echo -e "${GREEN}Running all IDL validation tests...${NC}"
        npx ts-mocha -p ./tsconfig.json -t 1000000 tests/6_metadata.ts tests/8_update_metadata.ts tests/9_set_aum_limits.ts
        ;;
    9)
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

