import { expect, test } from '@playwright/test';

for (const unavailable of [false, true]) {
  test(`identity SDK fixture: ${unavailable ? 'fixed failure with no leaked fields' : 'public identity readout and unchanged offline state'}`, async ({
    page,
  }) => {
    await page.addInitScript((failed) => {
      const host = window as unknown as {
        isTauri: boolean;
        __TAURI_INTERNALS__: { invoke: (command: string) => Promise<unknown> };
      };
      host.isTauri = true;
      host.__TAURI_INTERNALS__ = {
        async invoke(command) {
          if (command !== 'get_runtime_info')
            throw new Error('UNEXPECTED_COMMAND_FIXTURE');
          return {
            app_version: 'P201-TEST-FIXTURE',
            os: 'fixture-os',
            arch: 'fixture-arch',
            capture_backend: 'fixture-backend',
            remote_access_enabled: false,
            identity: failed
              ? {
                  status: 'unavailable',
                  error_code: 'IDENTITY_UNPROTECT_FAILED',
                }
              : {
                  status: 'ready',
                  device_id: 'RH-0123-4567-89AB',
                  fingerprint: '0123456789ab'.repeat(5) + '0123',
                },
          };
        },
      };
      document.addEventListener('DOMContentLoaded', () => {
        const notice = document.createElement('p');
        notice.textContent = 'P2-01 SDK替身 · 非原生身份/设备或网络验收';
        document.body.prepend(notice);
      });
    }, unavailable);
    await page.goto('/#/settings');
    const section = page.getByRole('region', { name: '本机设备身份' });
    if (unavailable) {
      await expect(section.getByRole('alert')).toContainText('已保留原文件');
      await expect(section.locator('code')).toHaveCount(0);
    } else {
      await expect(
        section.getByText('RH-0123-4567-89AB', { exact: true }),
      ).toBeVisible();
      await expect(section.locator('code')).toHaveCount(2);
      await expect(section).toContainText('不代表已联网');
    }
    await expect(
      page.getByText('远程访问关闭', { exact: true }).first(),
    ).toBeVisible();
  });
}
