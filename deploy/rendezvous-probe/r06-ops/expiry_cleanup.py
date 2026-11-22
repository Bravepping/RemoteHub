"""Dedicated R06 expiry guard. Installation/execution requires a new explicit authorization."""
import json
import os
import pathlib
import re
import stat
import subprocess
import sys
import time

SERVICES = ['remotehub-p007b-r06-wss.service', 'remotehub-turn-g6.service', 'remotehub-p007b-r06-observer.service']
SECRET_FILES = ['server.json', 'client-private.json', 'server-key.pem', 'ca-key.pem', 'server.csr', 'ca.srl',
                'server.ext', 'observer-private.json', 'observer-next-private.json', 'control-private.pcap',
                'capture-stderr-private.txt', 'observer-stderr-private.txt', 'observer-stdout-private.txt', 'baseline-private.json']


def checked_directory(value):
    path = pathlib.Path(value)
    if path.parent != pathlib.Path('/run') or not re.fullmatch(r'remotehub-p007b-r06-\d{8}T\d{6}Z', path.name):
        raise ValueError('R06_SCOPE_DIRECTORY')
    if path.is_symlink() or path.resolve() != path:
        raise ValueError('R06_SCOPE_SYMLINK')
    meta = path.stat()
    if not stat.S_ISDIR(meta.st_mode) or meta.st_uid != 0 or meta.st_mode & 0o027:
        raise ValueError('R06_SCOPE_OWNER_OR_MODE')
    return path


def command(args):
    return subprocess.run(args, capture_output=True, text=True, timeout=3)


def stopped(name, call):
    p = call(['systemctl', 'show', name, '-p', 'MainPID', '-p', 'ActiveState'])
    fields = dict(s.split('=', 1) for s in p.stdout.splitlines() if '=' in s)
    return fields.get('MainPID') == '0' and fields.get('ActiveState') in ['inactive', 'failed']


def cleanup(folder, *, reason, call=command, clock=time.monotonic, sleep=time.sleep):
    # Scope was checked by the CLI; tests inject a new temporary directory and fake runner.
    if reason not in ['deadline', 'manual', 'budget', 'error']:
        raise ValueError('R06_STOP_REASON')
    result = dict(version=1, reason=reason, requestedAtNs=time.time_ns(), serviceStopIsNotAllocationReleaseEvidence=True,
                  allocationReleaseVerdict='not_inferred_by_cleanup', forceKillRequested=False, secretsRemoved=[], failures=[])
    (folder / 'stop-observer').touch()
    # Request stop before inspecting/importing any optional observer/classifier module.
    for name in SERVICES:
        try:
            p = call(['systemctl', 'stop', '--no-block', name])
            # A transient unit may already have been collected (or never started).
            # Its confirmed inactive/PID 0 state is a valid idempotent stop.
            if p.returncode and not stopped(name, call):
                result['failures'].append('DEDICATED_STOP_REQUEST_FAILED')
        except Exception:
            result['failures'].append('DEDICATED_STOP_REQUEST_FAILED')
    until = clock() + 5
    while clock() < until:
        try:
            if all(stopped(name, call) for name in SERVICES):
                break
        except Exception:
            pass
        sleep(.2)
    for name in SERVICES:
        try:
            already_stopped = stopped(name, call)
        except Exception:
            already_stopped = False
            result['failures'].append('DEDICATED_STATE_QUERY_FAILED')
        if not already_stopped:
            result['forceKillRequested'] = True
            try:
                call(['systemctl', 'kill', '--kill-whom=all', '--signal=SIGKILL', name])
            except Exception:
                result['failures'].append('DEDICATED_FORCE_STOP_FAILED')
    for name in SECRET_FILES:
        try:
            p = folder / name
            if p.is_symlink() or (p.exists() and not p.is_file()):
                result['failures'].append('PRIVATE_FILE_TYPE_REJECTED')
                continue
            if p.exists():
                p.unlink()
            result['secretsRemoved'].append(name)
        except Exception:
            result['failures'].append('TEMPORARY_PRIVATE_FILE_DELETE_FAILED')
    states = {}
    for name in SERVICES:
        try:
            states[name] = stopped(name, call)
        except Exception:
            states[name] = False
    result.update(finishedAtNs=time.time_ns(), dedicatedServicesStopped=states,
                  temporarySecretFilesAbsent=all(not (folder/n).exists() for n in SECRET_FILES))
    # Retain a bounded non-secret receipt; no raw command output or exception text.
    (folder / 'cleanup-receipt.json').write_text(json.dumps(result, indent=2) + '\n')
    return result


def main():
    if len(sys.argv) != 4 or sys.argv[1] != '--execute-after-authorization' or sys.argv[3] not in ['deadline', 'manual', 'budget', 'error']:
        raise ValueError('R06_AUTHORIZED_EXECUTION_ARGUMENTS_REQUIRED')
    if os.name != 'posix' or os.geteuid() != 0:
        raise ValueError('R06_LINUX_ROOT_GUARD_REQUIRED')
    folder = checked_directory(sys.argv[2])
    result = cleanup(folder, reason=sys.argv[3])
    print(json.dumps(result))
    if result['failures'] or not all(result['dedicatedServicesStopped'].values()) or not result['temporarySecretFilesAbsent']:
        raise ValueError('R06_DEDICATED_CLEANUP_INCOMPLETE')


if __name__ == '__main__':
    try:
        main()
    except Exception:
        raise SystemExit('R06_CLEANUP_FAILED_NO_PRIVATE_ECHO') from None
