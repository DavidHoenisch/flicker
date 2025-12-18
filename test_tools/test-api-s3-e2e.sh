#!/bin/bash
# Ultimate end-to-end test: API tailing + S3 registry
# Tests: Mock API -> Flicker (with S3 state tracking) -> Receiver
# This simulates a stateless container deployment pulling vendor API logs

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "=========================================="
echo "API Tailing + S3 Registry E2E Test"
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

    if [ ! -z "$RECEIVER_PID" ]; then
        kill $RECEIVER_PID 2>/dev/null || true
    fi

    if [ ! -z "$API_SERVER_PID" ]; then
        kill $API_SERVER_PID 2>/dev/null || true
    fi

    if [ ! -z "$MINIO_PID" ]; then
        kill $MINIO_PID 2>/dev/null || true
    fi

    if [ ! -z "$FLICKER_PID" ]; then
        kill $FLICKER_PID 2>/dev/null || true
    fi

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
MINIO_DATA_DIR="/tmp/flicker-test-api-s3-minio"
rm -rf "$MINIO_DATA_DIR"
mkdir -p "$MINIO_DATA_DIR"

echo -e "${GREEN}[1/6] Starting MinIO (local S3) on port 9001...${NC}"
MINIO_ROOT_USER=minioadmin MINIO_ROOT_PASSWORD=minioadmin \
    minio server "$MINIO_DATA_DIR" --address ":9001" --console-address ":9002" > /tmp/minio-api.log 2>&1 &
MINIO_PID=$!
sleep 3

# Configure AWS environment for S3
echo -e "${GREEN}[2/6] Configuring S3 access and creating bucket...${NC}"
export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
export AWS_ENDPOINT_URL=http://localhost:9001
export AWS_REGION=us-east-1
export AWS_S3_FORCE_PATH_STYLE=true

# Create bucket
if command -v aws &> /dev/null; then
    aws s3 mb s3://flicker-api-test --endpoint-url http://localhost:9001 2>/dev/null || true
    echo -e "${GREEN}✓ Bucket 'flicker-api-test' created${NC}"
else
    echo -e "${YELLOW}Warning: AWS CLI not found. Install with: pip install awscli${NC}"
fi

# Start test receiver
echo -e "${GREEN}[3/6] Starting test receiver on port 8000...${NC}"
./test_tools/test-receiver.py &
RECEIVER_PID=$!
sleep 2

# Start mock API server
echo -e "${GREEN}[4/6] Starting mock vendor API server on port 9000...${NC}"
./test_tools/test-api-server.py --port 9000 > /dev/null 2>&1 &
API_SERVER_PID=$!

# Wait for API server to be ready (with retry)
echo -e "${GREEN}[5/6] Waiting for API server to be ready...${NC}"
for i in {1..10}; do
    if curl -s "http://localhost:9000/api/events?limit=5" > /dev/null 2>&1; then
        echo -e "${GREEN}✓ API server is responding with audit logs${NC}"
        break
    fi
    if [ $i -eq 10 ]; then
        echo -e "${RED}✗ API server is not responding after 10 seconds${NC}"
        exit 1
    fi
    sleep 1
done

# Start Flicker with API tailing + S3 registry
echo -e "${GREEN}[6/6] Starting Flicker with API tailing + S3 registry...${NC}"
echo -e "${BLUE}Registry: s3://flicker-api-test/api-registry.json${NC}"
./target/release/flicker \
    -c test_tools/test-api-s3-config.yaml \
    --track \
    --registry-file s3://flicker-api-test/api-registry.json &
FLICKER_PID=$!

echo ""
echo -e "${GREEN}=========================================="
echo "All components started successfully!"
echo "==========================================${NC}"
echo ""
echo "MinIO S3:   http://localhost:9001 (PID: $MINIO_PID)"
echo "MinIO Web:  http://localhost:9002 (credentials: minioadmin/minioadmin)"
echo "API Server: http://localhost:9000 (PID: $API_SERVER_PID)"
echo "Receiver:   http://localhost:8000 (PID: $RECEIVER_PID)"
echo "Flicker:    Running with S3 registry (PID: $FLICKER_PID)"
echo ""
echo -e "${BLUE}=========================================="
echo "Test Scenario: Stateless Vendor API Integration"
echo "==========================================${NC}"
echo ""
echo -e "${YELLOW}What's happening:${NC}"
echo "  1. Mock vendor API generates audit events every 5 seconds"
echo "  2. Flicker polls the API every 3 seconds"
echo "  3. New events are extracted and shipped to the receiver"
echo "  4. Registry state (last timestamp + cursor) saved to S3"
echo "  5. On restart, Flicker resumes from last position"
echo ""
echo -e "${YELLOW}This simulates:${NC}"
echo "  ✓ Pulling vendor audit logs from a SaaS API"
echo "  ✓ Shipping them to your SIEM"
echo "  ✓ Running in a stateless container (Kubernetes/ECS)"
echo "  ✓ State persistence in S3 across pod restarts"
echo ""
echo -e "${YELLOW}Verify S3 registry state:${NC}"
echo "  aws s3 cp s3://flicker-api-test/api-registry.json - --endpoint-url http://localhost:9001 | jq"
echo ""
echo -e "${YELLOW}Check API source logs:${NC}"
echo "  You should see logs from: ${BLUE}api://mock-vendor-api-s3${NC}"
echo ""
echo -e "${YELLOW}Test registry persistence:${NC}"
echo "  1. Let it run for 30 seconds (collect some events)"
echo "  2. Press Ctrl+C to stop Flicker"
echo "  3. Check S3: aws s3 cp s3://flicker-api-test/api-registry.json -"
echo "  4. Restart Flicker manually (it should resume from last timestamp)"
echo ""
echo "Press Ctrl+C to stop all processes"
echo ""

# Wait for user interrupt
wait $FLICKER_PID
