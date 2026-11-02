#!/usr/bin/env python3
"""Prepare an immutable devnet release from the current clean, specified commit.

Run under the build lock; never uploads. See docs/release/MINAWEB.md.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import uuid

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO / "tools/mesa-circuits"))
from web_pins import build_pins


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def git(*args):
    return subprocess.check_output(["git", "-C", str(REPO), *args], text=True).strip()


def verify_caches(directory):
    for name, source in (("block", "blockchain"), ("transaction", "transaction")):
        path = directory / "4.0.2devnet" / f"{name}_verifier_index.postcard"
        expected = sha(REPO / "crates/ledger/src/proofs/data" /
                       f"devnet_{source}_verifier_index.json")
        if not path.is_file() or path.stat().st_size <= 32:
            raise SystemExit(f"missing or empty {name} verifier cache")
        with path.open("rb") as stream:
            if stream.read(32).hex() != expected:
                raise SystemExit(f"{name} verifier cache does not match this source")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("commit", help="must be checked out and clean")
    parser.add_argument("output", type=Path, help="new output directory")
    args = parser.parse_args()
    commit = git("rev-parse", args.commit + "^{commit}")
    if commit != git("rev-parse", "HEAD") or git("status", "--porcelain"):
        parser.error("check out the requested commit in a clean worktree first")
    out = args.output.resolve()
    if out.exists():
        parser.error("output exists; a release must never replace another release")
    peer = os.environ.get("RELAY_PEER_ID", "")
    if not re.fullmatch(r"12D3KooW[1-9A-HJ-NP-Za-km-z]{44}", peer):
        parser.error("set RELAY_PEER_ID to the relay's public peer ID")
    domain = os.environ.get("RELAY_DOMAIN", "relay.minaweb.org")
    assets = os.environ.get("ASSETS_ORIGIN", "https://assets.minaweb.org").rstrip("/")
    if not re.fullmatch(r"[a-zA-Z0-9.-]+", domain):
        parser.error("RELAY_DOMAIN must be a DNS name")
    if not re.fullmatch(r"https://[a-zA-Z0-9.-]+", assets):
        parser.error("ASSETS_ORIGIN must be an HTTPS origin without a path")
    # Fail before compiling; the generic builder can omit either stale cache.
    verify_caches(Path.home() / ".mina/circuit-blobs")
    provers = os.environ.get("PROVER_CIRCUITS")
    if not provers:
        parser.error("set PROVER_CIRCUITS to the canonical devnet prover directory")
    try:
        build_pins("devnet", Path(provers), require_provers=True)
    except (ValueError, OSError) as error:
        parser.error(str(error))
    # The nonce also distinguishes configuration changes and rebuilds of one commit.
    release = f"releases/{commit}/{uuid.uuid4().hex}"
    env = dict(os.environ, UI="minaweb", NETWORK="devnet", NATIVE="", CONTROLS="",
               RELAY=f"/dns4/{domain}/tcp/443/webrtc/https/p2p/{peer}",
               WASM_URL=f"{assets}/{release}/wasm",
               CIRCUITS_URL=f"{assets}/{release}/circuit-blobs",
               ICE_SERVERS=str(HERE / "ice-servers.json"))
    env.pop("ALLOW_DIRTY", None)
    subprocess.run([str(REPO / "tools/regression/webnode/build.sh"), str(out)],
                   env=env, check=True, cwd=REPO)
    browser = out / "browser"
    shutil.copyfile(HERE / "_headers", browser / "_headers")
    (browser / "serve.json").unlink()
    # Fetch fresh time-limited credentials at each page boot, including rollback.
    (browser / "webnode-devnet/ice-servers.json").unlink()
    (browser / "_redirects").write_text(
        f"/webnode-devnet/ice-servers.json https://{domain}/ice-servers.json 302\n")
    # Publish raw WASM: no content-encoding ambiguity; CDN compression is optional.
    for suffix in (".br", ".gz", ".zst"):
        for path in (out / "upload/wasm").glob("*" + suffix):
            path.unlink()
    verify_caches(out / "upload/circuit-blobs")
    try:
        build_pins("devnet", out / "upload/circuit-blobs/4.0.2devnet", require_provers=True)
    except (ValueError, OSError) as error:
        raise SystemExit(str(error)) from error
    for path in browser.rglob("*"):
        if path.is_file() and path.stat().st_size > 25 * 1024**2:
            raise SystemExit(f"Pages file exceeds 25 MiB: {path.relative_to(browser)}")
    files = [{"path": p.relative_to(out).as_posix(), "bytes": p.stat().st_size,
              "sha256": sha(p)} for p in sorted(out.rglob("*")) if p.is_file()]
    info = json.loads((browser / "build-info.json").read_text())
    manifest = {"schema": 1, "commit": commit, "release": release,
                "network": "devnet", "build": info, "files": files}
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (out / "SHA256SUMS").write_text("".join(
        f"{f['sha256']}  {f['path']}\n" for f in files) +
        f"{sha(out / 'manifest.json')}  manifest.json\n")
    bucket = os.environ.get("R2_BUCKET", "minaweb-assets")
    project = os.environ.get("PAGES_PROJECT", "minaweb")
    if not all(re.fullmatch(r"[a-z0-9-]+", s) for s in (bucket, project)):
        raise SystemExit("invalid R2_BUCKET or PAGES_PROJECT")
    q = shlex.quote
    # rclone refuses changed objects instead of replacing an immutable release.
    # --checksum prevents equal-size changed files from being silently skipped.
    commands = ["#!/usr/bin/env bash", "set -euo pipefail",
                'cd -- "$(dirname -- "${BASH_SOURCE[0]}")"', "sha256sum -c SHA256SUMS",
                "# Configure the private rclone remote 'r2' before running.",
                f"rclone copy upload {q('r2:' + bucket + '/' + release)} --immutable --checksum",
                f"rclone check upload {q('r2:' + bucket + '/' + release)} --download --one-way",
                "# Verify asset CORS/CORP and MIME types before deploying Pages.",
                'test "${ASSET_HEADERS_VERIFIED:-}" = yes',
                f"npx wrangler pages deploy browser --project-name {q(project)} --branch main --commit-hash {commit}"]
    (out / "deploy.sh").write_text("\n".join(commands) + "\n")
    print(json.dumps({"manifest": str(out / "manifest.json"), "release": release,
                      "files": len(files), "bytes": sum(f["bytes"] for f in files)}, indent=2))


if __name__ == "__main__":
    main()
