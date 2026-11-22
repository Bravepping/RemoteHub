"""Validate only this dated review's evidence, links and change scope; no network tests."""
import datetime
import json
import pathlib
import re
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parent.parent
out = root / 'artifacts/p007a'
out.mkdir(parents=True, exist_ok=True)
checks = []

def run(name, command):
    p = subprocess.run(command, cwd=root, capture_output=True, text=True, encoding='utf-8', errors='replace')
    checks.append(dict(name=name, command=command, exitCode=p.returncode, stdout=p.stdout.strip(), stderr=p.stderr.strip()))
    if p.returncode:
        raise RuntimeError(name + ': ' + p.stderr)
    return p.stdout

run('evidence-index-read-only-check', [sys.executable, 'scripts/p007a-review-evidence.py', '--check'])
run('python-syntax', [sys.executable, '-m', 'py_compile', 'scripts/p007a-review-evidence.py', 'scripts/p007a-review-checks.py'])
base = '246dcea74f120897b47c2b6f00a7980f266afb69'
names = run('changed-tracked-paths', ['git', 'diff', '--name-only', base]).splitlines()
allowed = {'.ai-memory/handoff.md', 'README.md', 'TASKS.md', 'scripts/p007a-review-evidence.py',
           'scripts/p007a-review-checks.py',
           'docs/PHASE0_P007A_REVIEW.md', 'docs/ADR/0012-p007a-phase0-exit-review.md'}
assert all(p in allowed or p.startswith('artifacts/p007a/') for p in names), names
checks.append(dict(name='business-code-and-historical-evidence-unchanged', passed=True,
                   note='Only current status documents and new P007A review files may differ from the baseline.'))
run('whitespace', ['git', 'diff', '--check'])
docs = ['README.md', 'TASKS.md', 'docs/PHASE0_P007A_REVIEW.md', 'docs/ADR/0012-p007a-phase0-exit-review.md']
count = 0
for name in docs:
    p = root / name
    for url in re.findall(r'\[[^\]]+\]\(([^)]+)\)', p.read_text(encoding='utf-8')):
        if re.match(r'^[a-z]+:', url) or url.startswith('#'):
            continue
        target = (p.parent / url.split('#')[0]).resolve()
        # This verification artifact is created only after all assertions pass.
        if target == out / 'verification.json':
            continue
        assert target.exists(), (name, url)
        count += 1
checks.append(dict(name='relative-document-links', passed=True, checked=count))
value = dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
             baselineCommit=base, scope='P007A review-only checks; no runtime build, network, desktop or cloud test',
             checks=checks, newRuntimeTests=False, historicalFilesUnchanged=True,
             businessCodeUnchanged=True, cloudServicesStarted=False, formalPhase0Exit='hold_for_review')
(out / 'verification.json').write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')
print(json.dumps(dict(checks=len(checks), localLinks=count, historicalFilesUnchanged=True,
                     businessCodeUnchanged=True, newRuntimeTests=False)))
