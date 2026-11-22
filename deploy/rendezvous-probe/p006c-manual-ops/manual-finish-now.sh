set -eu
cd /opt/remotehub-p006c-manual-20261122
python3 - <<'PY'
import json,pathlib,subprocess
from turn_lifecycle_observer import relay_sockets
from turn_lifecycle_analyze import classify,read_jsonl
f=pathlib.Path.cwd();c=json.loads((f/'observer-private.json').read_text())
events=read_jsonl(f/'events.jsonl');cases={e['case'] for e in events if e.get('kind')=='0x103'}
rows=[classify(events,k) for k in cases]
assert rows and all(r['classification']=='refresh_zero_immediate' for r in rows)
assert relay_sockets(c['turn_pid'])==[]
assert int(subprocess.check_output(['systemctl','show','remotehub-turn-g6.service','-p','MainPID','--value']))==c['turn_pid']
assert not (f/'discovery-error-private.txt').exists()
PY
python3 manual-cleanup.py
systemctl stop remotehub-p006c-manual-expiry.timer
python3 - <<'PY'
import pathlib,subprocess,json,time
f=pathlib.Path('/opt/remotehub-p006c-manual-20261122');j=json.loads((f/'shutdown.json').read_text())
def run(a):return subprocess.check_output(a,text=True)
remaining=[]
for l in run(['ss','-H','-lntup']).splitlines():
 try:port=int(l.split()[4].rsplit(':',1)[1])
 except (IndexError,ValueError):continue
 if port in (3478,8443) or 49160<=port<=49259:remaining.append(port)
old=run(['systemctl','show','remotehub-p006b.service','-p','ActiveState','-p','UnitFileState','-p','MainPID'])
base=json.loads((f/'baseline-private.json').read_text());before={l.split()[0] for l in base['services'].splitlines() if l.split()}
after={l.split()[0] for l in run(['systemctl','list-units','--type=service','--state=running','--no-pager','--plain','--no-legend']).splitlines() if l.split()}
j.update(finalVerificationAtNs=time.time_ns(),remainingDedicatedPortListeners=remaining,
oldWssInactiveDisabled='ActiveState=inactive' in old and 'UnitFileState=disabled' in old and 'MainPID=0' in old,
expiryTimerCancelled=subprocess.run(['systemctl','is-active','--quiet','remotehub-p006c-manual-expiry.timer']).returncode!=0,
baselineRunningServiceCount=len(before),additionalRunningUnits=sorted(after-before),missingBaselineUnits=sorted(before-after),
allAllocationsReleaseConfirmedBeforeServiceStop=True)
(f/'shutdown.json').write_text(json.dumps(j,indent=2)+'\n')
assert j['coturnInactiveDisabled'] and j['wssInactive'] and j['noPersistentWssUnit'] and j['oldWssInactiveDisabled'] and not remaining
assert not j['relaySocketsBeforeStop'] and j['observerExit']=='0'
assert j['protectedHashesUnchanged'] and j['firewallPoliciesUnchanged'] and j['businessContainerStartTimesUnchanged'] and j['temporaryServerCredentialsAndRawCaptureDeleted']
print(json.dumps(j))
PY
