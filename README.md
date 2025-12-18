<div align="center">
  <img style="height: 50px; width: 50px;" src="assets/flicker.png" alt="logo">
  <h2>Flicker<h2>
</div>

A lightweight, high-performance log shipping agent written in Rust. Flicker
efficiently tails multiple log files and Docker container logs, shipping them
to various destinations with intelligent buffering.

## Overview

Flicker is designed to be a simple yet powerful log shipper similar to Filebeat
or Fluentd, but with a focus on simplicity and performance. It reads log
files from disk and Docker container logs, buffers entries intelligently, and
ships them in batches to configured destinations.

## Key Features

### 🎯 Per-Source Configuration
Each log source (file or Docker container) is configured independently with its own:
- Polling frequency
- Buffer size
- Flush interval
- Destination endpoint
- Regex filters (match/exclude patterns)

### 🐋 Docker Container Support
Capture logs directly from Docker containers:
- Target containers by name or ID
- Captures both stdout and stderr
- Same buffering and filtering capabilities as file tailing
- Ships to any supported destination

### 🌐 API Tailing Support
Pull audit logs and events from vendor APIs:
- Perfect for onboarding SaaS vendors to your SIEM
- Supports bearer token, basic auth, and custom headers
- Automatic JSON parsing and log extraction
- Pagination support (offset, cursor, and page-based)
- Time-based filtering to fetch only new logs
- State tracking with S3 for stateless deployments

### 🔍 Regex-Based Filtering
Powerful filtering to ship only relevant logs:
- **match_on**: Whitelist - only ship lines matching at least one pattern
- **exclude_on**: Blacklist - skip lines matching any pattern
- Both can be used together for fine-grained control
- Regexes compiled once at startup for efficiency

### 📦 Intelligent Buffering
Dual-trigger buffering system that flushes when **either** condition is met:
- **Size trigger**: Buffer reaches configured line count (e.g., 100 lines)
- **Time trigger**: Configured interval elapsed (e.g., 30 seconds)

This ensures high-volume logs flush frequently for low latency, while low-volume
logs don't sit in the buffer indefinitely.

### 🚀 Concurrent Processing
- One independent async task per log source (file or container)
- No shared state between sources
- Each source can have different polling rates and destinations

### 🔄 File Rotation & Truncation Handling
- Detects file rotation via inode changes (Unix/Linux)
- Handles file truncation gracefully
- Automatically reopens rotated files

### 🔁 Retry Queue with Exponential Backoff
- Failed batches are queued for retry instead of being dropped
- Exponential backoff prevents overwhelming failed destinations
- Configurable retry limits and delays
- Per-source retry queue to isolate failures

