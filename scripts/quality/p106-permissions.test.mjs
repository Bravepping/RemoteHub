import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { audit } from './p106-permissions.mjs';
const actual = JSON.parse(
  readFileSync(
    new URL(
      '../../apps/desktop/src-tauri/capabilities/default.json',
      import.meta.url,
    ),
    'utf8',
  ),
);
test('resolved real Tauri manifest removes JS tray/menu without adding any allowed command', () => {
  const result = audit();
  assert.equal(result.addedAllowedCommands, 0);
  assert.equal(result.jsTrayAndMenuCommandsAllowed, 0);
  assert.ok(result.removedAllowedCommands > 0);
});
for (const permission of [
  'core:default',
  'core:tray:allow-new',
  'core:menu:allow-set-checked',
  'shell:allow-execute',
]) {
  test(`rejects unauthorized permission ${permission}`, () => {
    assert.throws(
      () =>
        audit({ ...actual, permissions: [...actual.permissions, permission] }),
      /P106_CAPABILITY_REJECTED/,
    );
  });
}
test('rejects duplicate permissions instead of mistaking array length for equivalence', () => {
  const permissions = [...actual.permissions];
  permissions[0] = permissions[1];
  assert.throws(
    () => audit({ ...actual, permissions }),
    /P106_CAPABILITY_REJECTED/,
  );
});
test('rejects foreign windows or remote source permissions', () => {
  assert.throws(
    () => audit({ ...actual, windows: ['other'] }),
    /P106_CAPABILITY_REJECTED/,
  );
  assert.throws(
    () => audit({ ...actual, remote: { urls: ['https://example.com'] } }),
    /P106_CAPABILITY_REJECTED/,
  );
});
