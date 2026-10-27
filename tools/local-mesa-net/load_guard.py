#!/usr/bin/env python3
"""Pause only this isolated experiment's proof processes under host contention."""
import argparse
import datetime
import json
import os
from pathlib import Path
import shutil
import signal
import time

from processes import snapshot


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dir', type=Path, required=True)
    parser.add_argument('--max-load', type=float, default=32)
    parser.add_argument('--resume-load', type=float, default=24)
    args = parser.parse_args()
    work = args.dir.resolve()
    namespace = json.loads((work/'namespace.json').read_text())
    assert namespace['netns'] != os.readlink('/proc/self/ns/net'), 'guard must never target its host namespace'
    (work/'load-guard.pid').write_text(str(os.getpid())+'\n')
    owned = {}
    stopped = set()
    paused = False
    previous_cpu = None

    def in_namespace(pid):
        try:
            return os.readlink(f'/proc/{pid}/ns/net') == namespace['netns']
        except OSError:
            return False

    def valid(pid, current):
        return current.get(pid, {}).get('start') == owned[pid] and in_namespace(pid)

    with (work/'load-guard.jsonl').open('a', buffering=1) as events, \
            (work/'host-resources.jsonl').open('a', buffering=1) as resources:
        try:
            while in_namespace(namespace['pid']):
                current = snapshot()
                rust_pid = int((work/'rust.pid').read_text()) if (work/'rust.pid').exists() else None
                # Inspect command lines only inside our private namespace.
                # This includes the daemon's worker and external workers.
                for pid, row in current.items():
                    if not in_namespace(pid):
                        continue
                    try:
                        argv = Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0')
                    except OSError:
                        continue
                    if pid == rust_pid or b'snark-worker' in argv:
                        owned.setdefault(pid, row['start'])
                now = datetime.datetime.now(datetime.timezone.utc).isoformat()
                load = os.getloadavg()
                if max(load) > args.max_load:
                    paused = True
                elif load[0] < args.resume_load and max(load) < args.max_load:
                    paused = False
                cpu = [int(x) for x in Path('/proc/stat').read_text().splitlines()[0].split()[1:9]]
                cpu_pair = (sum(cpu), cpu[3]+cpu[4])
                busy = None
                if previous_cpu and cpu_pair[0] > previous_cpu[0]:
                    busy = 100*(1-(cpu_pair[1]-previous_cpu[1])/(cpu_pair[0]-previous_cpu[0]))
                previous_cpu = cpu_pair
                resources.write(json.dumps({'time':now, 'load':load,
                    'free_bytes':shutil.disk_usage(work).free, 'cpu_busy_pct':busy})+'\n')
                state = {'time':now, 'paused':paused, 'load':load}
                temporary = work/'load-guard-state.tmp'
                temporary.write_text(json.dumps(state)+'\n')
                temporary.replace(work/'load-guard-state.json')
                manual = (work/'manual-producer-pause.json').exists()
                changed = {'pause':[], 'resume':[]}
                for pid in owned:
                    if not valid(pid, current):
                        continue
                    # The supervisor owns deliberate test pauses; never end
                    # one early because host load has fallen.
                    if pid == rust_pid and manual:
                        continue
                    try:
                        status = Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()[0]
                        if paused and status not in ('T','t'):
                            os.kill(pid, signal.SIGSTOP)
                            stopped.add(pid)
                            changed['pause'].append(pid)
                        elif not paused and pid in stopped:
                            os.kill(pid, signal.SIGCONT)
                            stopped.remove(pid)
                            changed['resume'].append(pid)
                    except (FileNotFoundError, ProcessLookupError):
                        pass
                for action, pids in changed.items():
                    if pids:
                        events.write(json.dumps({'time':now, 'action':action,
                            'pids':pids, 'load':load,
                            'reason':'owned proof processes wait above load 32; resume below 24 with all averages below 32'})+'\n')
                time.sleep(10)
        finally:
            current = snapshot()
            for pid in stopped:
                if valid(pid, current) and not (pid == rust_pid and (work/'manual-producer-pause.json').exists()):
                    try:
                        os.kill(pid, signal.SIGCONT)
                    except ProcessLookupError:
                        pass


if __name__ == '__main__':
    main()
