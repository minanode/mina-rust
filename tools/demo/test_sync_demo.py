#!/usr/bin/env python3
"""Runner lifecycle tests use a local HTTP stub, never a public network node."""

import importlib.util
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent


class DemoTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="sync-demo-test-")
        self.root = Path(self.temp.name)
        self.fake = self.root / "mina-stub"
        # Endpoint shape comes from the checked-in real-node status fixture.
        fixture = REPO / "crates/cli/tests/fixtures/status/relay2-status.json"
        self.fake.write_text('''#!/usr/bin/env python3
import json, os, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
if sys.argv[1] == '--version':
    print('mina test'); sys.exit()
if sys.argv[1] == 'build-info':
    print('Commit SHA: ' + 'a' * 40); sys.exit()
assert 'MINA_PRODUCER_KEY' not in os.environ
assert 'MINA_NO_DASHBOARD' not in os.environ
Path(__file__).with_suffix('.pid').write_text(str(os.getpid()))
Path(__file__).with_suffix('.home').write_text(os.environ['HOME'])
start = time.monotonic()
class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_GET(self):
        status = json.loads(Path(FIXTURE).read_text())
        phase = 'Bootstrap' if time.monotonic() - start < 1 else 'Synced'
        status['transition_frontier']['sync']['phase'] = phase
        body = json.dumps(status if self.path == '/status' else {'commit': 'a' * 40}).encode()
        self.send_response(200); self.end_headers(); self.wfile.write(body)
HTTPServer(('127.0.0.1', int(sys.argv[sys.argv.index('--port') + 1])), Handler).serve_forever()
'''.replace('FIXTURE', repr(str(fixture))))
        self.fake.chmod(0o755)
        self.ports = []
        for _ in range(2):
            with socket.socket() as s:
                s.bind(("127.0.0.1", 0))
                self.ports.append(s.getsockname()[1])

    def tearDown(self):
        self.temp.cleanup()

    def command(self, output="result"):
        return [str(HERE / "fresh-sync.sh"), "--binary", str(self.fake),
                "--port", str(self.ports[0]), "--libp2p-port", str(self.ports[1]),
                "--temp-root", str(self.root), "--output", str(self.root / output), "--timeout", "10"]

    def assert_clean(self):
        self.assertFalse(list(self.root.glob("mina-fresh-sync-*")))
        pid_file = self.fake.with_suffix(".pid")
        if pid_file.exists():
            with self.assertRaises(ProcessLookupError):
                os.kill(int(pid_file.read_text()), 0)

    def test_success_and_environment_isolation(self):
        env = dict(os.environ, MINA_PRODUCER_KEY="must-not-be-used", MINA_NO_DASHBOARD="true")
        run = subprocess.run(self.command(), env=env, capture_output=True, text=True, timeout=15)
        self.assertEqual(run.returncode, 0, run.stderr)
        row = json.loads((self.root / "result/result.json").read_text())
        self.assertEqual(row["outcome"], "synced")
        self.assertEqual(row["height"], 564635)
        self.assertGreater(row["elapsed_s"], 1)
        self.assertIn("Bootstrap", row["phase_seconds"])
        self.assertEqual(run.stdout.count("Synced devnet at height"), 1)
        self.assert_clean()

    def test_timeout_cleans_up(self):
        run = subprocess.run(self.command() + ["--timeout", "0.2"], capture_output=True, timeout=15)
        self.assertNotEqual(run.returncode, 0)
        self.assertEqual(json.loads((self.root / "result/result.json").read_text())["outcome"], "failed")
        self.assert_clean()

    def test_keep_sigterm_cleans_up(self):
        self.keep_signal(signal.SIGTERM)

    def test_keep_sigint_cleans_up(self):
        self.keep_signal(signal.SIGINT)

    def keep_signal(self, sig):
        run = subprocess.Popen(self.command() + ["--keep"], stdout=subprocess.DEVNULL)
        try:
            deadline = time.monotonic() + 10
            while not (self.root / "result/result.json").exists() and time.monotonic() < deadline:
                time.sleep(.1)
            self.assertIsNone(run.poll())
            run.send_signal(sig)
            self.assertEqual(run.wait(timeout=10), 0)
        finally:
            if run.poll() is None:
                run.kill(); run.wait()
        self.assert_clean()

    def mock_docker(self, mode):
        """Model daemon state independently of a client that exits with code 7."""
        docker = self.root / "docker"
        docker.write_text(r'''#!/usr/bin/env python3
import json, sys
from pathlib import Path
root = Path(__file__).parent
mode = MODE
args = sys.argv[1:]
record = root / 'container.json'
if args[:2] == ['image', 'inspect']:
    print('sha256:test'); sys.exit()
if args[0] == 'run' and '--name' not in args:
    print('mina test' if args[-1] == '--version' else 'Commit SHA: ' + 'a' * 40)
    sys.exit()
if args[0] == 'run':
    name = args[args.index('--name') + 1]
    mount = args[args.index('--mount') + 1]
    data = mount.split('source=', 1)[1].split(',target=', 1)[0]
    (Path(data) / 'daemon-owned-data').write_text('must survive until shutdown is confirmed')
    record.write_text(json.dumps(dict(name=name, data=data, running=True, removed=False)))
    sys.exit(7)
state = json.loads(record.read_text())
if args[0] == 'top': sys.exit()
assert args[-1] == state['name'], args
with (root / 'docker-calls.jsonl').open('a') as log:
    log.write(json.dumps(dict(command=args[0], data_exists=Path(state['data']).exists())) + '\n')
if mode == 'unavailable':
    print('Cannot connect to the Docker daemon', file=sys.stderr); sys.exit(1)
if args[0] == 'stop':
    if mode != 'running': state['running'] = False
elif args[0] == 'rm':
    if not state['running'] and mode != 'stopped': state['removed'] = True
elif args[0] == 'inspect':
    if state['removed']:
        print('Error: No such container: ' + state['name'], file=sys.stderr); sys.exit(1)
    print('true' if state['running'] else 'false'); sys.exit()
else:
    raise AssertionError(args)
record.write_text(json.dumps(state))
'''.replace('MODE', repr(mode)))
        docker.chmod(0o755)
        command = self.command()
        index = command.index("--binary")
        command[index:index + 2] = ["--docker", "test-image"]
        env = dict(os.environ, PATH=str(self.root) + os.pathsep + os.environ["PATH"])
        run = subprocess.run(command, env=env, capture_output=True, text=True, timeout=15)
        self.assertNotEqual(run.returncode, 0, run.stdout + run.stderr)
        calls = [json.loads(line) for line in (self.root / "docker-calls.jsonl").read_text().splitlines()]
        self.assertEqual([c["command"] for c in calls], ["stop", "rm", "inspect"])
        self.assertTrue(all(c["data_exists"] for c in calls))
        state = json.loads((self.root / "container.json").read_text())
        return run, state

    def test_docker_exited_client_still_stops_and_confirms_cleanup(self):
        run, state = self.mock_docker("removed")
        self.assertTrue(state["removed"])
        self.assertNotIn("shutdown unconfirmed", run.stderr)
        self.assert_clean()

    def test_docker_inspect_stopped_allows_data_cleanup(self):
        run, state = self.mock_docker("stopped")
        self.assertFalse(state["running"])
        self.assertNotIn("shutdown unconfirmed", run.stderr)
        self.assert_clean()

    def test_docker_daemon_unavailable_retains_data(self):
        run, state = self.mock_docker("unavailable")
        self.assertTrue((Path(state["data"]) / "daemon-owned-data").exists())
        self.assertIn("data retained", run.stderr)
        self.assertIn("Cannot connect to the Docker daemon", run.stderr)

    def test_docker_still_running_retains_data(self):
        run, state = self.mock_docker("running")
        self.assertTrue((Path(state["data"]) / "daemon-owned-data").exists())
        self.assertIn("shutdown unconfirmed", run.stderr)
        self.assertIn("docker inspect: true", run.stderr)

    def test_trickling_status_obeys_total_deadline(self):
        source = self.fake.read_text().replace(
            "self.send_response(200); self.end_headers(); self.wfile.write(body)",
            "self.send_response(200); self.end_headers()\n"
            "        while True:\n"
            "            self.wfile.write(b' '); self.wfile.flush(); time.sleep(.2)",
        )
        self.fake.write_text(source)
        run = subprocess.Popen(self.command() + ["--timeout", "1"],
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            try:
                stdout, stderr = run.communicate(timeout=4)
            except subprocess.TimeoutExpired:
                run.terminate()
                run.communicate(timeout=10)
                self.fail("a trickling response bypassed the one-second sync deadline")
            self.assertNotEqual(run.returncode, 0, stdout + stderr)
            self.assertIn("sync timed out", stderr)
            self.assertNotIn("Synced devnet at height", stdout)
        finally:
            if run.poll() is None:
                run.terminate(); run.communicate(timeout=10)
        self.assert_clean()

    def test_oversized_synced_response_is_rejected(self):
        source = self.fake.read_text().replace(
            "phase = 'Bootstrap' if time.monotonic() - start < 1 else 'Synced'", "phase = 'Synced'",
        ).replace("self.wfile.write(body)", "self.wfile.write(b' ' * (4 * 1024 * 1024) + body)")
        self.fake.write_text(source)
        run = subprocess.run(self.command() + ["--timeout", "1.5"],
                             capture_output=True, text=True, timeout=10)
        self.assertNotEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertIn("sync timed out", run.stderr)
        self.assertNotIn("Synced devnet at height", run.stdout)
        self.assert_clean()

    def patched_runner(self, patch, extra):
        # Run patches in a child so the runner's signal handlers stay isolated.
        launcher = self.root / "patched-runner.py"
        launcher.write_text(
            "import importlib.util, json, time\nfrom pathlib import Path\n"
            f"spec = importlib.util.spec_from_file_location('runner', {str(HERE / 'sync-run.py')!r})\n"
            "runner = importlib.util.module_from_spec(spec)\nspec.loader.exec_module(runner)\n"
            + patch + "\nraise SystemExit(runner.main())\n"
        )
        return subprocess.run([sys.executable, str(launcher), *self.command()[1:], *extra],
                              capture_output=True, text=True, timeout=10)

    def test_synced_after_overall_deadline_is_not_accepted(self):
        fixture = REPO / "crates/cli/tests/fixtures/status/relay2-status.json"
        run = self.patched_runner(
            "def late_status(*args):\n"
            "    time.sleep(.2)\n"
            f"    return json.loads(Path({str(fixture)!r}).read_text())\n"
            "runner.get_json = late_status\n", ["--timeout", "0.1"],
        )
        self.assertNotEqual(run.returncode, 0)
        self.assertIn("sync timed out", run.stderr)
        self.assertFalse((self.root / "result/synced-status.json").exists())
        self.assert_clean()

    def test_keep_rechecks_output_disk_floor(self):
        run = self.patched_runner(
            "original = runner.check_disk\n"
            "def disk(path, floor):\n"
            f"    if Path(path) == Path({str(self.root / 'result')!r}) and (Path(path) / 'result.json').exists():\n"
            "        raise RuntimeError('disk floor reached on evidence filesystem')\n"
            "    return original(path, floor)\n"
            "runner.check_disk = disk\n", ["--keep"],
        )
        self.assertNotEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertIn("Synced devnet at height", run.stdout)
        self.assertIn("disk floor reached on evidence filesystem", run.stderr)
        self.assert_clean()

    def test_missing_temp_root_parent_is_created(self):
        root = self.root / "new" / "nested"
        run = subprocess.run(self.command() + ["--temp-root", str(root)],
                             capture_output=True, text=True, timeout=10)
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertTrue(root.is_dir())
        self.assertFalse(list(root.glob("mina-fresh-sync-*")))
        self.assert_clean()

    def test_existing_evidence_is_not_overwritten(self):
        output = self.root / "result"
        output.mkdir()
        (output / "result.json").write_text("keep this")
        run = subprocess.run(self.command(), capture_output=True, timeout=15)
        self.assertNotEqual(run.returncode, 0)
        self.assertEqual((output / "result.json").read_text(), "keep this")
        self.assert_clean()

    def test_occupied_http_port_is_not_measured(self):
        with socket.socket() as listener:
            listener.bind(("0.0.0.0", self.ports[0]))
            listener.listen()
            run = subprocess.run(self.command(), capture_output=True, timeout=15)
        self.assertNotEqual(run.returncode, 0)
        self.assertFalse((self.root / "result").exists())
        self.assert_clean()

    def test_matrix_reuses_ports_after_cleanup(self):
        command = ["python3", str(HERE / "sync-matrix.py"), "--runs", "2",
                   "--binary", str(self.fake), "--port", str(self.ports[0]),
                   "--libp2p-port", str(self.ports[1]), "--temp-root", str(self.root),
                   "--output", str(self.root / "matrix"), "--timeout", "10"]
        run = subprocess.run(command, capture_output=True, text=True, timeout=30)
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        rows = [json.loads(line) for line in (self.root / "matrix/results.jsonl").read_text().splitlines()]
        self.assertEqual([r["outcome"] for r in rows], ["synced", "synced"])
        self.assert_clean()

    def test_summary_excludes_failures_and_marks_missing_phase(self):
        spec = importlib.util.spec_from_file_location("matrix", HERE / "sync-matrix.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        rows = [{"outcome": "synced", "elapsed_s": t, "phase_seconds": {"Bootstrap": t}}
                for t in [10, 20, 30, 40, 50]] + [{"outcome": "failed", "elapsed_s": 900}]
        path = self.root / "summary.md"
        module.summarize(rows, path)
        summary = path.read_text()
        self.assertIn("Time to Synced (s) | 30.00 | 50.00 | 5", summary)
        self.assertIn("Catchup (s) | unavailable | unavailable | 0", summary)


if __name__ == "__main__":
    unittest.main()
