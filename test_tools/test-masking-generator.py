#!/usr/bin/env python3
"""
PII Log Generator for Masking Tests
Generates log entries containing PII data (emails, credit cards, SSNs, etc.)
for testing Flicker's data masking functionality.
"""

import signal
import sys
import time
from datetime import datetime
import random
import os

LOG_FILE = "./test-masking.log"
log_handle = None

# Sample PII data for testing
EMAILS = [
    "john.doe@example.com",
    "jane.smith@company.org",
    "user123@gmail.com",
    "contact@business.net",
    "support@helpdesk.io",
]

CREDIT_CARDS = [
    "4111111111111111",
    "5500000000000004",
    "340000000000009",
    "4111-1111-1111-1111",
    "5500 0000 0000 0004",
]

SSNS = [
    "123-45-6789",
    "987-65-4321",
    "555-12-3456",
    "001-23-4567",
    "999-88-7777",
]

PHONES = [
    "555-123-4567",
    "(555) 987-6543",
    "555.456.7890",
    "+1 555-789-0123",
    "5555555555",
]

IP_ADDRESSES = [
    "192.168.1.100",
    "10.0.0.50",
    "172.16.254.1",
    "8.8.8.8",
    "127.0.0.1",
]

SESSION_TOKENS = [
    "session_token=abc123def45678901234567890123456",
    "session_token=xyz987abc65432109876543210987654",
    "session_token=def456abc78901234567890123456789",
]


def cleanup_and_exit(signum=None, frame=None):
    """Clear log file and exit gracefully"""
    print("\n\n[SHUTDOWN] Received interrupt signal")
    if log_handle and not log_handle.closed:
        log_handle.close()
    try:
        with open(LOG_FILE, "w") as f:
            f.write("")
        print(f"[SHUTDOWN] {LOG_FILE} cleared")
    except Exception as e:
        print(f"[SHUTDOWN] Error clearing {LOG_FILE}: {e}", file=sys.stderr)
    print("[SHUTDOWN] Exiting...")
    sys.exit(0)


def generate_log_with_pii():
    """Generate a log entry containing various PII types"""
    log_types = [
        ("INFO", "User login attempt from {}"),
        ("INFO", "Payment processed for {}"),
        ("WARN", "Failed login attempt - username: {}"),
        ("INFO", "Customer profile updated - contact: {}"),
        ("DEBUG", "Session created: {}"),
        ("INFO", "Request from IP {} processed"),
        ("ERROR", "Authentication failed for user at {}"),
        ("INFO", "New registration: {}"),
    ]

    level, template = random.choice(log_types)

    # Determine which PII type to use based on template
    if "login" in template.lower():
        pii = random.choice(IP_ADDRESSES + EMAILS)
    elif "payment" in template.lower():
        pii = random.choice(CREDIT_CARDS + EMAILS)
    elif "profile" in template.lower() or "registration" in template.lower():
        pii = random.choice(EMAILS + PHONES)
    elif "session" in template.lower():
        pii = random.choice(SESSION_TOKENS)
    elif "ip" in template.lower():
        pii = random.choice(IP_ADDRESSES)
    elif "authentication" in template.lower():
        pii = random.choice(EMAILS + SSNS)
    else:
        # Mixed PII log with multiple types
        email = random.choice(EMAILS)
        ssn = random.choice(SSNS)
        cc = random.choice(CREDIT_CARDS)
        phone = random.choice(PHONES)
        ip = random.choice(IP_ADDRESSES)
        session = random.choice(SESSION_TOKENS)
        return f"[{datetime.now().strftime('%Y-%m-%d %H:%M:%S.%f')[:-3]}] {level:5s} - Full customer record: email={email}, ssn={ssn}, card={cc}, phone={phone}, ip={ip}, {session}\n"

    timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S.%f")[:-3]
    message = template.format(pii)
    return f"[{timestamp}] {level:5s} - {message}\n"


def main():
    global log_handle

    # Setup signal handlers
    signal.signal(signal.SIGINT, cleanup_and_exit)
    signal.signal(signal.SIGTERM, cleanup_and_exit)

    print("=" * 70)
    print("PII Log Generator for Masking Tests")
    print("=" * 70)
    print(f"Generating logs to: {LOG_FILE}")
    print("Contains: emails, credit cards, SSNs, phone numbers, IPs, session tokens")
    print("Press Ctrl+C to stop")
    print("=" * 70)
    print()

    # Clear existing file
    if os.path.exists(LOG_FILE):
        with open(LOG_FILE, "w") as f:
            f.write("")

    # Open file for writing
    log_handle = open(LOG_FILE, "a")

    entry_count = 0
    try:
        while True:
            entry = generate_log_with_pii()
            log_handle.write(entry)
            log_handle.flush()
            entry_count += 1

            if entry_count % 10 == 0:
                print(f"Generated {entry_count} log entries with PII...")

            # Random delay between 200-800ms
            time.sleep(random.uniform(0.2, 0.8))
    except KeyboardInterrupt:
        cleanup_and_exit()


if __name__ == "__main__":
    main()
