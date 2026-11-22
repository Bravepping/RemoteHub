"""Whitelist counters from a private Phase3 report; never echo endpoints, SDP or inputs."""
import argparse,datetime,json,re,math
from pathlib import Path

def summary(value):
    native=value.get('native') or {}
    source=native.get('source') or {}
    control=source.get('control') or {}
    final=value.get('receiverFinal')
    receiver=final or (native.get('receiver_samples') or [{}])[-1]
    browser=receiver.get('control') or native.get('browser_control') or {}
    number=lambda obj,key:obj.get(key) if type(obj.get(key))in(int,float)and math.isfinite(obj[key]) else None
    boolean=lambda obj,key:obj.get(key) if type(obj.get(key))is bool else None
    errors={'INPUT_SELECTED_PAIR_NOT_APPROVED','INPUT_PAIR_MISSING','INPUT_PAIR_READ_FAILED','INPUT_SCTP_MISSING',
        'INPUT_NOT_AUTHORIZED','INPUT_MESSAGE_SIZE_OR_TYPE','INPUT_RATE_LIMIT','INPUT_SCHEMA','INPUT_SESSION_OR_SEQUENCE',
        'INPUT_COORDINATE','INPUT_BUTTON','INPUT_KEY_NOT_ALLOWED','INPUT_WHEEL','INPUT_SEND_BLOCKED','INPUT_RELEASE_FAILED',
        'INPUT_DPI_CONTEXT_FAILED','INPUT_DISPLAY_CHANGED','INPUT_LOCAL_KEY_HELD','INPUT_LOCAL_BUTTON_HELD','INPUT_STATE_FAILED',
        'CONTROL_CHANNEL_ERROR','CONTROL_CHANNEL_CLOSED','CONTROL_SEND_FAILED','SIGNAL_DISCONNECTED','LAN_SIGNAL_FAILED',
        'ICE_CONNECT_TIMEOUT','INPUT_NO_PUBLISHED_HOST_CANDIDATE','BROWSER_SELECTED_OFF_PEER','BROWSER_PEER_UNVERIFIED'}
    fixed=lambda obj,key:obj.get(key) if isinstance(obj.get(key),str)and (obj[key]in errors or re.fullmatch(r'SPS_PROFILE_INCOMPATIBLE_[0-9a-f]{6}',obj[key])) else None
    return dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
        source='PRIVATE_REPORT_WHITELIST',buildTag=value.get('buildTag') if value.get('buildTag')in('p3-lan-v1','p3-lan-v2','p3-lan-v3')else None,
        role=native.get('role') if native.get('role')in('sender','receiver')else None,
        nativeState=native.get('state') if native.get('state')in('waiting','negotiating','connected','stopping','stopped','failed','finishing')else None,
        authenticated=boolean(native,'authenticated'),resourcesReleased=boolean(native,'resources_released'),
        browserClosed=boolean(value,'browserClosed'),independentReceiverFinal=final is not None,
        sourceFrames=number(source,'frames_sent'),sourceInputFrames=number(source.get('stream')or{},'frames_input'),
        sourceKind=source.get('source_kind')if source.get('source_kind')in('desktop','synthetic')else None,
        sourceWidth=number(source.get('stream')or{},'source_width'),sourceHeight=number(source.get('stream')or{},'source_height'),
        hostPairApproved=boolean(source,'input_pair_approved'),
        hostPairDiagnostic=pair_summary(source.get('input_pair_diagnostic')),
        browserCandidateSummary=count_summary(value.get('browserCandidateSummary')),
        browserLocalCandidateType=receiver.get('localCandidateType')if receiver.get('localCandidateType')in('host','prflx','srflx','relay')else None,
        browserRemoteCandidateType=receiver.get('remoteCandidateType')if receiver.get('remoteCandidateType')in('host','prflx','srflx','relay')else None,
        browserConnectionState=receiver.get('connectionState')if receiver.get('connectionState')in('new','connecting','connected','disconnected','failed','closed')else None,
        browserIceState=receiver.get('iceState')if receiver.get('iceState')in('new','checking','connected','completed','disconnected','failed','closed')else None,
        hostInputAccepted=number(control,'accepted_moves'),hostAcksWritten=number(control,'acks_sent'),
        hostInputRejected=number(control,'rejected'),hostControlJoined=boolean(control,'task_joined'),
        browserFramesDecoded=number(receiver,'framesDecoded'),browserFramesRendered=number(receiver,'framesRendered'),
        browserVideoWidth=number(receiver,'videoWidth'),browserVideoHeight=number(receiver,'videoHeight'),
        browserPeerMatches=boolean(receiver,'peerMatches'),browserInputSent=number(browser,'sentMoves'),
        browserAckReceived=number(browser,'ackReceived'),browserPending=number(browser,'pending'),browserAnomalies=number(browser,'anomalies'),
        hostLastRxSeq=number(control,'last_rx_seq'),hostLastTxSeq=number(control,'last_tx_seq'),
        browserLastTxSeq=number(browser,'lastTxSeq'),browserLastRxSeq=number(browser,'lastRxSeq'),
        error=fixed(native,'error'),sourceError=fixed(source,'error'),controlError=fixed(control,'last_error'),browserControlError=fixed(browser,'lastError'),
        desktopPixelsIncluded=False,inputContentIncluded=False,endpointOrPairingCodeIncluded=False)

def count_summary(value):
    if not isinstance(value,dict):return None
    keys=('before','mdns','published','mdnsPublished')
    if any(type(value.get(k))is not int or not 0<=value[k]<=128 for k in keys):return None
    if value['mdns']>value['before']or value['published']>value['before']or value['mdnsPublished']>min(value['mdns'],value['published']):return None
    return {k:value[k]for k in keys}

def pair_summary(value):
    if not isinstance(value,dict):return None
    result={}
    reasons=('ADDRESS_OUT_OF_APPROVED_SCOPE','PROTOCOL_NOT_APPROVED','CANDIDATE_TYPE_NOT_APPROVED','NO_PUBLISHED_CANDIDATES','EXACT_MATCH','CANDIDATE_TYPE_OR_PROTOCOL_MISMATCH','PORT_CHANGED','ADDRESS_CHANGED','ENDPOINT_NOT_PUBLISHED')
    for side in ('local','remote'):
        row=value.get(side)
        if not isinstance(row,dict):return None
        out={key:row.get(key)if row.get(key)in allowed else None for key,allowed in (
            ('reason',reasons),('candidate_type',('host','prflx','srflx','relay','unknown')),('protocol',('udp','tcp','unknown')))}
        out['approved']=row.get('approved')if type(row.get('approved'))is bool else None
        out.update({k:row[k]if type(row.get(k))is int and 0<=row[k]<=128 else None for k in ('approved_count','same_address_count','same_port_count','exact_match_count')})
        result[side]=out
    return result

if __name__=='__main__':
    class SafeParser(argparse.ArgumentParser):
        def error(self,message):raise ValueError('ARGUMENTS_REJECTED')
    parser=SafeParser(description=__doc__)
    parser.add_argument('report',type=Path);parser.add_argument('--output',type=Path)
    try:
        args=parser.parse_args()
        if args.report.stat().st_size>256*1024:raise ValueError('REPORT_SIZE_REJECTED')
        result=summary(json.loads(args.report.read_text(encoding='utf-8-sig')))
        text=json.dumps(result,indent=2)+'\n'
        if args.output:args.output.write_text(text,encoding='utf-8',newline='\n')
        print(text)
    except Exception:raise SystemExit('PHASE3_SUMMARY_FAILED_NO_PRIVATE_ECHO')from None
