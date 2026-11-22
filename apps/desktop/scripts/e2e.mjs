import { createServer } from 'vite';
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { resolve, relative, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const root = fileURLToPath(new URL('../../../', import.meta.url));
const output = resolve(
  root,
  process.env.REMOTEHUB_VERIFY_DIR ?? 'artifacts/current-browser',
  'browser-results',
);
const outputWithinArtifacts = relative(resolve(root, 'artifacts'), output);
if (outputWithinArtifacts.startsWith('..') || isAbsolute(outputWithinArtifacts))
  throw new Error('BROWSER_EVIDENCE_DIRECTORY_REJECTED');
const server = await createServer({
  server: { host: '127.0.0.1', port: 1420, strictPort: true },
});
try {
  await server.listen();
  const child = spawn(
    process.execPath,
    [require.resolve('@playwright/test/cli'), 'test', '--output', output],
    {
      stdio: 'inherit',
    },
  );
  const exit = await new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('exit', resolve);
  });
  process.exitCode = exit ?? 1;
} finally {
  await server.close();
}
