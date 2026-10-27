#!/usr/bin/env bash
# Keep a devnet node running, and record its health once a minute: the
# watchdog of a devnet soak node or relay. See ../README.md, "Devnet: a relay
# and a web node".
#
#   tools/regression/devnet/watch.sh
#
# Every INTERVAL seconds it:
#
# - checks that the node in PIDFILE runs: the PID is alive and its command line
#   has `--libp2p-port P2P` (or is node.sh's for HTTP and P2P). If not, and
#   something already answers on HTTP or listens on P2P (a node started by
#   hand, or one whose PID file is lost), it starts nothing: it adopts that
#   node if `ss` shows a process with that command line listening on P2P,
#   writing its PID to PIDFILE, and otherwise says in EVENTS that the ports are
#   taken. Only when both ports are free does it record that the node is gone,
#   move LOG aside to LOG.<UTC time>, start `node.sh RUN_DIR HTTP P2P KEY` with
#   MINA and GENESIS in a new session, its output appended to LOG, and write
#   its PID to PIDFILE. So a node already running when it starts is adopted,
#   not restarted or doubled, and a node that runs, however unhealthy, is never
#   restarted. After MAX_RESTARTS starts within an hour it says so in EVENTS
#   and starts none until the oldest of them is an hour old.
# - reads the node's /status (only /status) and appends one JSON line to
#   HEALTH: time, pid, resident memory (MB, from /proc), log size, sync status
#   and phase, best tip height, the highest best tip height its connected peers
#   report and the difference, and the connected peers, libp2p and WebRTC.
#   summary.py summarises HEALTH and EVENTS.
# - with SERVE_DIR and SERVE_PORT set, also keeps a static server for a web
#   node build in SERVE_DIR running on SERVE_PORT (`npx -y serve@14 -s . -l
#   SERVE_PORT --no-port-switching -c serve.json`, in SERVE_DIR, its PID, the
#   leader of its process group, in SERVE_PIDFILE). It starts one only when
#   nothing answers on the port, nothing listens on it, and the server in
#   SERVE_PIDFILE has gone, at most MAX_RESTARTS times an hour; one that runs
#   but does not answer is left alone, like the node.
#
# It needs `ss` (iproute2) and `flock` (util-linux). Only one watchdog runs for
# a PIDFILE: it holds a lock on PIDFILE.lock and exits if another has it. It
# runs in the foreground: start it with setsid and nohup, and stop it by its
# PID. Stopping it leaves the node and the static server running.
#
# Environment (required unless a default is given):
#   MINA           the mina binary: a pinned path, not a build directory
#   GENESIS        the genesis config node.sh passes to the node
#   RUN_DIR, HTTP, P2P, KEY
#                  node.sh's arguments (KEY: the p2p key file, so that the
#                  peer id, and the relay address in a web build, stay the same)
#   LOG            the node's log
#   PIDFILE        the node's PID file
#   HEALTH         the health log (JSON lines)
#   EVENTS         the events log (default: HEALTH with .events for .health)
#   NODE_SH        node.sh (default: the one next to this script)
#   INTERVAL       seconds between checks (default 60)
#   MAX_RESTARTS   node starts, and static server starts, allowed in an hour
#                  (default 5 each)
#   SERVE_DIR, SERVE_PORT, SERVE_PIDFILE, SERVE_LOG
#                  the optional static server (SERVE_LOG default /dev/null)
set -uo pipefail

usage() { sed -n '2,/^[^#]/s/^# \{0,1\}//p' "${BASH_SOURCE[0]}"; }
if [[ ${1:-} == -h || ${1:-} == --help ]]; then
	usage
	exit 0
fi
missing=()
for name in MINA GENESIS RUN_DIR HTTP P2P KEY LOG PIDFILE HEALTH; do
	[[ -n ${!name:-} ]] || missing+=("$name")
