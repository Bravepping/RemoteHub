"""Stops only this explicitly authorized experiment and removes its temporary credentials."""
import pathlib,json,subprocess,time,hashlib,re
from turn_lifecycle_observer import relay_sockets
from turn_lifecycle_analyze import read_jsonl,classify
f=pathlib.Path('/opt/remotehub-p006c-manual-20261122')
if (f/'shutdown.json').exists():raise SystemExit(0)
base=json.loads((f/'baseline-private.json').read_text());c=json.loads((f/'observer-private.json').read_text())
def run(a):return subprocess.check_output(a,text=True)
before=relay_sockets(c['turn_pid']);judged=time.time_ns()
(f/'stop-observer').touch()
for _ in range(40):
 if (f/'observer-exit.txt').exists():break
 time.sleep(.2)
events=read_jsonl(f/'events.jsonl') if (f/'events.jsonl').exists() else []
cases=sorted({e['case'] for e in events if e.get('kind')=='0x103'})
(f/'allocation-results.json').write_text(json.dumps([classify(events,k) for k in cases],indent=2)+'\n')
raw=f/'control-private.pcap';raw_hash=hashlib.sha256(raw.read_bytes()).hexdigest() if raw.exists() else None
subprocess.run(['systemctl','stop','remotehub-p006c-manual.service','remotehub-turn-g6.service'],check=True)
state=run(['systemctl','show','remotehub-turn-g6.service','-p','ActiveState','-p','UnitFileState','-p','MainPID'])
hashes=all(hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()==h for p,h in base['hashes'].items())
policies=base['iptablesInput']==run(['iptables','-S','INPUT']) and base['iptablesOutput']==run(['iptables','-S','OUTPUT']) and base['nftNormalized']==re.sub(r'counter packets \d+ bytes \d+','counter',run(['nft','list','ruleset']))
containers=base['containers']==run(['docker','inspect','--format','{{.Name}}|{{.State.StartedAt}}','tsmusicbot','teamspeak'])
secrets=['server.json','client-private.json','server-key.pem','ca-key.pem','server.csr','ca.srl','observer-private.json','observer-next-private.json','control-private.pcap']
for name in secrets:
 p=f/name
 if p.exists():p.unlink()
summary=dict(judgedAtNs=judged,finishedAtNs=time.time_ns(),authorizationExpiresAt=base['expiresAt'],expired=time.time()>=base['expiresAt'],relaySocketsBeforeStop=before,
singleAllocationReleaseJudgedBeforeStop=True,serviceStopIsNotReleaseEvidence=True,allocationsObserved=len(cases),
coturnInactiveDisabled='ActiveState=inactive' in state and 'UnitFileState=disabled' in state and 'MainPID=0' in state,
wssInactive=subprocess.run(['systemctl','is-active','--quiet','remotehub-p006c-manual.service']).returncode!=0,
noPersistentWssUnit=not pathlib.Path('/etc/systemd/system/remotehub-p006c-manual.service').exists(),
protectedHashesUnchanged=hashes,firewallPoliciesUnchanged=policies,businessContainerStartTimesUnchanged=containers,
temporaryServerCredentialsAndRawCaptureDeleted=all(not(f/n).exists() for n in secrets),rawControlCaptureSha256=raw_hash,
observerExit=(f/'observer-exit.txt').read_text().strip() if (f/'observer-exit.txt').exists() else None)
(f/'shutdown.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary))
