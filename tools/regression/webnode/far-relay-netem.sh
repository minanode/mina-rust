#!/usr/bin/env bash
# Bootstrap a web node from a relay on the same Linux machine with the round
# trip of a far relay added on loopback (tc netem), and summarise how it went:
# time to Synced, megabytes per second, and every relay connection it lost,
# with the reason the page logged (WEB-FAR-RELAY-LOOP).
#
#   tools/regression/webnode/far-relay-netem.sh OUT DELAY_MS [LOSS_PCT] [MINUTES]
#
# DELAY_MS is added each way, so the round trip grows by twice that: 80 gives
# the 160 ms of a relay an ocean away. LOSS_PCT (default 0) drops that share
# of loopback packets. MINUTES (default 20) bounds the run; it stops 20 s
# after Synced.
#
# Run it on a machine you have to yourself, with passwordless sudo for tc: it
# replaces the root qdisc of `lo`, which slows everything on loopback, and
# removes it at the end. Before it, start a relay on this machine whose
# WebRTC multiaddr the page dials, and let it sync, e.g.
#
#   mina node --network devnet --port 4290 --http-addr 127.0.0.1 \
#     --libp2p-port 4291 --log-level debug
#
# and build the page for it (build.sh, NETWORK=devnet,
# RELAY=/ip4/127.0.0.1/tcp/4290/webrtc/http/p2p/<the relay's libp2p id>).
# Chrome connects to the relay's host candidates, whose packets go through
# `lo` and so get the delay; the summary prints the selected pair.
#
# Environment:
#   BUNDLE   build.sh's OUT/browser (required)
#   CHROME   chrome-headless-shell (required); LD_LIBRARY_PATH as it needs
#   PORT     where the page is served (default 4292)
#   RELAY_LOG  the relay's log, to count its SCTP retransmission timer expiries
#            during the run (optional; needs --log-level debug)
#
# It runs run.mjs (MINA_NO_LOCK=1) and writes its files to OUT, plus
# netem.txt and summary.json.
set -euo pipefail

