# Flicker Test Tools

This directory contains testing tools for Flicker log shipping.

## Quick Start with Docker Compose

For S3 registry testing, we provide a `docker-compose.yml` that runs MinIO (S3-compatible storage) locally:

```bash
# Start MinIO in Docker
cd test_tools
docker-compose up -d

# MinIO will be available at:
# - API: http://localhost:9001
# - Web Console: http://localhost:9002 (login: minioadmin/minioadmin)
# - Buckets created: flicker-test, flicker-api-test

# Stop MinIO when done
docker-compose down -v
```

**Recommended:** Use the `-docker.sh` versions of S3 tests (`test-s3-registry-docker.sh`, `test-api-s3-e2e-docker.sh`) which automatically manage MinIO containers.

## Tools

### 1. HTTP Test Receiver
**File:** `test-receiver.py`

A simple HTTP server that receives and displays log batches from Flicker.

```bash
./test-receiver.py
```

Starts server on `http://localhost:8000/ingest`. Displays received batches with color-coded output.

### 2. Log Generator
**File:** `test-log-generator.py`

Generates realistic log data at configurable rates.

```bash
# Generate high-volume logs across 5 files
./test-log-generator.py --volume high --multi-file 5

# Generate low-volume logs to a single file
./test-log-generator.py --volume low --path /tmp/myapp.log

# Custom delay between log entries
./test-log-generator.py --delay 500  # 500ms between entries
```

**Volume modes:**
- `high`: 10-50ms delay (~20-100 entries/sec) - tests buffer size trigger
- `medium`: 100-500ms delay (~2-10 entries/sec) - balanced
- `low`: 1-3s delay (~0.3-1 entries/sec) - tests time-based flush trigger

### 3. Docker Test Container
**File:** `docker-test-container.sh`

Runs a Docker container that generates structured logs for testing.

```bash
./docker-test-container.sh
```

The container generates logs with various levels:
- INFO messages
- WARN messages
- ERROR messages (every 5th cycle)
- DEBUG messages

Use with `docker-test-config.yaml` to test Docker log capture.

**Environment variables:**
- `LOG_INTERVAL`: Seconds between log entries (default: 2)

Example:
```bash
LOG_INTERVAL=1 ./docker-test-container.sh
```

### 4. Mock API Server
**File:** `test-api-server.py`

A mock vendor API server that simulates a SaaS audit log API.

```bash
# Start on default port 9000
./test-api-server.py

# Start on custom port
./test-api-server.py --port 8080
```

**Features:**
- Auto-generates audit events every 5 seconds
- Supports pagination (offset-based)
- Supports time-based filtering (`?since=2024-01-01T00:00:00Z`)
- Returns JSON responses with metadata
- Simulates real vendor API behavior

**API Endpoint:**
```
GET /api/events?limit=20&offset=0&since=2024-01-01T00:00:00Z
```

**Response format:**
```json
{
  "data": [
    {
      "timestamp": "2024-01-01T12:00:00Z",
      "event_type": "user.login",
      "user": "alice",
      "message": "User alice logged in from 192.168.1.1"
    }
  ],
  "pagination": {
    "total": 100,
    "limit": 20,
    "offset": 0
  }
}
```

Use with `test-api-config.yaml` or `test-api-s3-config.yaml` to test API tailing.

### 5. End-to-End Tests

#### File Tailing E2E
**File:** `test-e2e.sh`

Automated test that starts receiver, Flicker, and log generator for file tailing.

```bash
./test-e2e.sh
```

Press Ctrl+C to stop all processes.

#### Docker E2E
**File:** `docker-test-e2e.sh`

Automated test that starts receiver, Flicker, and Docker test container.

```bash
./docker-test-e2e.sh
```

Press Ctrl+C to stop all processes.

#### API Tailing E2E
**File:** `test-api-e2e.sh`

Automated test for API tailing functionality with a mock vendor API.

```bash
./test-api-e2e.sh
```

**What it tests:**
- Mock vendor API server generating audit events
- Flicker polling REST API endpoints
- Pagination (offset-based)
- Time-based filtering
- JSON response parsing
- Shipping API logs to receiver

Press Ctrl+C to stop all processes.

#### S3 Registry E2E (MinIO)
**File:** `test-s3-registry.sh`

Automated test for S3 registry functionality using MinIO as local S3.

```bash
./test-s3-registry.sh
```

**Prerequisites:**
- MinIO installed (`brew install minio/stable/minio` on macOS)
- AWS CLI installed (`pip install awscli`)

**What it tests:**
- S3-compatible registry storage
- State persistence across restarts
- MinIO local S3 server
- File tailing with S3 registry

**Verify registry in S3:**
```bash
aws s3 cp s3://flicker-test/registry.json - --endpoint-url http://localhost:9001 | jq
```

Press Ctrl+C to stop all processes.

#### API Tailing + S3 Registry E2E
**File:** `test-api-s3-e2e.sh`

Ultimate end-to-end test combining API tailing with S3 registry storage.

```bash
./test-api-s3-e2e.sh
```

