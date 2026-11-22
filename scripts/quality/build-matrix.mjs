// Read-only Cargo resolution audit; no application, capture or cloud process is run.
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

export const root = fileURLToPath(new URL('../../', import.meta.url));
export const policy = JSON.parse(
  readFileSync(new URL('./desktop-builds.json', import.meta.url), 'utf8'),
);
const ids = [
  'default',
  'loopback',
  'lan',
  'control',
  'turn-probe',
  'relay',
  'direct',
  'r06-diagnostic',
];
const requested = [
  null,
  'p004-probe',
  'p004-lan',
  'p005-control',
  'p006-turn-probe',
  'p006-relay',
  'p006-direct',
  'p007b-diag',
];

function requireValue(value) {
  if (!value) throw new Error('BUILD_MATRIX_POLICY_REJECTED');
}
function sameSet(a, b) {
  return (
    Array.isArray(a) &&
    Array.isArray(b) &&
    a.length === new Set(a).size &&
    b.length === new Set(b).size &&
    JSON.stringify([...a].sort()) === JSON.stringify([...b].sort())
  );
}

export function validatePolicy(value) {
  requireValue(
    value.schemaVersion === 1 &&
      value.package === 'remotehub-desktop' &&
      value.target === 'x86_64-pc-windows-msvc' &&
      value.profiles?.length === ids.length,
  );
  for (const [index, p] of value.profiles.entries()) {
    requireValue(
      p.id === ids[index] &&
        sameSet(p.features, requested[index] ? [requested[index]] : []) &&
        typeof p.webrtcDependency === 'boolean' &&
        typeof p.syntheticStream === 'boolean',
    );
  }
}

export function validateResolution(metadata, profile) {
  const packages = metadata.packages;
  requireValue(
    Array.isArray(packages) && Array.isArray(metadata.resolve?.nodes),
  );
  const getPackage = (name) => {
    const found = packages.find((p) => p.name === name);
    requireValue(found);
    return found;
  };
  const node = (pkg) => {
    const found = metadata.resolve.nodes.find((n) => n.id === pkg.id);
    requireValue(found);
    return found;
  };
  const desktop = getPackage('remotehub-desktop');
  const rtc = getPackage('hub-webrtc');
  const encoder = getPackage('hub-encode-win');
  // Default experiments must stay opt-in, including transitive workspace features.
  for (const pkg of [desktop, rtc, encoder]) {
    requireValue((pkg.features?.default ?? []).length === 0);
  }
  const d = node(desktop),
    w = node(rtc),
    e = node(encoder);
  requireValue(sameSet(d.features, profile.desktopFeatures));
  requireValue(sameSet(w.features, profile.webrtcFeatures));
  requireValue(Array.isArray(d.dependencies));
  requireValue(d.dependencies.includes(rtc.id) === profile.webrtcDependency);
  requireValue(
    e.features.includes('synthetic-stream') === profile.syntheticStream,
  );
  const wire = packages.find((p) => p.name === 'hub-probe-wire');
  if (profile.id === 'default') {
    requireValue(!wire || !d.dependencies.includes(wire.id));
  }
  return {
    profile: profile.id,
    requestedFeatures: profile.features,
    desktopFeatures: [...d.features].sort(),
    webrtcFeatures: [...w.features].sort(),
    optionalWebRtcDependency: profile.webrtcDependency,
    syntheticStream: profile.syntheticStream,
    automaticApplicationStart: false,
  };
}

export function audit() {
  validatePolicy(policy);
  const profiles = [];
  for (const p of policy.profiles) {
    const args = [
      'metadata',
      '--format-version',
      '1',
      '--locked',
      '--offline',
      '--no-default-features',
      '--filter-platform',
      policy.target,
    ];
    if (p.features.length)
      args.push(
        '--features',
        ...p.features.map((f) => `${policy.package}/${f}`),
      );
    const r = spawnSync('cargo', args, {
      cwd: root,
      encoding: 'utf8',
      maxBuffer: 20 * 1024 * 1024,
    });
    requireValue(r.status === 0);
    profiles.push(validateResolution(JSON.parse(r.stdout), p));
  }
  return {
    schemaVersion: 1,
    recordedAt: new Date().toISOString(),
    scope:
      'Actual offline Cargo feature resolution; not media or runtime permission verification',
    target: policy.target,
    profiles,
    passed: true,
    applicationStarted: false,
    cloudContacted: false,
    desktopCaptured: false,
  };
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    requireValue(process.argv.length === 2);
    const value = audit();
    const out = new URL('../../artifacts/p101/', import.meta.url);
    mkdirSync(out, { recursive: true });
    writeFileSync(
      new URL('build-matrix.json', out),
      JSON.stringify(value, null, 2) + '\n',
    );
    console.log(
      JSON.stringify({
        profiles: value.profiles.length,
        passed: true,
        applicationStarted: false,
      }),
    );
  } catch {
    console.error('BUILD_MATRIX_CHECK_FAILED_NO_PRIVATE_ECHO');
    process.exitCode = 1;
  }
}
