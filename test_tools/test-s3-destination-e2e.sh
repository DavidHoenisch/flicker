#!/bin/bash
# End-to-end test for S3 destination with file tailing
# Tests: File tailing -> Flicker -> S3 (MinIO)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "=========================================="
echo "S3 Destination E2E Test (File Tailing)"
echo "=========================================="
echo ""

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to cleanup
cleanup() {
    echo ""
    echo -e "${YELLOW}Cleaning up...${NC}"

    if [ ! -z "$FLICKER_PID" ]; then
        kill $FLICKER_PID 2>/dev/null || true
    fi

    if [ ! -z "$GENERATOR_PID" ]; then
        kill $GENERATOR_PID 2>/dev/null || true
    fi

    # Cleanup test files
    rm -f test_tools/test-s3-dest.log

    # Stop MinIO containers
    echo -e "${YELLOW}Stopping MinIO containers...${NC}"
    cd test_tools
    docker-compose down -v
    cd ..

    echo -e "${GREEN}Cleanup complete${NC}"
}

trap cleanup EXIT INT TERM

# Check if Docker is installed
if ! command -v docker &> /dev/null; then
    echo -e "${RED}Error: Docker is not installed${NC}"
    echo "Please install Docker: https://docs.docker.com/get-docker/"
    exit 1
fi

# Check if docker-compose is installed
if ! command -v docker-compose &> /dev/null; then
    echo -e "${RED}Error: docker-compose is not installed${NC}"
    echo "Please install docker-compose: https://docs.docker.com/compose/install/"
    exit 1
fi

# Check if binary exists
if [ ! -f "./target/release/flicker" ]; then
    echo -e "${YELLOW}Building Flicker in release mode...${NC}"
    cargo build --release
fi

echo -e "${GREEN}[1/4] Starting MinIO in Docker...${NC}"
cd test_tools
docker-compose up -d
cd ..

# Wait for MinIO to be healthy
echo -e "${GREEN}[2/4] Waiting for MinIO to be ready...${NC}"
for i in {1..30}; do
    if curl -s http://localhost:9001/minio/health/live > /dev/null 2>&1; then
        echo -e "${GREEN}✓ MinIO is healthy${NC}"
        break
    fi
    if [ $i -eq 30 ]; then
        echo -e "${RED}✗ MinIO is not responding after 30 seconds${NC}"
        exit 1
    fi
    sleep 1
done

# Configure AWS environment for S3
export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
export AWS_ENDPOINT_URL=http://localhost:9001
export AWS_REGION=us-east-1
export AWS_S3_FORCE_PATH_STYLE=true

# Verify/create bucket
echo -e "${GREEN}[3/4] Verifying S3 bucket...${NC}"
if command -v aws &> /dev/null; then
    if aws s3 ls s3://flicker-logs --endpoint-url http://localhost:9001 > /dev/null 2>&1; then
        echo -e "${GREEN}✓ Bucket 'flicker-logs' is ready${NC}"
    else
        echo -e "${YELLOW}Creating bucket 'flicker-logs'...${NC}"
        aws s3 mb s3://flicker-logs --endpoint-url http://localhost:9001
    fi
else
    echo -e "${YELLOW}Warning: AWS CLI not found. Install with: pip install awscli${NC}"
    echo -e "${YELLOW}Attempting to continue (bucket may be auto-created)${NC}"
fi

# Create test log file
TEST_LOG="test_tools/test-s3-dest.log"
echo "[$(date)] Initial log entry" > "$TEST_LOG"

# Start Flicker with S3 destination
echo -e "${GREEN}[4/4] Starting Flicker with S3 destination...${NC}"
./target/release/flicker -c test_tools/test-s3-destination-config.yaml &
FLICKER_PID=$!
sleep 3

# Start log generator
echo -e "${GREEN}Starting log generator...${NC}"
./test_tools/test-log-generator.py \
    --path "$TEST_LOG" \
    --volume medium \
    --delay 2000 &
GENERATOR_PID=$!

echo ""
echo -e "${GREEN}==========================================="
echo "All components started successfully!"
echo "==========================================${NC}"
echo ""
echo "MinIO S3:   http://localhost:9001 (Docker)"
echo "MinIO Web:  http://localhost:9002 (credentials: minioadmin/minioadmin)"
echo "Flicker:    Running with S3 destination (PID: $FLICKER_PID)"
echo "Generator:  Running (PID: $GENERATOR_PID)"
echo ""
echo -e "${BLUE}==========================================="
echo "Test Scenario: File Tailing to S3"
echo "==========================================${NC}"
echo ""
echo -e "${YELLOW}What's happening:${NC}"
echo "  1. MinIO S3 running in Docker container"
echo "  2. Log entries being generated every 2 seconds"
echo "  3. Flicker tailing logs and shipping to S3"
echo "  4. Logs stored in s3://flicker-logs/file-logs/logs/..."
echo ""
echo -e "${YELLOW}List uploaded files in S3:${NC}"
echo "  aws s3 ls s3://flicker-logs/file-logs/logs/ --recursive --endpoint-url http://localhost:9001"
echo ""
echo -e "${YELLOW}Download and view a log file:${NC}"
echo "  aws s3 ls s3://flicker-logs/file-logs/logs/ --recursive --endpoint-url http://localhost:9001"
echo "  aws s3 cp s3://flicker-logs/file-logs/logs/YYYY-MM-DD/HH-MM-SS-uuid.jsonl - --endpoint-url http://localhost:9001"
echo ""
echo -e "${YELLOW}View MinIO Web Console:${NC}"
echo "  Open http://localhost:9002 in your browser"
echo "  Login: minioadmin / minioadmin"
echo "  Browse to 'flicker-logs' bucket"
echo ""
echo "Press Ctrl+C to stop all processes"
echo ""

# Wait for user interrupt
wait $FLICKER_PID