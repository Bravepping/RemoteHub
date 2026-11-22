"""Package an inactive Windows build. Never signs credentials or starts any service."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import zipfile


def package(repo, destination):
    repo=repo.resolve()
    destination=destination.resolve()
    if destination.parent != repo.parent or destination.exists() or destination.with_suffix('.zip').exists():
        raise ValueError('NEW_DELIVERY_DIRECTORY_IN_WORKSPACE_REQUIRED')
    binary=repo/'target/probe/remotehub-desktop.exe'
    if not binary.is_file():
        raise ValueError('COMPILED_DIRECT_PREFERRED_DESKTOP_REQUIRED')
    evidence=json.loads((repo/'artifacts/p006c-video/cloud/summary.json').read_text())
    shutdown=json.loads((repo/'artifacts/p006c-video/cloud/shutdown.json').read_text())
    if not evidence['passed'] or not all(shutdown[k] for k in ['coturnInactiveDisabled','transientWssInactive','temporaryPrivateKeysInvitationsLabelsAndCaptureDeleted']):
        raise ValueError('VERIFIED_CLOUD_TEARDOWN_REQUIRED')
    if hashlib.sha256(binary.read_bytes()).hexdigest()!=evidence['binaries']['remotehub-desktop.exe']:
        raise ValueError('DESKTOP_DIFFERS_FROM_VERIFIED_BUILD')
    commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip()
    destination.mkdir()
    shutil.copy2(binary,destination/'RemoteHub.exe')
    for source,name in [
        ('docs/P006C_MANUAL_ACCEPTANCE.md','开始这里.txt'),
        ('docs/PHASE0_P006C_VIDEO_TEST_REPORT.md','本地集成测试报告.md'),
        ('docs/PHASE0_P006C_VIDEO_CLOUD_TEST_REPORT.md','云端自动测试报告.md'),
    ]:
        shutil.copy2(repo/source,destination/name)
    (destination/'p006c-private.json.example').write_text(json.dumps(dict(
        server_ipv4='APPROVED_PUBLIC_IPV4',signaling_port=8443,
        ca_pem='NEW_PUBLIC_CA_PEM_FROM_AUTHORIZED_ACTIVATION',
        sender_invitation='NEW_SHORT_LIVED_SENDER_INVITATION',
        receiver_invitation='NEW_SHORT_LIVED_RECEIVER_INVITATION',invitation_expires_at=0,
    ),indent=2)+'\n',encoding='utf-8')
    (destination/'BUILD_INFO.json').write_text(json.dumps(dict(
        source_commit=commit,github_source_url='https://github.com/Bravepping/RemoteHub/tree/'+commit,feature='custom-protocol,p006-direct',
        desktop_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
        active_credentials_included=False,cloud_services_stopped=True,
        actual_desktop_capture_tested_this_round=False,
        physical_two_pc_acceptance='pending separately authorized manual test',
    ),indent=2)+'\n',encoding='utf-8')
    archive=destination.with_suffix('.zip')
    with zipfile.ZipFile(archive,'x',compression=zipfile.ZIP_DEFLATED) as zipped:
        for item in sorted(destination.iterdir()):
            zipped.write(item,arcname=destination.name+'/'+item.name)
    with zipfile.ZipFile(archive) as zipped:
        assert zipped.testzip() is None
        assert len(zipped.namelist())==6
        assert not any(p.endswith('/p006c-private.json') for p in zipped.namelist())
    return dict(source_commit=commit,archive_name=archive.name,archive_bytes=archive.stat().st_size,
                archive_sha256=hashlib.sha256(archive.read_bytes()).hexdigest(),
                desktop_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                active_credentials_included=False)


if __name__=='__main__':
    args=argparse.ArgumentParser(description=__doc__)
    args.add_argument('--destination',type=Path,required=True)
    opts=args.parse_args()
    print(json.dumps(package(Path(__file__).resolve().parent.parent,opts.destination),indent=2))
