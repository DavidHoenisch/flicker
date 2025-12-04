#!/usr/bin/env python3
"""
Simple HTTP server to receive and display Flicker log batches.
Listens on port 8000 and prints received log entries to stdout.
"""

import json
from http.server import HTTPServer, BaseHTTPRequestHandler
from datetime import datetime
import sys


class FlickerReceiver(BaseHTTPRequestHandler):
    """HTTP request handler that receives and displays log batches from Flicker"""

    def log_message(self, format, *args):
        """Override to customize server logging"""
        # Only log errors, not every request
        if "error" in format.lower() or "exception" in format.lower():
            sys.stderr.write(f"[{datetime.now().strftime('%H:%M:%S')}] {format % args}\n")

    def do_POST(self):
        """Handle POST requests from Flicker"""
        try:
            # Read content length
            content_length = int(self.headers.get('Content-Length', 0))

            if content_length == 0:
                self.send_error(400, "Empty request body")
                return

            # Read and parse JSON body
            body = self.rfile.read(content_length)

            try:
                data = json.loads(body.decode('utf-8'))
            except json.JSONDecodeError as e:
                print(f"[ERROR] Invalid JSON: {e}")
                print(f"[ERROR] Raw body: {body[:200]}")  # Print first 200 bytes
                self.send_error(400, f"Invalid JSON: {e}")
                return

            # Print received batch
            timestamp = datetime.now().strftime('%H:%M:%S.%f')[:-3]

            # Handle both single entry and batch formats
            entries = data if isinstance(data, list) else [data]

            print(f"\n{'='*80}")
            print(f"[{timestamp}] Received batch of {len(entries)} log entries:")
            print(f"{'='*80}")

            for i, entry in enumerate(entries, 1):
                path = entry.get('path', 'unknown')
                line = entry.get('line', '')
                print(f"{i:3d}. [{path}] {line}")

            print(f"{'='*80}\n")
            sys.stdout.flush()

            # Send success response
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            response = json.dumps({"status": "ok", "received": len(entries)})
            self.wfile.write(response.encode('utf-8'))

        except Exception as e:
            print(f"[ERROR] Exception handling request: {e}")
            import traceback
            traceback.print_exc()
            self.send_error(500, str(e))

    def do_GET(self):
        """Handle GET requests - just return a status page"""
        self.send_response(200)
        self.send_header('Content-Type', 'text/html')
        self.end_headers()

        html = """
        <html>
        <head><title>Flicker Test Receiver</title></head>
        <body>
            <h1>Flicker Test Receiver</h1>
            <p>Status: <span style="color: green;">Running</span></p>
            <p>Listening for POST requests on /ingest</p>
            <p>Check the terminal for received log entries.</p>
        </body>
        </html>
        """
        self.wfile.write(html.encode('utf-8'))


def main():
    """Start the HTTP server"""
    host = '0.0.0.0'  # Listen on all interfaces
    port = 8000

    server = HTTPServer((host, port), FlickerReceiver)

    print("="*80)
    print("Flicker Test Receiver")
    print("="*80)
    print(f"Listening on http://{host}:{port}")
    print(f"Endpoint: http://localhost:{port}/ingest")
    print("Waiting for log batches from Flicker...")
    print("Press Ctrl+C to stop")
    print("="*80)
    print()

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\n\nShutting down...")
        server.shutdown()
        print("Server stopped.")


if __name__ == '__main__':
    main()
