import { invoke } from '@tauri-apps/api/core';
import { tauriDesktopApi } from './ipc/tauriDesktopApi';
import type { RuntimeInfo } from './ipc/contracts';
export type { RuntimeInfo } from './ipc/contracts';

export interface ProbeReport {
  width: number;
  height: number;
  frames: number;
  timeouts: number;
  elapsed_ms: number;
  observed_fps: number;
  frame_timestamps_us: number[];
  adapter: string;
}

export async function getRuntimeInfo(): Promise<RuntimeInfo> {
  return tauriDesktopApi.readRuntime();
}

export async function runCaptureProbe(): Promise<ProbeReport> {
  return invoke<ProbeReport>('run_capture_probe', { consent: true });
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