**Prerequisites:**
- MinIO installed (`brew install minio/stable/minio` on macOS)
- AWS CLI installed (`pip install awscli`)

**What it tests:**
- Full stateless container simulation
- Vendor API → Flicker (with S3 state) → SIEM receiver
- Registry state persistence in S3
- Resume from last position on restart

**This simulates:**
- Pulling vendor audit logs from a SaaS API
- Shipping them to your SIEM
- Running in a stateless container (Kubernetes/ECS)
- State persistence in S3 across pod restarts

**Verify registry state:**
```bash
aws s3 cp s3://flicker-api-test/api-registry.json - --endpoint-url http://localhost:9001 | jq
```

**Test registry persistence:**
1. Let it run for 30 seconds (collect some events)
2. Press Ctrl+C to stop Flicker
3. Check S3: `aws s3 cp s3://flicker-api-test/api-registry.json - --endpoint-url http://localhost:9001 | jq`
4. Restart Flicker manually (it should resume from last timestamp)

Press Ctrl+C to stop all processes.

#### S3 Registry E2E (Docker)
**File:** `test-s3-registry-docker.sh`

Automated test for S3 registry functionality using MinIO in Docker (recommended).

```bash
./test-s3-registry-docker.sh
```

**Prerequisites:**
- Docker installed
- docker-compose installed

**What it tests:**
- S3-compatible registry storage with MinIO in Docker
- State persistence across restarts
- File tailing with S3 registry
- No need to install MinIO locally

**Verify registry in S3:**
```bash
# Requires AWS CLI: pip install awscli
aws s3 cp s3://flicker-test/registry.json - --endpoint-url http://localhost:9001 | jq
```

**View MinIO Web Console:**
- Open http://localhost:9002
- Login: minioadmin / minioadmin

Press Ctrl+C to stop all processes (MinIO containers will be cleaned up automatically).

#### API Tailing + S3 Registry E2E (Docker)
**File:** `test-api-s3-e2e-docker.sh`

Ultimate end-to-end test combining API tailing with S3 registry storage using Docker (recommended).

```bash
./test-api-s3-e2e-docker.sh
```

**Prerequisites:**
- Docker installed
- docker-compose installed

**What it tests:**
- Full stateless container simulation with MinIO in Docker
- Vendor API → Flicker (with S3 state) → SIEM receiver
- Registry state persistence in S3
- Resume from last position on restart
- No need to install MinIO locally

**This simulates:**
- Pulling vendor audit logs from a SaaS API
- Shipping them to your SIEM
- Running in a stateless container (Kubernetes/ECS)
- State persistence in S3 across pod restarts

**Verify registry state:**
```bash
# Requires AWS CLI: pip install awscli
aws s3 cp s3://flicker-api-test/api-registry.json - --endpoint-url http://localhost:9001 | jq
```

**View MinIO Web Console:**
- Open http://localhost:9002
- Login: minioadmin / minioadmin
- Browse to 'flicker-api-test' bucket to see api-registry.json

**Test registry persistence:**
1. Let it run for 30 seconds (collect some events)
2. Press Ctrl+C to stop Flicker
3. Check S3: `aws s3 cp s3://flicker-api-test/api-registry.json - --endpoint-url http://localhost:9001 | jq`
4. Restart Flicker manually (it should resume from last timestamp)

Press Ctrl+C to stop all processes (MinIO containers will be cleaned up automatically).

#### Data Masking E2E
**File:** `test-masking-e2e.sh`

Automated test for data masking functionality that redacts PII from logs before shipping.

```bash
./test-masking-e2e.sh
```

**Prerequisites:**
- None (uses standard test setup)

**What it tests:**
- Credit card number masking
- Email address masking
- Social Security Number (SSN) masking
- Phone number masking
- IP address masking
- API key masking
- UUID masking
- JSON field-specific masking
- Regex-based custom masking rules
- Log buffering and shipping with masked data

**Expected output format:**
The receiver will display log batches where PII has been replaced with `[REDACTED]` or pattern-specific masks:
```
Credit card: 4111-XXXX-XXXX-1111 → 4111-[REDACTED]-1111
Email: user@example.com → [REDACTED]@example.com
SSN: 123-45-6789 → XXX-XX-6789
Phone: +1-555-123-4567 → +1-XXX-XXX-4567
IP: 192.168.1.100 → 192.168.xxx.xxx
```

Press Ctrl+C to stop all processes.

## Configuration Files

### docker-compose.yml
Docker Compose configuration for running MinIO (S3-compatible storage) locally.

Features:
- MinIO S3 service on ports 9001 (API) and 9002 (Web Console)
- Automatic bucket creation: `flicker-test`, `flicker-api-test`
- Health checks and service dependencies
- Credentials: minioadmin / minioadmin
- Automatic cleanup on shutdown

Usage:
```bash
cd test_tools
docker-compose up -d    # Start MinIO
docker-compose down -v  # Stop and remove volumes
```

### test-config.yaml
Configuration for testing file tailing with 5 test log files.

### docker-test-config.yaml
Configuration for testing Docker container log capture.

Includes examples of:
- Basic Docker container tailing
- Filtering with `match_on` (commented out)
- Filtering with `exclude_on` (commented out)

