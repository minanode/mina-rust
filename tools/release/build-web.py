#!/usr/bin/env python3
"""Build an unsigned, self-contained devnet web archive; run under the build lock."""
import argparse
import gzip
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
spec = importlib.util.spec_from_file_location("serve_web", HERE / "serve-web.py")
server = importlib.util.module_from_spec(spec)
spec.loader.exec_module(server)
spec = importlib.util.spec_from_file_location("deterministic_bindgen", HERE / "deterministic-bindgen.py")
bindgen = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bindgen)


def git(*args):
    return subprocess.check_output(["git", "-C", str(REPO), *args], text=True).strip()


def build_environment(relay, provers, epoch, commit):
    # Only tool discovery/cache locations and scheduling controls are inherited.
    # Genesis, features, Vite overrides and host verifier caches are not inputs.
    allowed = ("PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME", "TMPDIR",
               "CARGO_BUILD_JOBS", "CARGO_TARGET_DIR", "CARGO_PROFILE_DEV_DEBUG",
               "CARGO_INCREMENTAL")
    env = {key: os.environ[key] for key in allowed if key in os.environ}
    env.update(MINAWEB_GLUE_URL="/assets/webnode/pkg/mina_node_web.js",
               UI="minaweb", NETWORK="devnet", RELAY=relay, MINAWEB_LOCAL="1",
               PROVER_CIRCUITS=str(provers), WASM_URL="", CIRCUITS_URL="",
               ICE_SERVERS="", NATIVE="", CONTROLS="", WASM_FEATURES="",
               FRONTIER_STORE="on", VERIFIER_CACHES="omit",
               GENESIS=str(REPO / "genesis_ledgers/mesa-devnet-without-ledgers.json"),
               SOURCE_DATE_EPOCH=str(epoch), TZ="UTC", LC_ALL="C",
               VERGEN_GIT_BRANCH="release", VERGEN_GIT_DESCRIBE=commit)
    return env


def write_archive(bundle, destination, epoch):
    with destination.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as gz:
            with tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT) as tar:
                for path in sorted(bundle.rglob("*")):
                    info = tar.gettarinfo(str(path), arcname=f"{bundle.name}/{path.relative_to(bundle)}")
                    info.uid = info.gid = 0
                    info.uname = info.gname = ""
                    info.mtime = epoch
                    info.mode = 0o755 if path.is_dir() else 0o644
                    if path.is_file():
                        with path.open("rb") as stream:
                            tar.addfile(info, stream)
                    else:
                        tar.addfile(info)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("--output", type=Path, default=REPO / "dist")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.]+)?", args.version):
        parser.error("invalid version")
    version = tomllib.loads((REPO / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    if args.version != version:
        parser.error("archive version differs from workspace version")
    try:
        relay = server.relay_address(os.environ.get("RELAY", ""))
    except argparse.ArgumentTypeError as error:
        parser.error(f"set RELAY: {error}")
    if git("status", "--porcelain"):
        parser.error("release build requires a clean checkout")
    if shutil.disk_usage(REPO).free < 55 * 1024**3:
        parser.error("release build requires at least 55 GiB free")
    commit = git("rev-parse", "HEAD")
    epoch = int(git("show", "-s", "--format=%ct", "HEAD"))
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    name = f"mina-web-{args.version}.tar.gz"
    if (out / name).exists() or (out / "SHA256SUMS.sig").exists():
        parser.error("refusing to replace an archive or change signed checksums")
    with tempfile.TemporaryDirectory(prefix=".web-build-", dir=out) as scratch:
        stage = Path(scratch)
        provers = stage / "provers"
        provers.mkdir()
        pins = (REPO / "tools/mesa-circuits/devnet-blobs.sha256").read_text().splitlines()
        if len(pins) != 24:
            raise ValueError("expected 24 canonical devnet prover files")
        for line in pins:
            digest, filename = line.split()
            if Path(filename).name != filename:
                raise ValueError("invalid canonical filename")
            compressed = REPO / "crates/ledger/src/proofs/data/circuits/4.0.2devnet" / (filename + ".zst")
            with (provers / filename).open("wb") as stream:
                subprocess.run(["zstd", "-d", "-q", "-c", str(compressed)], stdout=stream, check=True)
            if server.sha(provers / filename) != digest:
                raise ValueError(f"canonical prover SHA-256 mismatch: {filename}")
        bundle = stage / f"mina-web-{args.version}"
        env = build_environment(relay, provers, epoch, commit)
        # Keep the patched tool private to this build; never change a global CLI
        # or the Cargo registry. SOURCE_DATE_EPOCH alone cannot order its HashMap.
        env["WASM_BINDGEN"] = str(bindgen.build(stage / "bindgen", env))
        subprocess.run([str(REPO / "tools/regression/webnode/build.sh"), str(bundle)],
                       cwd=REPO, env=env, check=True)
        # The archive already compresses the raw WASM. Do not ship three copies.
        for path in (bundle / "browser/assets/webnode/pkg").iterdir():
            if path.suffix in (".br", ".gz", ".zst"):
                path.unlink()
        (bundle / "browser/serve.json").unlink()
        (bundle / "browser/local-relay.json").write_text(json.dumps({"relays": [relay]}) + "\n")
        shutil.copyfile(HERE / "serve-web.py", bundle / "serve.py")
        shutil.copyfile(HERE / "WEB-README.md", bundle / "README.md")
        notices = HERE / "web-licenses"
        subprocess.run(["python3", "-B", str(HERE / "generate-notices.py"),
                        "--web-built", str(bundle / "browser"),
                        "--output", str(notices), "--check"], check=True)
        shutil.copyfile(REPO / "LICENSE", bundle / "LICENSE")
        for source in sorted(notices.rglob("*")):
            if source.is_file():
                destination = bundle / source.relative_to(notices)
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, destination)
        files = [{"path": p.relative_to(bundle).as_posix(), "bytes": p.stat().st_size,
                  "sha256": server.sha(p)} for p in sorted(bundle.rglob("*")) if p.is_file()]
        manifest = {"schema": 1, "version": args.version, "commit": commit,
                    "network": "devnet", "relay": relay, "provers": "included",
                    "build": json.loads((bundle / "browser/build-info.json").read_text()), "files": files}
        (bundle / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        (bundle / "SHA256SUMS").write_text("".join(f"{f['sha256']}  {f['path']}\n" for f in files)
                                          + f"{server.sha(bundle / 'manifest.json')}  manifest.json\n")
        server.verify(bundle)
        write_archive(bundle, stage / name, epoch)
        (stage / name).rename(out / name)
    # Preserve the native archives' entries; refuse to alter a signed set above.
    sums = out / "SHA256SUMS"
    previous = sums.read_text().splitlines() if sums.exists() else []
    previous = [line for line in previous if line.split()[-1] != name]
    sums.write_text("\n".join(previous + [f"{server.sha(out / name)}  {name}"]) + "\n")
    print(json.dumps({"archive": name, "bytes": (out / name).stat().st_size,
                      "sha256": server.sha(out / name), "commit": commit}, indent=2))


if __name__ == "__main__":
    main()
