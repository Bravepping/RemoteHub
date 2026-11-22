import { expect, test } from '@playwright/test';

const routes = [
  ['home', '首页'],
  ['devices', '设备'],
  ['connect', '连接'],
  ['sessions', '会话'],
  ['settings', '设置'],
] as const;
test('browser workspace exposes five real pages with honest unavailable states', async ({
  page,
}, testInfo) => {
  await page.goto('/');
  await expect(page.getByRole('alert')).toContainText('浏览器预览');
  for (const [id, title] of routes) {
    await page
      .getByRole('navigation', { name: '主导航' })
      .getByRole('link', { name: title, exact: true })
      .click();
    await expect(
      page.getByRole('heading', { level: 1, name: title, exact: true }),
    ).toBeVisible();
    await expect(page.locator('nav [aria-current="page"]')).toHaveText(title);
    await expect(page).toHaveURL(new RegExp(`#/${id}$`));
    await page.screenshot({
      path: testInfo.outputPath(`${id}.png`),
      fullPage: true,
    });
  }
  await expect(
    page.locator('dt').filter({ hasText: '应用版本' }),
  ).toBeVisible();
  await expect(page.getByText('未读取', { exact: true })).toHaveCount(5);
  await expect(page.getByRole('checkbox')).toHaveCount(0);
  await expect(page.getByRole('button', { name: '开始采集实验' })).toHaveCount(
    0,
  );
});
test('deep links reload and browser history remains local navigation', async ({
  page,
}) => {
  await page.goto('/#/devices');
  await expect(
    page.getByRole('heading', { name: '设备列表尚未接入' }),
  ).toBeVisible();
  await expect(page.getByRole('button', { name: '添加设备' })).toBeDisabled();
  await page.getByRole('link', { name: '了解连接状态' }).click();
  await expect(
    page.getByRole('heading', { name: '连接能力暂不可用' }),
  ).toBeVisible();
  await expect(page.getByRole('button', { name: '连接设备' })).toBeDisabled();
  await page.reload();
  await expect(
    page.getByRole('heading', { level: 1, name: '连接', exact: true }),
  ).toBeVisible();
  await page.goBack();
  await expect(
    page.getByRole('heading', { level: 1, name: '设备', exact: true }),
  ).toBeVisible();
  await page.goForward();
  await expect(
    page.getByRole('heading', { level: 1, name: '连接', exact: true }),
  ).toBeVisible();
});
test('unknown and auto-start-looking hashes fall back without starting a session', async ({
  page,
}) => {
  await page.goto('/#/connect?autoStart=true');
  await expect(
    page.getByRole('heading', { level: 1, name: '首页' }),
  ).toBeVisible();
  await expect(page.getByText('暂不可用', { exact: true })).toBeVisible();
  await expect(page.locator('video')).toHaveCount(0);
  await expect(page.locator('input')).toHaveCount(0);
});
test('keyboard navigation and skip link preserve the selected page', async ({
  page,
}) => {
  await page.goto('/#/settings');
  await page.keyboard.press('Tab');
  await expect(page.getByRole('link', { name: '跳到页面内容' })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.locator('main')).toBeFocused();
  await expect(page).toHaveURL(/#\/settings$/);
  const link = page
    .getByRole('navigation')
    .getByRole('link', { name: '设备', exact: true });
  await link.focus();
  await page.keyboard.press('Enter');
  await expect(
    page.getByRole('heading', { level: 1, name: '设备', exact: true }),
  ).toBeFocused();
});
for (const viewport of [
  { width: 900, height: 600 },
  { width: 640, height: 720 },
  { width: 450, height: 700 },
]) {
  test(`layout stays reachable at ${viewport.width} pixels`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize(viewport);
    await page.goto('/#/connect');
    await expect(page.getByRole('navigation')).toBeVisible();
    await expect(
      page.getByRole('heading', { name: '连接能力暂不可用' }),
    ).toBeVisible();
    const overflows = await page.evaluate(
      () => document.documentElement.scrollWidth > window.innerWidth,
    );
    expect(overflows).toBe(false);
    await page
      .getByRole('navigation')
      .getByRole('link', { name: '设置', exact: true })
      .click();
    await expect(
      page.getByRole('heading', { level: 1, name: '设置' }),
    ).toBeVisible();
    await page.screenshot({
      path: testInfo.outputPath(`settings-${viewport.width}.png`),
      fullPage: true,
    });
  });
}
