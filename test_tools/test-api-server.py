#!/usr/bin/env python3
"""
Mock vendor API server for testing Flicker API tailing functionality.
Simulates a REST API with audit logs, pagination, and time-based filtering.
"""

import json
import time
from http.server import HTTPServer, BaseHTTPRequestHandler
from datetime import datetime, timezone
from urllib.parse import urlparse, parse_qs
import sys
import threading


class MockAPIServer(BaseHTTPRequestHandler):
    """Mock API server that simulates a vendor audit log API"""

    # Shared state across requests
    events = []
    event_counter = 0
    lock = threading.Lock()

    def log_message(self, format, *args):
        """Override to customize server logging"""
        sys.stderr.write(f"[API {datetime.now().strftime('%H:%M:%S')}] {format % args}\n")

    def do_GET(self):
        """Handle GET requests to /api/events endpoint"""
        parsed_url = urlparse(self.path)
        path = parsed_url.path
        query_params = parse_qs(parsed_url.query)

        # Only handle /api/events endpoint
        if path != "/api/events":
            self.send_error(404, "Not Found")
            return

        try:
            # Parse query parameters
            limit = int(query_params.get('limit', ['10'])[0])
            offset = int(query_params.get('offset', ['0'])[0])
            since = query_params.get('since', [None])[0]

            # Filter events by timestamp if 'since' parameter provided
            with self.lock:
                filtered_events = self.events.copy()

            if since:
                # Parse RFC3339 timestamp
                try:
                    since_dt = datetime.fromisoformat(since.replace('Z', '+00:00'))
                    filtered_events = [
                        e for e in filtered_events
                        if datetime.fromisoformat(e['timestamp'].replace('Z', '+00:00')) > since_dt
                    ]
                except Exception as e:
                    print(f"[WARN] Failed to parse 'since' parameter: {e}")

            # Apply pagination
            total = len(filtered_events)
            paginated_events = filtered_events[offset:offset + limit]

            # Build response
            response = {
                "data": paginated_events,
                "pagination": {
                    "total": total,
                    "limit": limit,
                    "offset": offset,
                    "has_more": (offset + limit) < total
                }
            }

            # Send response
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps(response).encode('utf-8'))

            print(f"[API] Served {len(paginated_events)} events (offset={offset}, since={since})")

        except Exception as e:
            print(f"[ERROR] Request failed: {e}")
            self.send_error(500, f"Internal Server Error: {e}")

    def do_POST(self):
        """Handle POST to add events (for testing)"""
        if self.path != "/api/add_event":
            self.send_error(404, "Not Found")
            return

        try:
            content_length = int(self.headers.get('Content-Length', 0))
            body = self.rfile.read(content_length)
            data = json.loads(body.decode('utf-8'))

            with self.lock:
                event = {
                    "id": str(self.event_counter),
                    "timestamp": datetime.now(timezone.utc).isoformat(),
                    "event_type": data.get("event_type", "audit"),
                    "message": data.get("message", "Test event"),
                    "user": data.get("user", "test_user"),
                    "severity": data.get("severity", "info")
                }
                self.events.append(event)
                self.event_counter += 1

            self.send_response(201)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({"success": True, "event": event}).encode('utf-8'))

            print(f"[API] Added event: {event['message']}")

        except Exception as e:
            print(f"[ERROR] Failed to add event: {e}")
            self.send_error(500, f"Internal Server Error: {e}")


def generate_initial_events(num_events=50):
    """Generate initial audit log events"""
    events = []
    base_time = time.time() - (num_events * 10)  # Events spaced 10 seconds apart

    event_types = ["login", "logout", "file_access", "config_change", "api_call"]
    users = ["alice", "bob", "charlie", "admin", "service_account"]
    severities = ["info", "warning", "error"]

    for i in range(num_events):
        event = {
            "id": str(i),
            "timestamp": datetime.fromtimestamp(base_time + (i * 10), timezone.utc).isoformat(),
            "event_type": event_types[i % len(event_types)],
            "message": f"Event #{i}: {event_types[i % len(event_types)]} by {users[i % len(users)]}",
            "user": users[i % len(users)],
            "severity": severities[i % len(severities)]
        }
        events.append(event)

    return events


def event_generator(server_address):
    """Background thread that generates new events periodically"""
    print("[Generator] Starting event generator thread...")
    event_num = 1000  # Start from 1000 to avoid conflicts with initial events

    while True:
        time.sleep(5)  # Generate a new event every 5 seconds

        event = {
            "id": str(event_num),
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "event_type": "periodic_check",
            "message": f"Periodic event #{event_num} generated at {datetime.now().strftime('%H:%M:%S')}",
            "user": "system",
            "severity": "info"
        }

        with MockAPIServer.lock:
            MockAPIServer.events.append(event)
            MockAPIServer.event_counter = event_num + 1

        print(f"[Generator] Generated event #{event_num}")
        event_num += 1


def run_server(port=9000):
    """Run the mock API server"""
    server_address = ('', port)

    # Initialize with some events
    print(f"[Server] Generating initial events...")
    MockAPIServer.events = generate_initial_events(50)
    MockAPIServer.event_counter = 50

    print(f"[Server] Starting mock API server on port {port}...")
    print(f"[Server] Endpoints:")
    print(f"  GET  http://localhost:{port}/api/events?limit=10&offset=0&since=<timestamp>")
    print(f"  POST http://localhost:{port}/api/add_event")
    print(f"\n[Server] Press Ctrl+C to stop\n")

    # Start event generator thread
    generator_thread = threading.Thread(target=event_generator, args=(server_address,), daemon=True)
    generator_thread.start()

    # Start server
    httpd = HTTPServer(server_address, MockAPIServer)

    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("\n[Server] Shutting down...")
        httpd.shutdown()


if __name__ == '__main__':
    import argparse

    parser = argparse.ArgumentParser(description='Mock vendor API server for testing')
    parser.add_argument('--port', type=int, default=9000, help='Port to listen on (default: 9000)')
    args = parser.parse_args()

    run_server(args.port)