### test-api-config.yaml
Configuration for testing API tailing with the mock vendor API.

Features:
- Connects to `test-api-server.py` on port 9000
- Polls every 5 seconds
- Offset-based pagination
- Time-based filtering with RFC3339 timestamps
- Ships logs to test receiver

### test-api-s3-config.yaml
Configuration for testing API tailing with S3 registry storage.

Features:
- Connects to mock vendor API on port 9000
- Polls every 3 seconds
- Uses S3 for registry state tracking
- Simulates stateless container deployment
- Offset-based pagination with time filtering

### test-masking-config.yaml
Configuration for testing data masking functionality.

Features:
- Enables masking for multiple PII types
- Credit card, email, SSN, phone, IP, API key, and UUID masking
- JSON field-specific masking rules
- Custom regex-based masking patterns
- Ships masked logs to test receiver

## Manual Testing Workflows

### File Tailing (3 terminals)

**Terminal 1:**
```bash
./test-receiver.py
```

**Terminal 2:**
```bash
cd ..
cargo run -- -c test_tools/test-config.yaml
```

**Terminal 3:**
```bash
./test-log-generator.py --volume high --multi-file 5
```

### Docker Log Capture (3 terminals)

**Terminal 1:**
```bash
./test-receiver.py
```

**Terminal 2:**
```bash
cd ..
cargo run -- -c test_tools/docker-test-config.yaml
```

**Terminal 3:**
```bash
./docker-test-container.sh
```

### API Tailing (3 terminals)

**Terminal 1:**
```bash
./test-receiver.py
```

**Terminal 2:**
```bash
./test-api-server.py --port 9000
```

**Terminal 3:**
```bash
cd ..
cargo run -- -c test_tools/test-api-config.yaml
```

### API Tailing + S3 Registry (4 terminals)

**Prerequisites:**
```bash
# Install MinIO (macOS)
brew install minio/stable/minio

# Install AWS CLI
pip install awscli
```

**Terminal 1: Start MinIO**
```bash
mkdir -p /tmp/flicker-test-minio
MINIO_ROOT_USER=minioadmin MINIO_ROOT_PASSWORD=minioadmin \
    minio server /tmp/flicker-test-minio --address ":9001" --console-address ":9002"
```

**Terminal 2: Create bucket and start receiver**
```bash
export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
export AWS_ENDPOINT_URL=http://localhost:9001
export AWS_REGION=us-east-1
export AWS_S3_FORCE_PATH_STYLE=true

aws s3 mb s3://flicker-api-test --endpoint-url http://localhost:9001

./test-receiver.py
```

**Terminal 3: Start mock API**
```bash
./test-api-server.py --port 9000
```

**Terminal 4: Run Flicker with S3 registry**
```bash
cd ..
export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
export AWS_ENDPOINT_URL=http://localhost:9001
export AWS_REGION=us-east-1
export AWS_S3_FORCE_PATH_STYLE=true

cargo run -- -c test_tools/test-api-s3-config.yaml --track --registry-file s3://flicker-api-test/api-registry.json
```

**Verify S3 registry state:**
```bash
aws s3 cp s3://flicker-api-test/api-registry.json - --endpoint-url http://localhost:9001 | jq
```

## Testing Checklist

### File Tailing
- [ ] Verify buffering triggers on size (use `--volume high`)
- [ ] Verify buffering triggers on time (use `--volume low`)
- [ ] Test multiple concurrent files
- [ ] Test regex filtering (`match_on`)
- [ ] Test regex exclusion (`exclude_on`)
- [ ] Test file rotation handling

### Docker Container Tailing
- [ ] Test Docker container log capture
- [ ] Test Docker container restart handling
- [ ] Test filtering Docker logs with regex

### API Tailing
- [ ] Test API polling and pagination (offset-based)
- [ ] Test time-based filtering with `since` parameter
- [ ] Test JSON response parsing
- [ ] Test API authentication (Bearer token, Basic Auth)
- [ ] Test cursor-based pagination
- [ ] Test page-based pagination
- [ ] Test API error handling and retries

### S3 Registry
- [ ] Test S3 registry save/load functionality
- [ ] Test registry persistence across restarts
- [ ] Test with MinIO (local S3)
- [ ] Test with real AWS S3
- [ ] Test with other S3-compatible services (Wasabi, DigitalOcean Spaces)

### End-to-End Scenarios
- [ ] File tailing → HTTP receiver
- [ ] Docker logs → HTTP receiver
- [ ] API tailing → HTTP receiver
- [ ] API tailing + S3 registry → stateless deployment simulation
- [ ] Test different destinations (HTTP, syslog, elasticsearch, file)
- [ ] Test authentication end-to-end

### Data Masking
- [ ] Test credit card number masking
- [ ] Test email address masking
- [ ] Test SSN masking
- [ ] Test phone number masking
- [ ] Test IP address masking
- [ ] Test API key masking
- [ ] Test UUID masking
- [ ] Test JSON field-specific masking
- [ ] Test custom regex masking rules
- [ ] Verify masked data is properly buffered and shipped
