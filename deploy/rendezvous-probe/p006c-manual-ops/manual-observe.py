"""Explicit manual experiment observer. Labels remain private; no media capture."""
import json,pathlib,threading,time,uuid,os
from turn_lifecycle_observer import run,read_capture
f=pathlib.Path('/opt/remotehub-p006c-manual-20261122');config=f/'observer-private.json'
base=json.loads((f/'baseline-private.json').read_text());done=threading.Event()
def discover():
    try:
        while not done.is_set():
            c=json.loads(config.read_text());changed=False
            for _,kind,_,values,_,_ in read_capture(f/'control-private.pcap'):
                if kind not in (3,4):continue
                user=values.get(6,(b'',0))[0].decode('ascii',errors='replace')
                parts=user.split(':')
                if len(parts)!=2 or not parts[0].isdigit():continue
                try:uuid.UUID(parts[1])
                except ValueError:continue
                if not base['timeNs']//10**9-5<=int(parts[0])<=base['expiresAt']+600:continue
                if user not in c['labels']:
                    c['labels'][user]='manual-session-'+str(len(c['labels'])+1);changed=True
            if changed:
                p=f/'observer-next-private.json';p.write_text(json.dumps(c));p.chmod(0o600);os.replace(p,config)
            done.wait(.25)
    except Exception as e:
        (f/'discovery-error-private.txt').write_text(type(e).__name__)
        (f/'stop-observer').touch()
t=threading.Thread(target=discover);t.start()
try:run(config,f,5400,max_allocations=4,split_allocations=True)
finally:done.set();t.join(timeout=3)
