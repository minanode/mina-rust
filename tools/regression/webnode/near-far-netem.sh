#!/usr/bin/env bash
# Bootstrap a web node from two relays on the same Linux machine, one near and
# one far, with each relay's round trip added on loopback (tc netem), and
# summarise how it went: time to Synced, MiB received from each relay, and
# every relay connection it lost (WEB-NEAREST-RELAY).
#
#   tools/regression/webnode/near-far-netem.sh OUT NEAR_MS FAR_MS [STOP_NEAR_AT_S] [MINUTES]
#
# NEAR_MS and FAR_MS are added each way, so 7 and 95 give round trips of 14
# and 190 ms. With STOP_NEAR_AT_S, the near relay (NEAR_PID) is sent SIGTERM
# that many seconds after the page opens, to check that the sync carries on
# from the far one; start it again yourself afterwards. MINUTES (default 20)
# bounds the run; it stops 20 s after Synced.
#
# Run it on a machine you have to yourself, with passwordless sudo for tc: it
# replaces the root qdisc of `lo` and removes it at the end. Only the relays'
# WebRTC UDP ports are delayed, so start them with fixed port ranges, the
# near one on 40960-41023 and the far one on 45056-45119, e.g.
#
#   mina node --network devnet -p 3010 --http-addr 127.0.0.1 \
#     --webrtc-signal-port 4290 --webrtc-signal-addr 127.0.0.1 \
#     --libp2p-port 8610 --webrtc-udp-ports 40960-41023
#   mina node --network devnet -p 3020 --http-addr 127.0.0.1 \
#     --webrtc-signal-port 4294 --webrtc-signal-addr 127.0.0.1 \
#     --libp2p-port 8620 --webrtc-udp-ports 45056-45119
#
# each with its own work directory and key, let them sync, and build the page
# for both (build.sh, NETWORK=devnet, RELAY=/ip4/127.0.0.1/tcp/4290/webrtc/
# http/p2p/<near id>,/ip4/127.0.0.1/tcp/4294/webrtc/http/p2p/<far id>).
#
# Environment:
#   BUNDLE    build.sh's OUT/browser (required)
#   CHROME    chrome-headless-shell (required); LD_LIBRARY_PATH as it needs
#   PORT      where the page is served (default 4280)
#   NEAR_HTTP the near relay's HTTP port, for its tip (default 3010)
#   NEAR_PID  the near relay's PID (required with STOP_NEAR_AT_S)
#
# It runs run.mjs (MINA_NO_LOCK=1) and writes its files to OUT, plus
# netem.txt and summary.json.
set -euo pipefail

usage() { sed -n '2,/^set -e/s/^# \{0,1\}//p' "${BASH_SOURCE[0]}" | sed '$d'; }
[[ $# -ge 3 ]] || {
	usage
	exit 2
}
out=$1 near=$2 far=$3 stop=${4:-} minutes=${5:-20}
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
: "${BUNDLE:?set BUNDLE to the page, build.sh OUT/browser}" "${CHROME:?set CHROME}"
[[ -z $stop || -n ${NEAR_PID:-} ]] || {
	echo "set NEAR_PID to stop the near relay" >&2
	exit 2
}
port=${PORT:-4280}
mkdir -p "$out"

tip=$(curl -s "127.0.0.1:${NEAR_HTTP:-3010}/status" |
	python3 -c 'import json,sys; print(json.load(sys.stdin)["transition_frontier"]["best_tip"]["height"])')

python3 "$here/serve.py" "$BUNDLE" "$port" >"$out/serve.log" 2>&1 &
serve=$!
stopper=
cleanup() {
	sudo tc qdisc del dev lo root 2>/dev/null || true
	kill "$serve" $stopper 2>/dev/null || true
}
trap cleanup EXIT

# Bands 1 and 2 delay the near and far relay's ports, both ways; the rest
# goes to band 4, undelayed.
sudo tc qdisc del dev lo root 2>/dev/null || true
sudo tc qdisc add dev lo root handle 1: prio bands 4 priomap 3 3 3 3 3 3 3 3 3 3 3 3 3 3 3 3
sudo tc qdisc add dev lo parent 1:1 handle 10: netem delay "${near}ms" limit 100000
sudo tc qdisc add dev lo parent 1:2 handle 20: netem delay "${far}ms" limit 100000
sudo tc qdisc add dev lo parent 1:4 handle 40: pfifo limit 100000
for dir in sport dport; do
	for proto in ip ip6; do
		family=ip prio=1
		[[ $proto == ip6 ]] && family=ipv6 prio=2
		sudo tc filter add dev lo parent 1: protocol "$family" prio "$prio" u32 \
			match "$proto" protocol 17 0xff match "$proto" "$dir" 40960 0xffc0 flowid 1:1
		sudo tc filter add dev lo parent 1: protocol "$family" prio "$prio" u32 \
			match "$proto" protocol 17 0xff match "$proto" "$dir" 45056 0xffc0 flowid 1:2
	done
done
tc qdisc show dev lo >"$out/netem.txt"

if [[ -n $stop ]]; then
	(
		sleep "$stop"
		kill "$NEAR_PID"
		echo "near relay stopped at ${stop}s" >"$out/near-stopped.txt"
	) &
	stopper=$!
fi

BASE=http://127.0.0.1:$port PAGE=/ BUTTON="Start the web node in this tab" \
	MIN_SYNC_HEIGHT=$((tip - 1)) EVERY_MS=5000 UNTIL_SYNCED=20 MINA_NO_LOCK=1 OUT="$out" \
	node "$here/run.mjs" "$minutes" >"$out/run.log" 2>&1 || true

python3 - "$out" "$near" "$far" "$stop" <<'EOF'
import json, re, sys
out, near, far, stop = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]

def relay(port):
    try:
        p = int(port)
    except ValueError:
        return "closed"
    return "near" if 40960 <= p <= 41023 else "far" if 45056 <= p <= 45119 else "other"

synced = end = last = None
phases = {}
for line in open(f"{out}/run.log"):
    m = re.match(r"(\d+)s .*status=(\{.*?\}) rtc=(\{.*\})$", line.strip())
    if not m:
        continue
    t, rtc = int(m[1]), json.loads(m[3])
    try:
        status = json.loads(m[2])
    except ValueError:
        status = {}
    end = t
    if synced is None:
        last = rtc
        phases.setdefault(status.get("sync"), t)
        if status.get("sync") == "Synced":
            synced = t
by_relay = {}
for port, mib in (last or {}).get("rx_by_port", {}).items():
    by_relay[relay(port)] = round(by_relay.get(relay(port), 0) + mib, 1)
console = open(f"{out}/console.log", errors="replace").read()
summary = {
    "delay_ms_each_way": {"near": near, "far": far},
    "near_stopped_at_s": int(stop) if stop else None,
    "synced_after_s": synced,
    "ran_s": end,
    "phase_started_s": phases,
    "mib": (last or {}).get("rx_mb"),
    "mib_by_relay": by_relay,
    "disconnect_reasons": re.findall(r'kind = "P2pDisconnectionInit";.*?reason = "?([^";\n]+)', console, re.S),
    "rpc_timeouts": re.findall(r'summary = "(\w+) request timed out', console),
    "pairs": (last or {}).get("pairs"),
}
json.dump(summary, open(f"{out}/summary.json", "w"), indent=1)
print(json.dumps(summary))
EOF
