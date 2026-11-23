import {
  publicV4,
  relayOptions,
  type IceConfig,
  type RelayApproval,
} from './ice';
import { r06Candidate, r06Metadata } from './r06Sdp';
export type ConnectionPath = 'direct' | 'relay' | 'unknown';
export interface DirectScope {
  serverIp: string;
  localIp: string;
  privatePeerIps: string[];
  publishPrivateLocal: boolean;
  expiresAt: number;
}
export interface Candidate {
  address: string;
  port: number;
  type: 'host' | 'srflx' | 'relay';
}
export function privateV4(ip: string): boolean {
  const p = ip.split('.');
  if (
    p.length !== 4 ||
    p.some(
      (v) => !/^\d{1,3}$/.test(v) || String(Number(v)) !== v || Number(v) > 255,
    )
  )
    return false;
  const [a, b] = p.map(Number);
  return (
    a === 10 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168)
  );
}
export function directOptions(
  config: IceConfig,
  approval: RelayApproval,
  now: number,
  featureEnabled: boolean,
): RTCConfiguration {
  if (!featureEnabled || config.mode !== 'direct_preferred')
    throw new Error('DIRECT_MODE_NOT_ENABLED');
  const validated = relayOptions(
    { ...config, mode: 'relay_only' },
    approval,
    now,
    true,
  );
  return {
    ...validated,
    iceTransportPolicy: 'all',
    iceServers: [
      ...validated.iceServers!,
      { urls: [`stun:${approval.serverIp}:${approval.serverPort}`] },
    ],
  };
}
function parsed(line: string): Candidate | null {
  const f = line.slice(12).trim().split(/\s+/);
  if (
    f.length < 8 ||
    f.length > 32 ||
    f[1] !== '1' ||
    f[2]?.toLowerCase() !== 'udp' ||
    f[6] !== 'typ' ||
    !['host', 'srflx', 'relay'].includes(f[7]) ||
    !/^\d+$/.test(f[5])
  )
    return null;
  const port = Number(f[5]);
  if (!Number.isInteger(port) || port < 1024 || port > 65535) return null;
  return { address: f[4], port, type: f[7] as Candidate['type'] };
}
function permitted(c: Candidate, s: DirectScope, local: boolean): boolean {
  if (c.type === 'relay')
    return c.address === s.serverIp && c.port >= 49160 && c.port <= 49259;
  if (c.type === 'srflx') return publicV4(c.address);
  return local
    ? c.address === s.localIp && s.publishPrivateLocal
    : (s.privatePeerIps.includes(c.address) && privateV4(c.address)) ||
        publicV4(c.address);
}
function scope(s: DirectScope) {
  if (
    !publicV4(s.serverIp) ||
    !privateV4(s.localIp) ||
    s.privatePeerIps.length > 2 ||
    new Set(s.privatePeerIps).size !== s.privatePeerIps.length ||
    s.privatePeerIps.some((ip) => !privateV4(ip)) ||
    !Number.isSafeInteger(s.expiresAt) ||
    Date.now() / 1000 >= s.expiresAt ||
    s.expiresAt - Date.now() / 1000 > 600
  )
    throw new Error('DIRECT_SCOPE_EXPIRED_OR_INVALID');
}
export function directSdp(
  sdp: string,
  s: DirectScope,
  diagnostic = false,
): { sdp: string; candidates: Candidate[] } {
  scope(s);
  if (sdp.length > 65536 || sdp.includes('\0'))
    throw new Error('DIRECT_SDP_SIZE');
  const candidates: Candidate[] = [];
  const lines = sdp.split(/\r?\n/);
  if (diagnostic) r06DirectLayout(lines);
  const result = lines
    .flatMap((l) => {
      if (!l.startsWith('a=candidate:'))
        return [
          diagnostic
            ? r06Metadata(l, 'R06_DIRECT_SDP_INVALID')
            : l.startsWith('c=')
              ? 'c=IN IP4 0.0.0.0'
              : l,
        ];
      const c = parsed(l);
      if (!c || !permitted(c, s, true)) return [];
      candidates.push(c);
      if (candidates.length > 16) throw new Error('DIRECT_CANDIDATE_BUDGET');
      return [diagnostic ? canonicalDirect(l) : l];
    })
    .join('\r\n');
  if (!candidates.length) throw new Error('DIRECT_LOCAL_CANDIDATE_MISSING');
  return { sdp: result, candidates };
}
function canonicalDirect(line: string): string {
  const fields = line.slice(12).trim().split(/\s+/);
  if (
    fields[0].length > 64 ||
    !/^\d{1,10}$/.test(fields[3]) ||
    Number(fields[3]) > 4294967295
  )
    throw new Error('R06_DIRECT_SDP_INVALID');
  return r06Candidate(fields);
}

