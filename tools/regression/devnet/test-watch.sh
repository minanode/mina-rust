#!/usr/bin/env bash
# Test watch.sh with stand-ins: no mina binary, no real node, no static server
# and no network beyond 127.0.0.1. It takes a few seconds under a minute.
#
#   tools/regression/devnet/test-watch.sh
#
# The stand-in node is a Python process with `--libp2p-port P2P` on its
# command line that answers /status on HTTP and listens on P2P; the stand-in
# `npx` (first on PATH) records its arguments and fails at once or runs without
# answering. Each case runs a watchdog with INTERVAL=1 and MAX_RESTARTS=3 in its
# own directory with free local ports, then reads its events and the stand-ins'
# records:
#
# 1. A node started by hand, without a PID file (the recipe's step 2): the
#    watchdog adopts it and starts no second node; when it exits, the
#    watchdog starts one.
# 2. Another process answers on HTTP, or listens on P2P: no node is started,
#    and the event is written once.
# 3. A second watchdog for the same PIDFILE exits at once; once the first is
#    stopped, a new one starts at once (the node and the sleep it leaves
#    behind do not hold the lock) and adopts the node from PIDFILE.
# 4. The static server: started with --no-port-switching; one that runs
#    without answering is not doubled; one that keeps failing is started
#    MAX_RESTARTS times, then not again; a port held by another process is
#    left alone.
#
# It stops every process it started, by PID, and leaves its directory in
# TMPDIR when a check fails (KEEP=1 keeps it anyway).
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
WATCH=${WATCH:-$here/watch.sh}
T=$(mktemp -d "${TMPDIR:-/tmp}/test-watch.XXXXXX")
failures=0
pids=()

