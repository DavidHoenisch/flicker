#!/bin/bash

# End-to-End Docker Test
# Starts receiver, Flicker, and Docker test container

set -e

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PROJECT_DIR=$(dirname "$SCRIPT_DIR")

echo "==================================="
echo "Flicker Docker E2E Test"
echo "==================================="
echo ""
echo "This script will:"
echo "  1. Start the HTTP receiver on port 8000"
echo "  2. Start Flicker with Docker test config"
echo "  3. Start the Docker test container"
echo ""
echo "Press Ctrl+C to stop all processes"
echo ""

# Cleanup function
cleanup() {
    echo ""
    echo "Shutting down..."

    # Kill all background jobs
    kill $(jobs -p) 2>/dev/null || true

    # Stop Docker container
    docker rm -f flicker-test-logger 2>/dev/null || true

    echo "Cleanup complete"
    exit 0
}

# Set up trap
trap cleanup SIGINT SIGTERM EXIT

# Step 1: Start receiver
echo "Starting HTTP receiver..."
cd "$SCRIPT_DIR"
python3 test-receiver.py &
RECEIVER_PID=$!

# Wait for receiver to start
sleep 2

# Step 2: Start Flicker
echo "Starting Flicker..."
cd "$PROJECT_DIR"
cargo run --quiet -- -c test_tools/docker-test-config.yaml &
FLICKER_PID=$!

# Wait for Flicker to initialize
sleep 3

# Step 3: Start Docker test container
echo "Starting Docker test container..."
cd "$SCRIPT_DIR"
./docker-test-container.sh &
DOCKER_PID=$!

echo ""
echo "==================================="
echo "All processes started!"
echo "==================================="
echo "Receiver PID:  $RECEIVER_PID"
echo "Flicker PID:   $FLICKER_PID"
echo "Container PID: $DOCKER_PID"
echo ""
echo "Watch for log batches in the receiver output above."
echo "Press Ctrl+C to stop all processes."
echo ""

# Wait for user interrupt
wait
