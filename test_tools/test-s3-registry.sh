#!/bin/bash
# End-to-end test for S3 registry functionality using MinIO
# Tests: File tailing with S3 registry state tracking

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "=========================================="
echo "S3 Registry End-to-End Test (MinIO)"
echo "=========================================="
echo ""

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Function to cleanup
cleanup() {
    echo ""
    echo -e "${YELLOW}Cleaning up...${NC}"

    if [ ! -z "$RECEIVER_PID" ]; then
        kill $RECEIVER_PID 2>/dev/null || true
    fi

    if [ ! -z "$MINIO_PID" ]; then
        kill $MINIO_PID 2>/dev/null || true
    fi

    if [ ! -z "$FLICKER_PID" ]; then
        kill $FLICKER_PID 2>/dev/null || true
    fi

    if [ ! -z "$GENERATOR_PID" ]; then
        kill $GENERATOR_PID 2>/dev/null || true
    fi

    # Cleanup test files
    rm -f test_tools/test-s3*.log

    echo -e "${GREEN}Cleanup complete${NC}"
}

trap cleanup EXIT INT TERM

# Check if MinIO is installed
if ! command -v minio &> /dev/null; then
    echo -e "${RED}Error: MinIO is not installed${NC}"
    echo "Please install MinIO:"
    echo "  macOS: brew install minio/stable/minio"
    echo "  Linux: wget https://dl.min.io/server/minio/release/linux-amd64/minio && chmod +x minio"
    exit 1
fi

# Check if binary exists
if [ ! -f "./target/release/flicker" ]; then
    echo -e "${YELLOW}Building Flicker in release mode...${NC}"
    cargo build --release
fi

# Create MinIO data directory
MINIO_DATA_DIR="/tmp/flicker-test-minio-data"
mkdir -p "$MINIO_DATA_DIR"

echo -e "${GREEN}[1/5] Starting MinIO (local S3) on port 9001...${NC}"
MINIO_ROOT_USER=minioadmin MINIO_ROOT_PASSWORD=minioadmin \
    minio server "$MINIO_DATA_DIR" --address ":9001" --console-address ":9002" > /tmp/minio.log 2>&1 &
MINIO_PID=$!
sleep 3

# Configure MinIO client (mc) if available, otherwise use AWS CLI
echo -e "${GREEN}[2/5] Creating S3 bucket 'flicker-test'...${NC}"
export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
export AWS_ENDPOINT_URL=http://localhost:9001
export AWS_REGION=us-east-1
export AWS_S3_FORCE_PATH_STYLE=true

# Create bucket using AWS CLI or mc
if command -v aws &> /dev/null; then
    aws s3 mb s3://flicker-test --endpoint-url http://localhost:9001 2>/dev/null || true
    echo -e "${GREEN}✓ Bucket created (using AWS CLI)${NC}"
elif command -v mc &> /dev/null; then
    mc alias set minio http://localhost:9001 minioadmin minioadmin 2>/dev/null || true
    mc mb minio/flicker-test 2>/dev/null || true
    echo -e "${GREEN}✓ Bucket created (using mc)${NC}"
else
    echo -e "${YELLOW}Warning: Neither 'aws' nor 'mc' CLI found. Bucket may not be created.${NC}"
    echo "Install AWS CLI: pip install awscli"
fi

# Start test receiver
echo -e "${GREEN}[3/5] Starting test receiver on port 8000...${NC}"
./test_tools/test-receiver.py &
RECEIVER_PID=$!
sleep 2

# Create test log file
TEST_LOG="test_tools/test-s3-registry.log"
echo "[$(date)] Initial log entry" > "$TEST_LOG"

# Start Flicker with S3 registry
echo -e "${GREEN}[4/5] Starting Flicker with S3 registry...${NC}"
./target/release/flicker \
    -c test_tools/test-config.yaml \
    --track \
    --registry-file s3://flicker-test/registry.json &
FLICKER_PID=$!
sleep 3

# Start log generator
echo -e "${GREEN}[5/5] Starting log generator...${NC}"
./test_tools/test-log-generator.py \
    --path "$TEST_LOG" \
    --volume medium \
    --delay 2000 &
GENERATOR_PID=$!

echo ""
echo -e "${GREEN}=========================================="
echo "All components started successfully!"
echo "==========================================${NC}"
echo ""
echo "MinIO S3:   http://localhost:9001 (PID: $MINIO_PID)"
echo "MinIO Web:  http://localhost:9002 (credentials: minioadmin/minioadmin)"
echo "Receiver:   http://localhost:8000 (PID: $RECEIVER_PID)"
echo "Flicker:    Running with S3 registry (PID: $FLICKER_PID)"
echo "Generator:  Running (PID: $GENERATOR_PID)"
echo ""
echo -e "${YELLOW}Test is running. You should see:${NC}"
echo "  1. Log entries being generated every 2 seconds"
echo "  2. Flicker shipping logs to the receiver"
echo "  3. Registry state being saved to S3 (s3://flicker-test/registry.json)"
echo ""
echo -e "${YELLOW}To verify S3 registry:${NC}"
echo "  aws s3 cp s3://flicker-test/registry.json - --endpoint-url http://localhost:9001"
echo ""
echo "Press Ctrl+C to stop all processes"
echo ""

# Wait for user interrupt
wait $FLICKER_PID
