set -eu
python3 - <<'PY'
import pathlib,subprocess,json,hashlib,re,time,os,pwd
f=pathlib.Path('/opt/remotehub-p006c-video-20261122');assert not f.exists()
def run(a):return subprocess.check_output(a,text=True)
for u in ('remotehub-turn-g6.service','remotehub-p006b.service'):
 assert run(['systemctl','show',u,'-p','ActiveState','--value']).strip()=='inactive'
 assert run(['systemctl','show',u,'-p','UnitFileState','--value']).strip()=='disabled'
assert not any(l.split()[3].rsplit(':',1)[-1] in ('8443','3478') for l in run(['ss','-H','-lntup']).splitlines())
f.mkdir(mode=0o750);os.chown(f,0,pwd.getpwnam('remotehub-p006b').pw_gid)
protected=['/etc/remotehub-turn-g6/turnserver.conf','/etc/remotehub-turn-g6/private.env','/etc/systemd/system/remotehub-turn-g6.service','/etc/systemd/system/remotehub-p006b.service','/etc/remotehub-p006b/server.json','/etc/remotehub-p006b/issuer.env','/opt/remotehub-p006b/hub-rendezvous-probe']
base=dict(timeNs=time.time_ns(),expiresAt=int(time.time())+5400,hashes={p:hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest() for p in protected},
 services=run(['systemctl','list-units','--type=service','--state=running','--no-pager','--plain','--no-legend']),
 containers=run(['docker','inspect','--format','{{.Name}}|{{.State.StartedAt}}','tsmusicbot','teamspeak']),
 iptablesInput=run(['iptables','-S','INPUT']),iptablesOutput=run(['iptables','-S','OUTPUT']),
 nftNormalized=re.sub(r'counter packets \d+ bytes \d+','counter',run(['nft','list','ruleset'])))
p=f/'baseline-private.json';p.write_text(json.dumps(base));p.chmod(0o600)
subprocess.run(['systemd-run','--unit=remotehub-p006c-video-expiry','--on-active=5400s','--collect','/usr/bin/systemctl','stop','remotehub-p006c-video.service','remotehub-turn-g6.service'],check=True,capture_output=True)
print(json.dumps(dict(prepared=True,timeNs=base['timeNs'],expiresAt=base['expiresAt'],servicesStarted=False)))
PY
