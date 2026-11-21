// Preparation-only factory. No page or IPC invokes it in P006a.
export type IceMode =
  'loopback' | 'lan_pinned' | 'relay_only' | 'direct_preferred';
export interface IceConfig {
  mode: IceMode;
  ice_servers: { urls: string[]; username: string; credential: string }[];
  expires_at: number;
  session_id: string;
}
export interface RelayApproval {
  bothApproved: boolean;
  sessionId: string;
  expiresAt: number;
  serverIp: string;
  serverPort: number;
}
function publicV4(value: string): boolean {
  const parts = value.split('.');
  if (
    parts.length !== 4 ||
    parts.some(
      (p) => !/^\d{1,3}$/.test(p) || String(Number(p)) !== p || Number(p) > 255,
    )
  )
    return false;
  const [a, b, c] = parts.map(Number);
  return (
    a > 0 &&
    a < 224 &&
    a !== 10 &&
    a !== 127 &&
    !(a === 169 && b === 254) &&
    !(a === 172 && b >= 16 && b <= 31) &&
    !(a === 192 && (b === 168 || b === 0)) &&
    !(a === 100 && b >= 64 && b <= 127) &&
    !(a === 198 && (b === 18 || b === 19 || (b === 51 && c === 100))) &&
    !(a === 203 && b === 0 && c === 113)
  );
}
export function relayOptions(
  config: IceConfig,
  approval: RelayApproval,
  now: number,
  featureEnabled: boolean,
): RTCConfiguration {
  if (!featureEnabled || !approval.bothApproved)
    throw new Error('RELAY_APPROVAL_REQUIRED');
  if (config.mode !== 'relay_only')
    throw new Error('ICE_MODE_NOT_ENABLED_IN_P006A');
  if (
    !Number.isSafeInteger(config.expires_at) ||
    !Number.isSafeInteger(now) ||
    !Number.isSafeInteger(approval.expiresAt) ||
    config.expires_at - now < 60 ||
    config.expires_at - now > 600 ||
    config.expires_at > approval.expiresAt ||
    config.session_id !== approval.sessionId ||
    config.session_id === '00000000-0000-0000-0000-000000000000' ||
    !/^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(config.session_id) ||
    !publicV4(approval.serverIp) ||
    !Number.isInteger(approval.serverPort) ||
    approval.serverPort < 1024 ||
    approval.serverPort > 65535
  )
    throw new Error('ICE_CONFIG_INVALID');
  if (config.ice_servers.length < 1 || config.ice_servers.length > 2)
    throw new Error('ICE_SERVERS_LIMIT');
  const endpoint = `turn:${approval.serverIp}:${approval.serverPort}?transport=udp`;
  for (const s of config.ice_servers) {
    if (
      s.urls.length < 1 ||
      s.urls.length > 2 ||
      s.urls.some((url) => url !== endpoint) ||
      s.username !== `${config.expires_at}:${config.session_id}` ||
      !/^[A-Za-z0-9+/]{27}=$/.test(s.credential)
    )
      throw new Error('ICE_SHORT_CREDENTIAL_OR_ENDPOINT_INVALID');
  }
  return {
    iceTransportPolicy: 'relay',
    bundlePolicy: 'max-bundle',
    iceServers: config.ice_servers.map((s) => ({
      urls: [...s.urls],
      username: s.username,
      credential: s.credential,
    })),
  };
}
export function relayPairSummary(
  stats: RTCStatsReport,
): { localType: string; remoteType: string; relay: boolean } | null {
  let selected: {
    localType: string;
    remoteType: string;
    relay: boolean;
  } | null = null;
  stats.forEach((item) => {
    if (item.type !== 'transport' || !item.selectedCandidatePairId) return;
    const pair = stats.get(item.selectedCandidatePairId);
    const local = pair && stats.get(pair.localCandidateId);
    const remote = pair && stats.get(pair.remoteCandidateId);
    if (local && remote)
      selected = {
        localType: local.candidateType,
        remoteType: remote.candidateType,
        relay:
          local.candidateType === 'relay' &&
          remote.candidateType === 'relay' &&
          local.protocol === 'udp' &&
          remote.protocol === 'udp',
      };
  });
  return selected;
}
