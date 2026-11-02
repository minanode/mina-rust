#!/usr/bin/env python3
"""Build the pinned bindgen CLI with stable cast ordering, under the build lock."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import urllib.request

VERSION = "0.2.106"
PACKAGES = {
    "wasm-bindgen-cli": "5b6ba7fc8c393116b30111576d9889d78df0709fc4ea5a0896b4d54380e24b36",
    "wasm-bindgen-cli-support": "03794299fa80bda34aef2784a496c6440fbc75fb1977c4e05750ddcd617e5a09",
}


def crate_bytes(name):
    filename = f"{name}-{VERSION}.crate"
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    cached = sorted((cargo_home / "registry/cache").glob(f"*/{filename}"))
    if cached:
        data = cached[0].read_bytes()
    else:
        url = f"https://static.crates.io/crates/{name}/{filename}"
        with urllib.request.urlopen(url, timeout=60) as response:
            data = response.read()
    if hashlib.sha256(data).hexdigest() != PACKAGES[name]:
        raise ValueError(f"pinned crate checksum mismatch: {name}")
    return data


def prepare(root):
    root.mkdir(parents=True, exist_ok=True)
    for name in PACKAGES:
        with tarfile.open(fileobj=io.BytesIO(crate_bytes(name))) as archive:
            # Never unpack links or paths outside the one pinned crate directory.
            for member in archive.getmembers():
                parts = Path(member.name).parts
                if (not parts or parts[0] != f"{name}-{VERSION}" or ".." in parts
                        or not (member.isfile() or member.isdir())):
                    raise ValueError(f"unsafe member in pinned crate: {name}")
            archive.extractall(root, filter="data")
    cli = root / f"wasm-bindgen-cli-{VERSION}"
    support = root / f"wasm-bindgen-cli-support-{VERSION}"
    source = support / "src/wit/mod.rs"
    old = "            for (descriptor, orig_func_ids) in cast_imports {"
    # Upstream 0.2.106 consumes a HashMap here. Its random order changes imports,
    # closure exports, function indices, JS adapters and TypeScript declarations.
    # Original function IDs retain the input module's deterministic order.
    new = """            let mut cast_imports: Vec<_> = cast_imports.into_iter().collect();
            cast_imports.sort_by_key(|(_, ids)| ids[0].index());
            for (descriptor, orig_func_ids) in cast_imports {"""
    text = source.read_text()
    if text.count(old) != 1:
        raise ValueError("pinned bindgen cast-order patch no longer applies")
    source.write_text(text.replace(old, new))
    manifest = cli / "Cargo.toml"
    manifest.write_text(manifest.read_text() + '\n[workspace]\n\n[patch.crates-io]\n'
                        f'wasm-bindgen-cli-support = {{ path = "../{support.name}" }}\n')
    # Preserve every upstream locked dependency; only support becomes a path dep.
    lock = cli / "Cargo.lock"
    old = (f'name = "wasm-bindgen-cli-support"\nversion = "{VERSION}"\n'
           'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
           f'checksum = "{PACKAGES["wasm-bindgen-cli-support"]}"\n')
    text = lock.read_text()
    if text.count(old) != 1:
        raise ValueError("pinned bindgen lock entry no longer matches")
    lock.write_text(text.replace(old, f'name = "wasm-bindgen-cli-support"\nversion = "{VERSION}"\n'))
    return cli


def build(root, env):
    cli = prepare(root)
    build_env = dict(env, CARGO_TARGET_DIR=str(root / "build"))
    subprocess.run(["cargo", "+nightly-2026-07-15", "build", "--locked", "--release",
                    "--bin", "wasm-bindgen", "--manifest-path", str(cli / "Cargo.toml")],
                   cwd=cli, env=build_env, check=True)
    return root / "build/release/wasm-bindgen"
