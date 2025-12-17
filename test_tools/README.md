# Flicker Test Tools

This directory contains testing tools for Flicker log shipping.

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

### 4. End-to-End Tests

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

## Configuration Files

### test-config.yaml
Configuration for testing file tailing with 5 test log files.

### docker-test-config.yaml
Configuration for testing Docker container log capture.

Includes examples of:
- Basic Docker container tailing
- Filtering with `match_on` (commented out)
- Filtering with `exclude_on` (commented out)

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

## Testing Checklist

- [ ] Verify buffering triggers on size (use `--volume high`)
- [ ] Verify buffering triggers on time (use `--volume low`)
- [ ] Test multiple concurrent files
- [ ] Test Docker container log capture
- [ ] Test regex filtering (`match_on`)
- [ ] Test regex exclusion (`exclude_on`)
- [ ] Test different destinations (HTTP, syslog, elasticsearch, file)
- [ ] Test authentication (API key, Basic Auth)
- [ ] Test file rotation handling
- [ ] Test Docker container restart handling
