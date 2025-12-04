#!/bin/bash
# End-to-end test for Flicker
# Tests the complete flow: generator -> Flicker -> receiver
# Now with MULTIPLE log files!

set -e

echo "=========================================="
echo "Flicker End-to-End Test (Multi-File)"
echo "=========================================="
echo ""

# Clean up any existing test files
rm -f ./test1.log ./test2.log ./test3.log ./test4.log ./test5.log
echo "Cleaned up test*.log files"
echo ""

# Start the receiver in background
echo "Starting HTTP receiver on port 8000..."
./test-receiver.py &
RECEIVER_PID=$!
sleep 2

# Start Flicker in background
echo "Starting Flicker (tailing 5 log files)..."
cargo run --quiet -- -c test-config.yaml &
FLICKER_PID=$!
sleep 3

# Start log generator in high volume mode with 5 files
echo ""
echo "=========================================="
echo "Starting log generator (high volume, 5 files)"
echo "Will write to test1.log through test5.log"
echo "Each Flicker task will tail independently!"
echo "Watch the receiver for batches from all files!"
echo ""
echo "Press Ctrl+C to stop all processes"
echo "=========================================="
echo ""

./test-log-generator.py --volume high --multi-file 5 &
GENERATOR_PID=$!

# Function to cleanup on exit
cleanup() {
    echo ""
    echo ""
    echo "=========================================="
    echo "Cleaning up..."
    echo "=========================================="
    kill $GENERATOR_PID 2>/dev/null || true
    kill $FLICKER_PID 2>/dev/null || true
    kill $RECEIVER_PID 2>/dev/null || true
    wait 2>/dev/null || true
    echo "All processes stopped"
}

trap cleanup EXIT INT TERM

# Wait for user interrupt
wait $GENERATOR_PID
