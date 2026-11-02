#!/usr/bin/env python3
"""Install a relocatable OCaml consensus oracle and its public build recipe."""
import argparse
import hashlib
from pathlib import Path
import shutil


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--runtime-libs", type=Path, required=True)
    parser.add_argument("--prefix", type=Path, required=True)
    args = parser.parse_args()
    prefix = args.prefix.resolve()
    prefix.mkdir(parents=True, exist_ok=False)
    for name in ("bin", "libexec", "lib", "source"):
        (prefix / name).mkdir()
    executable = prefix / "libexec/consensus_diff.exe"
    shutil.copy2(args.executable, executable)
    for library in args.runtime_libs.iterdir():
        if library.is_file():
            shutil.copy2(library, prefix / "lib" / library.name)
    here = Path(__file__).resolve().parent
    for name in ("prepare-ocaml.py", "ocaml/hook.ml", "ocaml/consensus_diff.ml", "ocaml/dune"):
        target = prefix / "source" / name
        target.parent.mkdir(exist_ok=True)
        shutil.copy2(here / name, target)
    wrapper = prefix / "bin/consensus_diff"
    wrapper.write_text('''#!/usr/bin/env bash
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
: "${TMPDIR:?Set TMPDIR to a private task directory}"
ulimit -s 65532
export LD_LIBRARY_PATH="$root/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$root/libexec/consensus_diff.exe" "$@"
''')
    wrapper.chmod(0o755)
    digest = hashlib.sha256(executable.read_bytes()).hexdigest()
    (prefix / "README.md").write_text(f'''# OCaml 4.0.4 consensus differential oracle

Source commit: `c9e2eac5344da621828c1e96c074b8bdce665b51`.
Annotated release tag object: `e09d43035917b1e4723935cec48fd0957c660dbb`.
Executable SHA-256: `{digest}`.

The adapter invokes upstream functions; it adds a JSON hook to the consensus
module interface without modifying protocol algorithms. The complete adapter
and source-pinning checks are retained under `source/`. Runtime libraries are
bundled under `lib/`; system glibc and the dynamic loader remain prerequisites.

Use `bin/consensus_diff` with `MINA_PROFILE=mainnet` or `MINA_PROFILE=devnet`
and a private `TMPDIR`. Requests and responses are one JSON object per line.
No node is started. Only synthetic scalar identifiers are accepted; no key file
is read. The bundle does not establish mainnet producer readiness.

## Rebuild

Start from the retained OCaml 4.0.3 build environment described in the
uptime-tools and prover-div bundles. Copy the archived source, opam compiler,
native build prefix and Go capnp includes into private scratch. Relocate
build-env.sh, findlib.conf and RocksDB dependency paths; restore the compiler
command symlinks to their .opt siblings. Correct the cached dune-build-root to
the new build directory, or remove the copied _build and rebuild it. Keep the
archive unchanged. Overlay an exact git archive of the source commit above,
remove obsolete source files, and retain the matching external proof-systems
and vendored dependencies. Initialize Git metadata at that commit. Keep the
existing no-op formatter for generated bindings.

The retained OCaml compiler is 4.14.2. As in the 4.0.4 replay tools, this build
uses the retained 4.0.3 dependency versions of alcotest/alcotest-async, fmt and
graphql_parser. It is a pinned source build in a documented environment, not an
official release binary. The source guard checks both consensus files before
instrumentation. The diff hook does not alter a protocol function.

From a mina-rust checkout, with BUILD set to the private environment:

```sh
python3 -B tools/consensus-diff/prepare-ocaml.py "$BUILD/mina-source"
nice -n 10 env CARGO_BUILD_JOBS=8 CARGO_PROFILE_DEV_DEBUG=0 \\
  CARGO_INCREMENTAL=0 tools/regression/lock.sh build -- \\
  bash "$BUILD/build-capnp.sh"
nice -n 10 env CARGO_BUILD_JOBS=8 CARGO_PROFILE_DEV_DEBUG=0 \\
  CARGO_INCREMENTAL=0 tools/regression/lock.sh build -- \\
  bash "$BUILD/build-rocksdb.sh"
```

Use the original build.sh environment setup, private TMPDIR, stack limit 65532,
and file limit 10240. Its final command must be:

```sh
dune build -j 8 --profile dev src/app/consensus_diff/consensus_diff.exe
```

Run that script through the same nice/env/build-lock prefix. Install the output
with tools/consensus-diff/install.py, providing the executable, the retained
4.0.4 tools bundle's runtime lib directory and a new prefix. Check ldd with the new
lib directory, verify SHA256SUMS, then run the differential runner before
removing the private build trees. The installed bundle is the durable tool.
''')
    files = sorted(p for p in prefix.rglob("*") if p.is_file())
    (prefix / "SHA256SUMS").write_text("".join(
        hashlib.sha256(p.read_bytes()).hexdigest()+"  "+str(p.relative_to(prefix))+"\n" for p in files))
    print("Executable SHA-256:", digest)


if __name__ == "__main__":
    main()
