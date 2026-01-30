#!/bin/bash
# End-to-end test for Flicker Data Masking feature
# Tests PII redaction: generator -> Flicker (with masking) -> receiver
# Verifies that sensitive data is properly masked in the output

set -e

echo "=========================================="
echo "Flicker Data Masking End-to-End Test"
echo "=========================================="
echo ""
echo "This test verifies that PII is properly redacted:"
echo "  - Email addresses"
echo "  - Credit card numbers"
echo "  - Social Security Numbers (SSN)"
echo "  - Phone numbers"
echo "  - IP addresses"
echo "  - Session tokens (custom pattern)"
echo ""

# Clean up any existing test files
rm -f ./test-masking.log
echo "Cleaned up test-masking.log"
echo ""

# Start the receiver in background
echo "Starting HTTP receiver on port 8000..."
./test-receiver.py &
RECEIVER_PID=$!
sleep 2

# Start Flicker in background from the test_tools directory
echo "Starting Flicker with masking enabled..."
cargo run --quiet --manifest-path ../Cargo.toml -- -c test-masking-config.yaml &
FLICKER_PID=$!
sleep 3

# Start PII log generator
echo ""
echo "=========================================="
echo "Starting PII log generator"
echo "Will write logs containing:"
echo "  - Emails (e.g., john.doe@example.com)"
echo "  - Credit cards (e.g., 4111111111111111)"
echo "  - SSNs (e.g., 123-45-6789)"
echo "  - Phone numbers (e.g., 555-123-4567)"
echo "  - IP addresses (e.g., 192.168.1.100)"
echo "  - Session tokens"
echo ""
echo "Expected output in receiver:"
echo "  - [EMAIL_REDACTED] instead of email addresses"
echo "  - [CC_REDACTED] instead of credit cards"
echo "  - [SSN_REDACTED] instead of SSNs"
echo "  - [PHONE_REDACTED] instead of phone numbers"
echo "  - [IP_REDACTED] instead of IP addresses"
echo "  - session_token=[TOKEN_REDACTED] instead of tokens"
echo ""
echo "Press Ctrl+C to stop all processes"
echo "=========================================="
echo ""

./test-masking-generator.py &
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
    rm -f ./test-masking.log
    echo "All processes stopped and test file cleaned"
}

trap cleanup EXIT INT TERM

# Wait for user interrupt
wait $GENERATOR_PID