cleanup() {
	local pid
	for pid in "${pids[@]}" $(cat "$T"/*/started 2>/dev/null); do
		# Stand-ins run in their own sessions: stop the whole group.
		kill -- "-$pid" 2>/dev/null || kill "$pid" 2>/dev/null || true
	done
	if ((failures)) || [[ -n ${KEEP:-} ]]; then
		echo "test-watch.sh: files kept in $T"
	else
		rm -rf "$T"
	fi
}
trap cleanup EXIT

check() {
	local what=$1
	shift
	if "$@"; then
		echo "ok   $what"
	else
		echo "FAIL $what"
		failures=$((failures + 1))
	fi
}
lines() { if [[ -f $1 ]]; then grep -c -- "${2:-}" "$1" || true; else echo 0; fi; }
equals() { [[ $1 == "$2" ]] || {
	echo "     expected '$2', got '$1'"
	return 1
}; }
free_port() { python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
wait_for() { # wait_for SECONDS COMMAND...: until COMMAND succeeds
	local until=$((SECONDS + $1))
	shift
	until "$@"; do
		((SECONDS < until)) || return 1
		sleep 0.2
	done
}
answers() { curl -s -o /dev/null -m 2 "http://127.0.0.1:$1/"; }

# The stand-ins.
mkdir -p "$T/bin"
printf '#!/bin/sh\nexit 0\n' >"$T/bin/mina"
cat >"$T/node.py" <<'EOF'
import http.server, json, socket, sys, threading
http_port, p2p_port = int(sys.argv[2]), int(sys.argv[4])
p2p = socket.create_server(("127.0.0.1", p2p_port))
class Status(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({"transition_frontier": {"sync": {"status": "Synced"},
                           "best_tip": {"height": 1}}, "peers": []}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args):
        pass
http.server.ThreadingHTTPServer(("127.0.0.1", http_port), Status).serve_forever()
EOF
# node.sh's stand-in: records the start, then becomes the node, as node.sh does.
cat >"$T/node.sh" <<'EOF'
#!/usr/bin/env bash
dir=$(dirname "$0")
case_dir=$(dirname "$1")
echo "$*" >>"$case_dir/node-starts"
echo $$ >>"$case_dir/started"
exec python3 -I "$dir/node.py" --http "$2" --libp2p-port "$3" --stand-in
EOF
# npx's stand-in: FAKE_NPX=fail exits at once, FAKE_NPX=hang runs, answering
# nothing.
cat >"$T/bin/npx" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$CASE/npx-calls"
echo $$ >>"$CASE/started"
[[ $FAKE_NPX == hang ]] && sleep 600
exit 1
EOF
chmod +x "$T/bin/mina" "$T/node.sh" "$T/bin/npx"

# setup NAME: a case directory and the watchdog's environment for it.
setup() {
	CASE=$T/$1
	mkdir -p "$CASE/run" "$CASE/web"
	HTTP=$(free_port) P2P=$(free_port) SERVE_PORT=$(free_port)
	export CASE HTTP P2P
	export MINA=$T/bin/mina GENESIS=$T/node.py NODE_SH=$T/node.sh
	export RUN_DIR=$CASE/run KEY=$CASE/no-key LOG=$CASE/node.log
	export PIDFILE=$CASE/node.pid HEALTH=$CASE/node.health
	export INTERVAL=1 MAX_RESTARTS=3 PATH=$T/bin:$PATH
	unset SERVE_DIR SERVE_PORT_SET SERVE_PIDFILE
	EVENTS=$CASE/node.events
}
serve_env() {
	export SERVE_DIR=$CASE/web SERVE_PORT SERVE_PIDFILE=$CASE/serve.pid
	export FAKE_NPX=$1
}
watchdog() { # starts one; its PID in $wd
	setsid "$WATCH" >>"$CASE/watch.out" 2>&1 </dev/null &
	wd=$!
	pids+=("$wd")
}
stop() {
	kill "$1" 2>/dev/null || true
	wait "$1" 2>/dev/null || true
}
by_hand() { # the recipe's step 2: node.sh in the background, no PID file
	setsid "$NODE_SH" "$RUN_DIR" "$HTTP" "$P2P" "$KEY" >>"$CASE/by-hand.log" 2>&1 </dev/null &
	hand=$!
	pids+=("$hand")
	wait_for 10 answers "$HTTP"
}

echo "1. a node started by hand is adopted, not doubled"
setup adopt
by_hand
watchdog
wait_for 10 grep -qs "adopted node $hand" "$EVENTS" || true
sleep 3
check "one node start, the one by hand" equals "$(lines "$CASE/node-starts")" 1
check "the node by hand adopted" equals "$(lines "$EVENTS" "adopted node $hand, which listens on $P2P")" 1
check "PIDFILE holds it" equals "$(cat "$PIDFILE")" "$hand"
check "health lines carry its PID" equals "$(tail -n 1 "$HEALTH" | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["pid"])')" "$hand"
stop "$hand"
wait_for 15 grep -qs "started node" "$EVENTS" || true
check "it is restarted once it exits" equals "$(lines "$CASE/node-starts")" 2
stop "$wd"

echo "2. ports held by another process: no node started"
setup http-taken
mkdir -p "$CASE/empty"
(cd "$CASE/empty" && exec setsid python3 -I -m http.server "$HTTP" --bind 127.0.0.1) >/dev/null 2>&1 &
other=$!
pids+=("$other")
wait_for 10 answers "$HTTP"
watchdog
sleep 5
check "no node started (HTTP taken)" equals "$(lines "$CASE/node-starts")" 0
check "said so once" equals "$(lines "$EVENTS" "not starting a node: HTTP $HTTP answers, by a process that is not the node")" 1
stop "$wd"
stop "$other"

setup p2p-taken
setsid python3 -I -c 'import socket, sys, time; s = socket.create_server(("127.0.0.1", int(sys.argv[1]))); time.sleep(600)' "$P2P" &
other=$!
pids+=("$other")
wait_for 10 bash -c "ss -Hltn 'sport = :$P2P' | grep -q ."
watchdog
sleep 4
check "no node started (P2P taken)" equals "$(lines "$CASE/node-starts")" 0
check "said so once" equals "$(lines "$EVENTS" "not starting a node: libp2p port $P2P is listening")" 1
stop "$wd"
stop "$other"

echo "3. one watchdog per PIDFILE"
setup lock
watchdog
first=$wd
wait_for 15 answers "$HTTP"
check "the first watchdog started the node" equals "$(lines "$CASE/node-starts")" 1
watchdog
second=$wd
sleep 2
check "a second watchdog exits at once" bash -c "! kill -0 $second 2>/dev/null"
check "saying the lock is held" grep -q "another watchdog holds $PIDFILE.lock" "$CASE/watch.out"
check "the first still runs" kill -0 "$first"
stop "$first"
watchdog
third=$wd
sleep 3
check "a new watchdog starts once the first is stopped" kill -0 "$third"
check "and adopts the node from PIDFILE: no new start" equals "$(lines "$CASE/node-starts")" 1
check "watchdog $third started with the node's PID" grep -q "watch.sh $third started: node pid $(cat "$PIDFILE")" "$EVENTS"
stop "$third"

echo "4. the static server"
setup serve-fails
serve_env fail
by_hand
watchdog
sleep 7
check "started MAX_RESTARTS (3) times when it keeps failing" equals "$(lines "$CASE/npx-calls")" 3
check "with --no-port-switching on its port" equals "$(lines "$CASE/npx-calls" "serve@14 -s . -l $SERVE_PORT --no-port-switching -c serve.json")" 3
check "then not again, said once" equals "$(lines "$EVENTS" "not starting the static server: 3 starts in the last hour")" 1
stop "$wd"

setup serve-hangs
serve_env hang
by_hand
watchdog
sleep 5
check "a server that runs without answering is started once" equals "$(lines "$CASE/npx-calls")" 1
check "and left alone, said once" equals "$(lines "$EVENTS" "static server $(cat "$SERVE_PIDFILE") runs but does not answer on $SERVE_PORT")" 1
stop "$wd"

setup serve-port-taken
serve_env fail
by_hand
# Listens on the port and closes each connection: it never answers.
setsid python3 -I -c '
import socket, sys
s = socket.create_server(("127.0.0.1", int(sys.argv[1])))
while True:
    s.accept()[0].close()' "$SERVE_PORT" &
other=$!
pids+=("$other")
wait_for 10 bash -c "ss -Hltn 'sport = :$SERVE_PORT' | grep -q ."
watchdog
sleep 4
check "no server started on a port another process holds" equals "$(lines "$CASE/npx-calls")" 0
check "said so once" equals "$(lines "$EVENTS" "port $SERVE_PORT is taken by a process other than the static server")" 1
stop "$wd"
stop "$other"

if ((failures)); then
	echo "test-watch.sh: $failures failed"
	exit 1
fi
echo "test-watch.sh: all passed"
