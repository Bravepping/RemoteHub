#!/usr/bin/env bash
set -euo pipefail
script_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
python_bin="${REMOTEHUB_PYTHON:-python3}"
case "${1:---template}" in
  --template) exec "$python_bin" "$script_dir/verify_config.py" --template ;;
  --config) test "$#" -eq 2; exec "$python_bin" "$script_dir/verify_config.py" --config "$2" ;;
  --inventory)
    test "$(uname -s)" = Linux || { echo 'BLOCKED: target Linux host required'; exit 2; }
    "$python_bin" - <<'PY'
import json,pathlib,shutil,subprocess
os_release={}
for line in pathlib.Path('/etc/os-release').read_text().splitlines():
    k,_,v=line.partition('=')
    if k in ['ID','VERSION_ID']:os_release[k]=v.strip('"')
version=None
if shutil.which('turnserver'):
    r=subprocess.run(['turnserver','--version'],capture_output=True,text=True,timeout=3)
    import re
    m=re.search(r'\b\d+\.\d+(?:\.\d+){0,2}\b',r.stdout+r.stderr)
    version=m.group(0) if m else 'installed-version-unparsed'
print(json.dumps({'os':os_release,'coturnVersion':version,'ipTool':bool(shutil.which('ip')),'ssTool':bool(shutil.which('ss')),'systemctl':bool(shutil.which('systemctl')),'networkProbePerformed':False,'noConfigurationChanged':True}))
PY
    ;;
  *) echo 'Usage: verify.sh [--template | --config PRIVATE_CONF | --inventory]'; exit 2 ;;
esac
