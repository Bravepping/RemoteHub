import { expect, test } from '@playwright/test';
test('Phase3 product UI fixture stays inside product navigation and requires explicit consent without starting a session', async ({
  page,
}) => {
  await page.addInitScript(() => {
    const host = window as unknown as {
      isTauri: boolean;
      __TAURI_INTERNALS__: { invoke: (command: string) => Promise<unknown> };
      __p3Calls: string[];
    };
    host.isTauri = true;
    host.__p3Calls = [];
    host.__TAURI_INTERNALS__ = {
      async invoke(command) {
        host.__p3Calls.push(command);
        if (command !== 'get_runtime_info')
          throw new Error('UNEXPECTED_COMMAND_FIXTURE');
        return {
          app_version: 'P3-UI-FIXTURE',
          os: 'fixture',
          arch: 'fixture',
          capture_backend: 'fixture',
          remote_access_enabled: false,
          p3_enabled: true,
          p004_enabled: true,
          p005_enabled: true,
        };
      },
    };
    document.addEventListener('DOMContentLoaded', () => {
      const p = document.createElement('p');
      p.textContent = 'Phase3 SDK替身 · 仅UI门禁检查 · 非原生远控实测';
      document.body.prepend(p);
    });
  });
  await page.goto('/#/connect');
  const section = page.getByRole('region', { name: '局域网远程会话' });
  await expect(section).toBeVisible();
  await expect(page.getByRole('navigation', { name: '主导航' })).toBeVisible();
  await expect(
    section.getByRole('button', { name: '确认后连接' }),
  ).toBeDisabled();
  await expect(
    section.getByRole('button', { name: '停止并回收' }),
  ).toBeDisabled();
  await expect(section).toContainText('未接入公网');
  await expect(section.getByText('连接后在此显示另一台主屏')).toBeVisible();
  await page.getByRole('link', { name: '设置', exact: true }).click();
  await expect(section).toBeVisible();
  expect(
    await page.evaluate(
      () => (window as unknown as { __p3Calls: string[] }).__p3Calls,
    ),
  ).toEqual(['get_runtime_info']);
});
