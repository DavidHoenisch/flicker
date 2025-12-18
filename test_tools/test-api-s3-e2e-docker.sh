#!/bin/bash
# Ultimate end-to-end test: API tailing + S3 registry (MinIO in Docker)
# Tests: Mock API -> Flicker (with S3 state tracking) -> Receiver
# This simulates a stateless container deployment pulling vendor API logs

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "=========================================="
echo "API Tailing + S3 Registry E2E Test"
echo "           (MinIO in Docker)"
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

    if [ ! -z "$FLICKER_PID" ]; then
        kill $FLICKER_PID 2>/dev/null || true
    fi

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

echo -e "${GREEN}[1/6] Starting MinIO in Docker...${NC}"
cd test_tools
docker-compose up -d
cd ..

# Wait for MinIO to be healthy
echo -e "${GREEN}[2/6] Waiting for MinIO to be ready...${NC}"
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
echo -e "${GREEN}[3/6] Configuring S3 access...${NC}"
export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
export AWS_ENDPOINT_URL=http://localhost:9001
export AWS_REGION=us-east-1
export AWS_S3_FORCE_PATH_STYLE=true

# Verify bucket exists
if command -v aws &> /dev/null; then
    if aws s3 ls s3://flicker-api-test --endpoint-url http://localhost:9001 > /dev/null 2>&1; then
        echo -e "${GREEN}✓ Bucket 'flicker-api-test' is ready${NC}"
    else
        echo -e "${YELLOW}Creating bucket 'flicker-api-test'...${NC}"
        aws s3 mb s3://flicker-api-test --endpoint-url http://localhost:9001
    fi
else
    echo -e "${YELLOW}Warning: AWS CLI not found. Install with: pip install awscli${NC}"
    echo -e "${YELLOW}Assuming bucket was created by docker-compose setup${NC}"
fi

# Start test receiver
echo -e "${GREEN}[4/6] Starting test receiver on port 8000...${NC}"
./test_tools/test-receiver.py &
RECEIVER_PID=$!
sleep 2

# Start mock API server
echo -e "${GREEN}[5/6] Starting mock vendor API server on port 9000...${NC}"
./test_tools/test-api-server.py --port 9000 > /dev/null 2>&1 &
API_SERVER_PID=$!

# Wait for API server to be ready (with retry)
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
echo -e "${GREEN}==========================================="
echo "All components started successfully!"
echo "==========================================${NC}"
echo ""
echo "MinIO S3:   http://localhost:9001 (Docker)"
echo "MinIO Web:  http://localhost:9002 (credentials: minioadmin/minioadmin)"
echo "API Server: http://localhost:9000 (PID: $API_SERVER_PID)"
echo "Receiver:   http://localhost:8000 (PID: $RECEIVER_PID)"
echo "Flicker:    Running with S3 registry (PID: $FLICKER_PID)"
echo ""
echo -e "${BLUE}==========================================="
echo "Test Scenario: Stateless Vendor API Integration"
echo "==========================================${NC}"
echo ""
echo -e "${YELLOW}What's happening:${NC}"
echo "  1. MinIO S3 running in Docker container"
echo "  2. Mock vendor API generates audit events every 5 seconds"
echo "  3. Flicker polls the API every 3 seconds"
echo "  4. New events are extracted and shipped to the receiver"
echo "  5. Registry state (last timestamp + cursor) saved to S3"
echo "  6. On restart, Flicker resumes from last position"
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
echo -e "${YELLOW}View MinIO Web Console:${NC}"
echo "  Open http://localhost:9002 in your browser"
echo "  Login: minioadmin / minioadmin"
echo "  Browse to 'flicker-api-test' bucket to see registry.json"
echo ""
echo -e "${YELLOW}Test registry persistence:${NC}"
echo "  1. Let it run for 30 seconds (collect some events)"
echo "  2. Press Ctrl+C to stop Flicker"
echo "  3. Check S3: aws s3 cp s3://flicker-api-test/api-registry.json - --endpoint-url http://localhost:9001 | jq"
echo "  4. Restart Flicker manually (it should resume from last timestamp)"
echo ""
echo "Press Ctrl+C to stop all processes"
echo ""

# Wait for user interrupt
wait $FLICKER_PID
