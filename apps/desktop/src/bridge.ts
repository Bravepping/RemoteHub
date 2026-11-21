import { invoke, isTauri } from '@tauri-apps/api/core';

export interface RuntimeInfo {
  app_version: string;
  os: string;
  arch: string;
  capture_backend: string;
  remote_access_enabled: boolean;
  p004_enabled?: boolean;
  p004_lan_enabled?: boolean;
}

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
  if (!isTauri()) {
    throw new Error('当前为浏览器预览。请启动桌面客户端以检测本机能力。');
  }
  return invoke<RuntimeInfo>('get_runtime_info');
}

export async function runCaptureProbe(): Promise<ProbeReport> {
  return invoke<ProbeReport>('run_capture_probe', { consent: true });
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
