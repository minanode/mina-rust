"""Read OCaml Mina's mainnet protocol constants into the P11d fixture.

Writes tests/files/ocaml/mainnet-4.0.2-constants.json, which the tests
ocaml_mainnet_constants.rs in crates/core, crates/ledger and crates/node
compare with the constants mina-rust hard-codes. Every constant is a list of
observations, each a value and how it was read, from these sources:

- the OCaml binary, through scripts/ocaml-mina: `version`,
  `advanced compile-time-constants`, `advanced constraint-system-digests`,
  `advanced print-signature-kind` and `internal chain-id`;
- the chain id input, traced with gdb by scripts/ocaml-chain-id-trace.sh: the
  fork-aware blockchain-step digest, the genesis state hash the chain id hashes
  and the protocol versions (skip with --no-trace; the tests then fail, as
  those values are missing);
- the runtime config the 4.0.2 deb ships (--config): the genesis timestamp,
  the fork point and the network name;
- the OCaml source at the binary's commit, fetched from GitHub (or read from
  --source-dir, recorded as such): the profile's values the binary does not
  print, such as the ledger depth, fees and zkApp limits;
- optionally, a live OCaml node's GraphQL (--graphql URL, only if it runs the
  binary's commit): genesisConstants and the consensus configuration;
- optionally, a precomputed Mesa mainnet block (--block FILE, .json or
  .json.zst): the protocol constants and protocol version OCaml put in it.

Mainnet only. Devnet's constants are being changed by P1; a devnet fixture
would run this with MINA_PROFILE=devnet and the devnet config.

Every OCaml command runs with a private TMPDIR, removed afterwards.

usage:
  python3 -I scripts/ocaml-constants-fixture.py \\
    [--config ~/.local/opt/mina-4.0.2/var/lib/coda/config_84b8d969.json] \\
    [--graphql https://mina-mainnet-graphql.aurowallet.com/graphql] \\
    [--block $ROOT/.work/replay/blocks/548148-<hash>.json.zst] \\
    [--source-dir $ROOT/repos/mina] [--no-trace] [--out FILE]
"""

import argparse
import datetime
import decimal
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
OCAML_MINA = os.path.join(HERE, "ocaml-mina")
TRACE = os.path.join(HERE, "ocaml-chain-id-trace.sh")
DEFAULT_OUT = os.path.join(REPO, "tests", "files", "ocaml", "mainnet-4.0.2-constants.json")
DEFAULT_CONFIG = "~/.local/opt/mina-4.0.2/var/lib/coda/config_84b8d969.json"
RAW = "https://raw.githubusercontent.com/MinaProtocol/mina/%s/%s"
USER_AGENT = "Mozilla/5.0 (X11; Linux x86_64) mina-rust-p11d"

# The digests in the order the chain id hashes them (chain_id.ml).
DIGEST_ORDER = [
    "transaction-merge",
    "transaction-base",
    "zkapp-opt_signed-opt_signed",
    "zkapp-opt_signed",
    "zkapp-proved",
    "blockchain-step",
]

PROFILE = "src/lib/node_config/profiled/mainnet.ml"
UNCONFIGURABLE = (
    "src/lib/node_config/unconfigurable_constants/node_config_unconfigurable_constants.ml"
)
VERSION = "src/lib/node_config/version/node_config_version.ml"
GENESIS_CONSTANTS = "src/lib/genesis_constants/genesis_constants.ml"
CONSENSUS_CONSTANTS = "src/lib/consensus/constants.ml"
HASH_PREFIXES = "src/lib/hash_prefixes/hash_prefixes.ml"
SCHNORR = "src/lib/crypto/signature_lib/schnorr.ml"
CONSENSUS_VRF = "src/lib/consensus/vrf/consensus_vrf.ml"
SOURCES = [
    PROFILE,
    UNCONFIGURABLE,
    VERSION,
    GENESIS_CONSTANTS,
    CONSENSUS_CONSTANTS,
    HASH_PREFIXES,
    SCHNORR,
    CONSENSUS_VRF,
]