done
if ((${#missing[@]})); then
	echo "watch.sh: set ${missing[*]}; see --help" >&2
	exit 2
fi
EVENTS=${EVENTS:-${HEALTH%.health}.events}
NODE_SH=${NODE_SH:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/node.sh}
INTERVAL=${INTERVAL:-60}
MAX_RESTARTS=${MAX_RESTARTS:-5}
[[ -x $MINA ]] || {
	echo "watch.sh: $MINA is not executable" >&2
	exit 2
}
[[ -x $NODE_SH ]] || {
	echo "watch.sh: $NODE_SH is not executable" >&2
	exit 2
}

now() { date -u +%FT%TZ; }
event() { echo "$(now) $*" >>"$EVENTS"; }
# Record a condition in EVENTS once, not every INTERVAL while it lasts: `note
# KEY MESSAGE` writes MESSAGE unless it is KEY's last one; `note KEY` forgets
# it, so that it is written again when it comes back.
declare -A noted=()
note() {
	local key=$1
	shift
	[[ $# -eq 0 || ${noted[$key]:-} == "$*" ]] || event "$*"
	noted[$key]=$*
}

# One watchdog per node. The lock is on descriptor 8, which every process
# started here closes, so that it goes when the watchdog does.
exec 8>>"$PIDFILE.lock"
if ! flock -n 8; then
	echo "watch.sh: another watchdog holds $PIDFILE.lock; not starting" >&2
	exit 1
fi

# Whether PID is alive and its command line matches the pattern (grep's).
runs_with() { { tr '\0' ' ' <"/proc/$1/cmdline"; } 2>/dev/null | grep -q -- "$2"; }
# The node, or node.sh for it in the second before it execs the node.
is_node() { runs_with "$1" "--libp2p-port $P2P " || runs_with "$1" "node\.sh [^ ]* $HTTP $P2P"; }

# The node's PID, if PIDFILE names a live process that is this node.
node_pid() {
	local pid
	pid=$(cat "$PIDFILE" 2>/dev/null) || return 1
	[[ $pid =~ ^[0-9]+$ ]] && is_node "$pid" || return 1
	echo "$pid"
}

listening() { ss -Hltn "sport = :$1" 2>/dev/null | grep -q .; }
# What already holds the node's ports, if anything: then no node is started.
ports_taken() {
	local web='' lp=''
	curl -s -o /dev/null -m 5 "http://127.0.0.1:$HTTP/status" && web="HTTP $HTTP answers"
	listening "$P2P" && lp="libp2p port $P2P is listening"
	[[ -n $web$lp ]] || return 1
	echo "${web}${web:+${lp:+ and }}${lp}"
}
# The PID of this node listening on P2P, if `ss` can tell (same user only).
listener_pid() {
	local pid
	for pid in $(ss -Hltnp "sport = :$P2P" 2>/dev/null | grep -o 'pid=[0-9]*' | cut -d= -f2 | sort -u); do
		is_node "$pid" && echo "$pid" && return 0
	done
	return 1
}

# Whether the named array of start times (epoch seconds) has fewer than
# MAX_RESTARTS in the last hour; it drops the older ones.
under_cap() {
	local -n times=$1
	local cutoff=$(($(date +%s) - 3600)) recent=() t
	for t in "${times[@]}"; do ((t > cutoff)) && recent+=("$t"); done
	times=("${recent[@]}")
	((${#times[@]} < MAX_RESTARTS))
}

node_starts=()
start_node() {
	if ! under_cap node_starts; then
		note node-cap "not starting the node: ${#node_starts[@]} starts in the last hour"
		return 1
	fi
	note node-cap
	[[ -s $LOG ]] && mv "$LOG" "$LOG.$(date -u +%Y%m%dT%H%M%SZ)"
	MINA=$MINA GENESIS=$GENESIS setsid nohup "$NODE_SH" "$RUN_DIR" "$HTTP" "$P2P" "$KEY" \
		>>"$LOG" 2>&1 </dev/null 8>&- &
	echo $! >"$PIDFILE"
	node_starts+=("$(date +%s)")
	event "started node $! ($MINA)"
}

serving() { [[ -n ${SERVE_DIR:-} && -n ${SERVE_PORT:-} && -n ${SERVE_PIDFILE:-} ]]; }
serve_up() { curl -s -o /dev/null -m 5 "http://127.0.0.1:$SERVE_PORT/"; }
# The static server's PID, if SERVE_PIDFILE names a live server for the port.
serve_pid() {
	local pid
	pid=$(cat "$SERVE_PIDFILE" 2>/dev/null) || return 1
	[[ $pid =~ ^[0-9]+$ ]] && runs_with "$pid" "-l $SERVE_PORT " || return 1
	echo "$pid"
}
serve_starts=()
check_serve() {
	local pid
	if serve_up; then
		note serve
	elif pid=$(serve_pid); then
		note serve "static server $pid runs but does not answer on $SERVE_PORT; not starting another"
	elif listening "$SERVE_PORT"; then
		note serve "port $SERVE_PORT is taken by a process other than the static server in $SERVE_PIDFILE; not starting one"
	elif ! under_cap serve_starts; then
		note serve "not starting the static server: ${#serve_starts[@]} starts in the last hour"
	else
		note serve
		event "static server not answering on $SERVE_PORT"
		(
			cd "$SERVE_DIR" || exit 1
			setsid nohup npx -y serve@14 -s . -l "$SERVE_PORT" --no-port-switching -c serve.json \
				>>"${SERVE_LOG:-/dev/null}" 2>&1 </dev/null 8>&- &
			echo $! >"$SERVE_PIDFILE"
		)
		serve_starts+=("$(date +%s)")
		event "started static server $(cat "$SERVE_PIDFILE") on $SERVE_PORT for $SERVE_DIR"
	fi
}

# One JSON line from the node's /status, read on stdin.
health_line() {
	python3 -I -c '
import json, sys
t, pid, rss, log = sys.argv[1:5]
line = {"t": t, "pid": int(pid) if pid else None,
        "rss_mb": round(int(rss) / 1024) if rss else None,
        "log_bytes": int(log) if log else None}
try:
    d = json.load(sys.stdin)
    tf = d.get("transition_frontier") or {}
    tip = tf.get("best_tip") or {}
    sync = tf.get("sync") or {}
    peers = [p for p in d.get("peers") or [] if p.get("connection_status") == "Connected"]
    heights = [p["best_tip_height"] for p in peers if p.get("best_tip_height") is not None]
    line.update(sync=sync.get("status"), phase=sync.get("phase"),
                height=tip.get("height"),
                peer_max_height=max(heights) if heights else None,
                peers=len(peers),
                libp2p=sum(1 for p in peers if p.get("is_libp2p")),
                webrtc=sum(1 for p in peers if not p.get("is_libp2p")))
    if line["height"] is not None and line["peer_max_height"] is not None:
        line["behind"] = line["peer_max_height"] - line["height"]
except Exception as error:
    line["error"] = f"status unreadable: {type(error).__name__}"
print(json.dumps(line))' "$@"
}

event "watch.sh $$ started: node pid $(node_pid || echo none), interval ${INTERVAL}s"
trap 'kill "${sleeper:-}" 2>/dev/null; event "watch.sh $$ stopped"; exit 0' TERM INT
while true; do
	if pid=$(node_pid); then
		note node
	else
		if taken=$(ports_taken); then
			if pid=$(listener_pid); then
				echo "$pid" >"$PIDFILE"
				note node
				event "adopted node $pid, which listens on $P2P (not in $PIDFILE)"
			else
				pid=
				note node "not starting a node: $taken, by a process that is not the node in $PIDFILE (pid file: $(cat "$PIDFILE" 2>/dev/null || echo none))"
			fi
		else
			note node
			[[ -n ${noted[node-cap]:-} ]] ||
				event "node not running (pid file: $(cat "$PIDFILE" 2>/dev/null || echo none))"
			# node.sh execs the node once it has the peer id.
			start_node && sleep 5
			pid=$(node_pid) || pid=
		fi
	fi
	serving && check_serve
	rss=
	[[ -n $pid ]] && rss=$(awk '/^VmRSS:/ {print $2}' "/proc/$pid/status" 2>/dev/null)
	log=$(stat -c %s "$LOG" 2>/dev/null)
	curl -s -m 10 "http://127.0.0.1:$HTTP/status" 2>/dev/null |
		health_line "$(now)" "$pid" "$rss" "$log" >>"$HEALTH"
	sleep "$INTERVAL" 8>&- &
	sleeper=$!
	wait $sleeper
done
