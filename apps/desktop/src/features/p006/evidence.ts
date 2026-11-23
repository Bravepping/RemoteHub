import type { LanReceiver } from '../p004/lan';
export interface ReceiverEvidence {
  version: 1;
  stage: 'pre_close' | 'cleanup';
  capture: 'live' | 'last_known' | 'unavailable';
  sampledAtMs: number | null;
  recordedAtMs: number;
  snapshot: LanReceiver | null;
  errors: string[];
  cleanup: {
    controlClosed: boolean | null;
    peerClosed: boolean | null;
    tracksStopped: boolean | null;
    nativeResourcesReleased: boolean | null;
  };
}
export function evidenceCode(error: unknown): string {
  const code = String(error).replace(/^Error:\s*/, '');
  const allowed = [
    'RECEIVER_INTERNAL_ERROR',
    'RECEIVER_EVIDENCE_TIMEOUT',
    'RECEIVER_EVIDENCE_SCHEMA',
    'RECEIVER_EVIDENCE_METRICS',
    'RECEIVER_EVIDENCE_SIZE',
    'RECEIVER_EVIDENCE_ENCODING',
    'RECEIVER_EVIDENCE_DIRECTORY',
    'RECEIVER_EVIDENCE_OWNER',
    'RECEIVER_EVIDENCE_STAGE_REPLAY',
    'RECEIVER_EVIDENCE_EXISTS_OR_WRITE',
    'RECEIVER_EVIDENCE_WRITE',
    'RECEIVER_EVIDENCE_WRONG_ROLE',
    'RECEIVER_EVIDENCE_LOCK',
    'BROWSER_PC_MISSING',
    'BROWSER_SELECTED_DIRECT_PATH_UNVERIFIED',
    'BROWSER_SELECTED_RELAY_UNVERIFIED',
    'BROWSER_RELAY_CANDIDATE_MISSING',
    'BROWSER_RELAY_GATHER_TIMEOUT',
    'BROWSER_RELAY_SDP_INVALID',
    'BROWSER_RELAY_CANDIDATE_UNVERIFIED',
    'BROWSER_RELAY_PC_CREATE_FAILED',
    'BROWSER_RELAY_OFFER_CREATE_FAILED',
    'BROWSER_RELAY_LOCAL_SDP_APPLY_FAILED',
    'BROWSER_RELAY_ANSWER_APPLY_FAILED',
    'R06_EXPLICIT_MODE_REQUIRED',
    'R06_REQUESTED_MODE_MISMATCH',
    'R06_RELAY_PATH_METRIC_INVALID',
    'R06_RELAY_SDP_INVALID',
    'R06_RELAY_SDP_NOT_REDACTED',
    'R06_RELAY_CANDIDATE_INVALID',
    'R06_SYNTHETIC_8_SECONDS_REQUIRED',
    'R06_INVITATION_WINDOW_EXCEEDS_20_MINUTES',
    'METRICS_INVALID',
    'RELAY_APPROVAL_REQUIRED',
    'RELAY_APPROVAL_EXPIRED',
    'RELAY_CANDIDATE_LIMIT',
    'RELAY_CANDIDATE_REQUIRED',
    'RELAY_GRANT_REQUIRED',
    'ICE_MODE_NOT_ENABLED_IN_P006A',
    'ICE_CONFIG_INVALID',
    'ICE_SERVERS_LIMIT',
    'ICE_SHORT_CREDENTIAL_OR_ENDPOINT_INVALID',
    'SDP_SIZE_OR_ENCODING',
    'H264_BASELINE_UNAVAILABLE',
    'H264_BASELINE_42001F_MODE1_REQUIRED',
    'P006_CONFIG_EXPIRED_OR_INVALID',
    'P006_CONFIG_FILE_POLICY',
    'P006_CONFIG_READ',
    'P006_CONFIG_SCHEMA',
    'P006_INVITATION_ROLE_MISMATCH',
    'P006_PRIVATE_CONFIG_REQUIRED',
    'P006_CONFIG_INVALID',
    'P006_ICE_CONFIG_INVALID',
    'P006_CA_INVALID',
    'P006_WSS_TLS_OR_AUTH_FAILED',
    'P006_WSS_CONNECT_TIMEOUT',
    'P006_CLEANUP_PENDING',
    'P006_WSS_CLOSED',
    'P006_SIGNAL_LEASE_EXPIRED',
    'P006_TASK_FAILED',
    'P006_STOPPED',
    'P006_RECEIVER_STOPPED',
    'P006_SIGNAL_CLOSED',
    'REPORT_WRITE',
    'REPORT_EXISTS_OR_WRITE',
    'WINDOW_FINAL_STATS_UNAVAILABLE',
    'DIRECT_SELECTED_PAIR_NOT_APPROVED',
    'CONTROL_CHANNEL_ERROR',
    'CONTROL_RESPONSE_INVALID',
    'CONTROL_SEND_FAILED',
    'CONTROL_SEND_LIMIT',
    'DIAG_GETSTATS_FAILURE',
  ];
  return allowed.includes(code) ? code : 'RECEIVER_INTERNAL_ERROR';
}
export async function evidenceBounded<T>(
  operation: Promise<T>,
  ms = 1800,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      operation,
      new Promise<T>((_, reject) => {
        timer = setTimeout(
          () => reject(new Error('RECEIVER_EVIDENCE_TIMEOUT')),
          ms,
        );
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
