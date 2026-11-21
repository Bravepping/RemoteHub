"""Offline template/config checks. Never starts coturn, reads secrets into logs, or probes a host."""
import argparse,ipaddress,pathlib,re,sys
HERE=pathlib.Path(__file__).resolve().parent
def rfc1918(ip):
    return any(ip in ipaddress.ip_network(n) for n in ['10.0.0.0/8','172.16.0.0/12','192.168.0.0/16'])
BASE_FLAGS={'fingerprint','use-auth-secret','no-tcp','no-tcp-relay','no-tls','no-dtls','no-cli','no-multicast-peers','no-stdout-log'}
NUMBERS={'listening-port':'3478','min-port':'49160','max-port':'49259','user-quota':'4','total-quota':'8','max-bps':'1000000','bps-capacity':'4000000','stale-nonce':'600','max-allocate-lifetime':'600','max-allocate-timeout':'15'}
def parse(text):
    if len(text)>16384:raise ValueError('CONFIG_SIZE')
    out={}
    for raw in text.splitlines():
        line=raw.strip()
        if not line or line.startswith('#'):continue
        key,sep,value=line.partition('=')
        if not re.fullmatch(r'[a-z-]+',key):raise ValueError('CONFIG_KEY')
        if key!='denied-peer-ip' and key in out:raise ValueError('CONFIG_DUPLICATE')
        out.setdefault(key,[]).append(value if sep else None)
    return out
def check(text,template=False):
    values=parse(text.replace('${TURN_EXTERNAL_IP_LINE}','# mapping decision required') if template else text)
    allowed=BASE_FLAGS|set(NUMBERS)|{'listening-ip','relay-ip','external-ip','static-auth-secret','realm','log-file','denied-peer-ip'}
    if template:
        # The external mapping is a whole optional directive, inserted only by the renderer.
        values=parse(text.replace('${TURN_EXTERNAL_IP_LINE}','# mapping decision required'))
    if set(values)-allowed:raise ValueError('CONFIG_UNAPPROVED_DIRECTIVE')
    if any(values.get(k)!=[None] for k in BASE_FLAGS):raise ValueError('CONFIG_SECURITY_FLAGS')
    if any(values.get(k)!=[v] for k,v in NUMBERS.items()):raise ValueError('CONFIG_LIMITS')
    if values.get('log-file')!=['/dev/null']:raise ValueError('CONFIG_LOG_POLICY')
    baseline=parse((HERE/'turnserver.conf.example').read_text(encoding='utf-8').replace('${TURN_EXTERNAL_IP_LINE}','# map'))
    if set(values.get('denied-peer-ip',[]))!=set(baseline['denied-peer-ip']):raise ValueError('CONFIG_PEER_DENY_POLICY')
    if template:
        names=set(re.findall(r'\$\{([A-Z_]+)\}',text))
        if names!={'TURN_LISTEN_IP','TURN_RELAY_IP','TURN_EXTERNAL_IP_LINE','TURN_SHARED_SECRET','TURN_REALM'}:raise ValueError('TEMPLATE_PLACEHOLDERS')
        return
    if '${' in text:raise ValueError('CONFIG_UNRENDERED')
    for key in ['listening-ip','relay-ip']:
        if key not in values:raise ValueError('CONFIG_INTERFACE_REQUIRED')
        ip=ipaddress.IPv4Address(values[key][0])
        if not (ip.is_global or rfc1918(ip)):raise ValueError('CONFIG_INTERFACE_INVALID')
    secret=values.get('static-auth-secret',[''])[0] or ''
    if not re.fullmatch(r'[A-Za-z0-9+/_=-]{32,128}',secret):raise ValueError('CONFIG_SECRET_REQUIRED')
    realm=values.get('realm',[''])[0] or ''
    if not re.fullmatch(r'[A-Za-z0-9.-]{1,128}',realm):raise ValueError('CONFIG_REALM_REQUIRED')
    if 'external-ip' in values:
        mapping=values['external-ip'][0].split('/')
        if len(mapping)!=2:raise ValueError('CONFIG_EXTERNAL_MAPPING')
        public,private=map(ipaddress.IPv4Address,mapping)
        if not public.is_global or not rfc1918(private) or str(private)!=values['relay-ip'][0]:raise ValueError('CONFIG_EXTERNAL_MAPPING')
    elif not ipaddress.IPv4Address(values['relay-ip'][0]).is_global:raise ValueError('CONFIG_NAT_MAPPING_REQUIRED')
def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--template',action='store_true');parser.add_argument('--config',type=pathlib.Path)
    args=parser.parse_args();path=args.config or HERE/'turnserver.conf.example'
    try:
        if not args.template and sys.platform!='win32' and path.stat().st_mode&0o007:raise ValueError('CONFIG_WORLD_READABLE')
        check(path.read_text(encoding='utf-8'),args.template)
    except (ValueError,OSError):
        # Never echo raw config lines, filenames containing user context, or secret values.
        print('FAIL: configuration/policy check; private details suppressed',file=sys.stderr);return 1
    print('PASS: offline configuration policy; coturn syntax/runtime and cloud network NOT tested');return 0
if __name__=='__main__':raise SystemExit(main())
