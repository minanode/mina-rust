"""Turn W2 remote JSONL samples into CSV, slopes and memory jumps.

Usage: w2-analyze.py BROWSER_JSONL RELAY_JSONL OUTPUT_DIRECTORY
"""
import csv
import json
import pathlib
import statistics
import sys


def read_jsonl(path):
    with open(path, encoding="utf8") as source:
        return [json.loads(line) for line in source if line.strip()]


def slope_per_hour(rows, field, start):
    points = [((row["unix"] - start) / 3600, row[field]) for row in rows
              if row["unix"] >= start and row.get(field) is not None]
    if len(points) < 3:
        return None
    xbar = statistics.mean(x for x, _ in points)
    ybar = statistics.mean(y for _, y in points)
    denominator = sum((x - xbar) ** 2 for x, _ in points)
    return sum((x - xbar) * (y - ybar) for x, y in points) / denominator


browser = read_jsonl(sys.argv[1])
relay = read_jsonl(sys.argv[2])
dest = pathlib.Path(sys.argv[3])
dest.mkdir(parents=True, exist_ok=True)
if not browser:
    raise SystemExit("no browser measurements")
if not relay:
    raise SystemExit("no relay measurements")
synced = next((row["unix"] for row in browser
               if (row.get("status") or {}).get("sync") == "Synced"
               and (row.get("status") or {}).get("height") is not None
               and (row.get("relay") or {}).get("height") is not None
               and int(row["status"]["height"]) >= int(row["relay"]["height"]) - 1), None)
if synced is None:
    raise SystemExit("browser never sampled Synced")

rows = []
process_rows = []
relay_index = 0
for sample in browser:
    while relay_index + 1 < len(relay) and abs(relay[relay_index + 1]["unix"] - sample["unix"]) < abs(relay[relay_index]["unix"] - sample["unix"]):
        relay_index += 1
    native = relay[relay_index] if relay else {}
    frames = sample.get("frames", [])
    node = next((f for f in frames if f.get("realm") == "node"), {})
    prover = next((f for f in frames if f.get("realm") == "prover"), {})
    processes = sample.get("processes", [])
    for process in processes:
        process_rows.append({
            "unix": sample["unix"],
            "hours_after_sync": (sample["unix"] - synced) / 3600,
            "pid": process["pid"], "role": process["role"],
            "rss_mib": process["rss_kib"] / 1024,
            "pss_mib": (process["pss_kib"] / 1024
                        if process["pss_kib"] is not None else None),
        })
    row = {
        "unix": sample["unix"], "hours_after_sync": (sample["unix"] - synced) / 3600,
        "sync": (sample.get("status") or {}).get("sync"),
        "height": (sample.get("status") or {}).get("height"),
        "relay_height": (sample.get("relay") or {}).get("height"),
        "peers": (sample.get("status") or {}).get("peers"),
        "node_wasm_bytes": sum(node.get("wasm_bytes", [])),
        "prover_wasm_bytes": sum(prover.get("wasm_bytes", [])),
        "node_wasm_instances": ";".join(map(str, node.get("wasm_bytes", []))),
        "prover_wasm_instances": ";".join(map(str, prover.get("wasm_bytes", []))),
        "js_heap_bytes": node.get("js_heap_bytes"),
        "browser_rss_mib": sample.get("browser_rss_mib"),
        "browser_pss_mib": sample.get("browser_pss_mib"),
        "relay_rss_mib": native.get("rss_kib", 0) / 1024 if native.get("rss_kib") else None,
        "relay_sample_age_seconds": abs(native["unix"] - sample["unix"]) if native else None,
        "renderer_rss_mib": sum(p["rss_kib"] for p in processes if p["role"] == "renderer") / 1024,
        "renderer_pss_mib": sum(p["pss_kib"] for p in processes if p["role"] == "renderer" and p["pss_kib"] is not None) / 1024,
    }
    rows.append(row)

