import { expect, test } from '@playwright/test';

test('explicit SDK transport fixture uses actual Pinia, clears failures and never calls media commands', async ({
  page,
}, testInfo) => {
  await page.addInitScript(() => {
    const fixtureHost = window as unknown as {
      isTauri: boolean;
      __TAURI_INTERNALS__: { invoke: (command: string) => Promise<unknown> };
      __p104Fixture: { fail: boolean; calls: string[] };
    };
    fixtureHost.isTauri = true;
    fixtureHost.__p104Fixture = { fail: false, calls: [] };
    fixtureHost.__TAURI_INTERNALS__ = {
      async invoke(command) {
        fixtureHost.__p104Fixture.calls.push(command);
        if (command !== 'get_runtime_info')
          throw new Error('TEST_UNEXPECTED_COMMAND');
        if (fixtureHost.__p104Fixture.fail)
          throw new Error('private-native-failure-fixture');
        return {
          app_version: 'P104-TEST-FIXTURE',
          os: 'fixture-os',
          arch: 'fixture-arch',
          capture_backend: 'fixture-backend',
          remote_access_enabled: false,
        };
      },
    };
    document.addEventListener('DOMContentLoaded', () => {
      const notice = document.createElement('p');
      notice.textContent =
        'P1-04 自动测试替身 · 非原生IPC实测 · 不含真实设备数据';
      notice.id = 'p104-test-fixture-notice';
      notice.style.cssText =
        'margin:0;padding:12px;background:#fff2c8;color:#5c4511';
      document.body.prepend(notice);
    });
  });
  await page.goto('/#/settings');
  await expect(
    page.getByText('P104-TEST-FIXTURE', { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText('fixture-backend', { exact: true }),
  ).toBeVisible();
  await page
    .getByRole('navigation')
    .getByRole('link', { name: '设备', exact: true })
    .click();
  await expect(
    page.getByRole('region', { name: '本机能力卡片' }),
  ).toContainText('fixture-os');
  expect(
    await page.evaluate(
      () =>
        (window as unknown as { __p104Fixture: { calls: string[] } })
          .__p104Fixture.calls,
    ),
  ).toEqual(['get_runtime_info']);
  await page
    .getByRole('navigation')
    .getByRole('link', { name: '设置', exact: true })
    .click();
  await page.evaluate(() => {
    (
      window as unknown as { __p104Fixture: { fail: boolean } }
    ).__p104Fixture.fail = true;
  });
  await page.getByRole('button', { name: '重新检测' }).click();
  await expect(page.getByRole('alert')).toContainText('暂时无法读取');
  await expect(page.getByText('未读取', { exact: true })).toHaveCount(5);
  await expect(
    page.getByText('P104-TEST-FIXTURE', { exact: true }),
  ).toHaveCount(0);
  await expect(page.locator('body')).not.toContainText(
    'private-native-failure-fixture',
  );
  await page.evaluate(() => {
    (
      window as unknown as { __p104Fixture: { fail: boolean } }
    ).__p104Fixture.fail = false;
  });
  await page.getByRole('button', { name: '重新检测' }).click();
  await expect(
    page.getByText('P104-TEST-FIXTURE', { exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as unknown as { __p104Fixture: { calls: string[] } })
          .__p104Fixture.calls,
    ),
  ).toEqual(['get_runtime_info', 'get_runtime_info', 'get_runtime_info']);
  await expect(page.locator('video, canvas')).toHaveCount(0);
  await page.screenshot({
    path: testInfo.outputPath('p104-explicit-sdk-fixture.png'),
    fullPage: true,
  });
});