constants = {}


def record(name, value, how):
    constants.setdefault(name, []).append({"value": value, "how": how})


def tilde(path):
    home = os.path.expanduser("~")
    return "~" + path[len(home) :] if path.startswith(home + os.sep) else path


def nanomina(mina):
    """Currency.Amount.of_mina_string_exn: a decimal MINA string in nanomina."""
    value = decimal.Decimal(mina) * 10**9
    if value != value.to_integral_value():
        sys.exit("%r MINA is not a whole number of nanomina" % mina)
    return int(value)


def ms(text):
    """A timestamp as OCaml prints or parses it, in ms since the epoch."""
    parsed = datetime.datetime.fromisoformat(text)
    if parsed.tzinfo is None:
        sys.exit("timestamp %r has no zone" % text)
    epoch = datetime.datetime(1970, 1, 1, tzinfo=datetime.timezone.utc)
    return (parsed - epoch) // datetime.timedelta(milliseconds=1)


def run(env, *args):
    result = subprocess.run(
        [OCAML_MINA, *args], env=env, capture_output=True, text=True, timeout=1800
    )
    if result.returncode != 0:
        sys.exit(
            "mina %s failed (%d):\n%s" % (" ".join(args), result.returncode, result.stderr[-2000:])
        )
    return result.stdout


def from_binary(env, conf_dir, config):
    version = run(env, "version").strip()
    match = re.fullmatch(r"Commit ([0-9a-f]{40})", version.splitlines()[0])
    if not match:
        sys.exit("unexpected `mina version` output: %r" % version)
    commit = match.group(1)

    how = "binary: mina advanced compile-time-constants, %s"
    ctc = json.loads(run(env, "advanced", "compile-time-constants"))
    for key in ("k", "delta", "slots_per_epoch", "slots_per_sub_window", "sub_windows_per_window"):
        record(key, ctc[key], how % key)
    record("slots_per_window", ctc["slots_per_window"], how % "slots_per_window")
    record("block_window_duration_ms", ctc["block_window_duration_ms"], how % "block_window_duration_ms")
    record(
        "coinbase_amount",
        nanomina(ctc["coinbase"]),
        (how % "coinbase") + ' ("%s" MINA)' % ctc["coinbase"],
    )
    record(
        "default_genesis_state_timestamp_ms",
        ms(ctc["genesis_state_timestamp"]),
        (how % "genesis_state_timestamp") + " (%s)" % ctc["genesis_state_timestamp"],
    )

    digests = {}
    for line in run(env, "advanced", "constraint-system-digests").splitlines():
        name, digest = line.split("\t")
        digests[name] = digest
    if sorted(digests) != sorted(DIGEST_ORDER):
        sys.exit("unexpected digests: %s" % sorted(digests))
    how = "binary: mina advanced constraint-system-digests, %s"
    for name in DIGEST_ORDER[:-1]:
        record("digest." + name, digests[name], how % name)
    # Compiled without the fork constants: not the digest a forked chain's id
    # commits to.
    record(
        "digest.blockchain-step-without-fork",
        digests["blockchain-step"],
        how % "blockchain-step" + " (compiled without the fork constants)",
    )

    kind = run(env, "advanced", "print-signature-kind").strip()
    record("signature_kind", kind, "binary: mina advanced print-signature-kind")

    chain_id = run(
        env,
        "internal",
        "chain-id",
        "--config-directory",
        conf_dir,
        "--config-file",
        config,
        "--from-config-hashes-only",
    ).strip()
    record(
        "chain_id",
        chain_id,
        "binary: mina internal chain-id --from-config-hashes-only with the packaged config",
    )
    return commit, version


