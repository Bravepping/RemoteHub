import { expect, test } from '@playwright/test';
import { resolve } from 'node:path';

test('browser preview does not impersonate a working desktop core', async ({
  page,
}) => {
  await page.goto('/');
  await expect(
    page.getByRole('heading', { name: '先让真实画面跑起来' }),
  ).toBeVisible();
  await expect(page.getByRole('alert')).toContainText('浏览器预览');
  await expect(page.getByRole('checkbox')).toBeDisabled();
  await expect(
    page.getByRole('button', { name: '开始采集实验' }),
  ).toBeDisabled();
  await page.getByRole('button', { name: '重新检测' }).click();
  await expect(page.getByRole('status')).toContainText('本机能力未连接');
  await page.screenshot({
    path: resolve(
      '../..',
      process.env.REMOTEHUB_VERIFY_DIR ?? 'artifacts',
      'browser-preview.png',
    ),
    fullPage: true,
  });
});
