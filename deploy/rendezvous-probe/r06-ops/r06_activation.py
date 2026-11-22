"""Explicit authorized activation; fed reviewed script sources in memory by the operator."""
import datetime
import hashlib
import json
import os
import pathlib
import pwd
import secrets
import subprocess
import time
import types


def activate(sources):
    if os.geteuid() != 0: raise ValueError('R06_ROOT_REQUIRED')
    start = int(time.time())
    run_id = datetime.datetime.fromtimestamp(start, datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
    folder = pathlib.Path('/run/remotehub-p007b-r06-'+run_id)
    account = pwd.getpwnam('remotehub-p006b')
    def command(args, check=True):
        return subprocess.run(args,capture_output=True,text=True,timeout=12,check=check)
    def state(name,key='ActiveState'):
        return command(['systemctl','show',name,'-p',key,'--value'],False).stdout.strip()
    def write(name,text,mode=0o600):
        p=folder/name; p.write_text(text); p.chmod(mode); os.chown(p,0,account.pw_gid)
    def module(name):
        m=types.ModuleType(name); exec(compile(sources[name+'.py'],name+'.py','exec'),m.__dict__); return m
    guard=module('expiry_cleanup'); planner=module('execution_plan')
    assert all(state(u)=='inactive' and state(u,'UnitFileState')=='disabled' for u in ['remotehub-turn-g6.service','remotehub-p006b.service'])
    assert not list(pathlib.Path('/run').glob('remotehub-p007b-r06-*'))
    binary=pathlib.Path(planner.WSS_BINARY)
    assert hashlib.sha256(binary.read_bytes()).hexdigest()=='946ebb86fc60e74d00b3db6107dddb0043132b4afcfc7873d0ca5b96087c2653'
    folder.mkdir(mode=0o750); os.chown(folder,0,account.pw_gid)
    try:
        for name in ['expiry_cleanup.py','r06_observer.py']:
            write(name,sources[name]); assert hashlib.sha256((folder/name).read_bytes()).hexdigest()==hashlib.sha256(sources[name].encode()).hexdigest()
        def arm(seconds):
            command(['systemd-run','--unit=remotehub-p007b-r06-expiry','--on-active='+str(seconds)+'s',
                     '--timer-property=AccuracySec=1s','--collect','/usr/bin/python3',str(folder/'expiry_cleanup.py'),
                     '--execute-after-authorization',str(folder),'deadline'])
        # Exercise the real timer and guard while both media services are still stopped.
        arm(3)
        assert state('remotehub-p007b-r06-expiry.timer')=='active'
        until=time.monotonic()+12
        while not (folder/'cleanup-receipt.json').exists() and time.monotonic()<until: time.sleep(.2)
        smoke=json.loads((folder/'cleanup-receipt.json').read_text())
        assert all(smoke['dedicatedServicesStopped'].values()) and smoke['temporarySecretFilesAbsent'] and not smoke['failures']
        (folder/'cleanup-receipt.json').rename(folder/'guard-smoke-receipt.json')
        (folder/'stop-observer').unlink()
        for unit in ['remotehub-p007b-r06-expiry.timer','remotehub-p007b-r06-expiry.service']:
            command(['systemctl','stop',unit],False)
        time.sleep(1)
        window=planner.plan(run_id,int(time.time()),start)
        write('window.json',json.dumps(window))
        arm(window['guardTarget']-int(time.time())-2)
        assert state('remotehub-p007b-r06-expiry.timer')=='active'
        bus_path='/org/freedesktop/systemd1/unit/remotehub_2dp007b_2dr06_2dexpiry_2etimer'
        timer=command(['busctl','get-property','org.freedesktop.systemd1',bus_path,'org.freedesktop.systemd1.Timer','NextElapseUSecMonotonic']).stdout.split()
        assert timer[0]=='t'; next_mono=int(timer[1])/1e6
        assert 60 < next_mono-time.monotonic() <= window['guardTarget']-time.time()
        write('guard-verified.json',json.dumps(dict(timeNs=time.time_ns(),smokePassed=True,timerActive=True,
              actualRemainingSeconds=next_mono-time.monotonic(),guardTarget=window['guardTarget'],authorizationDeadline=window['authorizationDeadline'])))
        private=dict(line.split('=',1) for line in pathlib.Path('/etc/remotehub-turn-g6/private.env').read_text().splitlines() if '=' in line and not line.startswith('#'))
        def openssl(args): command(['openssl']+args)
        openssl(['req','-x509','-newkey','rsa:2048','-nodes','-sha256','-days','1','-subj','/CN=RemoteHub R06 temporary CA','-addext','basicConstraints=critical,CA:TRUE','-addext','keyUsage=critical,keyCertSign,cRLSign','-keyout',str(folder/'ca-key.pem'),'-out',str(folder/'ca.pem')])
        write('server.ext','basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:'+private['TURN_PUBLIC_IP']+'\n')
        openssl(['req','-new','-newkey','rsa:2048','-nodes','-subj','/CN=RemoteHub R06 temporary WSS','-keyout',str(folder/'server-key.pem'),'-out',str(folder/'server.csr')])
        openssl(['x509','-req','-in',str(folder/'server.csr'),'-CA',str(folder/'ca.pem'),'-CAkey',str(folder/'ca-key.pem'),'-CAcreateserial','-days','1','-sha256','-extfile',str(folder/'server.ext'),'-out',str(folder/'server.pem')])
        (folder/'ca-key.pem').unlink(); (folder/'server-key.pem').chmod(0o640); os.chown(folder/'server-key.pem',0,account.pw_gid)
        sender=secrets.token_hex(32); receiver=secrets.token_hex(32)
        expires=window['lastNewAllocationAdmission']
        assert expires-int(time.time())>120
        write('server.json',json.dumps(dict(listen='0.0.0.0:8443',public_ipv4=private['TURN_PUBLIC_IP'],sender_invitation=sender,
            receiver_invitation=receiver,invitation_expires_at=expires,realm=private['TURN_REALM'],certificate=str(folder/'server.pem'),private_key=str(folder/'server-key.pem'))),0o640)
        write('client-private.json',json.dumps(dict(server_ipv4=private['TURN_PUBLIC_IP'],signaling_port=8443,ca_pem=(folder/'ca.pem').read_text(),
            sender_invitation=sender,receiver_invitation=receiver,invitation_expires_at=expires)))
        command(planner.plan(run_id,int(time.time()),start)['startTurnCommand'])
        pid=int(state('remotehub-turn-g6.service','MainPID')); assert pid>0
        write('observer-private.json',json.dumps(dict(labels={},turn_pid=pid)))
        command(planner.plan(run_id,int(time.time()),start)['startObserverCommand'])
        until=time.monotonic()+8
        snapshots=[]
        while time.monotonic()<until:
            if (folder/'events.jsonl').exists():
                events=[json.loads(line) for line in (folder/'events.jsonl').read_text().splitlines()]
                snapshots=[e for e in events if e['event']=='socket_snapshot']
                if snapshots: break
            time.sleep(.2)
        assert state('remotehub-p007b-r06-observer.service')=='active'
        assert snapshots and not snapshots[-1]['relayPorts']
        command(planner.plan(run_id,int(time.time()),start)['startWssCommand'])
        time.sleep(.5); assert state('remotehub-p007b-r06-wss.service')=='active'
        assert state('remotehub-turn-g6.service','UnitFileState')=='disabled'
        result=dict(activatedAtNs=time.time_ns(),runDirectory=str(folder),authorizationStart=start,
            authorizationDeadline=window['authorizationDeadline'],guardTarget=window['guardTarget'],invitationExpiresAt=expires,
            guardSmokePassed=True,guardTimerVerifiedBeforeCredentials=True,turnPid=pid,initialAllocations=0,
            wssBinarySha256=hashlib.sha256(binary.read_bytes()).hexdigest(),nonRootWss=state('remotehub-p007b-r06-wss.service','User')=='remotehub-p006b',
            observerActive=True,autostart=False,syntheticOnly=True)
        write('activation.json',json.dumps(result)); return result
    except Exception:
        guard.cleanup(guard.checked_directory(str(folder)),reason='error')
        command(['systemctl','stop','remotehub-p007b-r06-expiry.timer'],False)
        raise ValueError('R06_ACTIVATION_FAILED_CLEANUP_REQUESTED') from None
