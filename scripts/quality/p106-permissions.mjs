import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
export const root = fileURLToPath(new URL('../../', import.meta.url));
const baseline = JSON.parse(
  execFileSync(
    'git',
    [
      'show',
      '7f36b393658e2050fa9f976bc95524bb48893798:apps/desktop/src-tauri/capabilities/default.json',
    ],
    { cwd: root, encoding: 'utf8' },
  ),
);
const necessary = [
  'path',
  'event',
  'window',
  'webview',
  'app',
  'image',
  'resources',
].map((name) => `core:${name}:default`);
const custom = baseline.permissions.filter((name) => name !== 'core:default');
const expected = new Set([...necessary, ...custom]);
function requireValid(value) {
  if (!value) throw new Error('P106_CAPABILITY_REJECTED');
}
export function audit(
  capability = JSON.parse(
    readFileSync(
      new URL(
        '../../apps/desktop/src-tauri/capabilities/default.json',
        import.meta.url,
      ),
      'utf8',
    ),
  ),
) {
  requireValid(
    capability.identifier === baseline.identifier &&
      JSON.stringify(capability.windows) === JSON.stringify(['main']),
  );
  requireValid(!capability.remote && capability.local !== false);
  requireValid(
    Array.isArray(capability.permissions) &&
      capability.permissions.length === expected.size,
  );
  requireValid(
    new Set(capability.permissions).size === expected.size &&
      capability.permissions.every((name) => expected.has(name)),
  );
  const manifest = JSON.parse(
    readFileSync(
      new URL(
        '../../apps/desktop/src-tauri/gen/schemas/acl-manifests.json',
        import.meta.url,
      ),
      'utf8',
    ),
  );
  function expand(permissions, seen = new Set()) {
    const commands = new Set();
    for (const identifier of permissions) {
      requireValid(typeof identifier === 'string' && !seen.has(identifier));
      const index = identifier.lastIndexOf(':');
      const namespace = index < 0 ? '__app-acl__' : identifier.slice(0, index);
      const name = index < 0 ? identifier : identifier.slice(index + 1);
      const entry = manifest[namespace];
      requireValid(entry);
      const item =
        name === 'default'
          ? entry.default_permission
          : (entry.permissions[name] ?? entry.permission_sets[name]);
      requireValid(item);
      if (item.permissions)
        for (const child of expand(
          item.permissions.map((v) =>
            v.includes(':') ? v : `${namespace}:${v}`,
          ),
          new Set([...seen, identifier]),
        ))
          commands.add(child);
      else
        for (const command of item.commands?.allow ?? [])
          commands.add(`${namespace}|${command}`);
    }
    return commands;
  }
  const before = expand(baseline.permissions);
  const after = expand(capability.permissions);
  requireValid([...after].every((command) => before.has(command)));
  requireValid(
    ![...after].some(
      (command) =>
        command.startsWith('core:tray|') || command.startsWith('core:menu|'),
    ),
  );
  requireValid(
    custom.every((permission) => capability.permissions.includes(permission)),
  );
  return {
    passed: true,
    previousAllowedCommands: before.size,
    currentAllowedCommands: after.size,
    addedAllowedCommands: 0,
    removedAllowedCommands: before.size - after.size,
    jsTrayAndMenuCommandsAllowed: 0,
    customExperimentalPermissionsUnchanged: true,
    windowScope: 'main-local-only',
    capabilitySha256: createHash('sha256')
      .update(JSON.stringify(capability))
      .digest('hex'),
  };
}
