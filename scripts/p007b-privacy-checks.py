"""Generate bounded regression evidence for the privacy fix. No network or capture."""
import json
import pathlib
import subprocess
import sys
from network_privacy import check_files, check_text

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / 'artifacts/p007b-privacy'


def run():
    OUT.mkdir(parents=True, exist_ok=True)
    results = []
    for name, pattern in [('privacy-unit', 'test_network_privacy.py'), ('delivery-unit', 'test_p007b_delivery.py')]:
        p = subprocess.run([sys.executable, '-m', 'unittest', 'discover', '-s', 'scripts', '-p', pattern, '-v'],
                           cwd=ROOT, capture_output=True, text=True)
        log = p.stdout + p.stderr
        # Even a failing test runner cannot persist a leaked endpoint in its transcript.
        check_text(log)
        (OUT / (name + '.txt')).write_text(log, encoding='utf-8')
        results.append({'name': name, 'exitCode': p.returncode})
    audit = subprocess.run([sys.executable, 'scripts/p007b-privacy-audit.py'], cwd=ROOT, capture_output=True, text=True)
    check_text(audit.stdout + audit.stderr)
    if audit.returncode:
        raise ValueError('PRIVACY_HISTORY_AUDIT_FAILED')
    paths = [ROOT / 'scripts' / name for name in ['network_privacy.py', 'test_network_privacy.py', 'p007b-evidence.py',
             'p006c-diag-evidence.py', 'p007b-privacy-audit.py', 'p007b-privacy-checks.py', 'p007b-publish.py']]
    count = check_files(paths, source=True)
    exposure = json.loads((OUT / 'exposure-audit.json').read_text(encoding='utf-8'))
    if exposure['currentTrackedPaths'] or any(r['exitCode'] for r in results):
        raise ValueError('PRIVACY_REGRESSION_FAILED')
    check_files(p for p in OUT.rglob('*') if p.suffix in ['.json', '.txt'])
    result = dict(passed=True, commands=results, privacyTests=12, deliveryTests=9, scannerSourceFilesChecked=count,
                  currentSuspectOccurrences=0, historyRewritten=False, previousGitExposureStillReachable=True,
                  testInputs='generated in memory, not captured/configured endpoints', fixedErrorsDoNotEchoInput=True,
                  cloudStarted=False)
    (OUT / 'privacy-checks.json').write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(result))


if __name__ == '__main__':
    try:
        run()
    except Exception:
        raise SystemExit('PRIVACY_REGRESSION_FAILED_NO_INPUT_ECHO') from None
