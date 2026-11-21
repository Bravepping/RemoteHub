"""Explicit local rendering only. No install, service, firewall or network operations."""
import argparse,os,pathlib,re,stat,sys
from verify_config import HERE,check
KEYS={'TURN_LISTEN_IP','TURN_RELAY_IP','TURN_REALM','TURN_SHARED_SECRET','TURN_PUBLIC_IP','TURN_NAT_PRIVATE_IP'}
def render(env_path,output):
    if env_path.is_symlink() or output.exists():raise ValueError('INPUT_SYMLINK_OR_OUTPUT_EXISTS')
    if sys.platform!='win32' and env_path.stat().st_mode&0o077:raise ValueError('ENV_MUST_BE_PRIVATE')
    data={}
    for line in env_path.read_text(encoding='utf-8').splitlines():
        line=line.strip()
        if not line or line.startswith('#'):continue
        key,sep,value=line.partition('=')
        if not sep or key not in KEYS or key in data or not re.fullmatch(r'[A-Za-z0-9+/_=.:\-]*',value):raise ValueError('ENV_FORMAT')
        data[key]=value
    if set(data)!=KEYS or any(not data[k] for k in ['TURN_LISTEN_IP','TURN_RELAY_IP','TURN_REALM','TURN_SHARED_SECRET']):raise ValueError('ENV_REQUIRED')
    public,private=data['TURN_PUBLIC_IP'],data['TURN_NAT_PRIVATE_IP']
    if bool(public)!=bool(private):raise ValueError('MAPPING_INCOMPLETE')
    data['TURN_EXTERNAL_IP_LINE']=f'external-ip={public}/{private}' if public else '# direct addressing; no external-ip mapping'
    text=(HERE/'turnserver.conf.example').read_text(encoding='utf-8')
    text=re.sub(r'\$\{([A-Z_]+)\}',lambda m:data[m[1]],text)
    check(text)
    fd=os.open(output,os.O_WRONLY|os.O_CREAT|os.O_EXCL,stat.S_IRUSR|stat.S_IWUSR)
    with os.fdopen(fd,'w',encoding='utf-8',newline='\n') as f:f.write(text)
def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--env',required=True,type=pathlib.Path);p.add_argument('--output',required=True,type=pathlib.Path);a=p.parse_args()
    try:render(a.env,a.output)
    except (ValueError,OSError):print('FAIL: private input/policy/output check; values suppressed',file=sys.stderr);return 1
    print('PASS: private config created, no deployment performed');return 0
if __name__=='__main__':raise SystemExit(main())