def from_trace(env, config):
    result = subprocess.run(
        [TRACE, "mainnet", config], env=env, capture_output=True, text=True, timeout=1800
    )
    if result.returncode != 0:
        sys.exit("ocaml-chain-id-trace.sh failed:\n%s%s" % (result.stdout, result.stderr))
    how = "gdb trace of the chain id input (scripts/ocaml-chain-id-trace.sh), %s"
    order = []
    for line in result.stdout.splitlines():
        words = line.split()
        if line.startswith("genesis_state_hash (zero currencies) "):
            record("chain_id.genesis_state_hash", words[-1], how % "genesis state hash, zero currencies")
        elif line.startswith("digest "):
            order.append(words[1])
            record("digest." + words[1], words[2], how % ("digest " + words[1]))
        elif line.startswith("genesis_constants_hash "):
            record("chain_id.genesis_constants_hash", words[1], how % "genesis constants hash")
        elif line.startswith(("transaction_version md5 ", "network_version md5 ")):
            label = words[0].split("_")[0]
            version = re.fullmatch(r"md5\((\d+)\)", words[-1])
            if not version:
                sys.exit("no version for %r" % line)
            record(
                "protocol_version." + label,
                int(version.group(1)),
                how % ("%s version md5 %s" % (label, words[2])),
            )
        elif line.startswith("chain_id ") and len(words) == 2:
            record("chain_id", words[1], how % "Blake2b-256 of the traced input")
    if order != DIGEST_ORDER:
        sys.exit("traced digest order %s, expected %s" % (order, DIGEST_ORDER))
    record("digest_order", order, how % "order of the digests in the input")


def from_config(path):
    data = open(path, "rb").read()
    config = json.loads(data)
    how = "runtime config %s (sha256 %s), %%s" % (tilde(path), hashlib.sha256(data).hexdigest()[:16])
    timestamp = config["genesis"]["genesis_state_timestamp"]
    record("genesis_state_timestamp_ms", ms(timestamp), how % ("genesis.genesis_state_timestamp (%s)" % timestamp))
    fork = config["proof"]["fork"]
    record("fork.state_hash", fork["state_hash"], how % "proof.fork.state_hash")
    record("fork.blockchain_length", fork["blockchain_length"], how % "proof.fork.blockchain_length")
    record(
        "fork.global_slot_since_genesis",
        fork["global_slot_since_genesis"],
        how % "proof.fork.global_slot_since_genesis",
    )
    record("network_name", config["daemon"]["network_id"], how % "daemon.network_id")
    # Genesis inputs rather than constants: with the constants above they give
    # the genesis state hash the chain id hashes.
    record("genesis_ledger_hash", config["ledger"]["hash"], how % "ledger.hash")
    for epoch in ("staking", "next"):
        for key in ("hash", "seed"):
            record(
                "%s_epoch_%s" % (epoch, "ledger_hash" if key == "hash" else key),
                config["epoch_data"][epoch][key],
                how % ("epoch_data.%s.%s" % (epoch, key)),
            )
    return {"path": tilde(path), "sha256": hashlib.sha256(data).hexdigest()}


def fetch(url):
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.read()


def ml_values(text):
    """`let name = value` lines of an OCaml file, values parsed when simple."""
    values = {}
    for match in re.finditer(r"^let (\w+) = (.+?)\s*$", text, re.M):
        raw = match.group(2)
        if re.fullmatch(r"\d+", raw):
            value = int(raw)
        elif re.fullmatch(r'"[^"]*"', raw):
            value = raw[1:-1]
        elif re.fullmatch(r"Some \d+", raw):
            value = int(raw.split()[1])
        elif raw == "None":
            value = None
        else:
            value = raw
        values[match.group(1)] = value
    return values


def find(pattern, text, path):
    match = re.search(pattern, text)
    if not match:
        sys.exit("%s: no match for %r; the source changed, update this script" % (path, pattern))
    return match


