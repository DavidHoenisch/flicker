#!/bin/bash
# Test script to demonstrate dual-trigger buffering

echo "=== Testing Flicker Buffering ==="
echo ""
echo "This will demonstrate:"
echo "1. Size-based flush (buffer reaches 5 lines)"
echo "2. Time-based flush (10 seconds elapse with partial buffer)"
echo ""

# Clean up test files
rm -f /tmp/test.log /tmp/test2.log

# Start Flicker in background
echo "Starting Flicker..."
cargo run --quiet -- -c test-config.yaml &
FLICKER_PID=$!

sleep 2

echo ""
echo "--- Test 1: Size-based flush ---"
echo "Writing 7 lines to /tmp/test.log (buffer size = 5)"
for i in {1..7}; do
  echo "Line $i at $(date +%H:%M:%S)" >> /tmp/test.log
  sleep 0.2
done

echo ""
echo "You should see:"
echo "  - First flush after 5 lines (buffer full)"
echo "  - Second flush will happen after 10s (time elapsed) for remaining 2 lines"
echo ""

sleep 5

echo "--- Test 2: Time-based flush ---"
echo "Writing 2 lines to /tmp/test2.log (buffer size = 3)"
echo "Line A at $(date +%H:%M:%S)" >> /tmp/test2.log
sleep 1
echo "Line B at $(date +%H:%M:%S)" >> /tmp/test2.log

echo ""
echo "You should see a flush after 15 seconds (time elapsed) for these 2 lines"
echo ""
echo "Waiting 20 seconds to observe time-based flushes..."
sleep 20

echo ""
echo "--- Cleanup ---"
kill $FLICKER_PID 2>/dev/null
echo "Test complete!"
