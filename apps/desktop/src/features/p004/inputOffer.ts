// Product-only candidate publication after Rust has verified/approved the selected local NIC.
// This changes publication, never the selected-pair validator or UDP destination scope.
export function publishInputOffer(sdp: string, localIp: string) {
  const octets = localIp.split('.');
  const n = octets.map(Number);
  if (
    octets.length !== 4 ||
    octets.some(
      (x, i) => !/^\d{1,3}$/.test(x) || String(n[i]) !== x || n[i] > 255,
    ) ||
    !(
      n[0] === 10 ||
      (n[0] === 172 && n[1] >= 16 && n[1] <= 31) ||
      (n[0] === 192 && n[1] === 168)
    ) ||
    n[3] === 0 ||
    n[3] === 255
  )
    throw new Error('INPUT_LOCAL_ADDRESS_INVALID');
  const summary = { before: 0, mdns: 0, published: 0, mdnsPublished: 0 };
  const lines = sdp.split(/\r?\n/).flatMap((line) => {
    if (!line.startsWith('a=candidate:'))
      return [line.startsWith('c=') ? 'c=IN IP4 0.0.0.0' : line];
    summary.before++;
    const p = line.slice(12).trim().split(/\s+/);
    const mdns =
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.local$/i.test(
        p[4] ?? '',
      );
    if ((p[4] ?? '').toLowerCase().endsWith('.local')) summary.mdns++;
    const port = Number(p[5]);
    if (
      p[1] !== '1' ||
      p[2]?.toLowerCase() !== 'udp' ||
      p[6] !== 'typ' ||
      p[7] !== 'host' ||
      !Number.isInteger(port) ||
      port < 1024 ||
      port > 65535
    )
      return [];
    if (p[4] !== localIp && !mdns) return [];
    if (mdns) {
      p[4] = localIp;
      summary.mdnsPublished++;
    }
    summary.published++;
    return ['a=candidate:' + p.join(' ')];
  });
  if (summary.before > 128) throw new Error('INPUT_CANDIDATE_LIMIT');
  return { sdp: lines.join('\r\n'), summary };
}
