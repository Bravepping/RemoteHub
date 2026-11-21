import { createServer } from 'vite';
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const server = await createServer({
  server: { host: '127.0.0.1', port: 1420, strictPort: true },
});
try {
  await server.listen();
  const child = spawn(process.execPath, [require.resolve('@playwright/test/cli'), 'test'], {
    stdio: 'inherit',
  });
  const exit = await new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('exit', resolve);
  });
  process.exitCode = exit ?? 1;
} finally {
  await server.close();
}
