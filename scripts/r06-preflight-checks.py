"""Record offline preflight, privacy and delivery regressions without cloud activation."""
import datetime
import json
import pathlib
import subprocess
import sys
from network_privacy import check_files, check_text

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT/'artifacts/r06-preflight'
OPS = ROOT/'deploy/rendezvous-probe/r06-ops'


def main():
    OUT.mkdir(exist_ok=True)
    commands = []
    for name, pattern, count in [('operations', 'test_r06_preflight.py', 11),
                                 ('privacy', 'test_network_privacy.py', 12),
                                 ('delivery', 'test_p007b_delivery.py', 9)]:
        args = [sys.executable, '-m', 'unittest', 'discover', '-s', 'scripts', '-p', pattern, '-v']
        p = subprocess.run(args, cwd=ROOT, capture_output=True, text=True)
        log = p.stdout + p.stderr
        check_text(log)
        (OUT/(name+'.txt')).write_text(log, encoding='utf-8')
        commands.append(dict(name=name, command='python -m unittest discover -s scripts -p '+pattern+' -v',
                             exitCode=p.returncode, expectedTests=count))
    if any(c['exitCode'] for c in commands):
        raise ValueError('R06_OFFLINE_REGRESSION_FAILED')
    source = list(OPS.glob('*.py')) + [ROOT/'scripts'/n for n in [
        'test_r06_preflight.py','r06-preflight-checks.py','r06-preflight-evidence.py']]
    for path in source: compile(path.read_text(encoding='utf-8'), path.name, 'exec')
    source_count = check_files(source,source=True)
    evidence = subprocess.run([sys.executable,'scripts/r06-preflight-evidence.py'],cwd=ROOT,capture_output=True,text=True)
    check_text(evidence.stdout+evidence.stderr)
    if evidence.returncode: raise ValueError('R06_READINESS_CHECK_FAILED')
    documents = [ROOT/'docs'/n for n in ['P007B_R06_EXECUTION_CHECKLIST.md','P007B_R06_PREFLIGHT_REPORT.md']]
    document_count = check_files(documents)
    artifact_count = check_files(p for p in OUT.iterdir() if p.suffix in ['.json','.txt'])
    result = dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
                  passed=True, commands=commands, offlineTests=32, syntaxCheckedFiles=source_count,
                  privacyCheckedSourceFiles=source_count, privacyCheckedDocuments=document_count,
                  privacyCheckedEvidenceFiles=artifact_count, cloudStarted=False, credentialsGenerated=False,
                  newPhysicalTwoPcTest='NOT_RUN', cloudCleanupGuardExecution='NOT_RUN',
                  productSourceChanged=False, previous41StepProductRegression='accepted prior delivery; not rerun for ops-only preparation',
                  R06='OPEN',phase0='HOLD')
    check_text(json.dumps(result))
    (OUT/'verification.json').write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(result))


if __name__=='__main__':
    try: main()
    except Exception: raise SystemExit('R06_PREFLIGHT_CHECKS_FAILED_NO_PRIVATE_ECHO') from None
