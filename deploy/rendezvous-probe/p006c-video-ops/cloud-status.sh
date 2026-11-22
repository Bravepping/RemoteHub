set -eu
cd /opt/remotehub-p006c-video-20261122
python3 - <<'PY'
import pathlib,json,os,time,subprocess,collections
from turn_lifecycle_observer import relay_sockets
f=pathlib.Path.cwd();p=f/'observer-private.json';c=json.loads(p.read_text())
incoming=f/'labels-incoming-private.json'
if incoming.exists():
 labels=json.loads(incoming.read_text());assert set(labels)=={'labels'}
 assert all(isinstance(k,str) and isinstance(v,str) and len(v)<64 for k,v in labels['labels'].items())
 c['labels'].update(labels['labels']);tmp=f/'observer-next-private.json';tmp.write_text(json.dumps(c));tmp.chmod(0o600);os.replace(tmp,p);incoming.unlink();time.sleep(1)
events=[json.loads(l) for l in (f/'events.jsonl').read_text().splitlines() if l.strip()]
s=[e for e in events if e['event']=='socket_snapshot'];wire=[e for e in events if e['event']=='turn_wire']
summary=dict(timeNs=time.time_ns(),turnPid=c['turn_pid'],currentRelayPorts=relay_sockets(c['turn_pid']),maximumConcurrentAllocations=max(len(e['relayPorts']) for e in s),snapshots=len(s),wireEvents=len(wire),cases=dict(collections.Counter(e['case'] for e in wire)),observerExit=(f/'observer-exit.txt').read_text().strip() if (f/'observer-exit.txt').exists() else None,observerError=(f/'observer-stderr-private.txt').read_text(),wssActive=subprocess.check_output(['systemctl','show','remotehub-p006c-video.service','-p','ActiveState','--value'],text=True).strip()=='active')
print(json.dumps(summary))
PY
