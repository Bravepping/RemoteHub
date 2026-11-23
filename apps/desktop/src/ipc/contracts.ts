export const identityErrorCodes = [
  'IDENTITY_UNSUPPORTED',
  'IDENTITY_RANDOM_FAILED',
  'IDENTITY_PROTECT_FAILED',
  'IDENTITY_UNPROTECT_FAILED',
  'IDENTITY_STORAGE_FAILED',
  'IDENTITY_STORAGE_UNSAFE',
  'IDENTITY_BUSY',
  'IDENTITY_STORE_INVALID',
  'IDENTITY_STORE_INCOMPLETE',
] as const;
export type IdentityErrorCode = (typeof identityErrorCodes)[number];
export type DeviceIdentity = Readonly<
  | { status: 'ready'; device_id: string; fingerprint: string }
  | { status: 'unavailable'; error_code: IdentityErrorCode }
>;
export type PublicPeerPrefill = Readonly<{ card: string; fingerprint: string }>;
export interface RuntimeInfo {
  identity?: DeviceIdentity;
  p3_enabled?: boolean;
  v2_loopback_diagnostic_enabled?: boolean;
  v2_peer_pairing_enabled?: boolean;
  v2_peer_prefill?: PublicPeerPrefill;
  v2_verified_lan_enabled?: boolean;
  v2_private_wss_lan_enabled?: boolean;
  public_video_available?: boolean;
  lan_input_queue_test?: boolean;
  app_version: string;
  os: string;
  arch: string;
  capture_backend: string;
  remote_access_enabled: boolean;
  p004_enabled?: boolean;
  p004_lan_enabled?: boolean;
  p005_enabled?: boolean;
  p006_enabled?: boolean;
  p006c_enabled?: boolean;
  p007b_diag_enabled?: boolean;
}
export type DesktopIpcCode =
  | 'BROWSER_UNAVAILABLE'
  | 'IPC_FAILED'
  | 'IPC_RESPONSE_INVALID'
  | 'IPC_TIMEOUT'
  | 'IPC_CANCELLED';
const messages: Record<DesktopIpcCode, string> = {
  BROWSER_UNAVAILABLE:
    '暂时无法读取本机能力。请在桌面客户端中重新检测；浏览器预览无法连接本机核心。',
  IPC_FAILED:
    '暂时无法读取本机能力。请重新检测；若仍失败，请关闭并重新打开客户端。',
  IPC_RESPONSE_INVALID:
    '本机核心返回的信息无法识别，请检查客户端版本后重新检测。',
  IPC_TIMEOUT: '读取本机能力超时，请重新检测。',
  IPC_CANCELLED: '本次本机能力读取已取消。',
};
export class DesktopIpcError extends Error {
  constructor(readonly code: DesktopIpcCode) {
    super(messages[code]);
  }
}
export function ipcFailure(error: unknown): DesktopIpcError {
  return new DesktopIpcError(
    error instanceof DesktopIpcError && Object.hasOwn(messages, error.code)
      ? error.code
      : 'IPC_FAILED',
  );
}
const stringLimits = {
  app_version: 64,
  os: 32,
  arch: 32,
  capture_backend: 192,
} as const;
const flags = [
  'p3_enabled',
  'v2_loopback_diagnostic_enabled',
  'v2_peer_pairing_enabled',
  'v2_verified_lan_enabled',
  'v2_private_wss_lan_enabled',
  'public_video_available',
  'lan_input_queue_test',
  'p004_enabled',
  'p004_lan_enabled',
  'p005_enabled',
  'p006_enabled',
  'p006c_enabled',
  'p007b_diag_enabled',
] as const;
const keys = new Set([
  'identity',
  'v2_peer_prefill',
  ...Object.keys(stringLimits),
  'remote_access_enabled',
  ...flags,
]);
export function decodeRuntimeInfo(value: unknown): RuntimeInfo {
  const invalid = () => {
    throw new DesktopIpcError('IPC_RESPONSE_INVALID');
  };
  if (!value || typeof value !== 'object' || Array.isArray(value))
    return invalid();
  const record = value as Record<string, unknown>;
  if (
    ![Object.prototype, null].includes(Object.getPrototypeOf(record)) ||
    Object.keys(record).some((key) => !keys.has(key))
  )
    return invalid();
  for (const [key, limit] of Object.entries(stringLimits)) {
    const field = record[key];
    if (
      typeof field !== 'string' ||
      !field.trim() ||
      field.length > limit ||
      /[\u0000-\u001f\u007f]/.test(field)
    )
      return invalid();
  }
  if (typeof record.remote_access_enabled !== 'boolean') return invalid();
  for (const key of flags)
    if (key in record && typeof record[key] !== 'boolean') return invalid();
  // Copy only own JSON fields so prototype/inherited values cannot become capabilities.
  if (
    Object.keys(stringLimits).some((key) => !Object.hasOwn(record, key)) ||
    !Object.hasOwn(record, 'remote_access_enabled')
  )
    return invalid();
  return Object.freeze({
    ...record,
    ...(Object.hasOwn(record, 'identity')
      ? { identity: decodeIdentity(record.identity) }
      : {}),
    ...(Object.hasOwn(record, 'v2_peer_prefill')
      ? { v2_peer_prefill: decodePeerPrefill(record.v2_peer_prefill) }
      : {}),
  }) as unknown as RuntimeInfo;
}

