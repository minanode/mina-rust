"""Copy a W2 web build without optional private-key test fixtures.

Usage: w2-filter-bundle.py BUILD_BROWSER APPROVED_BROWSER REFERENCE_WASM
The reference is the already reviewed public m11 WASM; its two public protocol
constants are the only private-key-shaped byte strings permitted in this WASM.
The report contains filenames and counts only, never matched bytes.
"""
import hashlib
import json
import pathlib
import re
import shutil
import sys

source = pathlib.Path(sys.argv[1]).resolve()
dest = pathlib.Path(sys.argv[2]).resolve()
reference = pathlib.Path(sys.argv[3]).resolve()
pattern = re.compile(rb"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----|\bEK[1-9A-HJ-NP-Za-km-z]{48,}\b|\b(?:CAES|CAEQ|CAIS|CAM[QS])[A-Za-z0-9+/= ]{8,}")
reference_matches = pattern.findall(reference.read_bytes())
public = set(reference_matches)
if len(reference_matches) != 2:
    raise SystemExit("reference WASM did not have the expected two public constants")
wasm = json.loads((source / "build-info.json").read_text())["wasm"]["path"].lstrip("/")
omitted = []
copied = 0
for path in source.rglob("*"):
    if not path.is_file():
        continue
    rel = path.relative_to(source)
    data = path.read_bytes()
    matches = pattern.findall(data)
    if matches and not (str(rel) == wasm and set(matches) == public and len(matches) == 2):
        omitted.append(str(rel))
        continue
    target = dest / rel
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(path, target)
    if hashlib.sha256(target.read_bytes()).digest() != hashlib.sha256(data).digest():
        raise SystemExit(f"copy changed {rel}")
    copied += 1

report = {"source": str(source), "copied_files": copied, "omitted": sorted(omitted),
          "allowed_wasm_matches": 2, "reference_wasm_sha256": hashlib.sha256(reference.read_bytes()).hexdigest()}
(dest.parent / "filter-report.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report))
