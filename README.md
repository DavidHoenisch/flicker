# Flicker

A lightweight, high-performance log shipping agent written in Rust. Flicker
efficiently tails multiple log files and ships them to HTTP endpoints with
intelligent buffering.

## Overview

Flicker is designed to be a simple yet powerful log shipper similar to Filebeat
or Fluentd, but with a focus on simplicity and performance. It reads log
files from disk, buffers entries intelligently, and ships them in batches to
configured HTTP destinations.

## Key Features

### 🎯 Per-File Configuration
Each log file is configured independently with its own:
- Polling frequency
- Buffer size
- Flush interval
- Destination endpoint
- Regex filters (match/exclude patterns)

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
- One independent async task per log file
- No shared state between files
- Each file can have different polling rates and destinations

### 🔄 File Rotation & Truncation Handling
- Detects file rotation via inode changes (Unix/Linux)
- Handles file truncation gracefully
- Automatically reopens rotated files

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
┌─────────────┐   ├───▶│ Flicker Task │─────▶│   Buffer   │─────▶│ HTTP Dest 2  │
│ Log File 4  │───┤    │   (Tailer)   │      │ (10 lines) │      └──────────────┘
└─────────────┘   │    └──────────────┘      └────────────┘
                  │
┌─────────────┐   │
│ Log File 5  │───┘
└─────────────┘
```

Each log file runs in its own async task with independent buffering and destination.

## Installation

### Prerequisites
- Rust 1.70+ (install from [rustup.rs](https://rustup.rs))
- Python 3.7+ (for testing tools)

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
```

### Configuration Parameters

#### `log_files` (array)
Array of log file configurations. Each entry supports:

- **`path`** (string, required): Absolute or relative path to log file
- **`polling_frequency_ms`** (integer, required): How often to check for new lines (milliseconds)
- **`buffer_size`** (integer, default: 100): Flush when buffer reaches this many lines
- **`flush_interval_ms`** (integer, default: 30000): Flush after this many milliseconds
- **`match_on`** (array of strings, optional): List of regex patterns - only ship lines matching at least one
- **`exclude_on`** (array of strings, optional): List of regex patterns - skip lines matching any
- **`destination.type`** (string, required): Destination type: "http", "syslog", "elasticsearch", or "file"
- **`destination.endpoint`** (string, required for http): The HTTP endpoint to send logs to
- **`destination.require_auth`** (boolean, optional for http): If true, requires either `api_key` or `basic` to be set
- **`destination.api_key`** (string, optional for http): A bearer token to include in the `Authorization` header
- **`destination.basic`** (object, optional for http): An object with `username` and `password` for basic authentication
- **`destination.*`** (various, required): Destination-specific fields (see config-examples.yaml)

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

Flicker includes comprehensive testing tools:

### 1. Test Receiver (HTTP endpoint simulator)
Receives and displays log batches:
```bash
./test-receiver.py
```

### 2. Test Log Generator
Generates realistic log data at configurable rates:

```bash
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

### 3. End-to-End Test
Automated test that starts receiver, Flicker, and generator:
```bash
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
cargo run -- -c test-config.yaml
```

**Terminal 3 - Generate logs:**
```bash
./test-log-generator.py --volume high --multi-file 5
```

Watch Terminal 1 for batches arriving from all 5 log files!

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
  }
]
```

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
1. **No state persistence**: File positions not saved to disk (will re-read from end on restart)
2. **No retry logic**: Failed batches are dropped (logged to stderr)
3. **No compression**: HTTP payloads sent uncompressed
4. **Limited destinations**: HTTP, syslog, Elasticsearch, and file are the only supported destinations

### Planned Enhancements
- [ ] Persistent state (registry file like Filebeat)
- [ ] Retry queue with exponential backoff
- [ ] gzip compression for HTTP payloads
- [ ] Filtering/parsing (JSON parsing, field extraction)
- [ ] Metrics/monitoring (Prometheus endpoint)
- [ ] Additional destinations (Kafka, S3)
- [ ] TLS/mTLS support
- [X] Authentication schemes (Basic Auth, Bearer Token)

## Troubleshooting

### Logs not appearing in destination
1. Check Flicker is running: Look for startup messages
2. Check file paths: Ensure files exist and are readable
3. Check network: Can Flicker reach the destination endpoint?
4. Check destination logs: Is it receiving requests?
5. Check Flicker logs: Look for error messages

### High memory usage
- Reduce `buffer_size` in config
- Reduce number of files being tailed
- Check for very long log lines (buffers are line-based)

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
- [reqwest](https://docs.rs/reqwest/) - HTTP client
- [serde](https://serde.rs/) - Serialization
- [clap](https://docs.rs/clap/) - CLI parsing
