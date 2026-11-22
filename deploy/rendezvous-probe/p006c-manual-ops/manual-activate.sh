set -eu
python3 - <<'PY'
import pathlib,json,subprocess,secrets,time,pwd,os,hashlib
f=pathlib.Path('/opt/remotehub-p006c-manual-20261122');base=json.loads((f/'baseline-private.json').read_text())
remaining=base['expiresAt']-int(time.time());assert 120<remaining<=5400
assert (f/'manual-cleanup.py').is_file()
assert not (f/'server.json').exists()
account=pwd.getpwnam('remotehub-p006b')
private=dict(l.split('=',1) for l in pathlib.Path('/etc/remotehub-turn-g6/private.env').read_text().splitlines())
def openssl(args):subprocess.run(['openssl']+args,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
def write(name,value,mode=0o640):
 p=f/name;p.write_text(value);p.chmod(mode);os.chown(p,0,account.pw_gid)
openssl(['req','-x509','-newkey','rsa:2048','-nodes','-sha256','-days','1','-subj','/CN=RemoteHub P006c temporary CA','-addext','basicConstraints=critical,CA:TRUE','-addext','keyUsage=critical,keyCertSign,cRLSign','-keyout',str(f/'ca-key.pem'),'-out',str(f/'ca.pem')])
write('server.ext','basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:'+private['TURN_PUBLIC_IP']+'\n')
openssl(['req','-new','-newkey','rsa:2048','-nodes','-subj','/CN=RemoteHub P006c temporary WSS','-keyout',str(f/'server-key.pem'),'-out',str(f/'server.csr')])
openssl(['x509','-req','-in',str(f/'server.csr'),'-CA',str(f/'ca.pem'),'-CAkey',str(f/'ca-key.pem'),'-CAcreateserial','-days','1','-sha256','-extfile',str(f/'server.ext'),'-out',str(f/'server.pem')])
(f/'ca-key.pem').unlink();(f/'server-key.pem').chmod(0o640);os.chown(f/'server-key.pem',0,account.pw_gid)
sender=secrets.token_hex(32);receiver=secrets.token_hex(32)
config=dict(listen='0.0.0.0:8443',public_ipv4=private['TURN_PUBLIC_IP'],sender_invitation=sender,receiver_invitation=receiver,invitation_expires_at=base['expiresAt'],realm=private['TURN_REALM'],certificate=str(f/'server.pem'),private_key=str(f/'server-key.pem'))
write('server.json',json.dumps(config))
write('client-private.json',json.dumps(dict(server_ipv4=private['TURN_PUBLIC_IP'],signaling_port=8443,ca_pem=(f/'ca.pem').read_text(),sender_invitation=sender,receiver_invitation=receiver,invitation_expires_at=base['expiresAt'])),0o600)
assert hashlib.sha256((f/'hub-rendezvous-probe').read_bytes()).hexdigest()=='946ebb86fc60e74d00b3db6107dddb0043132b4afcfc7873d0ca5b96087c2653'
assert not any(l.split()[3].rsplit(':',1)[-1]=='8443' for l in subprocess.check_output(['ss','-H','-lnt'],text=True).splitlines())
subprocess.run(['systemd-run','--unit=remotehub-p006c-manual-expiry','--on-active='+str(remaining)+'s','--collect','/usr/bin/python3',str(f/'manual-cleanup.py')],check=True,capture_output=True)
subprocess.run(['systemctl','start','remotehub-turn-g6.service'],check=True)
pid=int(subprocess.check_output(['systemctl','show','remotehub-turn-g6.service','-p','MainPID','--value']));assert pid>0
write('observer-private.json',json.dumps(dict(labels={},turn_pid=pid)),0o600)
subprocess.run(['systemd-run','--unit=remotehub-p006c-manual','--collect','-p','User=remotehub-p006b','-p','Group=remotehub-p006b','-p','EnvironmentFile=/etc/remotehub-p006b/issuer.env','-p','RuntimeMaxSec='+str(remaining)+'s','-p','Restart=no','-p','NoNewPrivileges=yes','-p','ProtectSystem=strict','-p','ProtectHome=yes','-p','PrivateTmp=yes','-p','MemoryMax=192M','-p','CPUQuota=100%',str(f/'hub-rendezvous-probe'),'--private-config',str(f/'server.json')],check=True,capture_output=True)
time.sleep(1)
assert subprocess.check_output(['systemctl','show','remotehub-p006c-manual.service','-p','ActiveState','--value'],text=True).strip()=='active'
write('activation.json',json.dumps(dict(startedAtNs=time.time_ns(),turnPid=pid,expiresAt=base['expiresAt'],windowSeconds=5400,wssBinarySha256=hashlib.sha256((f/'hub-rendezvous-probe').read_bytes()).hexdigest(),nonRootWss=True,autostart=False)),0o600)
print('DEDICATED_WSS_AND_TURN_ACTIVE_WITH_EXPIRY')
PY
cd /opt/remotehub-p006c-manual-20261122
nohup sh -c 'umask 077; python3 manual-observe.py > observer-stdout-private.txt 2>observer-stderr-private.txt; echo $? > observer-exit.txt' </dev/null >/dev/null 2>&1 &
echo OBSERVER_STARTED
