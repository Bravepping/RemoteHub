export interface BrowserControlStats {
  state: string;
  sentMoves: number;
  ackReceived: number;
  pingsSent: number;
  pongsReceived: number;
  pending: number;
  lastTxSeq: number;
  lastRxSeq: number;
  anomalies: number;
  openEvents: number;
  messageEvents: number;
  closeEvents: number;
  errorEvents: number;
  sendClosed: boolean;
  lastError: string | null;
}
export interface HostControlStats {
  state: string;
  accepted_moves: number;
  acks_sent: number;
  pings_received: number;
  pongs_sent: number;
  last_rx_seq: number;
  last_tx_seq: number;
  rejected: number;
  open_events: number;
  message_events: number;
  close_events: number;
  error_events: number;
  close_requested: boolean;
  task_joined: boolean;
  last_error: string | null;
}
export class ControlClient {
  private inputReady = false;
  private inputEnabled = false;
  private dc: RTCDataChannel;
  private stopped = false;
  private batchStarted = false;
  private timer?: ReturnType<typeof setInterval>;
  private pending = new Map<number, 'mouse_move_sim' | 'ping'>();
  private stats: BrowserControlStats = {
    state: 'connecting',
    sentMoves: 0,
    ackReceived: 0,
    pingsSent: 0,
    pongsReceived: 0,
    pending: 0,
    lastTxSeq: 0,
    lastRxSeq: 0,
    anomalies: 0,
    openEvents: 0,
    messageEvents: 0,
    closeEvents: 0,
    errorEvents: 0,
    sendClosed: false,
    lastError: null,
  };
  constructor(
    pc: RTCPeerConnection,
    private session: string,
    private failed: (code: string) => void,
    private manual = false,
    private classifyChannelError?: () => void,
  ) {
    const label = manual ? 'remote-input.v1' : 'control.v1';
    this.dc = pc.createDataChannel(label, {
      ordered: true,
      protocol: label,
    });
    this.dc.onopen = () => {
      if (this.stopped) {
        this.dc.close();
        return;
      }
      this.stats.openEvents++;
      this.stats.state = 'open';
    };
    this.dc.onmessage = (event) => {
      if (!this.stopped) this.receive(event.data);
    };
    this.dc.onerror = () => {
      this.stats.errorEvents++;
      if (this.stopped) return;
      if (this.classifyChannelError) {
        this.revoke();
        this.classifyChannelError();
      } else this.confirmChannelError();
    };
    this.dc.onclose = () => {
      this.stats.closeEvents++;
      this.stats.state = this.stats.lastError ? 'failed' : 'closed';
      this.revoke();
    };
  }
  snapshot(): BrowserControlStats {
    return { ...this.stats, pending: this.pending.size };
  }
  private fail(code: string) {
    if (this.stats.lastError) return;
    this.stats.lastError = code;
    this.stats.anomalies++;
    this.stats.state = 'failed';
    this.revoke();
    this.failed(code);
  }
  confirmChannelError() {
    this.fail('CONTROL_CHANNEL_ERROR');
  }
  private send(
    kind: 'mouse_move_sim' | 'ping',
    extra: Record<string, unknown> = {},
  ) {
    if (this.stopped || this.dc.readyState !== 'open') return false;
    if (
      this.dc.bufferedAmount > 16384 ||
      this.pending.size >= 128 ||
      this.stats.lastTxSeq >= 1000
    ) {
      this.fail('CONTROL_SEND_LIMIT');
      return false;
    }
    const seq = this.stats.lastTxSeq + 1;
    const message = JSON.stringify(
      this.manual
        ? { v: 1, seq, session_id: this.session, event: extra }
        : {
            v: 1,
            kind,
            seq,
            session_id: this.session,
            ...extra,
          },
    );
    try {
      this.dc.send(message);
    } catch {
      this.fail('CONTROL_SEND_FAILED');
      return false;
    }
    this.stats.lastTxSeq = seq;
    this.pending.set(seq, kind);
    if (kind === 'mouse_move_sim') this.stats.sentMoves++;
    else this.stats.pingsSent++;
    return true;
  }
  beginBatch(videoPlaying: boolean) {
    if (this.manual) {
      if (this.inputReady && !videoPlaying) this.enableInput(false);
      this.inputReady = videoPlaying;
      return;
    }
    if (
      !videoPlaying ||
      this.batchStarted ||
      this.stopped ||
      this.dc.readyState !== 'open'
    )
      return;
    this.batchStarted = true;
    if (!this.send('ping')) return;
    this.timer = setInterval(() => {
      if (this.stopped || this.stats.sentMoves >= 100) {
        clearInterval(this.timer);
        return;
      }
      const index = this.stats.sentMoves;
      this.send('mouse_move_sim', {
        x_norm: (index % 20) / 19,
        y_norm: Math.floor(index / 20) / 4,
      });
    }, 50);
  }
  enableInput(value: boolean) {
    const release =
      this.manual &&
      !value &&
      this.inputEnabled &&
      this.dc.readyState === 'open';
    this.inputEnabled =
      this.manual &&
      value &&
      this.inputReady &&
      !this.stopped &&
      this.dc.readyState === 'open';
    if (release) this.send('mouse_move_sim', { kind: 'release_all' });
    return this.inputEnabled;
  }
  input(event: Record<string, unknown>) {
    if (!this.manual || !this.inputReady || !this.inputEnabled) return false;
    return this.send('mouse_move_sim', event);
  }
  private receive(data: unknown) {
    this.stats.messageEvents++;
    try {
      if (
        typeof data !== 'string' ||
        new TextEncoder().encode(data).length > 1024
      )
        throw new Error();
      const m = JSON.parse(data) as Record<string, unknown>;
      const field =
        m.kind === 'ack' ? 'ack_seq' : m.kind === 'pong' ? 'reply_to' : null;
      if (
        !field ||
        Object.keys(m).sort().join(',') !==
          ['v', 'kind', 'seq', 'session_id', field].sort().join(',') ||
        m.v !== 1 ||
        m.session_id !== this.session ||
        !Number.isSafeInteger(m.seq) ||
        m.seq !== this.stats.lastRxSeq + 1 ||
        !Number.isSafeInteger(m[field])
      )
        throw new Error();
      const request = m[field] as number;
      const expected = m.kind === 'ack' ? 'mouse_move_sim' : 'ping';
      if (this.pending.get(request) !== expected) throw new Error();
      this.pending.delete(request);
      this.stats.lastRxSeq = m.seq as number;
      if (m.kind === 'ack') this.stats.ackReceived++;
      else this.stats.pongsReceived++;
    } catch {
      this.fail('CONTROL_RESPONSE_INVALID');
    }
  }
  revoke() {
    this.stopped = true;
    this.inputReady = false;
    this.inputEnabled = false;
    clearInterval(this.timer);
    this.stats.sendClosed = true;
  }
  async close() {
    this.revoke();
    if (this.dc.readyState !== 'closed') {
      this.stats.state = 'closing';
      this.dc.close();
    }
    const end = performance.now() + 800;
    while (this.dc.readyState !== 'closed' && performance.now() < end)
      await new Promise((resolve) => setTimeout(resolve, 20));
    if (this.dc.readyState === 'closed')
      this.stats.state = this.stats.lastError ? 'failed' : 'closed';
  }
}