function decodePeerPrefill(value: unknown): PublicPeerPrefill {
  const invalid = (): never => {
    throw new DesktopIpcError('IPC_RESPONSE_INVALID');
  };
  if (
    !value ||
    typeof value !== 'object' ||
    ![Object.prototype, null].includes(Object.getPrototypeOf(value)) ||
    Object.keys(value).sort().join(',') !== 'card,fingerprint'
  )
    return invalid();
  const { card, fingerprint } = value as Record<string, unknown>;
  if (
    typeof card !== 'string' ||
    card.length !== 68 ||
    !/^RH2-[0-9a-f]{64}$/.test(card) ||
    typeof fingerprint !== 'string' ||
    fingerprint.length !== 64 ||
    !/^[0-9a-f]{64}$/.test(fingerprint)
  )
    return invalid();
  return Object.freeze({ card, fingerprint });
}
export function decodeIdentity(value: unknown): DeviceIdentity {
  const invalid = (): never => {
    throw new DesktopIpcError('IPC_RESPONSE_INVALID');
  };
  if (
    !value ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    ![Object.prototype, null].includes(Object.getPrototypeOf(value))
  )
    return invalid();
  const record = value as Record<string, unknown>;
  const own = Object.keys(record).sort().join(',');
  if (record.status === 'ready' && own === 'device_id,fingerprint,status') {
    if (
      typeof record.fingerprint !== 'string' ||
      !/^[0-9a-f]{64}$/.test(record.fingerprint) ||
      typeof record.device_id !== 'string' ||
      !/^RH-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}$/.test(record.device_id)
    )
      return invalid();
    const short = record.fingerprint.slice(0, 12).toUpperCase();
    if (
      record.device_id !==
      `RH-${short.slice(0, 4)}-${short.slice(4, 8)}-${short.slice(8)}`
    )
      return invalid();
    return Object.freeze({
      status: 'ready',
      device_id: record.device_id,
      fingerprint: record.fingerprint,
    });
  }
  if (
    record.status === 'unavailable' &&
    own === 'error_code,status' &&
    typeof record.error_code === 'string' &&
    identityErrorCodes.includes(record.error_code as IdentityErrorCode)
  ) {
    return Object.freeze({
      status: 'unavailable',
      error_code: record.error_code as IdentityErrorCode,
    });
  }
  return invalid();
}
export interface DesktopApi {
  readRuntime(options?: { signal?: AbortSignal }): Promise<RuntimeInfo>;
}
