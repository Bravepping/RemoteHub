"""Build an inactive, synthetic-only R06 delivery from a verified dedicated executable."""
import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess
import zipfile


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify_identity(binary):
    p = subprocess.run([str(binary), '--r06-self-check'], capture_output=True, text=True, timeout=5)
    if p.returncode:
        raise ValueError('R06_SELF_CHECK_FAILED')
    info = json.loads(p.stdout)
    if not (info.get('buildTag') == 'p007b-r06-v1' and info.get('sourceKind') == 'synthetic_nv12'
            and info.get('desktopCapture') is False and info.get('inputInjection') is False
            and info.get('mode') == 'direct_preferred' and info.get('seconds') == 8
            and info.get('privateConfigFile') == 'r06-private.json' and info.get('nativeConsentRequired') is True
            and info.get('maximumInvitationRemainingSeconds') == 1200 and info.get('automaticNetworkStart') is False):
        raise ValueError('R06_DEDICATED_BUILD_REQUIRED')
    return info


def package(repo, destination, privacy_round=False):
    repo = repo.resolve(); destination = destination.resolve()
    if destination.parent != repo.parent or destination.exists() or destination.with_suffix('.zip').exists():
        raise ValueError('R06_NEW_WORKSPACE_DELIVERY_REQUIRED')
    changed = subprocess.check_output(['git', 'diff', '--name-only', 'HEAD', '--', 'apps', 'crates'], cwd=repo).strip()
    if changed:
        raise ValueError('R06_COMMITTED_PRODUCT_SOURCE_REQUIRED')
    binary = repo / 'target/probe/remotehub-desktop.exe'
    if binary.is_symlink():
        raise ValueError('R06_BINARY_FILE_POLICY')
    identity = verify_identity(binary)
    evidence_dir = repo / ('artifacts/p007b-privacy' if privacy_round else 'artifacts/p007b')
    provenance = json.loads((evidence_dir / 'summary.json').read_text())
    if sha(binary) != provenance['programSha256'] or identity != provenance['selfCheck']:
        raise ValueError('R06_BINARY_DIFFERS_FROM_VERIFIED_BUILD')
    checks = json.loads((evidence_dir / 'automatic/results.json').read_text())
    if len(checks) != 41 or any(c['exitCode'] != c.get('expectedExitCode', 0) or c.get('metadataValid') is False for c in checks):
        raise ValueError('R06_SAFETY_REGRESSION_REQUIRED')
    local = json.loads((evidence_dir / 'local/results.json').read_text())
    if len(local) != 5 or any(c['exitCode'] != 0 for c in local):
        raise ValueError('R06_LOCAL_TRANSPORT_REQUIRED')
    for c in local:
        v = json.loads((evidence_dir / f'local/{c["scenario"]}.json').read_text())
        if not v['passed'] or v['sourceDurationSeconds'] != 8:
            raise ValueError('R06_LOCAL_SHORT_SOURCE_REQUIRED')
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip()
    destination.mkdir()
    shutil.copy2(binary, destination / 'RemoteHub-R06.exe')
    copy_items = [
        ('docs/P007B_R06_RUNBOOK.md', '开始这里.txt'),
        ('docs/P007B_R06_CONTRACT.md', '诊断契约.md'),
        ('docs/P007B_R06_ONLINE_PLAN.md', '待授权操作方案.md'),
        ('scripts/Export-R06Summary.ps1', 'Export-R06Summary.ps1'),
        ('scripts/Verify-R06SHA256.ps1', 'Verify-SHA256.ps1'),
    ]
    if privacy_round:
        privacy_checks = json.loads((evidence_dir / 'privacy-checks.json').read_text())
        if not privacy_checks['passed'] or privacy_checks['currentSuspectOccurrences']:
            raise ValueError('R06_PRIVACY_REGRESSION_REQUIRED')
        copy_items.append(('docs/P007B_R06_PRIVACY_NOTICE.md', 'Privacy-and-Verification.md'))
    for source, name in copy_items:
        data = (repo / source).read_text(encoding='utf-8-sig')
        # PS5.1 and Notepad must correctly interpret UTF-8 on either physical PC.
        (destination / name).write_text(data, encoding='utf-8-sig')
    (destination / 'r06-private.json.example').write_text(json.dumps(dict(
        server_ipv4='APPROVED_PUBLIC_IPV4', signaling_port=8443, ca_pem='NEW_AUTHORIZED_PUBLIC_CA_PEM',
        sender_invitation='NEW_SHORT_LIVED_SENDER_INVITATION', receiver_invitation='NEW_SHORT_LIVED_RECEIVER_INVITATION',
        invitation_expires_at=0), indent=2) + '\n', encoding='utf-8')
    info = dict(buildTag=identity['buildTag'], sourceCommit=commit,
                sourceUrl='https://github.com/Bravepping/RemoteHub/tree/' + commit,
                feature='custom-protocol,p007b-diag', programSha256=sha(binary), deliveryRevision='v2-privacy' if privacy_round else 'v1',
                sourceKind='synthetic_nv12', desktopCapture=False, inputInjection=False,
                activeCredentialsIncluded=False, activation='pending separate authorization',
                newCloudTest='NOT_RUN', newPhysicalTwoPcTest='NOT_RUN',
                localTestScope='one-PC real TLS/WSS/MF/RTP/Edge/SCTP via existing operator harness; not packaged WebView2 GUI test',
                selfCheck=identity)
    (destination / 'BUILD_INFO.json').write_text(json.dumps(info, indent=2) + '\n', encoding='utf-8')
    members = sorted(destination.iterdir())
    if any(p.name.endswith('-private.json') or p.suffix.lower() in ('.pem', '.key') for p in members):
        raise ValueError('R06_PRIVATE_MATERIAL_FORBIDDEN')
    if privacy_round:
        from network_privacy import check_text
        for item in members:
            if item.suffix != '.exe' and item.name != 'r06-private.json.example':
                check_text(item.read_text(encoding='utf-8-sig'))
    archive = destination.with_suffix('.zip')
    with zipfile.ZipFile(archive, 'x', compression=zipfile.ZIP_DEFLATED) as z:
        for item in members:
            z.write(item, arcname=destination.name + '/' + item.name)
    with zipfile.ZipFile(archive) as z:
        assert z.testzip() is None
        assert len(z.infolist()) == len(members)
        assert hashlib.sha256(z.read(destination.name + '/RemoteHub-R06.exe')).hexdigest() == info['programSha256']
    delivery = dict(sourceCommit=commit, archiveName=archive.name, archiveSha256=sha(archive),
                    programSha256=info['programSha256'], members={p.name: sha(p) for p in members},
                    crcVerified=True, activeCredentialsIncluded=False, cloudStartedThisRound=False,
                    realDesktopCaptured=False, physicalTwoPcTest='NOT_RUN')
    (evidence_dir / 'delivery.json').write_text(json.dumps(delivery, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')
    print(json.dumps(delivery, ensure_ascii=False))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--destination', type=pathlib.Path, required=True)
    parser.add_argument('--privacy-round', action='store_true')
    args = parser.parse_args()
    package(pathlib.Path(__file__).resolve().parent.parent, args.destination, args.privacy_round)
