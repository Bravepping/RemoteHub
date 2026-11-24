import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';

test('dedicated invitation main only observes, focuses and stops the actual remote state', async ({
  page,
}) => {
  await page.addInitScript(() => {
    const calls = [];
    let phase;
    let invitation = 0;
    Object.assign(window, {
      isTauri: true,
      ownedWindowCalls: calls,
      __TAURI_INTERNALS__: {
        metadata: { currentWindow: { label: 'main' } },
        async invoke(command, args) {
          calls.push(command);
          switch (command) {
            case 'get_runtime_info':
              return {
                app_version: 'fixture',
                os: 'windows',
                arch: 'x86_64',
                capture_backend: 'fixture',
                remote_access_enabled: false,
                public_video_available: true,
                public_control_available: true,
                public_invitation_picker: true,
                public_dedicated_session: true,
              };
            case 'select_control_invitation':
              invitation++;
              return [
                {
                  id: String(invitation).repeat(32),
                  role: 'viewer',
                  round: null,
                  expires_at: 2_000_000_000,
                },
              ];
            case 'public_session_window_open':
              if (
                args.role !== 'viewer' ||
                args.invitationId !== String(invitation).repeat(32)
              )
                throw new Error();
              phase = 'ready';
              return 'opened';
            case 'public_session_window_snapshot':
              return phase
                ? {
                    role: 'viewer',
                    status: { phase, connection_path: 'relay' },
                  }
                : {};
            case 'public_session_window_focus':
              return null;
            case 'public_session_window_stop':
              phase = 'closed';
              return null;
            default:
              throw new Error('UNEXPECTED_MAIN_COMMAND');
          }
        },
      },
    });
  });
  await page.goto('/');
  const panel = page.getByRole('region', { name: '公网远控测试' });
  await panel.getByRole('button', { name: '打开邀请 ZIP 或 JSON' }).click();
  await panel.getByRole('button', { name: '开始本次会话' }).click();
  await expect(panel.getByText('已建立远程连接')).toBeVisible();
  await expect(panel.getByText('TURN 中继', { exact: true })).toBeVisible();
  await expect(panel.locator('canvas')).toHaveCount(0);
  await panel.getByRole('button', { name: '显示远控窗口' }).click();
  await panel.getByRole('button', { name: '断开连接' }).click();
  await expect(panel.getByText('已建立远程连接')).toHaveCount(0);
  for (let cycle = 0; cycle < 2; cycle++) {
    await panel.getByRole('button', { name: '打开邀请 ZIP 或 JSON' }).click();
    await panel.getByRole('button', { name: '开始本次会话' }).click();
    await expect(panel.getByText('已建立远程连接')).toBeVisible();
    await panel.getByRole('button', { name: '断开连接' }).click();
    await expect(panel.getByText('已建立远程连接')).toHaveCount(0);
  }
  const calls = await page.evaluate(() => window.ownedWindowCalls);
  expect(
    calls.filter((call) => call === 'public_session_window_open'),
  ).toHaveLength(3);
  expect(calls).toContain('public_session_window_focus');
  expect(calls).toContain('public_session_window_stop');
  expect(calls).not.toContain('public_video_start');
  expect(calls).not.toContain('public_video_next_frame');
  expect(calls).not.toContain('public_control_send_input');
});

