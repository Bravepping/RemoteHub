"""Offline decisions from fresh sanitized read-only inspection. No SSH/service actions."""
import datetime
import hashlib
import json
import pathlib
from network_privacy import check_files

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT/'artifacts/r06-preflight'
OPS = ROOT/'deploy/rendezvous-probe/r06-ops'


def inspect():
    value = json.loads((OUT/'server-readonly.json').read_text())
    checks = {}
    units = value['units']; c = value['coturnConfig']
    for name in ['remotehub-turn-g6.service','remotehub-p006b.service']:
        checks[name+'-stopped-disabled'] = units[name]['ActiveState']=='inactive' and units[name]['MainPID']=='0' and units[name]['UnitFileState']=='disabled'
    checks['new-wss-observer-timer-not-installed'] = all(units[n]['LoadState']=='not-found' for n in ['remotehub-p007b-r06-wss.service','remotehub-p007b-r06-observer.service','remotehub-p007b-r06-expiry.timer','remotehub-p007b-r06-expiry.service'])
    checks['dedicated-ports-free'] = not value['dedicatedListeners']
    selected = next(b for b in value['wssBinaries'] if b['path'].startswith('/opt/remotehub-p006c-manual-'))
    checks['exact-selected-wss-matches'] = selected['expectedShaMatches'] and selected['rootOwned'] and not selected['symlink'] and not selected['groupWritable']
    checks['existing-configuration-within-preserved-policy'] = all(c[k] for k in ['authSecretEnabled','noTcp','noTcpRelay','noCli','noTls','noDtls','noMulticast','noStdoutLog','logDiscarded','secretMatchesPrivateEnv','issuerSecretMatches','externalMappingMatchesPrivateEnv','realmMatchesPrivateEnv']) and (c['listening-port'],c['min-port'],c['max-port'],c['user-quota'],c['total-quota'],c['max-allocate-lifetime'])==(3478,49160,49259,4,8,600)
    checks['prior-temporary-material-absent'] = all(x['remainingPrivateFileCount']==0 for x in value['previousWindowTemporaryFiles']) and not value['newR06Directories']
    checks['host-key-pinned-no-service-or-credential-change'] = value['sshPinnedHostKeyVerified'] and value['connectionClosedAfterInspection'] and value['readOnly'] and not value['servicesStarted'] and not value['credentialsGenerated'] and not value['filesCreatedOnServer']
    checks['tools-and-non-root-wss-account-present'] = all(value['dependencies'].values()) and value['existingLowPrivilegeAccount'] and not value['lowPrivilegeAccountIsRoot']
    checks['no-secret-file-world-readable'] = all(not value['protectedFiles'][p]['worldReadable'] for p in ['/etc/remotehub-turn-g6/private.env','/etc/remotehub-p006b/issuer.env','/etc/remotehub-turn-g6/turnserver.conf'])
    helper_hashes = {}
    for name in ['turn_lifecycle_observer.py','turn_lifecycle_analyze.py','g6_turn_wire.py']:
        actual = value['existingHelpers']['/opt/remotehub-p006c-manual-20261122/'+name]['fileSha256']
        helper_hashes[name] = hashlib.sha256((ROOT/'scripts'/name).read_bytes()).hexdigest()==actual
    result = dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
                  inspectionTimestamp=value['localRecordedAt'], checks=checks, readOnlyBaselineMatches=all(checks.values()),
                  observerHelpersMatchLocalSource=helper_hashes, selectedWssBinary=selected['path'], wssSha256=selected['fileSha256'],
                  limitations=['no startup or public TLS/data probe performed','new expiry/observer guard is staged locally, not installed or tested on server',
                               'coturn total quota is 8, user quota is 4; this experiment adds single-session sequencing and observer cap 4',
                               'old 90-minute cleanup is not reusable for the new run directory; future guard must be verified before startup'],
                  proposedLocalFileHashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in OPS.glob('*.py')},
                  cloudStartAuthorization='PENDING', cloudServicesStarted=False, realTwoPcTest='NOT_RUN', R06='OPEN')
    check_files([OUT/'server-readonly.json']); check_files(OPS.glob('*.py'),source=True)
    (OUT/'readiness.json').write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({'baselineChecks':len(checks),'baselineMatches':all(checks.values()),'helperSourcesMatch':all(helper_hashes.values()),
                      'cloudServicesStarted':False,'startAuthorization':'PENDING'}))


if __name__=='__main__':
    try: inspect()
    except Exception: raise SystemExit('R06_READINESS_FAILED_NO_PRIVATE_ECHO') from None
