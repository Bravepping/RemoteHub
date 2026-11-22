import { expect, test } from '@playwright/test';

test('connection preparation is modal, inert and returns focus on Escape and close', async ({
  page,
}, testInfo) => {
  const requests: string[] = [];
  page.on('request', (r) => {
    if (!new URL(r.url()).hostname.match(/^(127\.0\.0\.1|localhost)$/))
      requests.push(r.resourceType());
  });
  await page.goto('/#/connect');
  const launch = page.getByRole('button', {
    name: '查看连接准备',
    exact: true,
  });
  await launch.click();
  const dialog = page.getByRole('dialog', { name: '连接准备', exact: true });
  await expect(dialog).toBeVisible();
  await expect(
    dialog.getByRole('button', { name: '关闭连接准备' }),
  ).toBeFocused();
  await expect(dialog.getByRole('button', { name: '发起连接' })).toBeDisabled();
  await expect(dialog).toContainText('当前未创建会话');
  await expect(dialog.getByText('未开始', { exact: true })).toHaveCount(2);
  await expect(dialog.locator('input, video, canvas')).toHaveCount(0);
  await page.keyboard.press('Shift+Tab');
  // Native dialog keeps background inert, while Edge may move focus to browser chrome.
  expect(
    await page.evaluate(
      () =>
        !document.hasFocus() ||
        document.querySelector('dialog')!.contains(document.activeElement),
    ),
  ).toBe(true);
  await page.keyboard.press('Tab');
  await expect(
    dialog.getByRole('button', { name: '关闭连接准备' }),
  ).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(
    dialog.getByRole('button', { name: '关闭', exact: true }),
  ).toBeFocused();
  const outsideLink = page
    .getByRole('navigation')
    .getByRole('link', { name: '设备', exact: true });
  await outsideLink.evaluate((element: HTMLElement) => element.focus());
  await expect(outsideLink).not.toBeFocused();
  await page.screenshot({
    path: testInfo.outputPath('connection-dialog.png'),
    fullPage: true,
  });
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  await expect(launch).toBeFocused();
  await launch.click();
  await dialog.getByRole('button', { name: '关闭', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(launch).toBeFocused();
  await launch.click();
  await dialog.getByRole('button', { name: '关闭连接准备' }).click();
  await expect(dialog).toHaveCount(0);
  await expect(launch).toBeFocused();
  expect(requests).toEqual([]);
});

test('route departure removes modal and its focus isolation', async ({
  page,
}) => {
  await page.goto('/#/connect');
  await page.getByRole('button', { name: '查看连接准备', exact: true }).click();
  await page.evaluate(() => {
    window.location.hash = '#/devices';
  });
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(
    page.getByRole('heading', { level: 1, name: '设备', exact: true }),
  ).toBeFocused();
  await page
    .getByRole('navigation')
    .getByRole('link', { name: '首页', exact: true })
    .click();
  await expect(
    page.getByRole('heading', { level: 1, name: '首页', exact: true }),
  ).toBeVisible();
});

test('session toolbar changes only its local visibility and navigation', async ({
  page,
}, testInfo) => {
  await page.goto('/#/sessions');
  const toolbar = page.getByRole('toolbar', { name: '会话工具栏' });
  await expect(toolbar).toContainText('尚未连接');
  for (const name of ['全屏', '画质', '码率上限', '断开'])
    await expect(
      toolbar.getByRole('button', { name, exact: true }),
    ).toBeDisabled();
  await expect(toolbar.getByText('未取得', { exact: true })).toHaveCount(2);
  await expect(page.locator('video, canvas')).toHaveCount(0);
  await page.screenshot({
    path: testInfo.outputPath('session-toolbar.png'),
    fullPage: true,
  });
  await toolbar.getByRole('button', { name: '收起工具栏' }).click();
  await expect(toolbar.getByRole('button', { name: '全屏' })).toHaveCount(0);
  await page.screenshot({
    path: testInfo.outputPath('session-toolbar-collapsed.png'),
    fullPage: true,
  });
  await toolbar.getByRole('button', { name: '展开工具栏' }).focus();
  await page.keyboard.press('Enter');
  await expect(toolbar.getByRole('button', { name: '全屏' })).toBeDisabled();
  await toolbar.getByRole('button', { name: '查看连接准备' }).click();
  await expect(page).toHaveURL(/#\/connect$/);
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

for (const width of [900, 450]) {
  test(`new surfaces remain reachable at ${width} pixels`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize({ width, height: 600 });
    await page.goto('/#/connect');
    await page
      .getByRole('button', { name: '查看连接准备', exact: true })
      .click();
    const dialog = page.getByRole('dialog');
    const box = await dialog.boundingBox();
    expect(box!.x).toBeGreaterThanOrEqual(0);
    expect(box!.width).toBeLessThanOrEqual(width);
    expect(box!.y + box!.height).toBeLessThanOrEqual(601);
    expect(await dialog.evaluate((e) => e.scrollWidth <= e.clientWidth)).toBe(
      true,
    );
    await dialog
      .getByRole('button', { name: '关闭', exact: true })
      .scrollIntoViewIfNeeded();
    await expect(
      dialog.getByRole('heading', { name: '连接准备', exact: true }),
    ).toBeInViewport();
    await expect(
      dialog.getByRole('button', { name: '关闭连接准备' }),
    ).toBeInViewport();
    await page.screenshot({
      path: testInfo.outputPath(`dialog-${width}.png`),
      fullPage: false,
    });
    await dialog.getByRole('button', { name: '关闭', exact: true }).click();
    await page
      .getByRole('navigation')
      .getByRole('link', { name: '会话', exact: true })
      .click();
    await page
      .getByRole('toolbar', { name: '会话工具栏' })
      .scrollIntoViewIfNeeded();
    await expect(
      page.getByRole('button', { name: '查看连接准备' }),
    ).toBeInViewport();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth > window.innerWidth,
      ),
    ).toBe(false);
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.screenshot({
      path: testInfo.outputPath(`session-${width}.png`),
      fullPage: true,
    });
  });
}
