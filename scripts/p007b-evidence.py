"""Offline verification/provenance for R06 preparation; never starts cloud or capture."""
import datetime
import hashlib
import json
import pathlib
import re
import subprocess

ROOT = pathlib.Path(__file__).resolve().parent.parent
BASELINE = 'dbea0e357cdb4ab4899b910a93e6661fe23006f5'


def verify():
    folder = ROOT / 'artifacts/p007b'
    commands = json.loads((folder / 'automatic/results.json').read_text())
    assert len(commands) == 41 and all(c['exitCode'] == c.get('expectedExitCode', 0) and c.get('metadataValid', True) for c in commands)
    rust = sum(map(int, re.findall(r'test result: ok\. (\d+) passed', (folder / 'automatic/test.txt').read_text())))
    frontend = int(re.search(r'Tests\s+(\d+) passed', (folder / 'automatic/frontend-unit.txt').read_text()).group(1))
    runs = json.loads((folder / 'local/results.json').read_text())
    assert len(runs) == 5 and all(r['exitCode'] == 0 for r in runs)
    cases = []
    for run in runs:
        v = json.loads((folder / f'local/{run["scenario"]}.json').read_text())
        assert v['passed'] and v['sourceDurationSeconds'] == 8
        assert datetime.datetime.fromisoformat(v['finishedAt'].replace('Z', '+00:00')) <= datetime.datetime.fromisoformat(run['finishedAt'].replace('Z', '+00:00'))
        s = v['native']['sender']['source']; evidence = v['receiverEvidence']
        assert s['source_kind'] == 'synthetic_nv12' and s['direct_pair_diagnostic']['approved']
        assert s['control']['accepted_moves'] == s['control']['acks_sent'] == 100
        assert [e['stage'] for e in evidence] == ['pre_close', 'cleanup']
        assert evidence[0]['cleanup']['peerClosed'] is False
        assert all(evidence[-1]['cleanup'][key] for key in ['peerClosed', 'tracksStopped', 'nativeResourcesReleased'])
        assert v['securityReportChecks'] == {'senderRoleDenied': True, 'foreignOwnerDenied': True}
        if run['scenario'] == 'auto':
            assert s['frames_sent'] == evidence[-1]['snapshot']['framesDecoded'] == evidence[-1]['snapshot']['framesRendered'] == 240
        if run['scenario'] == 'stats-failure':
            assert all(e['capture'] == 'last_known' for e in evidence)
            assert evidence[0]['sampledAtMs'] == evidence[1]['sampledAtMs'] < evidence[1]['recordedAtMs']
        if run['scenario'] == 'wss-disconnect':
            assert v['exitCode'] == 1 and 'P006_WSS_CLOSED' in evidence[-1]['errors']
        cases.append(dict(scenario=run['scenario'], startedAt=v['startedAt'], finishedAt=v['finishedAt'],
                          sourceFrames=s['frames_sent'], decodedAtFinal=evidence[-1]['snapshot']['framesDecoded'],
                          capture=evidence[-1]['capture'], errors=evidence[-1]['errors'], cleanup=evidence[-1]['cleanup'],
                          expectedFailure=run['scenario'] in ['stats-failure', 'wss-disconnect'], passed=True))
    forbidden = ['47.122.108.86', '192.168.5.129', '192.168.5.112', 'BEGIN PRIVATE KEY', '"sdp":', '"sender_invitation":', '"receiver_invitation":']
    for path in folder.rglob('*'):
        if path.suffix in ['.json', '.txt']:
            text = path.read_text(encoding='utf-8-sig')
            assert not any(token in text for token in forbidden), path
    assert not list((ROOT / 'artifacts/private').glob('p006c-wss-*'))
    assert not list((ROOT / 'artifacts/private').glob('p007b-delivery-test-*'))
    files = subprocess.check_output(['git', 'ls-tree', '-r', '--name-only', BASELINE, 'docs', 'artifacts'], cwd=ROOT, text=True).splitlines()
    subprocess.run(['git', 'diff', '--exit-code', BASELINE, '--', *files], cwd=ROOT, check=True, stdout=subprocess.PIPE)
    exe = ROOT / 'target/probe/remotehub-desktop.exe'
    info = json.loads(subprocess.check_output([str(exe), '--r06-self-check'], text=True))
    assert info['sourceKind'] == 'synthetic_nv12' and info['desktopCapture'] is False and info['seconds'] == 8
    paths = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z', '--', 'apps', 'crates'], cwd=ROOT).decode().split('\0')
    product = hashlib.sha256()
    for path in sorted(filter(None, paths)):
        product.update(path.encode() + b'\0' + hashlib.sha256((ROOT / path).read_bytes()).digest())
    summary = dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
                   scope='offline regression and one-PC local synthetic transport via existing harness; no cloud/new physical two-PC/native GUI test',
                   automaticCommands=41, rustPassed=rust, frontendPassed=frontend, edgePassed=3,
                   pythonLifecyclePassed=12, deliveryTests='see delivery-tests.txt', localCases=cases,
                   programSha256=hashlib.sha256(exe.read_bytes()).hexdigest(), selfCheck=info,
                   productSourceDigest=product.hexdigest(), productSourceDigestScope='sorted apps/crates paths + NUL + each current file SHA256; not a reproducible-binary claim',
                   historicalReportsAndEvidenceUnchanged=True, localTemporaryCredentialsDeleted=True,
                   cloudStarted=False, desktopCaptured=False, physicalTwoPcTest='NOT_RUN',
                   E04='accepted_by_explicit_user_exception', R06='OPEN', phase0Exit='HOLD')
    (folder / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(dict(automaticCommands=41, rustPassed=rust, frontendPassed=frontend, localCases=5,
                          privacyPassed=True, historicalUnchanged=True, cloudStarted=False)))


if __name__ == '__main__':
    verify()
