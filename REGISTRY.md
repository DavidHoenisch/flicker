# Registry Tracking Feature

The registry tracking feature allows Flicker to persist file and container positions across restarts, similar to how Logstash and Filebeat work. This ensures you don't lose your place in log files when the application restarts.

## Usage

### Enable Registry Tracking

To enable registry tracking, use the `--track` flag:

```bash
# Use default registry file (.flicker-registry.json)
./flicker --track

# Use custom registry file location
./flicker --track --registry-file /var/lib/flicker/registry.json
```

### Disable Registry Tracking (Default)

By default, registry tracking is **disabled** to keep things lightweight and simple:

```bash
# Without --track, positions are NOT persisted
./flicker

# This is equivalent to:
./flicker --config flicker.yaml
```

## How It Works

### Architecture

The registry system uses a **channel-based architecture** for thread safety and performance:

1. **Registry Writer Task**: A dedicated async task that owns the registry state
2. **Channel Communication**: Tailer tasks send position updates via unbounded channels
3. **Batched Writes**: Registry is flushed to disk every 5 seconds (or on shutdown)
4. **Atomic Persistence**: Uses atomic file writes (temp file + rename) to prevent corruption

```
┌─────────────┐      Updates      ┌──────────────────┐
│ File Tailer ├──────────────────►│                  │
└─────────────┘                   │                  │
                                  │  Registry Writer │
┌─────────────┐      Updates      │      Task        │
│Docker Tailer├──────────────────►│                  │
└─────────────┘                   │  (Every 5s)      │
                                  │                  │
                                  └────────┬─────────┘
                                           │
                                           ▼
                                  registry.json (disk)
```

### Registry File Format

The registry is stored as JSON with the following structure:

```json
{
  "version": 1,
  "files": {
    "/var/log/app.log": {
      "position": 1024,
      "inode": 98765432,
      "last_updated": "2025-12-16T10:30:45Z"
    }
  },
  "containers": {
    "nginx": {
      "last_timestamp": "2025-12-16T10:30:45.123Z"
    }
  }
}
```

### For Files

- **Position**: Byte offset in the file (where reading stopped)
- **Inode**: Unix inode number to detect file rotation
- **Last Updated**: Timestamp of last registry update

**On Startup:**
- If inode matches: Resume from saved position
- If inode changed: File was rotated, start at end
- If no registry entry: Start at end (default behavior)

### For Docker Containers

- **Last Timestamp**: UTC timestamp of the last log entry processed

**On Startup:**
- Resume from last timestamp using Docker's `since` parameter

## Examples

### Example 1: File Tailer with Registry

```bash
# Terminal 1: Start Flicker with registry tracking
./flicker --track --config myconfig.yaml

# Terminal 2: Add some log lines
echo "Line 1" >> /var/log/app.log
echo "Line 2" >> /var/log/app.log
echo "Line 3" >> /var/log/app.log

# Wait for Flicker to process logs (check output)

# Terminal 1: Stop Flicker (Ctrl+C)
# The registry file now contains: position=21, inode=12345

# Terminal 2: Add more lines while Flicker is stopped
echo "Line 4" >> /var/log/app.log
echo "Line 5" >> /var/log/app.log

# Terminal 1: Restart Flicker with registry
./flicker --track --config myconfig.yaml

# Flicker resumes from position 21 and ships "Line 4" and "Line 5"
# Without --track, it would start at END and miss these lines
```

### Example 2: Docker Container with Registry

```bash
# Start Flicker with registry tracking
./flicker --track --config docker-config.yaml

# The registry tracks the last timestamp:
# {"containers": {"nginx": {"last_timestamp": "2025-12-16T14:30:00Z"}}}

# Stop and restart Flicker
# It resumes from 2025-12-16T14:30:00Z, not "now"
```

### Example 3: Switching Between Tracked and Untracked

```bash
# Run WITHOUT tracking (default behavior)
./flicker
# Positions are NOT saved, starts at end on restart

# Run WITH tracking
./flicker --track
# Positions ARE saved, resumes from last position on restart

# Run without tracking again
./flicker
# Previous registry file is ignored, starts at end again
```

## Performance Characteristics

### Overhead

- **Memory**: Negligible (~1KB for registry state)
- **CPU**: Minimal (channel send is non-blocking)
- **Disk I/O**: Atomic write every 5 seconds (only if dirty)

### Thread Safety

- **No Mutexes**: Uses channel-based communication
- **No Blocking**: Tailer tasks never wait for registry writes
- **Lock-Free**: Each tailer task operates independently

### Scalability

- **Channels**: Unbounded channels prevent backpressure
- **Batching**: Updates are batched automatically (5s interval)
- **Atomic Writes**: Prevents registry corruption on crashes

## Edge Cases Handled

### File Rotation

When a log file is rotated (renamed):

1. **Inode changes** → Flicker detects this
2. **Reopens the new file** → Starts at end of new file
3. **Updates registry** → New inode is recorded

