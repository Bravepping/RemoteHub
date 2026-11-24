import type { PublicVideoRole } from './publicVideoSession';

export interface WindowAttachment {
  generation: string;
  role: PublicVideoRole;
}

export type WindowPhase = 'starting' | 'ready' | 'stopping' | 'closed';
export interface WindowState {
  phase: WindowPhase;
  path?: 'direct' | 'relay';
}

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error();
  return value as Record<string, unknown>;
}

function validRole(value: unknown): value is PublicVideoRole {
  return value === 'viewer' || value === 'host';
}

function phase(value: unknown): WindowPhase {
  if (
    value === 'starting' ||
    value === 'ready' ||
    value === 'stopping' ||
    value === 'closed'
  )
    return value;
  throw new Error();
}

export function observerSnapshot(value: unknown): WindowState | undefined {
  const snapshot = record(value);
  if (Object.keys(snapshot).length === 0) return undefined;
  // Main is not entitled to a media generation.
  if (
    !validRole(snapshot.role) ||
    Object.keys(snapshot).sort().join(',') !== 'role,status'
  )
    throw new Error();
  const status = record(snapshot.status);
  const path = status.connection_path;
  if (path !== undefined && path !== 'direct' && path !== 'relay')
    throw new Error();
  return { phase: phase(status.phase), path };
}

export function windowAttachment(value: unknown): WindowAttachment {
  const snapshot = record(value);
  if (
    Object.keys(snapshot).sort().join(',') !== 'generation,role,status' ||
    !validRole(snapshot.role) ||
    typeof snapshot.generation !== 'string' ||
    !/^[a-f0-9]{32}$/.test(snapshot.generation)
  )
    throw new Error();
  phase(record(snapshot.status).phase);
  return { generation: snapshot.generation, role: snapshot.role };
}
