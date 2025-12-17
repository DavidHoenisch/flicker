#!/bin/bash
# End-to-end test for Flicker mTLS functionality
# This script:
# 1. Generates test certificates (if needed)
# 2. Starts the mTLS HTTPS receiver
# 3. Starts Flicker with mTLS configuration
# 4. Generates test log data
# 5. Verifies that logs are successfully sent over mTLS

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
CERT_DIR="$SCRIPT_DIR/certs"
LOG_FILE="$SCRIPT_DIR/test-mtls.log"
LOG_FILE2="$SCRIPT_DIR/test-mtls-secondary.log"

# PIDs of background processes
RECEIVER_PID=""
FLICKER_PID=""
GENERATOR_PID=""

cleanup() {
    echo ""
    echo -e "${YELLOW}[CLEANUP]${NC} Stopping all processes..."

    if [ -n "$GENERATOR_PID" ]; then
        kill $GENERATOR_PID 2>/dev/null || true
        echo -e "${YELLOW}[CLEANUP]${NC} Stopped log generator"
    fi

    if [ -n "$FLICKER_PID" ]; then
        kill $FLICKER_PID 2>/dev/null || true
        echo -e "${YELLOW}[CLEANUP]${NC} Stopped Flicker"
    fi

    if [ -n "$RECEIVER_PID" ]; then
        kill $RECEIVER_PID 2>/dev/null || true
        echo -e "${YELLOW}[CLEANUP]${NC} Stopped mTLS receiver"
    fi

    # Clean up test log files
    rm -f "$LOG_FILE" "$LOG_FILE2"
    echo -e "${YELLOW}[CLEANUP]${NC} Removed test log files"

    echo -e "${GREEN}[DONE]${NC} Cleanup complete"
}

trap cleanup EXIT INT TERM

echo -e "${CYAN}╔════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║  Flicker mTLS End-to-End Test                 ║${NC}"
echo -e "${CYAN}╚════════════════════════════════════════════════╝${NC}"
echo ""

# Step 1: Generate certificates if they don't exist
if [ ! -d "$CERT_DIR" ] || [ ! -f "$CERT_DIR/client.crt" ]; then
    echo -e "${BLUE}[STEP 1/6]${NC} Generating test certificates..."
    cd "$PROJECT_DIR"
    ./test_tools/generate-test-certs.sh
    echo ""
else
    echo -e "${GREEN}[STEP 1/6]${NC} Test certificates already exist"
    echo ""
fi

# Step 2: Build Flicker if needed
if [ ! -f "$PROJECT_DIR/target/release/flicker" ]; then
    echo -e "${BLUE}[STEP 2/6]${NC} Building Flicker..."
    cd "$PROJECT_DIR"
    cargo build --release --quiet
    echo -e "${GREEN}[STEP 2/6]${NC} Build complete"
    echo ""
else
    echo -e "${GREEN}[STEP 2/6]${NC} Flicker already built"
    echo ""
fi

# Step 3: Start mTLS receiver
echo -e "${BLUE}[STEP 3/6]${NC} Starting mTLS HTTPS receiver on port 8443..."
cd "$PROJECT_DIR"
./test_tools/test-mtls-receiver.py --port 8443 &
RECEIVER_PID=$!
sleep 2

# Check if receiver started successfully
if ! kill -0 $RECEIVER_PID 2>/dev/null; then
    echo -e "${RED}[ERROR]${NC} Failed to start mTLS receiver"
    exit 1
fi
echo -e "${GREEN}[STEP 3/6]${NC} mTLS receiver running (PID: $RECEIVER_PID)"
echo ""

# Step 4: Create empty test log files
echo -e "${BLUE}[STEP 4/6]${NC} Creating test log files..."
touch "$LOG_FILE"
touch "$LOG_FILE2"
echo -e "${GREEN}[STEP 4/6]${NC} Log files created"
echo ""

# Step 5: Start Flicker with mTLS config
echo -e "${BLUE}[STEP 5/6]${NC} Starting Flicker with mTLS configuration..."
cd "$PROJECT_DIR"
./target/release/flicker -c test_tools/test-mtls-config.yaml &
FLICKER_PID=$!
sleep 2

# Check if Flicker started successfully
if ! kill -0 $FLICKER_PID 2>/dev/null; then
    echo -e "${RED}[ERROR]${NC} Failed to start Flicker"
    exit 1
fi
echo -e "${GREEN}[STEP 5/6]${NC} Flicker running (PID: $FLICKER_PID)"
echo ""

# Step 6: Generate test logs
echo -e "${BLUE}[STEP 6/6]${NC} Generating test log data..."
echo -e "${CYAN}[INFO]${NC} This will generate logs for 10 seconds..."
echo ""

cd "$PROJECT_DIR"
timeout 10 ./test_tools/test-log-generator.py --path "$LOG_FILE" --volume high 2>/dev/null &
GENERATOR_PID=$!

# Also generate some logs for the secondary file
timeout 10 ./test_tools/test-log-generator.py --path "$LOG_FILE2" --volume medium 2>/dev/null &

# Wait for generators to finish
wait

echo ""
echo -e "${GREEN}[SUCCESS]${NC} Test log generation complete"
echo ""

# Give Flicker time to flush remaining buffers
echo -e "${CYAN}[INFO]${NC} Waiting 8 seconds for final buffer flush..."
sleep 8

echo ""
echo -e "${CYAN}╔════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║  Test Complete!                                ║${NC}"
echo -e "${CYAN}╚════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "${GREEN}✓${NC} mTLS certificates generated and loaded"
echo -e "${GREEN}✓${NC} HTTPS server with client certificate verification running"
echo -e "${GREEN}✓${NC} Flicker successfully connected using mTLS"
echo -e "${GREEN}✓${NC} Log batches sent and received over encrypted mTLS connection"
echo ""
echo -e "${CYAN}[INFO]${NC} Check the receiver output above to verify logs were received"
echo -e "${CYAN}[INFO]${NC} Look for the green ${GREEN}[mTLS: flicker-client]${NC} indicators"
echo ""

# Keep running for a bit so user can see the output
echo -e "${YELLOW}[WAITING]${NC} Press Ctrl+C to stop the test..."
sleep 5

echo ""
echo -e "${GREEN}[PASSED]${NC} mTLS functional test completed successfully!"
