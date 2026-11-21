import { invoke } from '@tauri-apps/api/core';

export type ProbeState =
  | 'idle'
  | 'starting'
  | 'negotiating'
  | 'connected'
  | 'playing'
  | 'stopping'
  | 'stopped'
  | 'failed';

export interface ProbeStats {
  state: ProbeState;
  framesEncoded: number;
  bytesSent: number;
  framesSent: number;
  packetsSent: number;
  rtpPacketsSent?: number | null;
  rtpBytesSent?: number | null;
  candidatePairType?: string | null;
  rtcpPliReceived?: number | null;
  rtcpFirReceived?: number | null;
  rtcpNackReceived?: number | null;
  rtcpReportedPacketsLost?: number | null;
  profileLevelId?: string | null;
  spsProfileLevelId?: string | null;
  packetizationMode?: number | null;
  payloadType?: number | null;
  submittedFrames?: number | null;
  missedTicks?: number | null;
  encoder?: string | null;
  idrFrames?: number | null;
  firstPts100ns?: number | null;
  lastPts100ns?: number | null;
  backpressureCount: number;
  elapsedMs: number;
  errorCode?: string | null;
}

interface Receipt {
  sessionId: string;
  offer: string;
}

export const startLoopback = (seconds: number) =>
  invoke<Receipt>('p004_start_loopback_probe', {
    config: {
      seconds,
      width: 1280,
      height: 720,
      fps: 30,
      bitrateBps: 4_000_000,
    },
  });

export const signalLoopback = (sessionId: string, answer: string) =>
  invoke<void>('p004_signal', { sessionId, answer });

export const getLoopbackStats = (sessionId: string) =>
  invoke<ProbeStats>('p004_get_stats', { sessionId });

export const stopLoopback = (sessionId?: string) =>
  invoke<void>('p004_stop_loopback_probe', sessionId ? { sessionId } : {});

export const probeErrorCode = (error: unknown): string => {
  const message = error instanceof Error ? error.message : error;
  // Avoid echoing underlying exceptions or sensitive SDP into the UI.
  return typeof message === 'string'
    ? (message.match(/^[A-Z][A-Z0-9_]{2,40}(?=[:\s]|$)/)?.[0] ?? 'PROBE_FAILED')
    : 'PROBE_FAILED';
};
