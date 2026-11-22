"""Run only the isolated Windows identity example, never the desktop or production store."""
import datetime
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import uuid

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT/'artifacts/p201'


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT/'native-restart.json').write_text(json.dumps(dict(passed=False,state='running'))+'\n',encoding='utf-8')
    build = subprocess.run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'scripts/p201-native.ps1'], cwd=ROOT, capture_output=True)
    log = (build.stdout+build.stderr).decode('utf-8', errors='replace').replace('\r\n', '\n').replace('\r', '\n')
    (OUT/'identity-final-native.txt').write_text(log+'\nEXIT='+str(build.returncode)+'\n', encoding='utf-8', newline='\n')
    if build.returncode: raise ValueError('IDENTITY_NATIVE_BUILD_FAILED')
    private = ROOT/'artifacts/private/p201'
    if not private.parent.resolve().is_relative_to(ROOT.resolve()):
        raise ValueError('IDENTITY_TEST_BASE_REJECTED')
    private.mkdir(parents=True, exist_ok=True)
    if private.resolve().parent != (ROOT/'artifacts/private').resolve() or not private.resolve().is_relative_to(ROOT.resolve()):
        raise ValueError('IDENTITY_TEST_BASE_REJECTED')
    work = private/('run-'+uuid.uuid4().hex)
    exe = ROOT/'target/probe/examples/restart_probe.exe'
    results = []
    before = None
    completed = False
    unchanged = False
    try:
        for stage in ['prepare', 'restore']:
            started = datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat()
            run = subprocess.run([str(exe), stage, str(work)], cwd=ROOT, capture_output=True, timeout=20)
            # Only accept fixed structured stdout. Do not print native paths, identifiers or blobs.
            try:
                native = json.loads(run.stdout)
                if (not isinstance(native, dict) or set(native) != {'stage','passed','identityOrKeyPrinted','processId'}
                    or native['stage'] != stage or native['passed'] is not True or native['identityOrKeyPrinted'] is not False
                    or type(native['processId']) is not int or native['processId'] <= 0): native = None
            except (ValueError, UnicodeError): native = None
            results.append(dict(stage=stage, startedAt=started, exitCode=run.returncode, native=native))
            if run.returncode or not native or native.get('passed') is not True: raise ValueError('IDENTITY_NATIVE_STAGE_FAILED')
            digest = hashlib.sha256((work/'identity.dpapi').read_bytes()).digest()
            if before is None: before = digest
            else:
                if before != digest: raise ValueError('IDENTITY_CIPHERTEXT_CHANGED')
                unchanged = True
        completed = True
    finally:
        if work.exists():
            resolved = work.resolve()
            if resolved.parent != private.resolve() or not resolved.is_relative_to(ROOT.resolve()) or resolved.name != work.name or not resolved.name.startswith('run-'):
                raise ValueError('IDENTITY_TEST_CLEANUP_REJECTED')
            shutil.rmtree(resolved)
        result = dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
                      scope='Real Windows CurrentUser DPAPI, two separate processes, isolated synthetic identity',
                      results=results, passed=completed and unchanged and not work.exists(),
                      programSha256=hashlib.sha256(exe.read_bytes()).hexdigest(), ciphertextUnchanged=unchanged,
                      temporaryStoreRemoved=not work.exists(), productionIdentityDirectoryAccessed=False,
                      guiStarted=False, desktopCapture=False, remoteInput=False, cloud=False,
                      crossWindowsAccountTest='NOT_RUN', crossMachineTest='NOT_RUN')
        (OUT/'native-restart.json').write_text(json.dumps(result, indent=2)+'\n', encoding='utf-8', newline='\n')
    if not result['passed']: raise ValueError('IDENTITY_NATIVE_TEST_FAILED')
    print(json.dumps(dict(passed=True, separateProcesses=2, stablePublicKeyAndSignature=True, storeRemoved=result['temporaryStoreRemoved'])))


if __name__ == '__main__':
    try: main()
    except Exception:
        raise SystemExit('IDENTITY_NATIVE_FAILED_NO_PRIVATE_ECHO') from None
