// Explicit operator diagnostic; no screen/media access or remote web page.
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
const args = process.argv.slice(2);
if (args.length !== 2 || args[0] !== '--approved-target') {
  console.error('EXPLICIT_APPROVED_TARGET_REQUIRED');
  process.exit(2);
}
const c = JSON.parse(readFileSync(args[1], 'utf8'));
const ticket = c.tickets['edge-close'];
if (
  Date.now() / 1000 >= c.approval_expires_at ||
  ticket.expires_at - Date.now() / 1000 < 60
)
  throw new Error('LIFECYCLE_APPROVAL_EXPIRED');
const require = createRequire(path.resolve('apps/desktop/package.json'));
const { chromium } = require('@playwright/test');
const browser = await chromium.launch({
  channel: 'msedge',
  headless: true,
  args: ['--force-webrtc-ip-handling-policy=default_public_interface_only'],
});
try {
  const page = await browser.newPage();
  await page.exposeFunction('lifecycleReport', (e) =>
    process.stdout.write(JSON.stringify(e) + '\n'),
  );
  await page.evaluate(
    async ({ target, ticket }) => {
      const report = async (event, extras = {}) =>
        window.lifecycleReport({
          case: 'edge-close',
          event,
          timeNs: String(BigInt(Date.now()) * 1000000n),
          clockResolutionNs: 1000000,
          ...extras,
        });
      const pc = new RTCPeerConnection({
        iceTransportPolicy: 'relay',
        bundlePolicy: 'max-bundle',
        iceCandidatePoolSize: 0,
        iceServers: [
          {
            urls: [`turn:${target}:3478?transport=udp`],
            username: ticket.username,
            credential: ticket.credential,
          },
        ],
      });
      const ports = [];
      let rejected = false;
      pc.onicecandidate = (e) => {
        if (!e.candidate) return;
        const a = e.candidate;
        if (
          a.type !== 'relay' ||
          a.protocol !== 'udp' ||
          a.address !== target ||
          a.port < 49160 ||
          a.port > 49259
        )
          rejected = true;
        ports.push(a.port);
        if (ports.length > 2) {
          rejected = true;
          pc.close();
        }
      };
      try {
        await report('before_gather', {
          browser: 'Edge Chromium; not embedded WebView2',
          mediaTracks: 0,
        });
        pc.createDataChannel('allocation.probe', { ordered: true });
        await pc.setLocalDescription(await pc.createOffer());
        await new Promise((resolve, reject) => {
          if (pc.iceGatheringState === 'complete') return resolve();
          const timer = setTimeout(
            () => reject(new Error('EDGE_GATHER_TIMEOUT')),
            12000,
          );
          pc.onicegatheringstatechange = () => {
            if (pc.iceGatheringState === 'complete') {
              clearTimeout(timer);
              resolve();
            }
          };
        });
        if (rejected || ports.length < 1 || ports.length > 2)
          throw new Error('EDGE_RELAY_SCOPE_OR_BUDGET');
        await report('allocation_gathered', {
          relayPorts: ports,
          selectedPair: null,
          remoteDescriptionSet: false,
        });
        await new Promise((resolve) => setTimeout(resolve, 2000));
        await report('pc_close_call');
        pc.close();
        await report('pc_close_returned', {
          localPeerConnectionClosed: pc.connectionState === 'closed',
        });
        await new Promise((resolve) => setTimeout(resolve, 3000));
      } finally {
        pc.close();
      }
    },
    { target: c.target, ticket },
  );
} catch {
  console.error('EDGE_LIFECYCLE_CASE_FAILED');
  process.exitCode = 1;
} finally {
  await browser.close();
}
