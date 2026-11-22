"""Run only with --read-only. No start, writes, credentials, probes or config changes."""
import hashlib
import json
import os
import pathlib
import re
import shutil
import stat
import subprocess
import sys
import time

EXPECTED_WSS_SHA = '946ebb86fc60e74d00b3db6107dddb0043132b4afcfc7873d0ca5b96087c2653'
WSS_PATHS = [
    '/opt/remotehub-p006c-manual-20261122/hub-rendezvous-probe',
    '/opt/remotehub-p006c-video-20261122/hub-rendezvous-probe',
    '/opt/remotehub-p006b/hub-rendezvous-probe',
]


def run(args):
    result = subprocess.run(args, capture_output=True, text=True, timeout=12)
    if result.returncode not in [0, 1, 3, 4]:
        raise ValueError('READ_ONLY_COMMAND_FAILED')
    return result.stdout


def unit(name):
    keys = ['LoadState', 'ActiveState', 'SubState', 'UnitFileState', 'MainPID', 'User', 'Group', 'NoNewPrivileges',
            'ProtectSystem', 'ProtectHome', 'RuntimeMaxUSec', 'Restart', 'FragmentPath']
    result = run(['systemctl', 'show', name, *[arg for key in keys for arg in ['-p', key]]])
    fields = dict(line.split('=', 1) for line in result.splitlines() if '=' in line)
    return {key: fields.get(key, '') for key in keys}


def properties(path, public_hash=False):
    p = pathlib.Path(path)
    if not p.exists():
        return {'exists': False}
    st = p.lstat()
    result = dict(exists=True, regular=stat.S_ISREG(st.st_mode), symlink=p.is_symlink(),
                  mode=oct(stat.S_IMODE(st.st_mode)), rootOwned=st.st_uid == 0, worldReadable=bool(st.st_mode & 4),
                  groupWritable=bool(st.st_mode & 16))
    if public_hash and result['regular'] and not result['symlink']:
        result['fileSha256'] = hashlib.sha256(p.read_bytes()).hexdigest()
    return result


def env(path):
    return dict(line.split('=', 1) for line in pathlib.Path(path).read_text().splitlines()
                if line and not line.startswith('#') and '=' in line)


def config_fields(path):
    data = {}
    for line in pathlib.Path(path).read_text().splitlines():
        line = line.strip()
        if line and not line.startswith('#'):
            key, _, value = line.partition('=')
            data[key] = value
    return data


