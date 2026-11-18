#!/bin/bash
# Flicker integration gauntlet
#
# Runs the release binary against real receivers (HTTP, mTLS, syslog TCP, mock
# Elasticsearch, mock vendor API, Docker, S3 via adobe/s3mock) and asserts on what arrives:
# every numbered line delivered exactly once, PII masked, no loss across
# crashes/restarts/rotation, etc. Exits non-zero if any scenario fails.
#
# Usage:
#   test_tools/gauntlet.sh                 # run everything (Docker scenarios skipped if Docker is down)
#   test_tools/gauntlet.sh file_multi api  # run selected scenarios
#   test_tools/gauntlet.sh --list
#   FLICKER_BIN=target/debug/flicker test_tools/gauntlet.sh
#   REQUIRE_DOCKER=1 test_tools/gauntlet.sh   # fail (not skip) Docker scenarios when Docker is down
#   KEEP_WORK=1 test_tools/gauntlet.sh        # keep scenario logs even when everything passes
#
# Requires: bash, python3, openssl (mTLS). Docker for the docker/s3 scenarios.

set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TT="$ROOT/test_tools"
HELPERS="$TT/gauntlet_helpers.py"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/flicker-gauntlet.XXXXXX")"
BIN="${FLICKER_BIN:-$ROOT/target/release/flicker}"
case "$BIN" in /*) ;; *) BIN="$ROOT/$BIN" ;; esac

ALL_SCENARIOS="file_multi buffering masking shutdown_flush crash_restart outage_retry rotation truncate partial_line api config_validation mtls syslog_tcp elasticsearch docker s3"
DOCKER_SCENARIOS="docker s3"

if [ -t 1 ]; then
    GREEN=$'\033[32m'; RED=$'\033[31m'; YELLOW=$'\033[33m'; BOLD=$'\033[1m'; NC=$'\033[0m'
else
    GREEN=""; RED=""; YELLOW=""; BOLD=""; NC=""
fi

# ---------------------------------------------------------------------------
# Helpers (each scenario runs in a subshell; PIDS/DOCKER_CLEANUP are per scenario)
# ---------------------------------------------------------------------------

PIDS=""
DOCKER_CLEANUP=""
FAILED=0

track() { PIDS="$PIDS $1"; }

cleanup_scenario() {
    for pid in $PIDS; do kill "$pid" 2>/dev/null; done
    sleep 0.2
    for pid in $PIDS; do kill -9 "$pid" 2>/dev/null; done
    if [ -n "$DOCKER_CLEANUP" ]; then eval "$DOCKER_CLEANUP" >/dev/null 2>&1; fi
}

fail() { echo "    ${RED}✗${NC} $*"; FAILED=1; }
ok() { echo "    ${GREEN}✓${NC} $*"; }

# wait_for <seconds> <command...>: poll until the command succeeds
wait_for() {
    local deadline=$(( $(date +%s) + $1 )); shift
    until "$@" >/dev/null 2>&1; do
        [ "$(date +%s)" -ge "$deadline" ] && return 1
        sleep 0.2
    done
}

port_open() { (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; }
log_has() { grep -qE "$2" "$1" 2>/dev/null; }
alive() { kill -0 "$1" 2>/dev/null; }

# write_lines <file> <prefix> <start> <count>
write_lines() {
    local i=$3 end=$(( $3 + $4 ))
    while [ "$i" -lt "$end" ]; do
        echo "$2 seq=$i payload $(printf '%04d' "$i")"
        i=$((i + 1))
    done >> "$1"
}

# check <jsonl-or-text> <prefix> <count> [start] [extra check-seq args...]
check() {
    local file=$1 prefix=$2 count=$3 start=${4:-0}; shift 4 2>/dev/null || shift $#
    local out
    if out=$(python3 "$HELPERS" check-seq "$file" --prefix "$prefix" --count "$count" --start "$start" --wait 20 "$@"); then
        ok "$out"
    else
        fail "$out"
    fi
}

start_receiver() { # <port> <output> [extra args]
    local port=$1 out=$2; shift 2
    python3 "$TT/test-receiver.py" --port "$port" --output "$out" --quiet "$@" > "$out.log" 2>&1 &
    track $!
    wait_for 10 port_open "$port" || fail "receiver on port $port did not start"
}

# start_flicker <dir> <args...>; sets FLICKER_PID. Runs in <dir> (registry files land there).
start_flicker() {
    local dir=$1; shift
    (cd "$dir" && exec "$BIN" "$@") >> "$dir/flicker.log" 2>&1 &
    FLICKER_PID=$!
    track "$FLICKER_PID"
}

# Wait until flicker has opened its sources (files are tailed from the end on first sight)
wait_tailing() {
    if ! wait_for 15 log_has "$1/flicker.log" "Now tailing|Resuming|starting at beginning"; then
        fail "flicker did not start tailing (see $1/flicker.log)"
        return 1
    fi
    sleep 0.5
}

# stop_flicker <signal>; waits up to 20s and sets FLICKER_EXIT
stop_flicker() {
    kill "-$1" "$FLICKER_PID" 2>/dev/null
    local i=0
    while alive "$FLICKER_PID" && [ $i -lt 100 ]; do sleep 0.2; i=$((i + 1)); done
    wait "$FLICKER_PID" 2>/dev/null
    FLICKER_EXIT=$?
}

# run_bounded <seconds> <dir> <logfile> <args...>: run flicker expecting it to exit by
# itself; kills it after <seconds>. Sets RUN_EXIT (124 if it had to be killed).
run_bounded() {
    local secs=$1 dir=$2 log=$3; shift 3
    (cd "$dir" && exec "$BIN" "$@") > "$log" 2>&1 &
    local pid=$! i=0
    while alive "$pid" && [ $i -lt $((secs * 5)) ]; do sleep 0.2; i=$((i + 1)); done
    if alive "$pid"; then
        kill -9 "$pid" 2>/dev/null; wait "$pid" 2>/dev/null; RUN_EXIT=124
    else
        wait "$pid"; RUN_EXIT=$?
    fi
}

http_dest() { # <port>
    cat <<EOF
    destination:
      type: http
      endpoint: "http://127.0.0.1:$1/ingest"
EOF
}

docker_ok() { docker info >/dev/null 2>&1; }

# ---------------------------------------------------------------------------
# Scenarios
# ---------------------------------------------------------------------------

desc_file_multi="three files -> HTTP, every line exactly once"
s_file_multi() {
    local d=$1
    start_receiver 18101 "$d/recv.jsonl"
    : > "$d/a.log"; : > "$d/b.log"; : > "$d/c.log"
    {
        echo "log_files:"
        for f in a b c; do
            echo "  - path: \"$d/$f.log\""
            echo "    polling_frequency_ms: 100"
            echo "    buffer_size: 25"
            echo "    flush_interval_ms: 300"
            http_dest 18101
        done
    } > "$d/flicker.yaml"
    start_flicker "$d" -c flicker.yaml
    wait_tailing "$d" || return
    for chunk in 0 1 2 3; do
        for f in a b c; do write_lines "$d/$f.log" "file-$f" $((chunk * 50)) 50; done
        sleep 0.2
    done
    for f in a b c; do check "$d/recv.jsonl" "file-$f" 200 0 --ordered; done
}

desc_buffering="batches capped at buffer_size; remainder flushed on interval"
s_buffering() {
    local d=$1
    start_receiver 18102 "$d/recv.jsonl"
    : > "$d/in.log"
    cat > "$d/flicker.yaml" <<EOF
log_files:
  - path: "$d/in.log"
    polling_frequency_ms: 100
    buffer_size: 50
    flush_interval_ms: 1500
$(http_dest 18102)
EOF
    start_flicker "$d" -c flicker.yaml
    wait_tailing "$d" || return
    write_lines "$d/in.log" buf 0 120
    check "$d/recv.jsonl" buf 120
    local sizes
    sizes=$(grep -oE "batch of [0-9]+" "$d/recv.jsonl.log" | grep -oE "[0-9]+" | tr '\n' ' ')
    local max=0 has50=0
    for s in $sizes; do
        [ "$s" -gt "$max" ] && max=$s
        [ "$s" -eq 50 ] && has50=1
    done
    if [ "$max" -le 50 ] && [ "$has50" -eq 1 ]; then
        ok "batch sizes: $sizes"
    else
        fail "unexpected batch sizes: $sizes (want max 50, at least one full batch)"
    fi
}

desc_masking="built-in masking rules redact PII, leave timestamps alone"
s_masking() {
    local d=$1
    start_receiver 18103 "$d/recv.jsonl"
    : > "$d/in.log"
    cat > "$d/flicker.yaml" <<EOF
log_files:
  - path: "$d/in.log"
    polling_frequency_ms: 100
    buffer_size: 10
    flush_interval_ms: 300
    masking:
      enabled: true
      rules:
        email: { enabled: true }
        credit_card: { enabled: true }
        ssn: { enabled: true }
        phone: { enabled: true }
        ip_address: { enabled: true }
        api_key: { enabled: true }
$(http_dest 18103)
EOF
    start_flicker "$d" -c flicker.yaml
    wait_tailing "$d" || return
    cat >> "$d/in.log" <<'EOF'
mask seq=0 user alice@example.com logged in
mask seq=1 paid with 4111 1111 1111 1111
mask seq=2 ssn 123-45-6789
mask seq=3 call (555) 123-4567
mask seq=4 from 192.168.1.100 and 2001:db8::1
mask seq=5 password=hunter2 Authorization: Bearer abcdef0123456789
mask seq=6 ts=1731801600123 epoch=1731801600 at 12:30:45
mask seq=7 José says hi
EOF
    check "$d/recv.jsonl" mask 8
    local out
    if out=$(python3 - "$d/recv.jsonl" <<'PY'
import json, sys
lines = {}
for raw in open(sys.argv[1]):
    line = json.loads(raw)["line"]
    lines[int(line.split("seq=")[1].split()[0])] = line
leaks = ["alice@example.com", "4111 1111 1111 1111", "123-45-6789", "555) 123-4567",
         "192.168.1.100", "2001:db8::1", "hunter2", "abcdef0123456789"]
kept = {6: "ts=1731801600123 epoch=1731801600 at 12:30:45", 7: "José says hi"}
masks = {0: "[EMAIL]", 1: "[CREDIT_CARD]", 2: "[SSN]", 3: "[PHONE]", 4: "[IP]", 5: "password=[API_KEY]"}
problems = [f"leaked {l!r}" for l in leaks if any(l in v for v in lines.values())]
problems += [f"seq {k} missing {m!r}: {lines.get(k)!r}" for k, m in masks.items() if m not in lines.get(k, "")]
problems += [f"seq {k} altered: {lines.get(k)!r}" for k, t in kept.items() if t not in lines.get(k, "")]
print("; ".join(problems) or "no PII leaked; timestamps and unicode intact")
sys.exit(1 if problems else 0)
PY
    ); then ok "$out"; else fail "$out"; fi
}

desc_shutdown_flush="SIGTERM flushes the buffer, exits 0, saves the registry"
s_shutdown_flush() {
    local d=$1
    start_receiver 18104 "$d/recv.jsonl"
    : > "$d/in.log"
    cat > "$d/flicker.yaml" <<EOF
log_files:
  - path: "$d/in.log"
    polling_frequency_ms: 100
    buffer_size: 1000
    flush_interval_ms: 60000
$(http_dest 18104)
EOF
    start_flicker "$d" -c flicker.yaml --track
    wait_tailing "$d" || return
    write_lines "$d/in.log" stop 0 10
    sleep 1
    if [ -s "$d/recv.jsonl" ]; then fail "lines shipped before the flush interval"; fi
    stop_flicker TERM
    [ "$FLICKER_EXIT" -eq 0 ] && ok "exit code 0" || fail "exit code $FLICKER_EXIT"
    check "$d/recv.jsonl" stop 10
    local size pos
    size=$(wc -c < "$d/in.log" | tr -d ' ')
    pos=$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(list(r["files"].values())[0]["position"])' "$d/.flicker-registry.json" 2>/dev/null)
    [ "$pos" = "$size" ] && ok "registry position $pos == file size" || fail "registry position '$pos' != file size $size"
}

desc_crash_restart="kill -9 with undelivered lines; restart re-reads them (at-least-once), clean restart has no duplicates"
s_crash_restart() {
    local d=$1
    : > "$d/in.log"
    cat > "$d/flicker.yaml" <<EOF
log_files:
  - path: "$d/in.log"
    polling_frequency_ms: 100
    buffer_size: 10
    flush_interval_ms: 300
$(http_dest 18105)
retry:
  initial_delay_ms: 200
  max_delay_ms: 500
EOF
    # Receiver is down: nothing can be delivered
    start_flicker "$d" -c flicker.yaml --track
    wait_tailing "$d" || return
    wait_for 10 log_has "$d/.flicker-registry.json" position || fail "initial registry position not saved"
    write_lines "$d/in.log" crash 0 40
    sleep 2
    stop_flicker KILL
    ok "killed flicker with 40 undelivered lines"

    start_receiver 18105 "$d/recv.jsonl"
    start_flicker "$d" -c flicker.yaml --track
    check "$d/recv.jsonl" crash 40

    write_lines "$d/in.log" crash 40 40
    check "$d/recv.jsonl" crash 80
    stop_flicker TERM
    start_flicker "$d" -c flicker.yaml --track
    wait_tailing "$d" || return
    write_lines "$d/in.log" crash 80 20
    check "$d/recv.jsonl" crash 100
}

desc_outage_retry="destination fails 3 times; retries deliver everything once, in order"
s_outage_retry() {
    local d=$1
    start_receiver 18106 "$d/recv.jsonl" --fail-first 3
    : > "$d/in.log"
    cat > "$d/flicker.yaml" <<EOF
log_files:
  - path: "$d/in.log"
    polling_frequency_ms: 100
    buffer_size: 10
    flush_interval_ms: 300
$(http_dest 18106)
retry:
  initial_delay_ms: 200
  max_delay_ms: 500
EOF
    start_flicker "$d" -c flicker.yaml
    wait_tailing "$d" || return
    write_lines "$d/in.log" retry 0 50
    check "$d/recv.jsonl" retry 50 0 --ordered
}

desc_rotation="rename + recreate between polls loses nothing"
s_rotation() {
    local d=$1
    start_receiver 18107 "$d/recv.jsonl"
    : > "$d/app.log"
    cat > "$d/flicker.yaml" <<EOF
log_files:
  - path: "$d/app.log"
    polling_frequency_ms: 1500
    buffer_size: 100
    flush_interval_ms: 300
$(http_dest 18107)
EOF
    start_flicker "$d" -c flicker.yaml
    wait_tailing "$d" || return
    write_lines "$d/app.log" rot 0 30
    check "$d/recv.jsonl" rot 30
    # Within one poll interval: late writes to the old file, rotate, writes to the new file
    write_lines "$d/app.log" rot 30 10
    mv "$d/app.log" "$d/app.log.1"
    write_lines "$d/app.log" rot 40 20
    check "$d/recv.jsonl" rot 60
}

desc_truncate="truncate-and-refill past the old offset is re-read from the start"
s_truncate() {
    local d=$1
    start_receiver 18108 "$d/recv.jsonl"
    : > "$d/app.log"
    cat > "$d/flicker.yaml" <<EOF
log_files:
  - path: "$d/app.log"
    polling_frequency_ms: 1500
    buffer_size: 100
    flush_interval_ms: 300
$(http_dest 18108)
EOF
    start_flicker "$d" -c flicker.yaml
    wait_tailing "$d" || return
    write_lines "$d/app.log" trunc 0 5
    check "$d/recv.jsonl" trunc 5
    : > "$d/app.log"
    write_lines "$d/app.log" trunc 5 20
    check "$d/recv.jsonl" trunc 25
}

desc_partial_line="an unterminated line waits for its newline"
s_partial_line() {
    local d=$1
    start_receiver 18109 "$d/recv.jsonl"
    : > "$d/in.log"
    cat > "$d/flicker.yaml" <<EOF
log_files:
  - path: "$d/in.log"
    polling_frequency_ms: 100
    buffer_size: 1
    flush_interval_ms: 100
$(http_dest 18109)
EOF
    start_flicker "$d" -c flicker.yaml
    wait_tailing "$d" || return
    printf 'partial seq=0 hel' >> "$d/in.log"
    sleep 1.5
    if grep -q "partial" "$d/recv.jsonl" 2>/dev/null; then fail "partial line shipped early"; else ok "half-written line held back"; fi
    printf 'lo world\n' >> "$d/in.log"
    check "$d/recv.jsonl" partial 1
    grep -q '"partial seq=0 hello world"' "$d/recv.jsonl" && ok "line shipped whole" || fail "line was split: $(cat "$d/recv.jsonl")"
}

desc_api="API source (offset pagination) -> HTTP, every event exactly once"
s_api() {
    local d=$1
    start_receiver 18110 "$d/recv.jsonl"
    python3 "$TT/test-api-server.py" --port 18910 --no-generator --no-initial > "$d/api.log" 2>&1 &
    track $!
    wait_for 10 port_open 18910 || { fail "API server did not start"; return; }
    cat > "$d/flicker.yaml" <<EOF
api_sources:
  - name: gauntlet-api
    endpoint: "http://127.0.0.1:18910/api/events"
    polling_frequency_ms: 300
    buffer_size: 10
    flush_interval_ms: 300
    results_field: data
    timestamp_field: timestamp
    message_field: message
    id_field: id
    time_filter_param: since
    pagination:
      pagination_type: offset
      limit_param: limit
      offset_param: offset
      page_size: 5
$(http_dest 18110)
EOF
    start_flicker "$d" -c flicker.yaml
    wait_for 15 log_has "$d/flicker.log" "Now tailing API source" || { fail "API source did not start"; return; }
    sleep 0.5
    local i=0
    while [ $i -lt 23 ]; do
        curl -s -X POST -H 'Content-Type: application/json' \
            -d "{\"message\": \"api seq=$i event\"}" http://127.0.0.1:18910/api/add_event > /dev/null
        i=$((i + 1))
    done
    check "$d/recv.jsonl" api 23
}

desc_config_validation="bad configs fail fast; \${VAR} substitution; --continue-on-error"
s_config_validation() {
    local d=$1
    : > "$d/in.log"
    cat > "$d/zero.yaml" <<EOF
log_files:
  - path: "$d/in.log"
    polling_frequency_ms: 0
$(http_dest 18111)
EOF
    run_bounded 10 "$d" "$d/zero.log" -c zero.yaml
    if [ "$RUN_EXIT" -ne 0 ] && [ "$RUN_EXIT" -ne 124 ] && log_has "$d/zero.log" "polling_frequency_ms must be > 0"; then
        ok "zero polling interval rejected"
    else
        fail "zero polling interval not rejected (exit $RUN_EXIT)"
    fi

    cat > "$d/env.yaml" <<'EOF'
log_files:
  - path: "IN_LOG"
    polling_frequency_ms: 100
    destination:
      type: http
      endpoint: "http://127.0.0.1:${GAUNTLET_PORT}/ingest"
      api_key: "${GAUNTLET_UNSET_VARIABLE}"
EOF
    sed -i.bak "s|IN_LOG|$d/in.log|" "$d/env.yaml"
    GAUNTLET_PORT=18111 run_bounded 10 "$d" "$d/env.log" -c env.yaml
    if [ "$RUN_EXIT" -ne 0 ] && [ "$RUN_EXIT" -ne 124 ] && grep -q "GAUNTLET_UNSET_VARIABLE is not set" "$d/env.log"; then
        ok "unset \${VAR} rejected"
    else
        fail "unset \${VAR} not rejected: $(tail -1 "$d/env.log")"
    fi

    cat > "$d/mixed.yaml" <<EOF
log_files:
  - path: "$d/bad.log"
    polling_frequency_ms: 100
    destination: { type: nonsense }
  - path: "$d/in.log"
    polling_frequency_ms: 100
    buffer_size: 5
    flush_interval_ms: 300
    destination:
      type: http
      endpoint: "http://127.0.0.1:\${GAUNTLET_PORT}/ingest"
EOF
    GAUNTLET_PORT=18111 run_bounded 10 "$d" "$d/mixed.log" -c mixed.yaml
    if [ "$RUN_EXIT" -ne 0 ] && [ "$RUN_EXIT" -ne 124 ]; then
        ok "bad destination aborts startup"
    else
        fail "bad destination did not abort startup (exit $RUN_EXIT)"
    fi

    start_receiver 18111 "$d/recv.jsonl"
    export GAUNTLET_PORT=18111
    start_flicker "$d" -c mixed.yaml --continue-on-error
    unset GAUNTLET_PORT
    wait_tailing "$d" || return
    write_lines "$d/in.log" cfg 0 5
    check "$d/recv.jsonl" cfg 5
}

desc_mtls="HTTPS with client certificates"
s_mtls() {
    local d=$1
    if [ ! -f "$TT/certs/client.crt" ]; then
        (cd "$ROOT" && ./test_tools/generate-test-certs.sh) > "$d/certs.log" 2>&1 || { fail "cert generation failed"; return; }
    fi
    (cd "$TT" && exec python3 test-mtls-receiver.py --port 18443 --output "$d/recv.jsonl") > "$d/recv.log" 2>&1 &
    track $!
    wait_for 10 port_open 18443 || { fail "mTLS receiver did not start"; return; }
    : > "$d/in.log"
    cat > "$d/flicker.yaml" <<EOF
log_files:
  - path: "$d/in.log"
    polling_frequency_ms: 100
    buffer_size: 10
    flush_interval_ms: 300
    destination:
      type: http
      endpoint: "https://localhost:18443/ingest"
      compression: true
      tls:
        cert_path: "$TT/certs/client.crt"
        key_path: "$TT/certs/client.key"
        ca_cert_path: "$TT/certs/ca.crt"
EOF
    start_flicker "$d" -c flicker.yaml
    wait_tailing "$d" || return
    write_lines "$d/in.log" mtls 0 30
    check "$d/recv.jsonl" mtls 30
}

desc_syslog_tcp="syslog over TCP, newline and octet-counting framing"
s_syslog_tcp() {
    local d=$1
    python3 "$HELPERS" tcp-sink --port 18514 --output "$d/newline.txt" &
    track $!
    python3 "$HELPERS" tcp-sink --port 18515 --output "$d/octet.txt" &
    track $!
    wait_for 10 port_open 18514 && wait_for 10 port_open 18515 || { fail "TCP sinks did not start"; return; }
    : > "$d/a.log"; : > "$d/b.log"
    cat > "$d/flicker.yaml" <<EOF
log_files:
  - path: "$d/a.log"
    polling_frequency_ms: 100
    buffer_size: 10
    flush_interval_ms: 300
    destination: { type: syslog, host: 127.0.0.1, port: 18514, protocol: tcp }
  - path: "$d/b.log"
    polling_frequency_ms: 100
    buffer_size: 10
    flush_interval_ms: 300
    destination: { type: syslog, host: 127.0.0.1, port: 18515, protocol: tcp, framing: octet-counting }
EOF
    start_flicker "$d" -c flicker.yaml
    wait_tailing "$d" || return
    write_lines "$d/a.log" sysnl 0 25
    write_lines "$d/b.log" sysoc 0 25
    check "$d/newline.txt" sysnl 25
    check "$d/octet.txt" sysoc 25
    if python3 - "$d/octet.txt" <<'PY'
import sys
data = open(sys.argv[1], "rb").read()
count = 0
while data:
    length, _, rest = data.partition(b" ")
    n = int(length)
    assert rest[:n].startswith(b"<134>"), rest[:40]
    data = rest[n:]
    count += 1
assert count == 25, count
PY
    then ok "octet-counted frames parse cleanly"; else fail "octet-counted stream is malformed"; fi
    local conns
    conns=$(grep -c "Sending batch" "$d/flicker.log")
    ok "$conns syslog batches sent"
}

desc_elasticsearch="ES bulk with auth; partial failure retried without duplicates"
s_elasticsearch() {
    local d=$1
    python3 "$HELPERS" es-mock --port 18920 --output "$d/es.json" --fail-first 1 &
    track $!
    wait_for 10 port_open 18920 || { fail "ES mock did not start"; return; }
    : > "$d/in.log"
    cat > "$d/flicker.yaml" <<EOF
log_files:
  - path: "$d/in.log"
    polling_frequency_ms: 100
    buffer_size: 10
    flush_interval_ms: 300
    destination:
      type: elasticsearch
      url: "http://127.0.0.1:18920"
      index: gauntlet
      api_key: gauntlet-key
retry:
  initial_delay_ms: 200
  max_delay_ms: 500
EOF
    start_flicker "$d" -c flicker.yaml
    wait_tailing "$d" || return
    write_lines "$d/in.log" es 0 30
    es_done() { python3 -c 'import json,sys; sys.exit(0 if len(json.load(open(sys.argv[1]))["docs"]) >= 30 else 1)' "$d/es.json"; }
    wait_for 20 es_done
    local out
    if out=$(python3 - "$d/es.json" <<'PY'
import json, re, sys
s = json.load(open(sys.argv[1]))
docs = s["docs"]
seqs = sorted(int(re.search(r"seq=(\d+)", d["message"]).group(1)) for d in docs.values())
problems = []
if seqs != list(range(30)):
    problems.append(f"documents {len(seqs)} (want 30 unique): {seqs}")
if s["index_ops"] <= 30:
    problems.append(f"no retry happened ({s['index_ops']} index ops)")
if s["auth_headers"] != ["Bearer gauntlet-key"]:
    problems.append(f"auth headers {s['auth_headers']}")
print("; ".join(problems) or f"30 unique docs from {s['index_ops']} index ops (retry overwrote by _id); auth sent")
sys.exit(1 if problems else 0)
PY
    ); then ok "$out"; else fail "$out"; fi
}

desc_docker="Docker container logs exactly once (no re-shipping of the last second)"
s_docker() {
    local d=$1
    local name="flicker-gauntlet-$$"
    DOCKER_CLEANUP="docker rm -f $name"
    docker run -d --name "$name" alpine sh -c \
        'sleep 4; i=0; while [ $i -lt 30 ]; do echo "dock seq=$i  indented"; i=$((i+1)); sleep 0.05; done; sleep 60' \
        > "$d/docker.log" 2>&1 || { fail "could not start container: $(tail -1 "$d/docker.log")"; return; }
    cat > "$d/flicker.yaml" <<EOF
docker_containers:
  - container: "$name"
    polling_frequency_ms: 300
    buffer_size: 10
    flush_interval_ms: 300
$(http_dest 18112)
EOF
    start_receiver 18112 "$d/recv.jsonl"
    start_flicker "$d" -c flicker.yaml
    check "$d/recv.jsonl" dock 30
    # Keep polling a while: an idle container must not re-ship its last lines
    sleep 3
    check "$d/recv.jsonl" dock 30
    grep -q 'seq=0  indented' "$d/recv.jsonl" && ok "inner whitespace preserved" || fail "line content altered"
}

desc_s3="S3 destination + S3 registry (adobe/s3mock)"
s_s3() {
    local d=$1
    # DESIGN CHOICE: adobe/s3mock rather than MinIO: MinIO's images are no longer
    # publicly pullable, and s3mock needs no client tooling (plain HTTP works).
    local name="flicker-gauntlet-s3-$$" url="http://127.0.0.1:18900"
    DOCKER_CLEANUP="docker rm -f $name"
    docker run -d --name "$name" -p 18900:9090 adobe/s3mock > "$d/s3mock.log" 2>&1 \
        || { fail "could not start s3mock: $(tail -1 "$d/s3mock.log")"; return; }
    s3_ready() { curl -sf "$url/" -o /dev/null; }
    wait_for 60 s3_ready || { fail "s3mock not ready"; return; }
    curl -sf -X PUT "$url/gauntlet" -o /dev/null || { fail "bucket creation failed"; return; }

    : > "$d/in.log"
    cat > "$d/flicker.yaml" <<EOF
log_files:
  - path: "$d/in.log"
    polling_frequency_ms: 100
    buffer_size: 10
    flush_interval_ms: 300
    destination:
      type: s3
      bucket: gauntlet
      prefix: shipped
      region: us-east-1
EOF
    export AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test AWS_REGION=us-east-1 \
        AWS_ENDPOINT_URL="$url" AWS_S3_FORCE_PATH_STYLE=true
    start_flicker "$d" -c flicker.yaml --track --registry-file s3://gauntlet/state/registry.json
    wait_tailing "$d" || return
    write_lines "$d/in.log" s3 0 25
    sleep 3
    stop_flicker TERM
    [ "$FLICKER_EXIT" -eq 0 ] && ok "clean shutdown" || fail "exit code $FLICKER_EXIT"

    local key
    for key in $(curl -sf "$url/gauntlet?list-type=2&prefix=shipped/" | grep -oE "<Key>[^<]+</Key>" | sed -E 's/<\/?Key>//g'); do
        curl -sf "$url/gauntlet/$key"
    done > "$d/objects.jsonl"
    check "$d/objects.jsonl" s3 25
    if curl -sf "$url/gauntlet/state/registry.json" | grep -q '"position"'; then
        ok "registry saved to S3"
    else
        fail "registry not found in S3"
    fi
}

# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

if [ "${1:-}" = "--list" ]; then
    for s in $ALL_SCENARIOS; do eval "printf '  %-20s %s\n' $s \"\$desc_$s\""; done
    exit 0
fi

SELECTED="${*:-$ALL_SCENARIOS}"
for s in $SELECTED; do
    case " $ALL_SCENARIOS " in *" $s "*) ;; *) echo "Unknown scenario: $s (try --list)"; exit 2 ;; esac
done

if [ ! -x "$BIN" ]; then
    echo "Building release binary..."
    # The repo pins its toolchain in rust-toolchain.toml; don't let a shell-wide
    # RUSTUP_TOOLCHAIN override it.
    (cd "$ROOT" && env -u RUSTUP_TOOLCHAIN cargo build --release) || exit 1
fi

DOCKER_UP=0
docker_ok && DOCKER_UP=1

echo "${BOLD}Flicker integration gauntlet${NC}"
echo "binary: $BIN"
echo "work:   $WORK"
echo

PASSED=""; FAILURES=""; SKIPPED=""
for s in $SELECTED; do
    eval "desc=\$desc_$s"
    echo "${BOLD}▶ $s${NC} — $desc"
    case " $DOCKER_SCENARIOS " in
        *" $s "*)
            if [ "$DOCKER_UP" -eq 0 ]; then
                if [ -n "${REQUIRE_DOCKER:-}" ]; then
                    echo "    ${RED}✗${NC} Docker is not available"
                    FAILURES="$FAILURES $s"
                else
                    echo "    ${YELLOW}-${NC} skipped (Docker is not available)"
                    SKIPPED="$SKIPPED $s"
                fi
                continue
            fi ;;
    esac
    dir="$WORK/$s"
    mkdir -p "$dir"
    start=$(date +%s)
    # Scenario stderr (incl. bash job-kill notices) goes to a log, not the report
    (
        trap cleanup_scenario EXIT
        "s_$s" "$dir"
        exit $FAILED
    ) 2> "$dir/scenario.err"
    status=$?
    took=$(( $(date +%s) - start ))
    if [ $status -eq 0 ]; then
        echo "    ${GREEN}PASS${NC} (${took}s)"
        PASSED="$PASSED $s"
    else
        echo "    ${RED}FAIL${NC} (${took}s) — logs in $dir"
        FAILURES="$FAILURES $s"
    fi
done

echo
count() { echo $#; }
echo "${BOLD}Summary:${NC} ${GREEN}$(count $PASSED) passed${NC}, ${RED}$(count $FAILURES) failed${NC}, ${YELLOW}$(count $SKIPPED) skipped${NC}"
[ -n "$FAILURES" ] && echo "Failed:$FAILURES"
[ -n "$SKIPPED" ] && echo "Skipped:$SKIPPED"

if [ -z "$FAILURES" ] && [ -z "${KEEP_WORK:-}" ]; then
    rm -rf "$WORK"
fi
[ -z "$FAILURES" ]
