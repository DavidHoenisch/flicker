#!/bin/bash
# Debug test for masking

set -e

echo "=== Testing Masking Config Loading ==="

# Create a minimal test config
cat > /tmp/test-masking-simple.yaml << 'EOF'
log_files:
  - path: "./test-masking.log"
    polling_frequency_ms: 250
    buffer_size: 1
    flush_interval_ms: 1000
    masking:
      enabled: true
      rules:
        email:
          enabled: true
          action: "redact"
    destination:
      endpoint: "http://localhost:8000/ingest"
      type: "http"
EOF

echo "Config created. Starting receiver..."
./test-receiver.py &
RECEIVER_PID=$!
sleep 2

echo "Starting Flicker with masking config..."
cd ..
cargo run -- -c /tmp/test-masking-simple.yaml 2>&1 &
FLICKER_PID=$!
cd test_tools
sleep 3

echo ""
echo "Writing test log with email: test@example.com"
echo "INFO - User email is test@example.com" > test-masking.log
sleep 3

echo ""
echo "=== Check receiver output above ==="
echo "If masking works, you should see [EMAIL] instead of test@example.com"
echo ""

# Cleanup
sleep 2
kill $FLICKER_PID 2>/dev/null || true
kill $RECEIVER_PID 2>/dev/null || true
wait 2>/dev/null || true
rm -f test-masking.log /tmp/test-masking-simple.yaml
echo "Test complete."
