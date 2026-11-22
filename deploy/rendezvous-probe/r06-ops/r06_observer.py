"""Scoped wrapper for the already-reviewed TURN observer. Not executed before authorization."""
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import threading
import time
import uuid
from expiry_cleanup import checked_directory

HELPERS = '/opt/remotehub-p006c-manual-20261122'
EXPECTED = {
    'turn_lifecycle_observer.py': '73b637f0be32e8c09e52e5f3269fc86d75bf87811d9872bd3fe53e05cdc5ea9f',
    'turn_lifecycle_analyze.py': 'c5e3e4f229224fdfe5658234e5ed0d659af4ff3df26309f6709c643377916074',
    'g6_turn_wire.py': '4cf97b751a1e01fdab2bbfef88cb1494214cddd320b1106ce40636e2f31045e5',
}


def load_helpers():
    for name, expected in EXPECTED.items():
        p = pathlib.Path(HELPERS) / name
        if p.is_symlink() or p.stat().st_uid != 0 or p.stat().st_mode & 0o022 or hashlib.sha256(p.read_bytes()).hexdigest() != expected:
            raise ValueError('R06_OBSERVER_HELPER_MISMATCH')
    sys.path.insert(0, HELPERS)
    from turn_lifecycle_observer import run, read_capture
    return run, read_capture


def remaining_window(window, now):
    keys = ['authorizationStart', 'authorizationDeadline', 'guardTarget', 'lastNewAllocationAdmission']
    if any(type(window.get(k)) is not int for k in keys) or type(now) is not int:
        raise ValueError('R06_OBSERVER_WINDOW')
    start = window['authorizationStart']
    remaining = window['guardTarget'] - now
    if (window['authorizationDeadline'] != start + 1200 or window['guardTarget'] != start + 1140
            or window['lastNewAllocationAdmission'] != start + 480 or not 30 <= remaining <= 1140):
        raise ValueError('R06_OBSERVER_WINDOW')
    return remaining


def observe(folder):
    window = json.loads((folder/'window.json').read_text())
    remaining = remaining_window(window, int(time.time()))
    run, read_capture = load_helpers()
    config = folder/'observer-private.json'
    done = threading.Event(); errors = []

    def discover():
        try:
            while not done.is_set():
                data = json.loads(config.read_text()); changed = False
                for ns, kind, _, values, _, _ in read_capture(folder/'control-private.pcap'):
                    if kind not in [3, 4]: continue
                    user = values.get(6, (b'', 0))[0].decode('ascii', errors='replace')
                    parts = user.split(':')
                    if len(parts) != 2 or not parts[0].isdigit(): continue
                    try: uuid.UUID(parts[1])
                    except ValueError: continue
                    if not window['authorizationStart']-5 <= int(parts[0]) <= window['authorizationDeadline']+600: continue
                    if user not in data['labels']:
                        if ns // 10**9 > window['lastNewAllocationAdmission']:
                            raise ValueError('R06_ADMISSION_WINDOW_EXCEEDED')
                        if len(data['labels']) >= 2: raise ValueError('R06_ROUND_BUDGET_EXCEEDED')
                        data['labels'][user] = 'r06-session-' + str(len(data['labels'])+1); changed = True
                if changed:
                    p = folder/'observer-next-private.json'; p.write_text(json.dumps(data)); p.chmod(0o600); os.replace(p, config)
                done.wait(.25)
        except Exception:
            errors.append('R06_LABEL_DISCOVERY_FAILED')
            (folder/'stop-observer').touch()

    worker = threading.Thread(target=discover); worker.start()
    try:
        run(config, folder, remaining, max_allocations=4, split_allocations=True)
        if errors: raise ValueError('R06_OBSERVER_FAILED')
    finally:
        done.set(); worker.join(timeout=3)


def main(args=None, *, root_allowed=None, validate=checked_directory, observe_run=observe, stop_call=subprocess.run):
    args = sys.argv[1:] if args is None else args
    scope = None
    try:
        allowed = os.name == 'posix' and os.geteuid() == 0 if root_allowed is None else root_allowed
        if len(args)!=2 or args[0]!='--execute-after-authorization' or not allowed:
            raise ValueError('R06_OBSERVER_AUTHORIZATION_ARGUMENTS')
        scope = validate(args[1])
        observe_run(scope)
    except Exception:
        # Stop just the media services on observer failure; the independent expiry guard cleans secrets.
        # Invalid arguments or an untrusted directory never authorize service actions.
        if scope is not None:
            for name in ['remotehub-p007b-r06-wss.service','remotehub-turn-g6.service']:
                try: stop_call(['systemctl','stop','--no-block',name],capture_output=True,timeout=3)
                except Exception: pass
        raise SystemExit('R06_OBSERVER_FAILED_NO_PRIVATE_ECHO') from None


if __name__=='__main__': main()