test('owned viewer attaches once; control center and blur pause input without ending video or auto-resuming', async ({
  page,
  browserName,
}) => {
  const frame = readFileSync(
    new URL(
      '../../../crates/hub-rendezvous-probe/src/bin/v2_signaling/synthetic_32x32.h264',
      import.meta.url,
    ),
  );
  const input = [];
  await page.exposeFunction('recordOwnedInput', (event) =>
    input.push(JSON.parse(event)),
  );
  await page.addInitScript((bytes) => {
    const calls = [];
    let sentFrame = false;
    const generation = 'b'.repeat(32);
    Object.assign(window, {
      isTauri: true,
      ownedWindowCalls: calls,
      __TAURI_INTERNALS__: {
        metadata: {
          currentWindow: { label: 'remote-session-' + 'c'.repeat(32) },
        },
        async invoke(command, args) {
          calls.push(command);
          if (command === 'public_session_window_attach') {
            return {
              generation,
              role: 'viewer',
              status: { phase: 'starting' },
            };
          }
          if (command === 'public_video_status')
            return {
              phase: 'ready',
              input_state: window.inputEndedFixture ? 'ended' : 'allowed',
              ...(window.sustainedFixture
                ? { input_wheel: true, round_seconds: 1800 }
                : {}),
              connection_path: 'direct',
            };
          if (command === 'public_video_next_frame') {
            if (sentFrame) return new ArrayBuffer(0);
            sentFrame = true;
            const frame = new Uint8Array(24 + bytes.length);
            new DataView(frame.buffer).setUint32(16, bytes.length, true);
            frame[20] = 1;
            frame.set(bytes, 24);
            return frame.buffer;
          }
          if (command === 'public_session_window_display') {
            if (
              args.generation !== generation ||
              !['minimize', 'fullscreen', 'restore'].includes(args.action)
            )
              throw new Error();
            (window.displayActions ??= []).push(args.action);
            return;
          }
          if (command === 'public_session_window_arm_input') {
            if (args.generation !== generation)
              throw new Error('UNEXPECTED_OWNED_GENERATION');
            return;
          }
          if (command === 'public_control_send_input') {
            if (window.rejectNextOwnedInput) {
              window.rejectNextOwnedInput = false;
              throw 'PUBLIC_CONTROL_WINDOW_PAUSED';
            }
            return window.recordOwnedInput(args.event);
          }
          throw new Error('UNEXPECTED_OWNED_COMMAND');
        },
      },
    });
  }, Array.from(frame));
  await page.goto('/');
  const panel = page.getByRole('region', { name: '公网远控测试' });
  await expect(
    panel.getByRole('heading', { name: '远控窗口 · 观看方' }),
  ).toBeVisible();
  await expect(panel.getByText('播放分辨率 32 × 32')).toBeVisible();
  await expect(panel.getByText(/FPS|bitrate|码率/)).toHaveCount(0);
  const enable = panel.getByRole('button', { name: '启用本次键鼠' });
  await enable.click();
  await page.keyboard.down('a');
  await expect.poll(() => input.at(-1)?.kind).toBe('key');
  await panel.getByRole('button', { name: '控制中心', exact: true }).click();
  await expect.poll(() => input.at(-1)?.kind).toBe('release_all');
  const drawer = panel.getByRole('complementary', { name: '控制中心' });
  await expect(
    drawer.getByRole('button', { name: '关闭控制中心' }),
  ).toBeFocused();
  await expect(drawer.getByText('实际播放分辨率 32 × 32')).toBeVisible();
  await drawer.getByRole('button', { name: '最小化' }).click();
  await drawer.getByRole('button', { name: '全屏', exact: true }).click();
  await drawer.getByRole('button', { name: '退出全屏' }).click();
  expect(await page.evaluate(() => window.displayActions)).toEqual([
    'minimize',
    'fullscreen',
    'restore',
  ]);
  await page.keyboard.press('Escape');
  await expect(drawer).toHaveCount(0);
  await expect(
    panel.getByRole('button', { name: '控制中心', exact: true }),
  ).toBeFocused();
  await expect(enable).toBeEnabled();
  await page.keyboard.up('a');
  const paused = input.length;
  await page.keyboard.press('b');
  expect(input).toHaveLength(paused);

  await enable.click();
  await page.keyboard.down('c');
  await expect.poll(() => input.at(-1)?.kind).toBe('key');
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await expect.poll(() => input.at(-1)?.kind).toBe('release_all');
  await expect(panel.getByRole('status')).toContainText('正在显示');
  await expect(enable).toBeEnabled();

  await enable.click();
  await page.evaluate(() => {
    window.rejectNextOwnedInput = true;
  });
  const beforeNativePause = input.length;
  await page.keyboard.press('d');
  await expect
    .poll(() => input.slice(beforeNativePause))
    .toEqual([{ kind: 'release_all' }]);
  await expect(enable).toBeEnabled();

  await enable.click();
  const surface = panel.locator('.public-video-surface');
  const box = await surface.boundingBox();
  const wheel = () =>
    surface.dispatchEvent('wheel', {
      clientX: box.x + box.width / 2,
      clientY: box.y + box.height / 2,
      deltaX: 0,
      deltaY: 300,
    });
  await wheel();
  expect(input.some((event) => event.kind === 'wheel')).toBe(false);
  await page.evaluate(() => {
    window.sustainedFixture = true;
  });
  await expect
    .poll(async () => {
      await wheel();
      return input.find((event) => event.kind === 'wheel')?.delta;
    })
    .toBe(-120);
  await page.evaluate(() => {
    window.inputEndedFixture = true;
  });
  await expect(
    panel.getByText('本次键鼠权限已结束，画面会继续显示。'),
  ).toBeVisible();
  await expect(panel.getByRole('status')).toContainText('正在显示');
  const calls = await page.evaluate(() => window.ownedWindowCalls);
  expect(
    calls.filter((command) => command === 'public_session_window_attach'),
  ).toHaveLength(1);
  expect(calls).not.toContain('get_runtime_info');
  expect(calls).not.toContain('public_video_start');
  expect(calls).not.toContain('public_video_stop');
});