def from_source(commit, source_dir):
    texts, files = {}, {}
    for path in SOURCES:
        if source_dir:
            data = open(os.path.join(source_dir, path), "rb").read()
        else:
            data = fetch(RAW % (commit, path))
        texts[path] = data.decode()
        files[path] = hashlib.sha256(data).hexdigest()
    if source_dir:
        where = "OCaml source in %s (a local checkout, not compared with %s), " % (
            tilde(os.path.abspath(source_dir)),
            commit[:8],
        )
        base = tilde(os.path.abspath(source_dir))
    else:
        where = "OCaml source at %s, " % commit[:8]
        base = RAW % (commit, "")

    def how(path, what):
        return where + "%s: %s" % (path, what)

    profile = ml_values(texts[PROFILE])
    for name, key in (
        ("ledger_depth", "ledger_depth"),
        ("k", "k"),
        ("delta", "delta"),
        ("slots_per_epoch", "slots_per_epoch"),
        ("slots_per_sub_window", "slots_per_sub_window"),
        ("sub_windows_per_window", "sub_windows_per_window"),
        ("grace_period_slots", "grace_period_slots"),
        ("transaction_capacity_log_2", "scan_state_transaction_capacity_log_2"),
        ("work_delay", "scan_state_work_delay"),
        ("txpool_max_size", "pool_max_size"),
        ("supercharged_coinbase_factor", "supercharged_coinbase_factor"),
        ("block_window_duration_ms", "block_window_duration"),
        ("zkapp_cmd_limit", "zkapp_cmd_limit"),
        ("sync_ledger_max_subtree_depth", "sync_ledger_max_subtree_depth"),
        ("sync_ledger_default_subtree_depth", "sync_ledger_default_subtree_depth"),
    ):
        record(name, profile[key], how(PROFILE, key))
    for name, key in (
        ("coinbase_amount", "coinbase"),
        ("account_creation_fee", "account_creation_fee_int"),
        ("minimum_user_command_fee", "minimum_user_command_fee"),
        ("default_transaction_fee", "default_transaction_fee"),
        ("default_snark_worker_fee", "default_snark_worker_fee"),
    ):
        record(name, nanomina(profile[key]), how(PROFILE, '%s "%s" MINA' % (key, profile[key])))
    record(
        "default_genesis_state_timestamp_ms",
        ms(profile["genesis_state_timestamp"]),
        how(PROFILE, "genesis_state_timestamp %s" % profile["genesis_state_timestamp"]),
    )
    record("network_name", profile["network"], how(PROFILE, "network"))

    # pending_coinbase_depth is not in the profile: Genesis_constants derives it.
    find(
        r"pending_coinbase_depth =\s*Core_kernel\.Int\.ceil_log2\s*"
        r"\(\s*\(transaction_capacity_log_2 \+ 1\)\s*\*\s*"
        r"\(Node_config\.scan_state_work_delay \+ 1\)\s*\+ 1\s*\)",
        texts[GENESIS_CONSTANTS],
        GENESIS_CONSTANTS,
    )
    capacity = profile["scan_state_transaction_capacity_log_2"]
    delay = profile["scan_state_work_delay"]
    n = (capacity + 1) * (delay + 1) + 1
    record(
        "pending_coinbase_depth",
        (n - 1).bit_length(),
        how(GENESIS_CONSTANTS, "ceil_log2((%d + 1) * (%d + 1) + 1)" % (capacity, delay)),
    )

    unconfigurable = ml_values(texts[UNCONFIGURABLE])
    for key in (
        "max_zkapp_segment_per_transaction",
        "max_event_elements",
        "max_action_elements",
        "zkapp_cmd_limit_hardcap",
    ):
        record(key, unconfigurable[key], how(UNCONFIGURABLE, key))

    version = ml_values(texts[VERSION])
    for label in ("transaction", "network", "patch"):
        key = "protocol_version_" + label
        record("protocol_version." + label, version[key], how(VERSION, key))

    per_year = int(find(r"let per_year = (\d+) in", texts[CONSENSUS_CONSTANTS], CONSENSUS_CONSTANTS).group(1))
    record("checkpoints_per_year", per_year, how(CONSENSUS_CONSTANTS, "per_year"))
    slots_per_year = 365 * 24 * 60 * 60 * 1000 // profile["block_window_duration"]
    record(
        "checkpoint_window_size_in_slots",
        slots_per_year // per_year,
        how(CONSENSUS_CONSTANTS, "slots per year (365 days / block_window_duration) / per_year"),
    )
    record(
        "epoch_duration_ms",
        profile["slots_per_epoch"] * profile["block_window_duration"],
        how(CONSENSUS_CONSTANTS, "Epoch.duration = slot duration * slots_per_epoch"),
    )
    record(
        "delta_duration_ms",
        profile["block_window_duration"] * (profile["delta"] + 1),
        how(CONSENSUS_CONSTANTS, "delta_duration = slot duration * (delta + 1)"),
    )

    for name, key in (
        ("signature_prefix", "signature_mainnet"),
        ("zkapp_body_hash_prefix", "zkapp_body_mainnet"),
    ):
        prefix = find(r'let %s = create "([^"]*)"' % key, texts[HASH_PREFIXES], HASH_PREFIXES)
        record(name, prefix.group(1), how(HASH_PREFIXES, key))
    byte = find(
        r"let network_id_mainnet = String\.of_char @@ Char\.of_int_exn (\d+)",
        texts[SCHNORR],
        SCHNORR,
    )
    record("network_id_byte", int(byte.group(1)), how(SCHNORR, "network_id_mainnet"))
    vrf = find(r"let f = Bignum\.\(of_int (\d+) / of_int (\d+)\)", texts[CONSENSUS_VRF], CONSENSUS_VRF)
    record("vrf_threshold_f", "%s/%s" % vrf.groups(), how(CONSENSUS_VRF, "f"))
    return {"base": base, "files": files}


