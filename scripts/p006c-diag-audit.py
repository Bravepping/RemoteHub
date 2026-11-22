"""Offline audit of old retained failures. Does not infer missing endpoints or start networking."""
import hashlib,json,pathlib,subprocess

def audit(root):
    baseline='acd19101b98f4c3c6d886442594c2927f30bde03'
    files=['crates/hub-webrtc/src/direct_policy.rs','crates/hub-webrtc/src/session_direct.rs']
    sources={p:subprocess.check_output(['git','show',baseline+':'+p],cwd=root) for p in files}
    policy=sources[files[0]].decode();session=sources[files[1]].decode()
    assert 'if !local_candidates.values.contains(&local) || !remote_candidates.values.contains(&remote)' in policy
    assert 'DIRECT_SELECTED_PAIR_NOT_APPROVED' in policy and 'Candidate::parse(&format!' in session
    rows=[]
    for i in (1,2):
        path=root/f'artifacts/p006c-manual/development/lan-attempt-{i}.json'
        content=path.read_bytes();record=json.loads(content);native=record['native'];s=native['source'];r=native['receiver_samples'][-1]
        assert native['error']=='DIRECT_SELECTED_PAIR_NOT_APPROVED' and s['frames_sent']==0
        rows.append(dict(attempt=i,retainedReportSha256=hashlib.sha256(content).hexdigest(),
            recordedError=native['error'],sourceFrames=s['frames_sent'],acceptedMoves=s['control']['accepted_moves'],nativeResourcesReleased=native['resources_released'],
            publishedLocalCandidateTypes=s['native_candidates'],
            nativeSelectedPair=s.get('native_path'),browserPeriodicPair=[r['localCandidateType'],r['remoteCandidateType']],
            conclusion='At least one native selected (IPv4 UDP endpoint, supported type) was absent from its corresponding validated set. Exact rejected side/address/port/type was not retained.',
            provenSpecificRootCause=None,notInferable=['local_or_remote_side','address_change','port_change','type_mismatch','exact_selected_endpoint'],
            excludedAtThisSpecificReturnSite=['missing_candidate_set','unsupported_candidate_type','non_udp_protocol'],
            permissionNote='A real channel open event does not grant control permission; source/media and accepted control remained zero.'))
    result=dict(scope='historical offline evidence plus exact baseline source; no cloud/no desktop/no new two-PC run',
        historicalImplementationCommit=baseline,sourceSha256={p:hashlib.sha256(b).hexdigest() for p,b in sources.items()},
        failures=rows,rootCauseLocated=False,
        observedContrast='Both failures retained local published srflx/relay only; successful native host/srflx round retained host/srflx/relay.',
        plausibleButUnprovenMechanism='A native selected local host absent from the published answer set would be rejected even though a browser reports srflx. Retained failure pair is missing, so this is a testable hypothesis, not historical attribution.',
        next='collect new redacted pair diagnosis only after separately approved minimum online scope')
    out=root/'artifacts/p006c-diag';out.mkdir(exist_ok=True,parents=True)
    (out/'historical-audit.json').write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8');print(json.dumps(result,indent=2))

if __name__=='__main__':audit(pathlib.Path(__file__).resolve().parent.parent)
