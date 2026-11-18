#!/usr/bin/env python3
"""
Helpers for gauntlet.sh (Flicker's integration test suite).

Subcommands:
  check-seq   Assert that numbered lines ("<prefix> seq=N") were received exactly once
  tcp-sink    Accept TCP connections and append everything received to a file
  es-mock     Minimal Elasticsearch _bulk endpoint that can report partial failures
"""

import argparse
import json
import re
import socket
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer


# --------------------------------------------------------------------------
# check-seq
# --------------------------------------------------------------------------

def read_seqs(path, prefix):
    """Sequence numbers of received lines with the given prefix, in arrival order."""
    pattern = re.compile(re.escape(prefix) + r" seq=(\d+)\b")
    seqs = []
    try:
        with open(path, errors="replace") as f:
            for raw in f:
                raw = raw.rstrip("\n")
                try:
                    entry = json.loads(raw)
                    text = entry.get("line", "") if isinstance(entry, dict) else raw
                except json.JSONDecodeError:
                    text = raw
                seqs.extend(int(m) for m in pattern.findall(text))
    except FileNotFoundError:
        pass
    return seqs


def check_seq(args):
    expected = set(range(args.start, args.start + args.count))
    deadline = time.time() + args.wait
    while True:
        seqs = read_seqs(args.file, args.prefix)
        if expected.issubset(seqs) or time.time() >= deadline:
            break
        time.sleep(0.2)

    got = set(seqs)
    missing = sorted(expected - got)
    dupes = sorted({s for s in seqs if seqs.count(s) > 1}) if not args.allow_dupes else []
    unexpected = sorted(got - expected)
    ordered = all(a < b for a, b in zip(seqs, seqs[1:])) if args.ordered else True

    problems = []
    if missing:
        problems.append(f"missing {len(missing)} (e.g. {missing[:10]})")
    if dupes:
        problems.append(f"duplicated {len(dupes)} (e.g. {dupes[:10]})")
    if unexpected and not args.allow_extra:
        problems.append(f"unexpected {len(unexpected)} (e.g. {unexpected[:10]})")
    if not ordered:
        problems.append("out of order")

    summary = f"{args.prefix}: {len(got & expected)}/{len(expected)} received, {len(seqs)} total"
    if problems:
        print(f"{summary}; " + "; ".join(problems))
        return 1
    print(summary)
    return 0


# --------------------------------------------------------------------------
# tcp-sink
# --------------------------------------------------------------------------

def tcp_sink(args):
    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("127.0.0.1", args.port))
    server.listen()
    lock = threading.Lock()

    def handle(conn):
        with conn:
            while True:
                data = conn.recv(65536)
                if not data:
                    return
                with lock, open(args.output, "ab") as out:
                    out.write(data)

    while True:
        conn, _ = server.accept()
        threading.Thread(target=handle, args=(conn,), daemon=True).start()


# --------------------------------------------------------------------------
# es-mock
# --------------------------------------------------------------------------

class EsState:
    lock = threading.Lock()
    docs = {}             # _id -> document
    index_ops = 0         # total index actions received (including retries)
    auth_headers = set()
    fail_remaining = 0    # bulk requests to answer with errors:true
    output = None

    @classmethod
    def save(cls):
        with open(cls.output, "w") as f:
            json.dump({
                "docs": cls.docs,
                "index_ops": cls.index_ops,
                "auth_headers": sorted(cls.auth_headers),
            }, f)


class EsHandler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        pass

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0))).decode()
        lines = [l for l in body.split("\n") if l]
        pairs = list(zip(lines[0::2], lines[1::2]))
        with EsState.lock:
            EsState.auth_headers.add(self.headers.get("Authorization", ""))
            failing = EsState.fail_remaining > 0
            if failing:
                EsState.fail_remaining -= 1
            items = []
            for i, (action, doc) in enumerate(pairs):
                EsState.index_ops += 1
                doc_id = json.loads(action)["index"].get("_id")
                # On a "partial failure" response, the first half of the batch succeeds
                if failing and i >= len(pairs) // 2:
                    items.append({"index": {"status": 429, "error": {"type": "es_rejected_execution_exception"}}})
                    continue
                EsState.docs[doc_id or f"auto-{EsState.index_ops}"] = json.loads(doc)
                items.append({"index": {"_id": doc_id, "status": 201}})
            EsState.save()
        response = json.dumps({"errors": failing, "items": items}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(response)))
        self.end_headers()
        self.wfile.write(response)


def es_mock(args):
    EsState.output = args.output
    EsState.fail_remaining = args.fail_first
    EsState.save()
    HTTPServer.allow_reuse_address = True
    HTTPServer(("127.0.0.1", args.port), EsHandler).serve_forever()


# --------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("check-seq", help="assert numbered lines arrived exactly once")
    p.add_argument("file", help="JSONL (entries with a 'line' field) or plain text")
    p.add_argument("--prefix", required=True)
    p.add_argument("--count", type=int, required=True)
    p.add_argument("--start", type=int, default=0)
    p.add_argument("--wait", type=float, default=0, help="seconds to wait for all lines")
    p.add_argument("--ordered", action="store_true", help="also require arrival order")
    p.add_argument("--allow-dupes", action="store_true")
    p.add_argument("--allow-extra", action="store_true")
    p.set_defaults(func=check_seq)

    p = sub.add_parser("tcp-sink", help="append all TCP input to a file")
    p.add_argument("--port", type=int, required=True)
    p.add_argument("--output", required=True)
    p.set_defaults(func=tcp_sink)

    p = sub.add_parser("es-mock", help="mock Elasticsearch _bulk endpoint")
    p.add_argument("--port", type=int, required=True)
    p.add_argument("--output", required=True)
    p.add_argument("--fail-first", type=int, default=0,
                   help="answer the first N bulk requests with a partial failure")
    p.set_defaults(func=es_mock)

    args = parser.parse_args()
    sys.exit(args.func(args) or 0)


if __name__ == "__main__":
    main()