def from_graphql(url, commit):
    query = (
        "{ genesisConstants { accountCreationFee coinbase genesisTimestamp }"
        " daemonStatus { commitId chainId consensusConfiguration"
        " { delta k slotsPerEpoch slotDuration epochDuration genesisStateTimestamp"
        " acceptableNetworkDelay } } }"
    )
    request = urllib.request.Request(
        url,
        data=json.dumps({"query": query}).encode(),
        headers={"Content-Type": "application/json", "User-Agent": USER_AGENT},
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        data = json.load(response)["data"]
    status = data["daemonStatus"]
    if status["commitId"] != commit:
        sys.exit(
            "%s runs %s, not the binary's commit %s; leave it out" % (url, status["commitId"], commit)
        )
    when = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%MZ")
    how = "GraphQL of a live OCaml node, %s (commit %s, %s), %%s" % (url, commit[:8], when)
    genesis = data["genesisConstants"]
    record("account_creation_fee", int(genesis["accountCreationFee"]), how % "genesisConstants.accountCreationFee")
    record("coinbase_amount", int(genesis["coinbase"]), how % "genesisConstants.coinbase")
    record(
        "genesis_state_timestamp_ms",
        ms(genesis["genesisTimestamp"]),
        how % ("genesisConstants.genesisTimestamp (%s)" % genesis["genesisTimestamp"]),
    )
    consensus = status["consensusConfiguration"]
    for name, key in (
        ("k", "k"),
        ("delta", "delta"),
        ("slots_per_epoch", "slotsPerEpoch"),
        ("block_window_duration_ms", "slotDuration"),
        ("epoch_duration_ms", "epochDuration"),
        ("delta_duration_ms", "acceptableNetworkDelay"),
    ):
        record(name, consensus[key], how % ("daemonStatus.consensusConfiguration." + key))
    record(
        "genesis_state_timestamp_ms",
        ms(consensus["genesisStateTimestamp"]),
        how % ("daemonStatus.consensusConfiguration.genesisStateTimestamp (%s)" % consensus["genesisStateTimestamp"]),
    )
    record("chain_id", status["chainId"], how % "daemonStatus.chainId")
    return {"url": url, "commit": status["commitId"], "queried": when}


def from_block(path):
    if path.endswith(".zst"):
        raw = subprocess.run(["zstd", "-dc", path], capture_output=True, check=True).stdout
    else:
        raw = open(path, "rb").read()
    block = json.loads(raw)
    data = block.get("data", block)
    body = data["protocol_state"]["body"]
    consensus = body["consensus_state"]
    height = int(consensus["blockchain_length"])
    name = os.path.basename(path)
    how = "precomputed block %s (height %d), %%s" % (name, height)
    constants_ = body["constants"]
    for key in ("k", "slots_per_epoch", "slots_per_sub_window", "grace_period_slots", "delta"):
        record(key, int(constants_[key]), how % ("protocol_state.body.constants." + key))
    record(
        "genesis_state_timestamp_ms",
        int(constants_["genesis_state_timestamp"]),
        how % "protocol_state.body.constants.genesis_state_timestamp",
    )
    record(
        "slots_per_epoch",
        int(consensus["curr_global_slot_since_hard_fork"]["slots_per_epoch"]),
        how % "consensus_state.curr_global_slot_since_hard_fork.slots_per_epoch",
    )
    record(
        "sub_windows_per_window",
        len(consensus["sub_window_densities"]),
        how % "length of consensus_state.sub_window_densities",
    )
    for label in ("transaction", "network", "patch"):
        record(
            "protocol_version." + label,
            int(data["protocol_version"][label]),
            how % ("protocol_version." + label),
        )
    return {"file": name, "height": height, "sha256": hashlib.sha256(raw).hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--config", default=os.path.expanduser(DEFAULT_CONFIG))
    parser.add_argument("--source-dir")
    parser.add_argument("--graphql", action="append", default=[])
    parser.add_argument("--block")
    parser.add_argument("--no-trace", action="store_true")
    parser.add_argument("--out", default=DEFAULT_OUT)
    args = parser.parse_args()
    config = os.path.abspath(os.path.expanduser(args.config))

    base = os.environ.get("TMPDIR") or "/tmp"
    private = tempfile.mkdtemp(prefix="ocaml-constants-", dir=base)
    env = dict(os.environ, TMPDIR=private, MINA_PROFILE="mainnet")
    try:
        conf_dir = os.path.join(private, "conf")
        os.mkdir(conf_dir)
        commit, version = from_binary(env, conf_dir, config)
        if not args.no_trace:
            from_trace(env, config)
    finally:
        shutil.rmtree(private, ignore_errors=True)

    inputs = {"binary": {"command": "scripts/ocaml-mina", "version": version}}
    inputs["runtime_config"] = from_config(config)
    inputs["source"] = from_source(commit, args.source_dir)
    if args.graphql:
        inputs["graphql"] = [from_graphql(url, commit) for url in args.graphql]
    if args.block:
        inputs["block"] = from_block(args.block)

    disagreements = [
        name
        for name, observations in constants.items()
        if len({json.dumps(o["value"]) for o in observations}) > 1
    ]
    fixture = {
        "about": (
            "Mainnet protocol constants of OCaml Mina, each with how it was read. "
            "Written by scripts/ocaml-constants-fixture.py: regenerate, don't edit. "
            "Checked by ocaml_mainnet_constants.rs in crates/core/tests, "
            "crates/ledger/tests and crates/node/tests (P11d)."
        ),
        "generated": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "ocaml_commit": commit,
        "profile": "mainnet",
        "inputs": inputs,
        "constants": dict(sorted(constants.items())),
    }
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "w") as out:
        json.dump(fixture, out, indent=2)
        out.write("\n")
    print("wrote %s: %d constants, %d observations" % (
        args.out,
        len(constants),
        sum(len(o) for o in constants.values()),
    ))
    if disagreements:
        print("OCaml's sources disagree on: %s" % ", ".join(disagreements))
        sys.exit(1)


if __name__ == "__main__":
    main()
