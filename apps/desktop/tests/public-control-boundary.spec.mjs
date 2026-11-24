import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';

test('entering a video letterbox during a drag releases input and keeps video open', async ({
  page,
}) => {
  const frame = readFileSync(
    new URL(
      '../../../crates/hub-rendezvous-probe/src/bin/v2_signaling/synthetic_32x32.h264',
      import.meta.url,
    ),
  );
  const events = [];
  await page.exposeFunction('recordInput', (event) => events.push(event));
  await page.addInitScript((bytes) => {
    let sentFrame = false;
    Object.assign(window, {
      isTauri: true,
      __TAURI_INTERNALS__: {
        async invoke(command, args) {
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
              };
            case 'public_video_start':
              return 'a'.repeat(32);
            case 'public_video_status':
              return { phase: 'ready', input_state: 'allowed' };
            case 'public_video_next_frame': {
              if (sentFrame) return new ArrayBuffer(0);
              sentFrame = true;
              const frame = new Uint8Array(24 + bytes.length);
              new DataView(frame.buffer).setUint32(16, bytes.length, true);
              frame[20] = 1;
              frame.set(bytes, 24);
              return frame.buffer;
            }
            case 'public_control_send_input':
              return window.recordInput(args.event);
            default:
              throw new Error('UNEXPECTED_COMMAND_FIXTURE');
          }
        },
      },
    });
  }, Array.from(frame));
  await page.goto('/');
  const panel = page.getByRole('region', { name: '公网远控测试' });
  await panel.getByLabel('观看屏幕').check();
  await panel.getByRole('button', { name: '开始本次会话' }).click();
  const enable = panel.getByRole('button', { name: '启用本次键鼠' });
  await enable.click();
  const canvas = panel.locator('canvas');
  await canvas.scrollIntoViewIfNeeded();
  const box = await canvas.boundingBox();
  if (!box || box.width <= box.height) throw new Error('Expected side bars');
  const center = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  await page.mouse.move(center.x, center.y);
  await page.mouse.down();
  await expect
    .poll(() => events.at(-1))
    .toBe('{"kind":"mouse_button","button":0,"down":true}');
  await page.mouse.move(box.x + 2, center.y);
  await expect(enable).toBeEnabled();
  await expect.poll(() => events.at(-1)).toBe('{"kind":"release_all"}');
  const paused = [...events];
  await page.mouse.move(center.x, center.y);
  await page.mouse.up();
  await page.keyboard.press('a');
  await expect(panel.getByRole('status')).toContainText('正在显示');
  expect(events).toEqual(paused);
});
