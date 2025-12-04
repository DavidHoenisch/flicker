#!/usr/bin/env python3
"""
Test log generator for Flicker.
Continuously writes log entries to ./test.log.
Clears the file on Ctrl+C (SIGINT) shutdown.

Supports different volume modes:
- high: Generate logs very fast (10-50ms delay) - ~20-100 entries/sec
- medium: Generate logs moderately (100-500ms delay) - ~2-10 entries/sec
- low: Generate logs slowly (1-3s delay) - ~0.3-1 entries/sec
- custom: Specify exact delay in milliseconds
"""

import argparse
import signal
import sys
import time
from datetime import datetime
import random
import os

# Global file handles - supports single or multiple files
log_files = []  # List of (path, file_handle) tuples
log_paths = []  # List of paths for cleanup

# Volume presets (min_delay_ms, max_delay_ms)
VOLUME_PRESETS = {
    'high': (10, 50),       # Very fast: 10-50ms between entries
    'medium': (100, 500),   # Moderate: 100-500ms between entries
    'low': (1000, 3000),    # Slow: 1-3 seconds between entries
}


def cleanup_and_exit(signum=None, frame=None):
    """Clear all log files and exit gracefully"""
    print("\n\n[SHUTDOWN] Received interrupt signal")
    print(f"[SHUTDOWN] Clearing {len(log_paths)} log file(s)...")

    # Close all open files
    for path, file_handle in log_files:
        if file_handle and not file_handle.closed:
            file_handle.close()

    # Clear all files
    for path in log_paths:
        try:
            with open(path, 'w') as f:
                f.write("")  # Empty the file
            print(f"[SHUTDOWN] {path} cleared")
        except Exception as e:
            print(f"[SHUTDOWN] Error clearing {path}: {e}", file=sys.stderr)

    print("[SHUTDOWN] Exiting...")
    sys.exit(0)


def generate_log_entry():
    """Generate a realistic-looking log entry"""
    log_types = [
        "INFO",
        "WARN",
        "ERROR",
        "DEBUG",
    ]

    messages = [
        "Application started successfully",
        "Processing user request",
        "Database query completed in {}ms",
        "Cache hit for key: user_{}",
        "HTTP request: GET /api/v1/users",
        "Authentication successful for user_{}",
        "Background job completed",
        "Memory usage: {}MB",
        "Connection established to database",
        "API response time: {}ms",
    ]

    level = random.choice(log_types)
    message = random.choice(messages)

    # Add random numbers to some messages
    if '{}' in message:
        if 'ms' in message:
            message = message.format(random.randint(10, 500))
        elif 'MB' in message:
            message = message.format(random.randint(100, 2000))
        else:
            message = message.format(random.randint(1000, 9999))

    timestamp = datetime.now().strftime('%Y-%m-%d %H:%M:%S.%f')[:-3]
    return f"[{timestamp}] {level:5s} - {message}\n"


def parse_args():
    """Parse command-line arguments"""
    parser = argparse.ArgumentParser(
        description='Generate test log data for Flicker',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Volume modes:
  high    - Very fast generation (~20-100 entries/sec) - tests buffer size trigger
  medium  - Moderate generation (~2-10 entries/sec) - balanced testing
  low     - Slow generation (~0.3-1 entries/sec) - tests time-based flush trigger

Examples:
  %(prog)s --volume high          # Stress test with lots of logs
  %(prog)s --volume low           # Test time-based buffer flushing
  %(prog)s --delay 250            # Custom 250ms delay between entries
  %(prog)s --path /tmp/app.log    # Write to custom path
  %(prog)s --multi-file 5         # Write to test1.log through test5.log
        """
    )

    parser.add_argument(
        '--volume', '-v',
        choices=['high', 'medium', 'low'],
        default='medium',
        help='Log generation volume preset (default: medium)'
    )

    parser.add_argument(
        '--delay', '-d',
        type=int,
        metavar='MS',
        help='Custom fixed delay in milliseconds between entries (overrides --volume)'
    )

    parser.add_argument(
        '--path', '-p',
        default='./test.log',
        help='Path to log file (default: ./test.log)'
    )

    parser.add_argument(
        '--multi-file', '-m',
        type=int,
        metavar='N',
        help='Write to N files (test1.log, test2.log, ..., testN.log) instead of single file'
    )

    return parser.parse_args()


def main():
    """Main log generation loop"""
    global log_files, log_paths

    args = parse_args()

    # Determine which files to write to
    if args.multi_file:
        # Multi-file mode: test1.log, test2.log, ..., testN.log
        log_paths = [f"./test{i}.log" for i in range(1, args.multi_file + 1)]
    else:
        # Single file mode
        log_paths = [args.path]

    # Determine delay range
    if args.delay is not None:
        # Fixed delay mode
        min_delay_ms = args.delay
        max_delay_ms = args.delay
        mode_desc = f"fixed {args.delay}ms delay"
    else:
        # Preset mode
        min_delay_ms, max_delay_ms = VOLUME_PRESETS[args.volume]
        mode_desc = f"{args.volume} volume ({min_delay_ms}-{max_delay_ms}ms delay)"

    # Register signal handlers for graceful shutdown
    signal.signal(signal.SIGINT, cleanup_and_exit)
    signal.signal(signal.SIGTERM, cleanup_and_exit)

    print("="*80)
    print("Flicker Test Log Generator")
    print("="*80)
    if len(log_paths) == 1:
        print(f"Writing logs to: {os.path.abspath(log_paths[0])}")
    else:
        print(f"Writing logs to {len(log_paths)} files:")
        for path in log_paths:
            print(f"  - {path}")
    print(f"Generation mode: {mode_desc}")
    print("Press Ctrl+C to stop (will clear log files)")
    print("="*80)
    print()

    # Open all log files in append mode
    for path in log_paths:
        try:
            file_handle = open(path, 'a', buffering=1)  # Line buffered
            log_files.append((path, file_handle))
        except Exception as e:
            print(f"[ERROR] Failed to open {path}: {e}", file=sys.stderr)
            sys.exit(1)

    entry_count = 0
    start_time = time.time()

    try:
        while True:
            # Generate and write log entry to ALL files
            entry = generate_log_entry()
            for path, file_handle in log_files:
                file_handle.write(entry)

            entry_count += 1

            # Print progress every 10 entries (or every 50 in high volume mode)
            report_interval = 10 if args.volume != 'high' else 50
            if entry_count % report_interval == 0:
                elapsed = time.time() - start_time
                rate = entry_count / elapsed if elapsed > 0 else 0
                total_lines = entry_count * len(log_files)
                print(f"[{datetime.now().strftime('%H:%M:%S')}] "
                      f"Written {entry_count} entries ({total_lines} total lines across {len(log_files)} files, {rate:.1f} entries/sec)")
                sys.stdout.flush()

            # Sleep with configured delay
            delay_sec = random.uniform(min_delay_ms, max_delay_ms) / 1000.0
            time.sleep(delay_sec)

    except KeyboardInterrupt:
        # This should be caught by signal handler, but just in case
        cleanup_and_exit()
    except Exception as e:
        print(f"\n[ERROR] Unexpected error: {e}", file=sys.stderr)
        import traceback
        traceback.print_exc()
        cleanup_and_exit()


if __name__ == '__main__':
    main()
