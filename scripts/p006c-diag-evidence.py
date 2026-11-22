"""Offline consistency/privacy check of DIAG test evidence. No sockets or desktop access."""
import hashlib,json,pathlib,re,subprocess,datetime
from network_privacy import check_files

def verify(root):
    out=root/'artifacts/p006c-diag'
    checks=json.loads((out/'automatic/results.json').read_text())
    assert len(checks)==35 and all(c['exitCode']==c.get('expectedExitCode',0) for c in checks)
    rust=sum(map(int,re.findall(r'test result: ok\. (\d+) passed',(out/'automatic/test.txt').read_text())))
    frontend=int(re.search(r'Tests\s+(\d+) passed',(out/'automatic/frontend-unit.txt').read_text()).group(1))
    runners=json.loads((out/'local/results.json').read_text())
    assert len(runners)==5 and all(r['exitCode']==0 for r in runners)
    rows=[]
    for scenario in ['auto','sender-stop','receiver-stop','stats-failure','wss-disconnect']:
        v=json.loads((out/f'local/{scenario}.json').read_text());s=v['native']['sender']['source'];records=v['receiverEvidence']
        runner=next(r for r in runners if r['scenario']==scenario)
        assert datetime.datetime.fromisoformat(v['finishedAt'].replace('Z','+00:00'))<=datetime.datetime.fromisoformat(runner['finishedAt'].replace('Z','+00:00'))
        assert v['passed'] and s['source_kind']=='synthetic_nv12' and s['ice_transport_policy']=='all'
        assert s['direct_pair_diagnostic']['approved'] and s['direct_pair_diagnostic']['local']['reason']=='matched'
        assert v['securityReportChecks']=={'senderRoleDenied':True,'foreignOwnerDenied':True}
        assert len(records)==2 and records[0]['stage']=='pre_close' and records[1]['stage']=='cleanup'
        assert records[0]['cleanup']['peerClosed'] is False and records[1]['cleanup']['peerClosed'] is True
        assert records[1]['cleanup']['tracksStopped'] and records[1]['cleanup']['nativeResourcesReleased']
        assert s['control']['accepted_moves']==s['control']['acks_sent']==100 and s['control']['task_joined']
        if scenario=='auto':
            assert records[1]['snapshot']['framesReceived']==records[1]['snapshot']['framesDecoded']==records[1]['snapshot']['framesRendered']==s['frames_sent']==300
        if scenario=='stats-failure':
            assert records[0]['capture']==records[1]['capture']=='last_known'
            assert records[0]['sampledAtMs']==records[1]['sampledAtMs']<records[1]['recordedAtMs']
            assert 'DIAG_GETSTATS_FAILURE' in records[1]['errors']
        if scenario=='wss-disconnect':assert v['exitCode']==1 and 'P006_WSS_CLOSED' in records[1]['errors']
        rows.append(dict(scenario=scenario,startedAt=v['startedAt'],finishedAt=v['finishedAt'],sourceFrames=s['frames_sent'],
            persisted=[dict(stage=e['stage'],capture=e['capture'],framesDecoded=(e['snapshot'] or {}).get('framesDecoded'),
                           sampledAtMs=e['sampledAtMs'],recordedAtMs=e['recordedAtMs'],errors=e['errors'],cleanup=e['cleanup']) for e in records],
            nativeExitCode=v['exitCode'],expectedFailure=scenario in ['stats-failure','wss-disconnect'],passed=True))
    check_files(p for p in out.rglob('*') if p.suffix in ('.json', '.txt'))
    check_files([root/'scripts/p006c-diag-evidence.py', root/'scripts/network_privacy.py'], source=True)
    private=root/'artifacts/private';assert not any(private.glob('p006c-wss-*'))
    names=subprocess.check_output(['git','ls-files','--cached','--others','--exclude-standard','-z','--','apps','crates','scripts'],cwd=root).decode().split('\0')
    digest=hashlib.sha256()
    for name in sorted(filter(None,names)):digest.update(name.encode()+b'\0'+hashlib.sha256((root/name).read_bytes()).digest())
    binaries={name:hashlib.sha256((root/'target/probe'/name).read_bytes()).hexdigest() for name in ['remotehub-desktop.exe','authenticated-session-probe.exe']}
    summary=dict(recordedAt=datetime.datetime.now().astimezone().isoformat(),scope='offline regression + one physical Windows generated-video real localhost TLS/WSS/MF/RTP/Edge/SCTP; no new cloud or two-PC/desktop test',
        regressionCommands=35,rustPassed=rust,frontendPassed=frontend,edgePassed=3,pythonPassed=12,
        localCases=rows,localTemporaryTlsConfigsAndKeysDeleted=True,cloudStartedThisRound=False,realDesktopCapturedThisRound=False,
        sourceDigest=digest.hexdigest(),sourceDigestAlgorithm='sha256 over sorted apps/crates/scripts path + NUL + each file SHA256 digest',binaries=binaries,
        historicalRootCauseLocated=False,minimumOnlinePlan='docs/P006C_DIAG_MINIMUM_ONLINE_PLAN.md')
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(dict(regressionCommands=35,rustPassed=rust,frontendPassed=frontend,localCases=len(rows),privacyAndCleanupPassed=True,cloudStarted=False)))

if __name__=='__main__':
    try:verify(pathlib.Path(__file__).resolve().parent.parent)
    except Exception:raise SystemExit('DIAG_EVIDENCE_VERIFICATION_FAILED_NO_INPUT_ECHO') from None