def main():
    if sys.argv[1:] != ['--read-only'] or os.name != 'posix':
        raise ValueError('READ_ONLY_FLAG_REQUIRED')
    import pwd
    started = time.time_ns()
    units = {name: unit(name) for name in ['remotehub-turn-g6.service', 'remotehub-p006b.service',
             'remotehub-p006c-video.service', 'remotehub-p006c-manual.service', 'remotehub-p007b-r06-wss.service',
             'remotehub-p007b-r06-observer.service',
             'remotehub-p006c-manual-expiry.timer', 'remotehub-p006c-video-expiry.timer',
             'remotehub-p007b-r06-expiry.timer', 'remotehub-p007b-r06-expiry.service']}
    sockets = []
    for line in run(['ss', '-H', '-lntup']).splitlines():
        try:
            port = int(line.split()[4].rsplit(':', 1)[1])
        except (ValueError, IndexError):
            continue
        if port in [3478, 8443] or 49160 <= port <= 49259:
            sockets.append({'protocol': line.split()[0], 'port': port})
    binaries = []
    for path in WSS_PATHS:
        item = {'path': path, **properties(path, True)}
        item['expectedShaMatches'] = item.get('fileSha256') == EXPECTED_WSS_SHA
        binaries.append(item)
    conf_path = '/etc/remotehub-turn-g6/turnserver.conf'
    c = config_fields(conf_path)
    turn_env = env('/etc/remotehub-turn-g6/private.env')
    issuer_env = env('/etc/remotehub-p006b/issuer.env')
    safe_config = {key: int(c[key]) for key in ['listening-port', 'min-port', 'max-port', 'user-quota', 'total-quota',
                  'max-allocate-lifetime', 'max-allocate-timeout']}
    safe_config.update(authSecretEnabled='use-auth-secret' in c,
                       noTcp='no-tcp' in c, noTcpRelay='no-tcp-relay' in c, noCli='no-cli' in c,
                       noTls='no-tls' in c, noDtls='no-dtls' in c, noMulticast='no-multicast-peers' in c,
                       noStdoutLog='no-stdout-log' in c, logDiscarded=c.get('log-file') == '/dev/null',
                       secretMatchesPrivateEnv=c.get('static-auth-secret') == turn_env.get('TURN_SHARED_SECRET'),
                       issuerSecretMatches=issuer_env.get('REMOTEHUB_TURN_SHARED_SECRET') == turn_env.get('TURN_SHARED_SECRET'),
                       externalMappingMatchesPrivateEnv=c.get('external-ip', '').split('/')[0] == turn_env.get('TURN_PUBLIC_IP'),
                       realmMatchesPrivateEnv=c.get('realm') == turn_env.get('TURN_REALM'),
                       deniedPeerRuleCount=len(re.findall(r'^denied-peer-ip=', pathlib.Path(conf_path).read_text(), re.M)))
    account = pwd.getpwnam('remotehub-p006b')
    files = {p: properties(p, not p.endswith('.env')) for p in [
        '/etc/systemd/system/remotehub-turn-g6.service', '/etc/systemd/system/remotehub-p006b.service',
        '/etc/remotehub-turn-g6/turnserver.conf', '/etc/remotehub-turn-g6/private.env', '/etc/remotehub-p006b/issuer.env']}
    old_private = []
    for parent in ['/opt/remotehub-p006c-manual-20261122', '/opt/remotehub-p006c-video-20261122']:
        names = ['server.json', 'client-private.json', 'server-key.pem', 'ca-key.pem', 'observer-private.json', 'control-private.pcap']
        old_private.append({'directory': parent, 'remainingPrivateFileCount': sum((pathlib.Path(parent) / name).exists() for name in names)})
    helpers = {}
    for name in ['manual-cleanup.py', 'manual-observe.py', 'turn_lifecycle_observer.py', 'turn_lifecycle_analyze.py', 'g6_turn_wire.py']:
        p = '/opt/remotehub-p006c-manual-20261122/' + name
        helpers[p] = properties(p, True)
    legacy_cleanup = pathlib.Path('/opt/remotehub-p006c-manual-20261122/manual-cleanup.py').read_text()
    legacy_observe = pathlib.Path('/opt/remotehub-p006c-manual-20261122/manual-observe.py').read_text()
    running_services = run(['systemctl', 'list-units', '--type=service', '--state=running', '--no-pager', '--plain', '--no-legend'])
    containers = run(['docker', 'inspect', '--format', '{{.Name}}|{{.State.StartedAt}}', 'tsmusicbot', 'teamspeak'])
    firewall = run(['iptables', '-S', 'INPUT']) + run(['iptables', '-S', 'OUTPUT'])
    nft = re.sub(r'counter packets \d+ bytes \d+', 'counter', run(['nft', 'list', 'ruleset']))
    result = dict(version=1, startedAtNs=started, finishedAtNs=time.time_ns(), readOnly=True,
                  servicesStarted=False, credentialsGenerated=False, filesCreatedOnServer=False,
                  units=units, dedicatedListeners=sockets, wssBinaries=binaries, coturnConfig=safe_config,
                  protectedFiles=files, existingLowPrivilegeAccount=True, lowPrivilegeAccountIsRoot=account.pw_uid == 0,
                  dependencies={name: shutil.which(name) is not None for name in ['python3', 'openssl', 'systemd-run', 'tcpdump', 'ss']},
                  previousWindowTemporaryFiles=old_private, existingHelpers=helpers,
                  legacyCleanupTargetsOldDirectory='/opt/remotehub-p006c-manual-20261122' in legacy_cleanup,
                  legacyObserverSeconds5400='5400' in legacy_observe,
                  newR06Directories=[p.name for p in pathlib.Path('/run').glob('remotehub-p007b-r06-*')],
                  runningServiceCount=len(running_services.splitlines()),
                  runningServiceSetSha256=hashlib.sha256(running_services.encode()).hexdigest(),
                  businessContainerStartTimesSha256=hashlib.sha256(containers.encode()).hexdigest(),
                  firewallPolicySha256=hashlib.sha256((firewall + nft).encode()).hexdigest(),
                  noOriginalAddressesOrSecretsExported=True)
    print(json.dumps(result))


if __name__ == '__main__':
    try:
        main()
    except Exception:
        raise SystemExit('R06_READ_ONLY_PREFLIGHT_FAILED_NO_PRIVATE_OUTPUT') from None
