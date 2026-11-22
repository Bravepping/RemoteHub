import test from 'node:test';
import assert from 'node:assert/strict';
import { policy, validatePolicy, validateResolution } from './build-matrix.mjs';

function fixture(profile) {
  return {
    packages: [
      'remotehub-desktop',
      'hub-webrtc',
      'hub-encode-win',
      'hub-probe-wire',
    ].map((name) => ({ name, id: name, features: {} })),
    resolve: {
      nodes: [
        {
          id: 'remotehub-desktop',
          features: [...profile.desktopFeatures],
          dependencies: profile.webrtcDependency ? ['hub-webrtc'] : [],
        },
        { id: 'hub-webrtc', features: [...profile.webrtcFeatures] },
        {
          id: 'hub-encode-win',
          features: profile.syntheticStream ? ['synthetic-stream'] : [],
        },
      ],
    },
  };
}

test('eight documented profiles accept their expected resolver shapes', () => {
  validatePolicy(policy);
  for (const p of policy.profiles)
    assert.equal(validateResolution(fixture(p), p).profile, p.id);
});
test('a default build cannot silently enable a relay feature', () => {
  const p = policy.profiles[0],
    m = fixture(p);
  m.resolve.nodes[0].features.push('p006-relay');
  assert.throws(() => validateResolution(m, p), /POLICY_REJECTED/);
});
test('default optional network dependency cannot be enabled without a reported feature', () => {
  const p = policy.profiles[0],
    m = fixture(p);
  m.resolve.nodes[0].dependencies.push('hub-webrtc');
  assert.throws(() => validateResolution(m, p));
});
test('experimental defaults are rejected even if a no-default-features audit hides them', () => {
  for (const i of [0, 1, 2]) {
    const p = policy.profiles[0],
      m = fixture(p);
    m.packages[i].features.default = ['experiment'];
    assert.throws(() => validateResolution(m, p));
  }
});
test('loopback cannot acquire LAN or control features', () => {
  const p = policy.profiles[1],
    m = fixture(p);
  m.resolve.nodes[0].features.push('p004-lan');
  assert.throws(() => validateResolution(m, p));
});
test('direct mode cannot lose its authenticated relay dependency', () => {
  const p = policy.profiles[6],
    m = fixture(p);
  m.resolve.nodes[1].features = ['p006-direct'];
  assert.throws(() => validateResolution(m, p));
});
test('R06 must resolve the synthetic-stream feature', () => {
  const p = policy.profiles[7],
    m = fixture(p);
  m.resolve.nodes[2].features = [];
  assert.throws(() => validateResolution(m, p));
});
test('a missing resolution is never treated as a default build', () => {
  assert.throws(() =>
    validateResolution({ packages: [], resolve: null }, policy.profiles[0]),
  );
});
test('policy cannot request arbitrary Cargo switches or duplicate build profiles', () => {
  for (const mutate of [
    (p) => p.profiles[0].features.push('--all-features'),
    (p) => {
      p.profiles[1].id = 'default';
    },
  ]) {
    const changed = structuredClone(policy);
    mutate(changed);
    assert.throws(() => validatePolicy(changed));
  }
});
