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
