"""Scan this scanner, R06 operations, documentation and sanitized evidence without echoing inputs."""
import datetime
import json
import pathlib
from network_privacy import check_files

ROOT=pathlib.Path(__file__).resolve().parent.parent


def main():
    sources=list((ROOT/'deploy/rendezvous-probe/r06-ops').glob('*.py'))+[ROOT/'scripts'/n for n in [
        'r06-online-checks.py','r06-online-evidence.py','r06-preflight-checks.py','r06-preflight-evidence.py','test_r06_preflight.py']]+[pathlib.Path(__file__)]
    for p in sources:compile(p.read_text(encoding='utf-8'),p.name,'exec')
    source_count=check_files(sources,source=True)
    docs=[ROOT/'README.md',ROOT/'TASKS.md',ROOT/'docs/P007B_R06_ONLINE_DIAGNOSTIC_REPORT.md']
    doc_count=check_files(docs)
    out=ROOT/'artifacts/r06-online'
    artifacts=[p for p in out.iterdir() if p.suffix in ['.json','.jsonl','.txt'] and p.name!='privacy.json']
    artifact_count=check_files(artifacts)
    result=dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),passed=True,
        sourceSyntaxAndAddressPrivacyFiles=source_count,scannerSelfChecked=True,documentAddressAndPayloadPrivacyFiles=doc_count,
        evidenceAddressAndPayloadPrivacyFiles=artifact_count,sourceAndScannerInputsNotEchoed=True,
        originalNetworkEndpointsExported=False,privateCredentialsCommitted=False)
    (out/'privacy.json').write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(result))


if __name__=='__main__':
    try:main()
    except Exception:raise SystemExit('R06_ONLINE_PRIVACY_FAILED_NO_PRIVATE_ECHO') from None
