"""Recheck retained inactive ZIP, program identity, hashes and documentation; no GUI/network."""
import datetime
import hashlib
import json
import pathlib
import re
import subprocess
import zipfile
import sys
import importlib.util

root = pathlib.Path(__file__).resolve().parent.parent
privacy_round = sys.argv[1:] == ['--privacy-round']
if sys.argv[1:] and not privacy_round:
    raise SystemExit('R06_PACKAGE_CHECK_ARGUMENTS')
evidence_dir = root / ('artifacts/p007b-privacy' if privacy_round else 'artifacts/p007b')
delivery = json.loads((evidence_dir / 'delivery.json').read_text(encoding='utf-8'))
archive = root.parent / delivery['archiveName']
folder = archive.with_suffix('')
assert hashlib.sha256(archive.read_bytes()).hexdigest() == delivery['archiveSha256']
with zipfile.ZipFile(archive) as z:
    assert z.testzip() is None
    names = z.namelist()
    assert len(names) == len(delivery['members']) == (9 if privacy_round else 8)
    assert not any(p.endswith('/r06-private.json') or p.endswith('.key') or p.endswith('.pem') for p in names)
    for name, digest in delivery['members'].items():
        assert hashlib.sha256(z.read(folder.name + '/' + name)).hexdigest() == digest
        assert hashlib.sha256((folder / name).read_bytes()).hexdigest() == digest
    info = json.loads(z.read(folder.name + '/BUILD_INFO.json'))
    assert info['sourceCommit'] == delivery['sourceCommit'] and not info['activeCredentialsIncluded']
    assert info['newCloudTest'] == info['newPhysicalTwoPcTest'] == 'NOT_RUN'
    if privacy_round:
        from network_privacy import check_text
        spec = importlib.util.spec_from_file_location('exposure_audit', root / 'scripts/p007b-privacy-audit.py')
        audit = importlib.util.module_from_spec(spec); spec.loader.exec_module(audit)
        suspects = audit.suspected_values()
        for member in names:
            data = z.read(member)
            assert not any(value in data for value, _ in suspects), 'R06_PACKAGE_NETWORK_LITERAL_REJECTED'
            if not member.endswith('.exe') and not member.endswith('/r06-private.json.example'):
                check_text(data.decode('utf-8-sig'))
        example = json.loads(z.read(folder.name + '/r06-private.json.example'))
        assert example['invitation_expires_at'] == 0 and example['server_ipv4'] == 'APPROVED_PUBLIC_IPV4'
verify = subprocess.run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(folder / 'Verify-SHA256.ps1')], capture_output=True, text=True, timeout=10)
assert verify.returncode == 0 and delivery['programSha256'] in verify.stdout
selfcheck = subprocess.run([str(folder / 'RemoteHub-R06.exe'), '--r06-self-check'], capture_output=True, text=True, timeout=5)
assert selfcheck.returncode == 0 and json.loads(selfcheck.stdout) == info['selfCheck']
docs = ['README.md', 'TASKS.md', 'docs/PHASE0_P007B_DECISION.md', 'docs/PHASE0_P007B_TEST_REPORT.md',
        'docs/P007B_R06_RUNBOOK.md', 'docs/P007B_R06_ONLINE_PLAN.md', 'docs/P007B_R06_CONTRACT.md',
        'docs/ADR/0013-p007b-r06-diagnostic-package.md']
links = 0
pending_links = []
for name in docs:
    p = root / name
    for url in re.findall(r'\[[^\]]+\]\(([^)]+)\)', p.read_text(encoding='utf-8')):
        if re.match(r'^[a-z]+:', url) or url.startswith('#'):
            continue
        target = (p.parent / url.split('#')[0]).resolve()
        if privacy_round and target == evidence_dir / 'published.json' and not target.exists():
            pending_links.append('published.json: upload not yet performed')
            continue
        assert target.exists(), (name, url)
        links += 1
subprocess.run(['git', 'diff', '--exit-code', 'dbea0e3', '--', 'docs/PHASE0_P004_TEST_REPORT.md', 'docs/PHASE0_P004B_TEST_REPORT.md',
                'docs/PHASE0_P007A_REVIEW.md', 'docs/RemoteHub_完整开发文档_v1.0.md'], cwd=root, check=True)
value = dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
             sourceCommit=delivery['sourceCommit'], archiveSha256=delivery['archiveSha256'], programSha256=delivery['programSha256'],
             membersVerified=len(delivery['members']), crcVerified=True, packagedProgramSelfCheckPassed=True,
             packagedShaScriptPassed=True, relativeLinksChecked=links, oldReportsUnchanged=True,
             cloudTest='NOT_RUN', physicalTwoPcTest='NOT_RUN', nativeGuiTest='NOT_RUN', desktopCaptured=False,
             note='Only self-check mode executed; no GUI window, configuration, consent or network activation.')
if privacy_round:
    value.update(allMembersHaveZeroBaselineSuspectMatches=True, textMembersGenericPrivacyPassed=True, invalidExampleConfigOnly=True,
                 pendingPublicationLinks=pending_links)
(evidence_dir / 'package-verification.json').write_text(json.dumps(value, indent=2) + '\n', encoding='utf-8')
print(json.dumps(value))
