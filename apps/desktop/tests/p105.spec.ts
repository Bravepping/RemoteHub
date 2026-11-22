import { expect, test, type Page } from '@playwright/test';

async function fixture(
  page: Page,
  initialMode: 'ready' | 'hold' | 'fail' = 'ready',
) {
  await page.addInitScript((mode) => {
    const host = window as unknown as {
      isTauri: boolean;
      __TAURI_INTERNALS__: { invoke: (command: string) => Promise<unknown> };
      __p105Fixture: { mode: string; calls: number; release?: () => void };
    };
    host.isTauri = true;
    host.__p105Fixture = { mode, calls: 0 };
    const data = () => ({
      app_version: 'P105-TEST-FIXTURE',
      os: 'fixture-os',
      arch: 'fixture-arch',
      capture_backend: 'fixture-backend',
      remote_access_enabled: false,
    });
    host.__TAURI_INTERNALS__ = {
      async invoke(command) {
        if (command !== 'get_runtime_info')
          throw new Error('UNEXPECTED_COMMAND_FIXTURE');
        const state = host.__p105Fixture;
        ++state.calls;
        if (state.mode === 'hold')
          return new Promise((resolve) => {
            state.release = () => resolve(data());
          });
        if (state.mode === 'fail')
          throw new Error(
            'SENSITIVE_TOKEN_FIXTURE@' + [192, 0, 2, 8].join('.'),
          );
        if (state.mode === 'invalid')
          return { ...data(), remote_access_enabled: 'private-fixture' };
        return data();
      },
    };
    document.addEventListener('DOMContentLoaded', () => {
      const notice = document.createElement('p');
      notice.textContent =
        'P1-05 SDK故障替身 · 非原生IPC验收 · 非真实设备或网络数据';
      notice.style.cssText =
        'margin:0;padding:12px;background:#fff2c8;color:#5c4511';
      document.body.prepend(notice);
    });
  }, initialMode);
}
async function mode(page: Page, value: string) {
  await page.evaluate((v) => {
    (
      window as unknown as { __p105Fixture: { mode: string } }
    ).__p105Fixture.mode = v;
  }, value);
}
test('SDK fixture: refresh clears stale fields, cancellation survives late success and retry recovers', async ({
  page,
}, info) => {
  await fixture(page);
  await page.goto('/#/settings');
  await expect(
    page.getByText('P105-TEST-FIXTURE', { exact: true }),
  ).toBeVisible();
  await mode(page, 'hold');
  await page.getByRole('button', { name: '重新检测' }).click();
  await expect(
    page.getByRole('region', { name: '本机信息读取状态' }),
  ).toContainText('正在刷新');
  await expect(
    page.getByText('P105-TEST-FIXTURE', { exact: true }),
  ).toHaveCount(0);
  await expect(page.locator('dd').filter({ hasText: '读取中…' })).toHaveCount(
    5,
  );
  await page.screenshot({
    path: info.outputPath('refreshing-sdk-fixture.png'),
    fullPage: true,
  });
  await page.getByRole('button', { name: '取消本次读取' }).click();
  await expect(
    page.getByRole('region', { name: '本机信息读取状态' }),
  ).toContainText('读取已取消');
  await page.evaluate(() =>
    (
      window as unknown as { __p105Fixture: { release?: () => void } }
    ).__p105Fixture.release?.(),
  );
  await expect(
    page.getByText('P105-TEST-FIXTURE', { exact: true }),
  ).toHaveCount(0);
  await expect(page.getByText('未读取', { exact: true })).toHaveCount(5);
  await mode(page, 'ready');
  await page.getByRole('button', { name: '重试读取' }).click();
  await expect(
    page.getByRole('region', { name: '本机信息读取状态' }),
  ).toContainText('已重新读取');
  await expect(
    page.getByText('P105-TEST-FIXTURE', { exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as unknown as { __p105Fixture: { calls: number } })
          .__p105Fixture.calls,
    ),
  ).toBe(3);
});
test('SDK fixture: native failure never mentions browser or leaks exception, malformed data can recover', async ({
  page,
}, info) => {
  await fixture(page, 'fail');
  await page.goto('/#/settings');
  const alert = page.getByRole('alert');
  await expect(alert).toContainText('请重新检测');
  await expect(alert).not.toContainText('浏览器');
  await expect(page.locator('body')).not.toContainText(
    'SENSITIVE_TOKEN_FIXTURE',
  );
  expect(await page.locator('body').innerText()).not.toMatch(
    /\b(?:\d{1,3}\.){3}\d{1,3}\b/,
  );
  await page.screenshot({
    path: info.outputPath('native-error-sdk-fixture.png'),
    fullPage: true,
  });
  await mode(page, 'invalid');
  await page.getByRole('button', { name: '重试读取' }).click();
  await expect(alert).toContainText('无法识别');
  await expect(alert).not.toContainText('private-fixture');
  await mode(page, 'ready');
  await page.getByRole('button', { name: '重试读取' }).click();
  await expect(
    page.getByText('P105-TEST-FIXTURE', { exact: true }),
  ).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
});
test('SDK fixture: actual five second read timeout stays bounded until a manual retry', async ({
  page,
}, info) => {
  await fixture(page, 'hold');
  await page.goto('/#/settings');
  await expect(
    page.getByRole('region', { name: '本机信息读取状态' }),
  ).toContainText('正在读取');
  await expect(page.getByRole('alert')).toContainText('读取本机能力超时', {
    timeout: 8000,
  });
  await expect(page.getByText('未读取', { exact: true })).toHaveCount(5);
  expect(
    await page.evaluate(
      () =>
        (window as unknown as { __p105Fixture: { calls: number } })
          .__p105Fixture.calls,
    ),
  ).toBe(1);
  await page.screenshot({
    path: info.outputPath('timeout-sdk-fixture.png'),
    fullPage: true,
  });
  await mode(page, 'ready');
  await page.getByRole('button', { name: '重试读取' }).click();
  await expect(
    page.getByRole('region', { name: '本机信息读取状态' }),
  ).toContainText('已重新读取');
  await expect(page.locator('video, canvas')).toHaveCount(0);
  await page.screenshot({
    path: info.outputPath('recovered-sdk-fixture.png'),
    fullPage: true,
  });
});
