#!/bin/bash
# End-to-end test for API tailing functionality
# Tests: Mock API server -> Flicker API tailing -> Test receiver

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "=========================================="
echo "API Tailing End-to-End Test"
echo "=========================================="
echo ""

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Function to cleanup background processes
cleanup() {
    echo ""
    echo -e "${YELLOW}Cleaning up...${NC}"

    if [ ! -z "$RECEIVER_PID" ]; then
        echo "Stopping receiver (PID: $RECEIVER_PID)..."
        kill $RECEIVER_PID 2>/dev/null || true
    fi

    if [ ! -z "$API_SERVER_PID" ]; then
        echo "Stopping API server (PID: $API_SERVER_PID)..."
        kill $API_SERVER_PID 2>/dev/null || true
    fi

    if [ ! -z "$FLICKER_PID" ]; then
        echo "Stopping Flicker (PID: $FLICKER_PID)..."
        kill $FLICKER_PID 2>/dev/null || true
    fi

    echo -e "${GREEN}Cleanup complete${NC}"
}

trap cleanup EXIT INT TERM

# Check if binary exists
if [ ! -f "./target/release/flicker" ]; then
    echo -e "${YELLOW}Building Flicker in release mode...${NC}"
    cargo build --release
fi

# Start test receiver
echo -e "${GREEN}[1/4] Starting test receiver on port 8000...${NC}"
./test_tools/test-receiver.py &
RECEIVER_PID=$!
sleep 2

# Start mock API server
echo -e "${GREEN}[2/4] Starting mock API server on port 9000...${NC}"
./test_tools/test-api-server.py --port 9000 > /dev/null 2>&1 &
API_SERVER_PID=$!

# Wait for API server to be ready (with retry)
echo -e "${GREEN}[3/4] Waiting for API server to be ready...${NC}"
for i in {1..10}; do
    if curl -s "http://localhost:9000/api/events?limit=5" > /dev/null 2>&1; then
        echo -e "${GREEN}✓ API server is responding${NC}"
        break
    fi
    if [ $i -eq 10 ]; then
        echo -e "${RED}✗ API server is not responding after 10 seconds${NC}"
        exit 1
    fi
    sleep 1
done

# Start Flicker with API tailing
echo -e "${GREEN}[4/4] Starting Flicker with API tailing...${NC}"
./target/release/flicker -c test_tools/test-api-config.yaml &
FLICKER_PID=$!

echo ""
echo -e "${GREEN}=========================================="
echo "All components started successfully!"
echo "==========================================${NC}"
echo ""
echo "Receiver:   http://localhost:8000 (PID: $RECEIVER_PID)"
echo "API Server: http://localhost:9000 (PID: $API_SERVER_PID)"
echo "Flicker:    Running (PID: $FLICKER_PID)"
echo ""
echo -e "${YELLOW}Test is running. You should see:${NC}"
echo "  1. API server generating periodic events every 5 seconds"
echo "  2. Flicker polling the API every 5 seconds"
echo "  3. Receiver showing batches of audit log events"
echo ""
echo -e "${YELLOW}Watch for logs being shipped from api://mock-vendor-api${NC}"
echo ""
echo "Press Ctrl+C to stop all processes"
echo ""

# Wait for user interrupt
wait $FLICKER_PID
