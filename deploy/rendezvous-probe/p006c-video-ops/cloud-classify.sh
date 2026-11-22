set -eu
cd /opt/remotehub-p006c-video-20261122
python3 - <<'PY'
import pathlib,json,time
from turn_lifecycle_analyze import classify,read_jsonl
from turn_lifecycle_observer import relay_sockets
f=pathlib.Path.cwd();c=json.loads((f/'observer-private.json').read_text());e=read_jsonl(f/'events.jsonl')
cases=sorted({x['case'] for x in e if x.get('kind')=='0x103'})
rows=[classify(e,case) for case in cases];ports=relay_sockets(c['turn_pid'])
out=dict(observedAtNs=time.time_ns(),beforeServiceStop=True,currentRelayPorts=ports,allocations=rows)
(f/'allocation-results.json').write_text(json.dumps(out,indent=2)+'\n')
print(json.dumps(out))
PY