### 📊 Efficient Tailing
- Seek-based reading (only reads new content)
- Line-buffered reading (never splits log lines)
- Starts at end-of-file (doesn't re-ship existing logs on startup)

## Architecture

```
┌─────────────┐
│ Log File 1  │───┐
└─────────────┘   │
                  │    ┌──────────────┐      ┌────────────┐      ┌──────────────┐
┌─────────────┐   ├───▶│ Flicker Task │─────▶│   Buffer   │─────▶│ HTTP Dest 1  │
│ Log File 2  │───┤    │   (Tailer)   │      │ (5 lines)  │      └──────────────┘
└─────────────┘   │    └──────────────┘      └────────────┘
                  │                               │
┌─────────────┐   │                               ├─ Size trigger: Buffer full
│ Log File 3  │───┤                               └─ Time trigger: 30s elapsed
└─────────────┘   │
                  │    ┌──────────────┐      ┌────────────┐      ┌──────────────┐
┌─────────────┐   ├───▶│ Flicker Task │─────▶│   Buffer   │─────▶│ Syslog Dest  │
│ Log File 4  │───┤    │   (Tailer)   │      │ (10 lines) │      └──────────────┘
└─────────────┘   │    └──────────────┘      └────────────┘
                  │
┌─────────────┐   │    ┌──────────────┐      ┌────────────┐      ┌──────────────┐
│  Container  │───┼───▶│ Docker Task  │─────▶│   Buffer   │─────▶│  ES Dest     │
│   (nginx)   │   │    │   (Tailer)   │      │ (20 lines) │      └──────────────┘
└─────────────┘   │    └──────────────┘      └────────────┘
                  │
┌─────────────┐   │
│  Container  │───┘
│ (postgres)  │
└─────────────┘
```

Each log source (file or container) runs in its own async task with independent
buffering and destination.

## Installation

### Prerequisites
- Rust 1.70+ (install from [rustup.rs](https://rustup.rs))
- Python 3.7+ (for testing tools)
- Docker (optional, only if using Docker container log capture)

### Build from Source
```bash
git clone <repository-url>
cd flicker
cargo build --release
```

The binary will be at `./target/release/flicker`.

## Configuration

Flicker uses YAML configuration. Create a `flicker.yaml` file:

```yaml
# Optional: Global retry configuration (these are the defaults)
retry:
  max_retries: 5           # Maximum retry attempts before dropping a batch
  initial_delay_ms: 1000   # Initial retry delay (1 second)
  max_delay_ms: 60000      # Maximum retry delay (60 seconds)
  max_queue_size: 100      # Maximum batches to keep in retry queue

log_files:
  # High-volume application logs
  - path: "/var/log/myapp/app.log"
    polling_frequency_ms: 250
    buffer_size: 100          # Flush every 100 lines
    flush_interval_ms: 30000  # OR flush after 30 seconds
    destination:
      type: "http"
      endpoint: "http://log-aggregator:8000/ingest"
      require_auth: true
      # Optional: API Key (Bearer token)
      # api_key: "your_secret_token"
      # Optional: Basic Auth
      basic:
        username: "flicker"
        password: "your_secret_password"
      # Optional: Enable gzip compression (default: false)
      compression: true
      # Optional: mTLS (mutual TLS) configuration
      # tls:
      #   cert_path: "/etc/flicker/certs/client.crt"
      #   key_path: "/etc/flicker/certs/client.key"
      #   ca_cert_path: "/etc/flicker/certs/ca.crt"  # Optional: custom CA certificate
      #   accept_invalid_certs: false  # Optional: accept self-signed certs (default: false)

  # Low-volume audit logs with filtering
  - path: "/var/log/myapp/audit.log"
    polling_frequency_ms: 1000
    buffer_size: 50
    flush_interval_ms: 60000  # Flush after 1 minute
    # Only ship ERROR and WARN level logs
    match_on:
      - "ERROR"
      - "WARN"
    destination:
      type: "http"
      endpoint: "http://security-system:9000/audit"
      require_auth: true
      api_key: "audit_key_456"

  # System logs with exclusion filter
  - path: "/var/log/syslog"
    polling_frequency_ms: 500
    buffer_size: 200
    flush_interval_ms: 45000
    # Ship everything except debug and trace
    exclude_on:
      - "DEBUG"
      - "TRACE"
    destination:
      type: "syslog"
      host: "syslog-server.local"
      protocol: "udp"

docker_containers:
  # Capture logs from Docker containers by name or ID
  - container: "my-app-container"
    polling_frequency_ms: 250
    buffer_size: 100
    flush_interval_ms: 30000
    # Optional: filter logs with regex patterns
    match_on:
      - "ERROR"
      - "WARN"
    destination:
      type: "http"
      endpoint: "http://log-aggregator:8000/ingest"
      require_auth: true
      api_key: "your_secret_token"

  # Ship container logs to Elasticsearch
  - container: "nginx"
    polling_frequency_ms: 500
    buffer_size: 50
    flush_interval_ms: 60000
    destination:
      type: "elasticsearch"
      url: "http://elasticsearch:9200"
      index: "nginx-logs"

  # Example: HTTP destination with mTLS (mutual TLS)
  - container: "secure-app"
    polling_frequency_ms: 500
    buffer_size: 100
    flush_interval_ms: 30000
    destination:
      type: "http"
      endpoint: "https://secure-log-server.example.com/ingest"
      compression: true
      tls:
        cert_path: "/etc/flicker/certs/client.crt"
        key_path: "/etc/flicker/certs/client.key"
        ca_cert_path: "/etc/flicker/certs/ca.crt"

api_sources:
  # Example: Vendor API with Bearer token auth and offset pagination
  - name: "vendor-audit-logs"
    endpoint: "https://api.vendor.com/v1/audit/events"
    polling_frequency_ms: 60000  # Poll every minute
    buffer_size: 100
    flush_interval_ms: 30000
    results_field: "data"  # JSON path to log array
    timestamp_field: "timestamp"  # Field containing event timestamp
    message_field: "message"  # Optional: extract just the message field
    api_key: "your_api_key_here"  # Bearer token authentication
    time_filter_param: "since"  # Query param for time filtering
    time_filter_format: "rfc3339"  # Timestamp format
    pagination:
      pagination_type: "offset"
      limit_param: "limit"
      offset_param: "offset"
      page_size: 100
    destination:
      type: "http"
      endpoint: "http://siem.company.com/ingest"
      require_auth: true
      api_key: "siem_api_key"

  # Example: API with cursor-based pagination
  - name: "saas-app-logs"
    endpoint: "https://api.saasapp.com/logs"
    polling_frequency_ms: 30000  # Poll every 30 seconds
    buffer_size: 50
    flush_interval_ms: 30000
    results_field: "events"
    timestamp_field: "created_at"
    api_key: "saas_bearer_token"
    time_filter_param: "start_time"
    time_filter_format: "unix"
    pagination:
      pagination_type: "cursor"
      cursor_param: "next_token"
      next_cursor_field: "pagination.next_cursor"
      page_size: 50
    destination:
      type: "elasticsearch"
      url: "http://elasticsearch:9200"
      index: "saas-logs"

  # Example: Basic auth with custom headers
  - name: "internal-api"
    endpoint: "https://internal.company.com/api/v2/events"
    polling_frequency_ms: 120000  # Poll every 2 minutes
    buffer_size: 200
    flush_interval_ms: 60000
    results_field: "logs"
    timestamp_field: "event_time"
    basic:
      username: "log_reader"
      password: "secure_password"
    headers:
      X-Custom-Header: "custom-value"
      X-Request-Source: "flicker"
    time_filter_param: "from_time"
    time_filter_format: "unix_ms"
    destination:
      type: "http"
      endpoint: "http://log-processor:8080/ingest"
```

### Configuration Parameters

#### `retry` (object, optional)
Global retry configuration for all log sources:

- **`max_retries`** (integer, default: 5): Maximum number of retry attempts before dropping a batch
- **`initial_delay_ms`** (integer, default: 1000): Initial delay before first retry in milliseconds
- **`max_delay_ms`** (integer, default: 60000): Maximum delay between retries in milliseconds (caps exponential growth)
- **`max_queue_size`** (integer, default: 100): Maximum number of batches to keep in retry queue per source

The retry logic uses exponential backoff: each retry doubles the delay time (1s, 2s, 4s, 8s, 16s...) until it reaches `max_delay_ms`. If a batch exceeds `max_retries`, it will be dropped and logged. If the retry queue reaches `max_queue_size`, the oldest batch will be dropped to make room.

#### `log_files` (array)
Array of log file configurations. Each entry supports:

- **`path`** (string, required): Absolute or relative path to log file
- **`polling_frequency_ms`** (integer, required): How often to check for new lines (milliseconds)
- **`buffer_size`** (integer, default: 100): Flush when buffer reaches this many lines
- **`flush_interval_ms`** (integer, default: 30000): Flush after this many milliseconds
- **`match_on`** (array of strings, optional): List of regex patterns - only ship lines matching at least one
- **`exclude_on`** (array of strings, optional): List of regex patterns - skip lines matching any
- **`destination`** (object, required): Destination configuration (see below)

#### `docker_containers` (array, optional)
Array of Docker container configurations. Each entry supports:

- **`container`** (string, required): Container name or ID
- **`polling_frequency_ms`** (integer, required): How often to check for new lines (milliseconds)
- **`buffer_size`** (integer, default: 100): Flush when buffer reaches this many lines
- **`flush_interval_ms`** (integer, default: 30000): Flush after this many milliseconds
- **`match_on`** (array of strings, optional): List of regex patterns - only ship lines matching at least one
- **`exclude_on`** (array of strings, optional): List of regex patterns - skip lines matching any
- **`destination`** (object, required): Destination configuration (see below)

#### `api_sources` (array, optional)
Array of API source configurations for pulling audit logs from REST APIs. Each entry supports:

- **`name`** (string, required): Unique identifier for this API source (used in registry for state tracking)
- **`endpoint`** (string, required): API endpoint URL
- **`polling_frequency_ms`** (integer, required): How often to poll the API (milliseconds)
- **`buffer_size`** (integer, default: 100): Flush when buffer reaches this many lines
- **`flush_interval_ms`** (integer, default: 30000): Flush after this many milliseconds
- **`match_on`** (array of strings, optional): List of regex patterns - only ship lines matching at least one
- **`exclude_on`** (array of strings, optional): List of regex patterns - skip lines matching any
- **`destination`** (object, required): Destination configuration (see below)

**Authentication:**
- **`api_key`** (string, optional): Bearer token or API key (sent as `Authorization: Bearer <token>`)
- **`basic`** (object, optional): Basic authentication with `username` and `password`
- **`headers`** (object, optional): Custom HTTP headers as key-value pairs

**Response Parsing:**
- **`results_field`** (string, required): JSON path to array of log entries (e.g., "data", "logs", "events")
- **`timestamp_field`** (string, required): Field in each log entry containing the timestamp
- **`message_field`** (string, optional): Field containing the log message (if not set, entire entry is serialized as JSON)

**Time Filtering:**
- **`time_filter_param`** (string, optional): Query parameter for filtering by time (e.g., "since", "start_time")
- **`time_filter_format`** (string, optional): Time format: "unix", "unix_ms", or "rfc3339" (default: "rfc3339")

**Pagination (optional):**
- **`pagination`** (object, optional): Pagination configuration
  - **`pagination_type`** (string, default: "offset"): Type of pagination: "offset", "cursor", or "page"
  - **`page_size`** (integer, default: 100): Number of items per page
  - **For offset-based:**
    - **`limit_param`** (string): Query param for page size (e.g., "limit")
    - **`offset_param`** (string): Query param for offset (e.g., "offset")
  - **For cursor-based:**
    - **`cursor_param`** (string): Query param for cursor (e.g., "cursor")
    - **`next_cursor_field`** (string): Response field containing next cursor
  - **For page-based:**
    - **`page_param`** (string): Query param for page number (e.g., "page")
    - **`next_page_field`** (string): Response field containing next page number
    - **`has_more_field`** (string): Response field indicating if more pages exist

#### `destination` (object)
Destination configuration for log files, Docker containers, and API sources:

- **`type`** (string, required): Destination type: "http", "syslog", "elasticsearch", or "file"
- **`endpoint`** (string, required for http): The HTTP endpoint to send logs to
- **`require_auth`** (boolean, optional for http): If true, requires either `api_key` or `basic` to be set
- **`api_key`** (string, optional for http): A bearer token to include in the `Authorization` header
- **`basic`** (object, optional for http): An object with `username` and `password` for basic authentication
- **`compression`** (boolean, optional for http, default: false): Enable gzip compression for HTTP payloads
- **`tls`** (object, optional for http): TLS/mTLS configuration for client certificate authentication
  - **`cert_path`** (string, required): Path to client certificate file in PEM format
  - **`key_path`** (string, required): Path to client private key file in PEM format
  - **`ca_cert_path`** (string, optional): Path to custom CA certificate for server verification
  - **`accept_invalid_certs`** (boolean, optional, default: false): Accept invalid/self-signed server certificates (not recommended for production)
- **Other fields** (various): Destination-specific fields (see examples/flicker-example.yaml)

## Usage

### Basic Usage
```bash
# Use default config file (flicker.yaml)
./flicker

# Specify custom config
./flicker --config /path/to/config.yaml
./flicker -c /path/to/config.yaml

# Show help
./flicker --help
```

### Registry Tracking (State Persistence)

Flicker can persist file positions and container timestamps across restarts using a registry file. This prevents re-processing logs after Flicker restarts.

#### Enable Registry Tracking
```bash
# Use local filesystem storage (default)
./flicker --track --registry-file .flicker-registry.json

# Use S3 storage
./flicker --track --registry-file s3://my-bucket/flicker-registry.json
```

#### Local Filesystem Storage

The simplest option - stores the registry as a JSON file on the local filesystem:

```bash
./flicker --track --registry-file /var/lib/flicker/registry.json
```

**Pros:**
- Fast and simple
- No external dependencies
- Works offline

**Cons:**
- Lost if container/instance is terminated
- Not suitable for stateless/ephemeral environments

#### S3 Storage (Recommended for Stateless Containers)

Store the registry in S3 or S3-compatible storage for stateless deployments:

```bash
./flicker --track --registry-file s3://my-bucket/path/to/registry.json
```

**Use cases:**
- Running Flicker in stateless containers (Kubernetes, ECS, etc.)
- Multi-instance deployments where state needs to be shared
- Disaster recovery scenarios
- Cloud-native deployments

**Supported S3-Compatible Services:**
- AWS S3
- MinIO
- Wasabi
- DigitalOcean Spaces
- Backblaze B2
- Any S3-compatible object storage

**Required Environment Variables:**

For AWS S3:
```bash
export AWS_ACCESS_KEY_ID="your-access-key"
export AWS_SECRET_ACCESS_KEY="your-secret-key"
export AWS_REGION="us-east-1"
```

For S3-compatible services (e.g., MinIO):
```bash
export AWS_ACCESS_KEY_ID="your-access-key"
export AWS_SECRET_ACCESS_KEY="your-secret-key"
export AWS_REGION="us-east-1"  # Can be any value for compatible services
export AWS_ENDPOINT_URL="https://minio.example.com"  # Custom endpoint
export AWS_S3_FORCE_PATH_STYLE="true"  # Required for MinIO and some services
```

For EC2/ECS instances with IAM roles:
```bash
# No credentials needed - will use instance metadata
export AWS_REGION="us-east-1"
```

**Docker Example with S3 Registry:**
```bash
docker run -e AWS_ACCESS_KEY_ID="..." \
           -e AWS_SECRET_ACCESS_KEY="..." \
           -e AWS_REGION="us-east-1" \
           flicker:latest \
           --track \
           --registry-file s3://my-bucket/flicker-registry.json
```

**Kubernetes Example:**
```yaml
apiVersion: v1
kind: Pod
metadata:
  name: flicker
spec:
  containers:
  - name: flicker
    image: flicker:latest
    args:
      - "--track"
      - "--registry-file"
      - "s3://my-bucket/flicker-registry.json"
    env:
    - name: AWS_ACCESS_KEY_ID
      valueFrom:
        secretKeyRef:
          name: aws-credentials
          key: access-key-id
    - name: AWS_SECRET_ACCESS_KEY
      valueFrom:
        secretKeyRef:
          name: aws-credentials
          key: secret-access-key
    - name: AWS_REGION
      value: "us-east-1"
```

**Registry Behavior:**
- Registry is loaded once at startup
- Updates are batched and written every 5 seconds (when dirty)
- On S3, writes are atomic (no partial updates)
- If the registry file doesn't exist, Flicker starts fresh
- If the registry file is corrupted, Flicker starts fresh with a warning

### Running as a Service

#### systemd (Linux)
Create `/etc/systemd/system/flicker.service`:

```ini
[Unit]
Description=Flicker Log Shipper
After=network.target

[Service]
Type=simple
User=flicker
Group=flicker
ExecStart=/usr/local/bin/flicker --config /etc/flicker/flicker.yaml
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Enable and start:
```bash
sudo systemctl enable flicker
sudo systemctl start flicker
sudo systemctl status flicker
```

## Testing

Flicker includes comprehensive testing tools in the `test_tools/` directory:

### 1. Test Receiver (HTTP endpoint simulator)
Receives and displays log batches:
```bash
cd test_tools
./test-receiver.py
```

### 2. Test Log Generator
Generates realistic log data at configurable rates:

```bash
cd test_tools
# High volume (stress test)
./test-log-generator.py --volume high --multi-file 5

# Low volume (test time-based flushing)
./test-log-generator.py --volume low --multi-file 5

# Custom delay
./test-log-generator.py --delay 250  # 250ms between entries

# Single file
./test-log-generator.py --path /tmp/myapp.log --volume medium
```

**Volume Modes:**
- `high`: 10-50ms delay (~20-100 entries/sec) - tests buffer size trigger
- `medium`: 100-500ms delay (~2-10 entries/sec) - balanced
- `low`: 1-3s delay (~0.3-1 entries/sec) - tests time-based flush trigger

### 3. Docker Test Container
Runs a container that generates logs for testing Docker log capture:

```bash
cd test_tools
# Start test container
./docker-test-container.sh

# In another terminal, run Flicker with Docker test config
cargo run -- -c test_tools/docker-test-config.yaml
```

The test container generates logs at a configurable rate for testing buffering
and filtering.

### 4. End-to-End Test
Automated test that starts receiver, Flicker, and generator:
```bash
cd test_tools
./test-e2e.sh
```

Press Ctrl+C to stop all processes.

### Manual Testing (3 terminals)

**Terminal 1 - Start receiver:**
```bash
./test-receiver.py
```

**Terminal 2 - Start Flicker:**
```bash
cargo run -- -c test_tools/test-config.yaml
```

**Terminal 3 - Generate logs:**
```bash
cd test_tools
./test-log-generator.py --volume high --multi-file 5
```

Watch Terminal 1 for batches arriving from all 5 log files!

### Docker Testing (3 terminals)

**Terminal 1 - Start receiver:**
```bash
cd test_tools
./test-receiver.py
```

**Terminal 2 - Start Flicker:**
```bash
cargo run -- -c test_tools/docker-test-config.yaml
```

**Terminal 3 - Start test container:**
```bash
cd test_tools
./docker-test-container.sh
```

Watch Terminal 1 for batches arriving from the Docker container!

### mTLS Testing (3 terminals)

**Terminal 1 - Start mTLS receiver:**
```bash
cd test_tools
./test-mtls-receiver.py
```

**Terminal 2 - Start Flicker with mTLS:**
```bash
# First generate test certificates if you haven't already
./test_tools/generate-test-certs.sh

# Then start Flicker
./target/release/flicker -c test_tools/test-mtls-config.yaml
```

**Terminal 3 - Generate logs:**
```bash
cd test_tools
./test-log-generator.py --path test-mtls.log --volume high
```

Watch Terminal 1 for batches arriving with mTLS client authentication!

For more details on mTLS testing, see `test_tools/README_MTLS.md`.

## Design Decisions

### Why Dual-Trigger Buffering?
The OR logic (size **OR** time) ensures:
- **High-volume logs**: Hit size trigger quickly → low latency, efficient batching
- **Low-volume logs**: Hit time trigger → data doesn't sit in buffer forever
- This is the industry standard (used by Filebeat, Fluentd, Vector)

### Why One Task Per File?
- **Isolation**: Files are completely independent
- **Different frequencies**: Each file can poll at its own rate
- **Different destinations**: Ship different logs to different systems
- **Simplicity**: No complex scheduling or resource sharing

Alternative considered: Group files by (frequency, destination). Rejected as premature optimization.

### Why Seek-Based Tailing?
- **Efficient**: Only reads new data, not entire file
- **Cross-platform**: Works on Unix and Windows
- **Simple**: No inotify/file watching complexity

Alternative considered: Event-based file watching (inotify). Rejected for added complexity and platform-specificity.

### Why Line-Based Reading?
- Always ship complete log lines, never partial
- Simple and predictable
- Works with any text-based log format

### Why Start at End of File?
- Don't re-ship existing logs on startup (like `tail -f`)
- Only ship new logs that arrive after Flicker starts
- Prevents duplicate data on restarts

Future enhancement: Persist file positions to disk for state recovery.

## Data Format

Flicker sends log batches as JSON arrays via HTTP POST:

```json
[
  {
    "path": "/var/log/app.log",
    "line": "[2025-12-03 14:23:45] INFO - User login successful"
  },
  {
    "path": "/var/log/app.log",
    "line": "[2025-12-03 14:23:46] WARN - High memory usage: 85%"
  },
  {
    "path": "docker://nginx",
    "line": "192.168.1.1 - - [03/Dec/2025:14:23:47 +0000] \"GET / HTTP/1.1\" 200"
  }
]
```

Docker container logs are prefixed with `docker://` followed by the container name.

### Destination Requirements
Your HTTP endpoint should:
- Accept POST requests
- Parse JSON body as array of log entries
- Return 2xx status code on success
- Handle batch sizes from 1 to `buffer_size` entries

## Performance Characteristics

### Resource Usage
- **CPU**: Minimal (mostly idle, wakes on poll intervals)
- **Memory**: ~1-2MB base + buffers (buffer_size × avg_line_size per file)
- **I/O**: Seek-based reads, line-buffered, no unnecessary file scans

### Scalability
- Tested with 5 files, should handle dozens efficiently
- Each file adds one lightweight async task
- Network batching reduces HTTP overhead significantly

### Latency
- Best case: `polling_frequency_ms` (if buffer fills immediately)
- Worst case: `flush_interval_ms` (for low-volume logs)
- Typical: Sub-second for active logs

## Limitations & Future Work

### Current Limitations
1. **Limited destinations**: HTTP, syslog, Elasticsearch, and file are the only supported destinations

### Planned Enhancements
- [X] Persistent state (registry file like Filebeat)
- [X] Retry queue with exponential backoff
- [X] gzip compression for HTTP payloads
- [ ] Filtering/parsing (JSON parsing, field extraction)
- [ ] Additional destinations (Kafka, S3)
- [X] TLS/mTLS support (client certificates for mutual TLS authentication)
- [X] Authentication schemes (Basic Auth, Bearer Token)

## Troubleshooting

### Logs not appearing in destination
1. Check Flicker is running: Look for startup messages
2. Check file paths: Ensure files exist and are readable
3. Check network: Can Flicker reach the destination endpoint?
4. Check destination logs: Is it receiving requests?
5. Check Flicker logs: Look for error messages
6. Check retry queue: Look for `[Retry]` messages indicating failed batches are being queued and retried

### High memory usage
- Reduce `buffer_size` in config
- Reduce number of files being tailed
- Check for very long log lines (buffers are line-based)
- Reduce `retry.max_queue_size` if retry queues are filling up

### Batches being dropped after retries
- Check destination availability and network connectivity
- Increase `retry.max_retries` if temporary outages are longer than expected
- Increase `retry.max_delay_ms` to allow more time between retries
- Check Flicker logs for specific error messages about why batches are failing

### Missed log entries after restart
- Expected behavior: Flicker starts at end-of-file
- Future enhancement: Persistent state will solve this

### File rotation not detected
- Ensure using Unix/Linux (inode tracking not available on Windows)
- Check file permissions (Flicker needs read access)

## Contributing

Contributions welcome! Areas of interest:
- Additional destination types
- Performance optimizations
- State persistence
- Better error handling
- Documentation improvements

## License

MIT

## Acknowledgments

Inspired by:
- [Filebeat](https://www.elastic.co/beats/filebeat)
- [Fluentd](https://www.fluentd.org/)
- [Vector](https://vector.dev/)

Built with:
- [Tokio](https://tokio.rs/) - Async runtime
- [Bollard](https://docs.rs/bollard/) - Docker API client
- [reqwest](https://docs.rs/reqwest/) - HTTP client
- [serde](https://serde.rs/) - Serialization
- [clap](https://docs.rs/clap/) - CLI parsing
