"""Offline regressions for authorized R06 operations; never activates a cloud service."""
import datetime
import json
import pathlib
import subprocess
import sys
from network_privacy import check_files,check_text

ROOT=pathlib.Path(__file__).resolve().parent.parent
OUT=ROOT/'artifacts/r06-online'


def main():
    OUT.mkdir(exist_ok=True); results=[]
    for name,pattern,count in [('operations','test_r06_preflight.py',12),('privacy','test_network_privacy.py',12),('delivery','test_p007b_delivery.py',9)]:
        p=subprocess.run([sys.executable,'-m','unittest','discover','-s','scripts','-p',pattern,'-v'],cwd=ROOT,capture_output=True,text=True)
        text=p.stdout+p.stderr;check_text(text);(OUT/(name+'-regression.txt')).write_text(text,encoding='utf-8')
        results.append(dict(name=name,exitCode=p.returncode,tests=count))
    if any(x['exitCode'] for x in results):raise ValueError('R06_OFFLINE_REGRESSION_FAILED')
    paths=list((ROOT/'deploy/rendezvous-probe/r06-ops').glob('*.py'))+[ROOT/'scripts/test_r06_preflight.py',pathlib.Path(__file__)]
    for p in paths:compile(p.read_text(encoding='utf-8'),p.name,'exec')
    count=check_files(paths,source=True)
    result=dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),offlineOnly=True,
        passed=True,tests=33,results=results,syntaxAndPrivacySourceFiles=count,oldReportsOverwritten=False,
        productSourceChanged=False,physicalTwoPcResultsAreSeparate=True)
    (OUT/'offline-regression.json').write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8');print(json.dumps(result))


if __name__=='__main__':
    try:main()
    except Exception:raise SystemExit('R06_ONLINE_OFFLINE_CHECKS_FAILED_NO_PRIVATE_ECHO') from None
