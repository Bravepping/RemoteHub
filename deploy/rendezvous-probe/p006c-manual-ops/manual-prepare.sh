set -eu
python3 - <<'PY'
import pathlib,subprocess,json,hashlib,re,time,os,pwd
f=pathlib.Path('/opt/remotehub-p006c-manual-20261122');assert not f.exists()
def run(a):return subprocess.check_output(a,text=True)
for u in ('remotehub-turn-g6.service','remotehub-p006b.service'):
 assert run(['systemctl','show',u,'-p','ActiveState','--value']).strip()=='inactive'
 assert run(['systemctl','show',u,'-p','UnitFileState','--value']).strip()=='disabled'
assert not any(l.split()[4].rsplit(':',1)[-1] in ('8443','3478') for l in run(['ss','-H','-lntup']).splitlines())
f.mkdir(mode=0o750);os.chown(f,0,pwd.getpwnam('remotehub-p006b').pw_gid)
protected=['/etc/remotehub-turn-g6/turnserver.conf','/etc/remotehub-turn-g6/private.env','/etc/systemd/system/remotehub-turn-g6.service','/etc/systemd/system/remotehub-p006b.service','/etc/remotehub-p006b/server.json','/etc/remotehub-p006b/issuer.env','/opt/remotehub-p006b/hub-rendezvous-probe']
base=dict(timeNs=time.time_ns(),expiresAt=int(time.time())+5400,hashes={p:hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest() for p in protected},services=run(['systemctl','list-units','--type=service','--state=running','--no-pager','--plain','--no-legend']),containers=run(['docker','inspect','--format','{{.Name}}|{{.State.StartedAt}}','tsmusicbot','teamspeak']),iptablesInput=run(['iptables','-S','INPUT']),iptablesOutput=run(['iptables','-S','OUTPUT']),nftNormalized=re.sub(r'counter packets \d+ bytes \d+','counter',run(['nft','list','ruleset'])))
p=f/'baseline-private.json';p.write_text(json.dumps(base));p.chmod(0o600)
binary=pathlib.Path('/opt/remotehub-p006c-video-20261122/hub-rendezvous-probe')
assert hashlib.sha256(binary.read_bytes()).hexdigest()=='946ebb86fc60e74d00b3db6107dddb0043132b4afcfc7873d0ca5b96087c2653'
subprocess.run(['install','-m','0755',str(binary),str(f/'hub-rendezvous-probe')],check=True)
print(json.dumps(dict(prepared=True,expiresAt=base['expiresAt'],servicesStarted=False)))
PY