with (dest / "samples.csv").open("w", newline="", encoding="utf8") as target:
    writer = csv.DictWriter(target, fieldnames=rows[0].keys())
    writer.writeheader()
    writer.writerows(rows)

with (dest / "processes.csv").open("w", newline="", encoding="utf8") as target:
    writer = csv.DictWriter(target, fieldnames=process_rows[0].keys())
    writer.writeheader()
    writer.writerows(process_rows)

jumps = []
for before, after in zip(rows, rows[1:]):
    delta = after["node_wasm_bytes"] - before["node_wasm_bytes"]
    if delta > 0 and after["hours_after_sync"] >= 0:
        jumps.append({"unix": after["unix"], "hours_after_sync": after["hours_after_sync"],
                      "delta_mib": delta / 2**20, "height_before": before["height"],
                      "height_after": after["height"], "relay_height": after["relay_height"]})

window_start = synced + 2 * 3600
summary = {
    "synced_unix": synced,
    "hours_observed_after_sync": (rows[-1]["unix"] - synced) / 3600,
    "samples_after_sync": sum(row["unix"] >= synced for row in rows),
    "last_10h_start_unix": window_start,
    "last_10h_slopes_per_hour": {
        field: slope_per_hour(rows, field, window_start)
        for field in ("node_wasm_bytes", "prover_wasm_bytes", "js_heap_bytes",
                      "browser_rss_mib", "browser_pss_mib", "relay_rss_mib")
    },
    "wasm_growth_jumps": jumps,
}
(dest / "analysis.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf8")

# Small dependency-free chart; source values remain in samples.csv.
plot_rows = [row for row in rows if row["hours_after_sync"] >= 0]
width, height = 800, 380
left, right, top, bottom = 55, 20, 20, 45
xmax = max(12, plot_rows[-1]["hours_after_sync"])
ymax = 3000


def xy(row, field):
    x = left + (width - left - right) * row["hours_after_sync"] / xmax
    y = height - bottom - (height - top - bottom) * row[field] / ymax
    return f"{x:.1f},{y:.1f}"


svg = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
       f'viewBox="0 0 {width} {height}">',
       '<rect width="100%" height="100%" fill="white"/>',
       '<style>text{font:12px sans-serif;fill:#333} .grid{stroke:#ddd}</style>']
for tick in range(0, ymax + 1, 500):
    y = height - bottom - (height - top - bottom) * tick / ymax
    svg += [f'<path class="grid" d="M {left} {y:.1f} H {width-right}"/>',
            f'<text x="{left-8}" y="{y+4:.1f}" text-anchor="end">{tick}</text>']
for tick in range(0, 13, 2):
    x = left + (width - left - right) * tick / xmax
    svg += [f'<text x="{x:.1f}" y="{height-20}" text-anchor="middle">{tick}h</text>']
for field, color, label in (("node_wasm_bytes", "#2563eb", "Node WASM"),
                            ("browser_rss_mib", "#dc2626", "Browser tree RSS"),
                            ("relay_rss_mib", "#16a34a", "Relay RSS")):
    points = []
    for row in plot_rows:
        value = row[field]
        if value is None:
            continue
        if field == "node_wasm_bytes":
            row = dict(row, node_wasm_bytes=value / 2**20)
        points.append(xy(row, field))
    svg.append(f'<polyline fill="none" stroke="{color}" stroke-width="2" '
               f'points="{" ".join(points)}"/>')
    legend_y = top + 14 + (("node_wasm_bytes", "browser_rss_mib", "relay_rss_mib").index(field) * 18)
    svg.append(f'<text x="{left+10}" y="{legend_y}" style="fill:{color}">{label}</text>')
svg += [f'<text x="{width/2}" y="{height-3}" text-anchor="middle">Hours after sync</text>',
        '<text x="9" y="15">MiB</text>', '</svg>']
(dest / "memory.svg").write_text("\n".join(svg) + "\n", encoding="utf8")
print(json.dumps(summary, indent=2))
