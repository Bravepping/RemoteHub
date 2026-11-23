// The ICE agent keeps raw SDP; only canonical network fields go over R06 signaling.
export function r06Metadata(
  line: string,
  invalid: string,
  forbidden = invalid,
): string {
  if (
    line.trim() !== line ||
    line.includes('\r') ||
    line.startsWith('a=remote-candidates:')
  )
    throw new Error(forbidden);
  if (line.startsWith('o=')) {
    const fields = line.slice(2).split(/\s+/);
    if (
      fields.length !== 6 ||
      !/^\d+$/.test(fields[1]) ||
      !/^\d+$/.test(fields[2]) ||
      fields[3] !== 'IN' ||
      !['IP4', 'IP6'].includes(fields[4])
    )
      throw new Error(invalid);
    return `o=- ${fields[1]} ${fields[2]} IN IP4 0.0.0.0`;
  }
  if (line.startsWith('c=')) return 'c=IN IP4 0.0.0.0';
  if (line.startsWith('a=rtcp:')) return 'a=rtcp:9 IN IP4 0.0.0.0';
  return line;
}

export function r06Candidate(fields: string[]): string {
  const base = [...fields.slice(0, 8)];
  base[2] = 'udp';
  return `a=candidate:${base.join(' ')}${base[7] === 'host' ? '' : ' raddr 0.0.0.0 rport 0'}`;
}
