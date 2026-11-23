Vendored webrtc 0.21.1 from the crates.io release (upstream commit
d658b5a803dd8dc389547450e245877310c9cbed; original archive SHA-256
b621e736302bef784948cc8d8099fd0a8fdb5757693c80a344ffbe38c49387d2).
The original MIT and Apache-2.0 licenses are retained.

RemoteHub adds one opt-in builder switch, with_host_candidates(false),
which skips local UDP and TCP host candidates before ICE agent registration.
The switch applies to every gathering generation; existing UDP socket,
STUN and TURN behavior is unchanged. Callers handle SDP disclosure of
related addresses separately.

An opt-in Runtime::allows_turn_peer hook checks logical peer addresses before
TURN data/permission creation, buffered flush and decapsulated delivery.
The default keeps the upstream behavior. Scoped runtimes still need a
physical socket boundary to block queued encoded writes after revocation.
The separate opt-in Runtime::requires_exact_turn_source disables same-local
socket and cross-source transaction fallbacks, without changing legacy TURN.

The vendored build omits upstream standalone examples, integration fixtures and their separate lockfile. Workspace resolution uses the root Cargo.lock. The focused mocked STUN test is in the wrapper source.

TURN ingress dispatches receiver-track events before draining media, so the first relayed RTP is not dropped before its track queue exists.
