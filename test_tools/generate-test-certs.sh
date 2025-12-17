#!/bin/bash
# Generate test certificates for mTLS testing
# This creates a CA, server cert, and client cert for testing purposes only

set -e

CERT_DIR="test_tools/certs"

# Create certs directory if it doesn't exist
mkdir -p "$CERT_DIR"

echo "=== Generating test certificates for mTLS testing ==="
echo ""

# 1. Generate CA private key and certificate
echo "[1/6] Generating CA private key..."
openssl genrsa -out "$CERT_DIR/ca.key" 4096 2>/dev/null

echo "[2/6] Generating CA certificate..."
openssl req -new -x509 -days 365 -key "$CERT_DIR/ca.key" \
    -out "$CERT_DIR/ca.crt" \
    -subj "/C=US/ST=Test/L=Test/O=Flicker Test CA/CN=Test CA" \
    2>/dev/null

# 2. Generate server private key and certificate
echo "[3/6] Generating server private key..."
openssl genrsa -out "$CERT_DIR/server.key" 4096 2>/dev/null

echo "[4/6] Generating server certificate signing request..."
openssl req -new -key "$CERT_DIR/server.key" \
    -out "$CERT_DIR/server.csr" \
    -subj "/C=US/ST=Test/L=Test/O=Flicker Test/CN=localhost" \
    2>/dev/null

echo "[5/6] Signing server certificate with CA..."
openssl x509 -req -days 365 -in "$CERT_DIR/server.csr" \
    -CA "$CERT_DIR/ca.crt" -CAkey "$CERT_DIR/ca.key" \
    -CAcreateserial -out "$CERT_DIR/server.crt" \
    -extfile <(printf "subjectAltName=DNS:localhost,IP:127.0.0.1") \
    2>/dev/null

# 3. Generate client private key and certificate
echo "[6/6] Generating client private key..."
openssl genrsa -out "$CERT_DIR/client.key" 4096 2>/dev/null

echo "[7/8] Generating client certificate signing request..."
openssl req -new -key "$CERT_DIR/client.key" \
    -out "$CERT_DIR/client.csr" \
    -subj "/C=US/ST=Test/L=Test/O=Flicker Test/CN=flicker-client" \
    2>/dev/null

echo "[8/8] Signing client certificate with CA..."
openssl x509 -req -days 365 -in "$CERT_DIR/client.csr" \
    -CA "$CERT_DIR/ca.crt" -CAkey "$CERT_DIR/ca.key" \
    -CAcreateserial -out "$CERT_DIR/client.crt" \
    2>/dev/null

# Clean up CSR files
rm -f "$CERT_DIR/server.csr" "$CERT_DIR/client.csr" "$CERT_DIR/ca.srl"

echo ""
echo "=== Certificate generation complete! ==="
echo ""
echo "Generated files in $CERT_DIR/:"
echo "  ca.crt         - CA certificate (for server and client trust)"
echo "  ca.key         - CA private key"
echo "  server.crt     - Server certificate"
echo "  server.key     - Server private key"
echo "  client.crt     - Client certificate (for mTLS)"
echo "  client.key     - Client private key (for mTLS)"
echo ""
echo "You can now use these certificates for testing mTLS with Flicker!"