function r06DirectLayout(lines: string[]) {
  const invalid = () => {
    throw new Error('R06_DIRECT_SDP_INVALID');
  };
  let origins = 0;
  let section = -1;
  const perSection = new Set<string>();
  for (const line of lines) {
    // Reject indented lookalikes before filtering candidate types or fields.
    r06Metadata(line, 'R06_DIRECT_SDP_INVALID');
    if (line.startsWith('m=')) section++;
    if (line.startsWith('o=') && (++origins > 1 || section !== -1)) invalid();
    for (const prefix of ['c=', 'a=rtcp:', 'a=mid:']) {
      if (!line.startsWith(prefix)) continue;
      const key = `${section}:${prefix}`;
      if (perSection.has(key) || (section < 0 && prefix !== 'c=')) invalid();
      perSection.add(key);
    }
    if (line.startsWith('a=candidate:') && section < 0) invalid();
  }
  // The synthetic media and simulated control channel are already bundled.
  // Each section must declare its own MID; the common parser checks order/types.
  if (
    section !== 1 ||
    !perSection.has('0:a=mid:') ||
    !perSection.has('1:a=mid:')
  )
    invalid();
  directLayout(lines, 'R06_DIRECT_SDP_INVALID');
}

function directLayout(lines: string[], code: string) {
  const media = lines.filter((l) => l.startsWith('m='));
  const mids = lines
    .filter((l) => l.startsWith('a=mid:'))
    .map((l) => l.slice(6));
  const bundle = lines.filter((l) => l.startsWith('a=group:BUNDLE '));
  const hashes = lines
    .filter((l) => l.startsWith('a=fingerprint:'))
    .map((l) => l.slice(14));
  if (
    media.length !== 2 ||
    !media[0].startsWith('m=video ') ||
    !media[1].startsWith('m=application ') ||
    mids.length !== 2 ||
    mids[0] === mids[1] ||
    bundle.length !== 1 ||
    bundle[0].slice(15) !== mids.join(' ') ||
    !hashes.length ||
    hashes.some((h) => !/^sha-256 ([0-9A-Fa-f]{2}:){31}[0-9A-Fa-f]{2}$/.test(h))
  )
    throw new Error(code);
}

export function directAnswerCandidates(
  sdp: string,
  s: DirectScope,
  diagnostic = false,
): Candidate[] {
  scope(s);
  if (sdp.length > 65536 || sdp.includes('\0'))
    throw new Error('DIRECT_SDP_SIZE');
  const lines = sdp.split(/\r?\n/);
  if (diagnostic) r06DirectLayout(lines);
  else directLayout(lines, 'DIRECT_ANSWER_MEDIA_OR_IDENTITY');
  const candidates: Candidate[] = [];
  for (const l of lines) {
    if (diagnostic && l !== r06Metadata(l, 'R06_DIRECT_SDP_INVALID'))
      throw new Error('R06_DIRECT_SDP_NOT_REDACTED');
    if (l.startsWith('a=candidate:')) {
      const c = parsed(l);
      if (!c || !permitted(c, s, false))
        throw new Error('DIRECT_REMOTE_CANDIDATE_REJECTED');
      if (diagnostic && l !== canonicalDirect(l))
        throw new Error('R06_DIRECT_SDP_NOT_REDACTED');
      candidates.push(c);
      if (candidates.length > 16) throw new Error('DIRECT_CANDIDATE_BUDGET');
    }
  }
  if (!candidates.length) throw new Error('DIRECT_REMOTE_CANDIDATE_MISSING');
  return candidates;
}
export function selectedDirect(
  stats: RTCStatsReport,
  local: Candidate[],
  remote: Candidate[],
): { verified: boolean | null; path: ConnectionPath } {
  let result: { verified: boolean | null; path: ConnectionPath } = {
    verified: null,
    path: 'unknown',
  };
  stats.forEach((t) => {
    if (t.type !== 'transport' || !t.selectedCandidatePairId) return;
    const pair = stats.get(t.selectedCandidatePairId),
      a = pair && stats.get(pair.localCandidateId),
      b = pair && stats.get(pair.remoteCandidateId);
    if (!a || !b) return;
    const matches = (v: Record<string, unknown>, set: Candidate[]) =>
      v.protocol === 'udp' &&
      set.some(
        (c) =>
          c.address === (v.address ?? v.ip) &&
          c.port === v.port &&
          c.type === v.candidateType,
      );
    const verified = matches(a, local) && matches(b, remote);
    result = {
      verified,
      path: verified
        ? a.candidateType === 'relay' || b.candidateType === 'relay'
          ? 'relay'
          : 'direct'
        : 'unknown',
    };
  });
  return result;
}