Example:
```bash
# Before rotation
/var/log/app.log (inode: 12345, position: 1000)

# Rotation happens
mv /var/log/app.log /var/log/app.log.1
touch /var/log/app.log  # New file, new inode

# Flicker detects inode mismatch
# Starts at end of NEW file, records new inode
```

### File Truncation

When a file is truncated (cleared):

1. **Size < position** → Flicker detects this
2. **Resets to position 0**
3. **Ships entire file contents**

### Corrupt Registry

If the registry file is corrupted or invalid JSON:

1. **Gracefully falls back** to empty registry
2. **Logs a warning** to stderr
3. **Continues normally** (starts at end of files)

### Missing Registry

If the registry file doesn't exist on startup:

1. **Creates a new empty registry**
2. **Starts at end of all files** (normal behavior)
3. **Saves positions on first flush**

## Best Practices

### When to Use `--track`

✅ **Use registry tracking when:**
- You need guaranteed delivery (no missed logs on restart)
- Running in production with critical logs
- Shipping to a centralized logging system
- Log files have low throughput (important not to miss lines)

❌ **Don't use registry tracking when:**
- Testing or development environments
- High-throughput logs where missing a few seconds is acceptable
- Running as a sidecar that restarts with the application
- You want minimal disk I/O

### Registry File Location

```bash
# Default: Current directory (might be lost on container restart)
./flicker --track

# Better: Persistent volume in containers
./flicker --track --registry-file /data/registry.json

# Production: System directory
./flicker --track --registry-file /var/lib/flicker/registry.json
```

### Monitoring

Check the registry file to see tracked positions:

```bash
# Pretty-print registry
cat .flicker-registry.json | jq .

# Check last update time
jq '.files | to_entries[] | {path: .key, last_updated: .value.last_updated}' .flicker-registry.json

# Check container timestamps
jq '.containers' .flicker-registry.json
```

## Testing the Feature

Quick test to verify registry tracking works:

```bash
# 1. Create a test log file
echo "Initial line" > /tmp/test.log

# 2. Create a minimal config
cat > test-config.yaml << 'EOF'
log_files:
  - path: /tmp/test.log
    polling_frequency_ms: 1000
    destination:
      type: file
      path: /tmp/output.jsonl
docker_containers: []
EOF

# 3. Start Flicker WITH tracking
./flicker --track --config test-config.yaml &
FLICKER_PID=$!

# 4. Add lines while running
sleep 2
echo "Line while running" >> /tmp/test.log
sleep 2

# 5. Stop Flicker
kill $FLICKER_PID
wait $FLICKER_PID 2>/dev/null

# 6. Check registry was created
cat .flicker-registry.json | jq .

# 7. Add lines while stopped
echo "Line while stopped" >> /tmp/test.log

# 8. Restart Flicker
./flicker --track --config test-config.yaml &
FLICKER_PID=$!

# 9. Verify it shipped "Line while stopped"
sleep 2
cat /tmp/output.jsonl | jq .

# 10. Clean up
kill $FLICKER_PID 2>/dev/null
rm /tmp/test.log /tmp/output.jsonl .flicker-registry.json test-config.yaml
```

## Troubleshooting

### Registry not persisting

**Symptom**: Flicker starts at end of file even with `--track`

**Causes:**
1. Not using `--track` flag
2. Registry file path is not writable
3. Flicker was killed before 5s flush interval

**Solutions:**
```bash
# Verify tracking is enabled
./flicker --track --config myconfig.yaml

# Check registry file permissions
ls -l .flicker-registry.json

# Use custom location with write permissions
./flicker --track --registry-file /tmp/registry.json
```

### Registry file corruption

**Symptom**: Error messages about parsing JSON

**Solution:**
```bash
# Delete corrupted registry (Flicker will create a new one)
rm .flicker-registry.json

# Or restore from backup
cp .flicker-registry.json.backup .flicker-registry.json
```

### Old positions causing issues

**Symptom**: Flicker re-ships old logs or misses new ones

**Solution:**
```bash
# Delete registry to start fresh
rm .flicker-registry.json

# Or edit registry manually
nano .flicker-registry.json
# Update position/timestamp to desired value
```

## Comparison to Other Tools

### vs Filebeat

**Similar:**
- JSON registry file
- Tracks byte position + inode
- Atomic writes

**Different:**
- Flicker: Optional via `--track` flag (default: disabled)
- Filebeat: Always tracks positions (no way to disable)

### vs Logstash File Input

**Similar:**
- Sincedb (Logstash's registry equivalent)
- Position tracking per file

**Different:**
- Flicker: Simple JSON file, human-readable
- Logstash: Binary sincedb file

### vs Fluentd

**Similar:**
- Position tracking for tail input

**Different:**
- Flicker: Channel-based (no locks)
- Fluentd: File-based with locks

## Future Enhancements

Potential improvements (not yet implemented):

- [ ] Configurable flush interval (currently hardcoded to 5s)
- [ ] Graceful shutdown signal handling (SIGTERM → flush registry)
- [ ] Registry compression for large deployments
- [ ] Registry garbage collection (remove stale entries)
- [ ] Metrics (registry size, flush latency, update rate)