usage() { sed -n '2,/^set -e/s/^# \{0,1\}//p' "${BASH_SOURCE[0]}" | sed '$d'; }
[[ $# -ge 2 ]] || {
	usage
	exit 2
}
out=$1 delay=$2 loss=${3:-0} minutes=${4:-20}
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
: "${BUNDLE:?set BUNDLE to the page, build.sh OUT/browser}" "${CHROME:?set CHROME}"
port=${PORT:-4292}
mkdir -p "$out"

python3 "$here/serve.py" "$BUNDLE" "$port" >"$out/serve.log" 2>&1 &
serve=$!
monitor=
cleanup() {
	sudo tc qdisc del dev lo root 2>/dev/null || true
	kill "$serve" 2>/dev/null || true
	if [[ -n $monitor ]]; then
		kill "$monitor" 2>/dev/null || true
		wait "$monitor" 2>/dev/null || true
	fi
}
trap cleanup EXIT

args=(delay "${delay}ms" limit 100000)
[[ $loss != 0 ]] && args+=(loss "${loss}%")
sudo tc qdisc replace dev lo root netem "${args[@]}"
tc qdisc show dev lo >"$out/netem.txt"
cp /proc/net/snmp "$out/udp-before.txt"
relay_log_start=0
[[ -n ${RELAY_LOG:-} ]] && relay_log_start=$(wc -c <"$RELAY_LOG")
relay_t3_before=0
[[ -n ${RELAY_LOG:-} ]] && relay_t3_before=$(grep -c "T3 Expired" "$RELAY_LOG" || true)

# Keep per-socket evidence while Chrome is alive; its drop counters disappear
# when the socket closes. ss includes process names, inodes and skmem drops.
python3 -u - >"$out/udp-sockets.log" 2>&1 <<'PYMON' &
import signal, subprocess, time
running = True
def stop(*_):
    global running
    running = False
signal.signal(signal.SIGTERM, stop)
while running:
    print(time.time(), flush=True)
    subprocess.run(["ss", "-u", "-a", "-n", "-m", "-p", "-e"], check=False)
    for _ in range(50):
        if not running:
            break
        time.sleep(0.1)
PYMON
monitor=$!

BASE=http://127.0.0.1:$port PAGE=/ BUTTON="Start the web node in this tab" \
	EVERY_MS=5000 UNTIL_SYNCED=20 MINA_NO_LOCK=1 OUT="$out" \
	node "$here/run.mjs" "$minutes" >"$out/driver.log" 2>&1 || true

cp /proc/net/snmp "$out/udp-after.txt"
tc -s qdisc show dev lo >"$out/netem-after.txt"
relay_t3=
if [[ -n ${RELAY_LOG:-} ]]; then
	tail -c +$((relay_log_start + 1)) "$RELAY_LOG" >"$out/relay-during.log"
	relay_t3=$(($(grep -c "T3 Expired" "$RELAY_LOG" || true) - relay_t3_before))
fi
python3 - "$out" "$delay" "$loss" "$relay_t3" <<'EOF'
import json, re, sys
out, delay, loss, t3 = sys.argv[1], int(sys.argv[2]), float(sys.argv[3]), sys.argv[4]
# run.mjs's rx_mb sums the open connections' bytes received, so a lost
# connection's count drops out: add each connection's peak.
synced = end = mib_at_sync = None
done = peak = 0.0
app_done = app_peak = 0.0
app_at_sync = None
has_app_stats = False
connections, pairs = 0, set()
for line in open(f"{out}/run.log"):
    m = re.match(r"(\d+)s .*status=(\{.*?\}) rtc=(\{.*\})$", line.strip())
    if not m:
        continue
    t, rtc = int(m[1]), json.loads(m[3])
    if "open" not in rtc:
        continue
    try:
        status = json.loads(m[2])
    except ValueError:
        status = {}
    end = t
    if rtc["open"]:
        connections = max(connections, 1)
        if rtc["rx_mb"] < peak:
            done, peak, connections = done + peak, 0.0, connections + 1
            app_done, app_peak = app_done + app_peak, 0.0
        peak = max(peak, rtc["rx_mb"])
        channels = [s for s in rtc.get("sctp", []) if s.get("type") == "data-channel"]
        if channels:
            has_app_stats = True
            app_peak = max(app_peak, sum(s.get("bytesReceived", 0) for s in channels) / 2**20)
    pairs.update(rtc.get("pairs", []))
    if status.get("sync") == "Synced" and synced is None:
        synced, mib_at_sync = t, done + peak
        app_at_sync = app_done + app_peak
mib = mib_at_sync if synced is not None else done + peak
secs = synced if synced is not None else end
app_mib = app_at_sync if synced is not None else app_done + app_peak
console = open(f"{out}/console.log", errors="replace").read()
reasons = re.findall(r'kind = "P2pDisconnectionInit";.*?reason = "?([^";\n]+)', console, re.S)
def udp_counters(path):
    lines = open(path).read().splitlines()
    for i, line in enumerate(lines[:-1]):
        if line.startswith("Udp:"):
            return dict(zip(line.split()[1:], map(int, lines[i+1].split()[1:])))
    return {}
before = udp_counters(f"{out}/udp-before.txt")
after = udp_counters(f"{out}/udp-after.txt")
summary = {
    "delay_ms_each_way": delay,
    "loss_pct": loss,
    "synced_after_s": synced,
    "ran_s": end,
    "mib": round(mib, 1),
    "mb_per_s": round(mib * 2**20 / 1e6 / secs, 3) if secs else None,
    "data_channel_mib": round(app_mib, 1) if has_app_stats else None,
    "data_channel_mb_per_s": round(app_mib * 2**20 / 1e6 / secs, 3) if has_app_stats and secs else None,
    "relay_connections": connections,
    "disconnect_reasons": reasons,
    "peer_closed": len(re.findall(r'kind = "P2pPeerClosed"', console)),
    "rpc_timeouts": re.findall(r'summary = "(\w+) request timed out', console),
    "relay_t3_expiries": int(t3) if t3 else None,
    "pairs": sorted(pairs),
    "udp_delta": {key: value - before.get(key, 0) for key, value in after.items()},
}
json.dump(summary, open(f"{out}/summary.json", "w"), indent=1)
print(json.dumps(summary))
EOF
