set -eu
cd /opt/remotehub-p006c-video-20261122
python3 - <<'PY'
import pathlib,json,subprocess,time,hashlib,re
from turn_lifecycle_analyze import classify,read_jsonl
from turn_lifecycle_observer import relay_sockets
f=pathlib.Path.cwd();base=json.loads((f/'baseline-private.json').read_text());config=json.loads((f/'observer-private.json').read_text());pid=config['turn_pid']
def run(a):return subprocess.check_output(a,text=True)
events=read_jsonl(f/'events.jsonl');cases=sorted({e['case'] for e in events if e.get('kind')=='0x103'})
rows=[classify(events,case) for case in cases]
assert len(rows)==9 and all(r['classification']=='refresh_zero_immediate' for r in rows)
assert all(r['checkpoints']['socketAbsentAtNominalLeaseExpiry'] is True and r['checkpoints']['socketAbsentAfterGrace'] is True for r in rows),'WAIT_FOR_ORIGINAL_LEASE_DEADLINE_PLUS_15_SECONDS'
snapshots=[e for e in events if e['event']=='socket_snapshot']
assert {e['servicePid'] for e in snapshots}=={pid} and max(len(e['relayPorts']) for e in snapshots)<=4
assert relay_sockets(pid)==[] and snapshots[-1]['relayPorts']==[]
assert int(run(['systemctl','show','remotehub-turn-g6.service','-p','MainPID','--value']))==pid
judged=time.time_ns();assert judged//10**9<base['expiresAt']
(f/'allocation-results.json').write_text(json.dumps(dict(observedAtNs=judged,beforeServiceStop=True,currentRelayPorts=[],allocations=rows),indent=2)+'\n')
(f/'stop-observer').touch()
for _ in range(40):
 if (f/'observer-exit.txt').exists():break
 time.sleep(.2)
assert (f/'observer-exit.txt').read_text().strip()=='0'
capture_hash=hashlib.sha256((f/'control-private.pcap').read_bytes()).hexdigest()
wss_user=run(['systemctl','show','remotehub-p006c-video.service','-p','User','--value']).strip()
subprocess.run(['systemctl','stop','remotehub-p006c-video.service','remotehub-turn-g6.service'],check=True)
subprocess.run(['systemctl','stop','remotehub-p006c-video-expiry.timer'],check=True)
state=run(['systemctl','show','remotehub-turn-g6.service','-p','ActiveState','-p','UnitFileState','-p','MainPID'])
old_wss=run(['systemctl','show','remotehub-p006b.service','-p','ActiveState','-p','UnitFileState','-p','MainPID'])
new_state=subprocess.run(['systemctl','is-active','remotehub-p006c-video.service'],capture_output=True,text=True)
listeners=[]
for l in run(['ss','-H','-lntup']).splitlines():
 try:port=int(l.split()[4].rsplit(':',1)[1])
 except (ValueError,IndexError):continue
 if port in (3478,8443) or 49160<=port<=49259:listeners.append(port)
hashes_match=all(hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()==h for p,h in base['hashes'].items())
containers=run(['docker','inspect','--format','{{.Name}}|{{.State.StartedAt}}','tsmusicbot','teamspeak'])==base['containers']
policies=base['iptablesInput']==run(['iptables','-S','INPUT']) and base['iptablesOutput']==run(['iptables','-S','OUTPUT']) and base['nftNormalized']==re.sub(r'counter packets \d+ bytes \d+','counter',run(['nft','list','ruleset']))
baseline={l.split()[0] for l in base['services'].splitlines() if l.split()}
current={l.split()[0] for l in run(['systemctl','list-units','--type=service','--state=running','--no-pager','--plain','--no-legend']).splitlines() if l.split()}
delete=['server.json','client-private.json','server-key.pem','server.csr','ca.srl','observer-private.json','observer-next-private.json','labels-incoming-private.json','control-private.pcap']
for name in delete:
 p=f/name
 if p.exists():p.unlink()
summary=dict(authorizedWindowSeconds=5400,baselineAtNs=base['timeNs'],judgedAtNs=judged,finishedAtNs=time.time_ns(),judgedBeforeServiceStop=True,
 originalLeaseAndGraceAllObserved=True,allocationCount=len(rows),classificationCounts={'refresh_zero_immediate':len(rows),'lease_timeout':0,'abnormal_residual':0},
 maxConcurrentAllocations=max(len(e['relayPorts']) for e in snapshots),sameTurnPidDuringAllCases=True,turnPid=pid,
 finalRelaySocketsBeforeServiceStop=0,remainingDedicatedPortListeners=listeners,
 coturnInactiveDisabled='ActiveState=inactive' in state and 'UnitFileState=disabled' in state and 'MainPID=0' in state,
 oldWssInactiveDisabled='ActiveState=inactive' in old_wss and 'UnitFileState=disabled' in old_wss and 'MainPID=0' in old_wss,
 transientWssInactive=new_state.returncode in (3,4) and new_state.stdout.strip() in ('inactive','unknown'),
 transientWssHasNoPersistentUnit=not pathlib.Path('/etc/systemd/system/remotehub-p006c-video.service').exists(),
 expiryTimerCancelled=subprocess.run(['systemctl','is-active','--quiet','remotehub-p006c-video-expiry.timer']).returncode!=0,
 wssWasNonRoot=wss_user=='remotehub-p006b',protectedConfigsUnitsAndOldBinaryUnchanged=hashes_match,
 firewallPoliciesUnchanged=policies,businessContainersStartTimesUnchanged=containers,
 baselineRunningServiceCount=len(baseline),additionalRunningUnits=sorted(current-baseline),missingBaselineUnits=sorted(baseline-current),
 rawControlCaptureSha256=capture_hash,temporaryPrivateKeysInvitationsLabelsAndCaptureDeleted=all(not(f/n).exists() for n in delete),
 observerExitCode=0,realDesktopCaptureStarted=False,physicalTwoPcTestPerformed=False)
(f/'shutdown.json').write_text(json.dumps(summary,indent=2)+'\n');print(json.dumps(summary))
assert summary['coturnInactiveDisabled'] and summary['oldWssInactiveDisabled'] and summary['transientWssInactive'] and not listeners
assert hashes_match and containers and policies and summary['temporaryPrivateKeysInvitationsLabelsAndCaptureDeleted']
PY
