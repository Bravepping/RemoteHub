import { expect, test } from '@playwright/test';

test('previews each locally selected invitation and starts only its own round', async ({
  page,
}) => {
  await page.addInitScript(() => {
    const starts: unknown[] = [];
    let picks = 0;
    Object.assign(window, {
      isTauri: true,
      invitationTestStarts: starts,
      __TAURI_INTERNALS__: {
        async invoke(command: string, args?: Record<string, unknown>) {
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
              };
            case 'select_control_invitation':
              switch (++picks) {
                case 1:
                  return null;
                case 2:
                  throw 'PUBLIC_VIDEO_PROFILE_EXPIRED';
                case 3:
                  return [
                    {
                      id: 'a'.repeat(32),
                      role: 'host',
                      round: null,
                      expires_at: 1900000000,
                    },
                  ];
                default:
                  return [1, 2].map((round) => ({
                    id: (round === 1 ? 'b' : 'd').repeat(32),
                    role: 'viewer',
                    round,
                    expires_at: 1900000001,
                  }));
              }
            case 'public_video_start':
              starts.push(args);
              return 'c'.repeat(32);
            case 'public_video_status':
              return { phase: 'closed', profile_consumed: true };
            case 'public_video_stop':
              return;
            default:
              throw new Error('UNEXPECTED_COMMAND_FIXTURE');
          }
        },
      },
    });
  });
  await page.goto('/');
  const panel = page.getByRole('region', { name: '公网远控测试' });
  const open = panel.getByRole('button', { name: '打开邀请 ZIP 或 JSON' });
  const start = panel.getByRole('button', { name: '开始本次会话' });
  const starts = () =>
    page.evaluate(
      () =>
        (window as unknown as { invitationTestStarts: unknown[] })
          .invitationTestStarts,
    );
  await expect(start).toBeDisabled();
  await open.click();
  await expect(start).toBeDisabled();
  await open.click();
  await expect(panel.getByRole('alert')).toContainText('邀请已过期');
  await open.click();
  await expect(panel).toContainText('本机共享屏幕');
  expect(await starts()).toEqual([]);
  await start.click();
  await expect(panel.getByRole('status')).toContainText('本次会话已停止');
  await expect(start).toBeDisabled();
  await open.click();
  await expect(panel).toContainText('本机观看屏幕');
  await expect(start).toBeDisabled();
  await panel.getByLabel('第 2 轮').check();
  await start.click();
  await expect(panel.getByRole('status')).toContainText('本次会话已停止');
  await expect(panel.getByLabel('第 2 轮')).toHaveCount(0);
  await expect(start).toBeDisabled();
  await panel.getByLabel('第 1 轮').check();
  await start.click();
  await expect(panel.getByRole('status')).toContainText('本次会话已停止');
  expect(await starts()).toEqual([
    { role: 'host', invitationId: 'a'.repeat(32) },
    { role: 'viewer', invitationId: 'd'.repeat(32) },
    { role: 'viewer', invitationId: 'b'.repeat(32) },
  ]);
});

for (const role of ['host', 'viewer'] as const) {
  test(
    'displays actual ' + role + ' path and clears it when stopped',
    async ({ page }) => {
      await page.addInitScript((role) => {
        const state = {
          phase: 'starting',
          path: undefined as string | undefined,
        };
        Object.assign(window, {
          routeFixture: state,
          isTauri: true,
          __TAURI_INTERNALS__: {
            async invoke(command: string) {
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
                  };
                case 'select_control_invitation':
                  return [
                    {
                      id: 'a'.repeat(32),
                      role,
                      round: 1,
                      expires_at: 1900000000,
                    },
                  ];
                case 'public_video_start':
                  return 'b'.repeat(32);
                case 'public_video_status':
                  return {
                    phase: state.phase,
                    connection_path: state.path,
                    profile_consumed: true,
                  };
                case 'public_video_next_frame':
                  return new ArrayBuffer(0);
                case 'public_video_stop':
                  state.phase = 'closed';
                  return;
                default:
                  throw new Error('UNEXPECTED_COMMAND_FIXTURE');
              }
            },
          },
        });
      }, role);
      await page.goto('/');
      await page.getByRole('button', { name: '打开邀请 ZIP 或 JSON' }).click();
      await page.getByLabel('第 1 轮').check();
      await page.getByRole('button', { name: '开始本次会话' }).click();
      const path = page.getByTestId('connection-path');
      await expect(path).toContainText('正在确认路径');
      for (const next of ['direct', 'relay']) {
        await page.evaluate((next) => {
          const state = (
            window as unknown as {
              routeFixture: { phase: string; path: string };
            }
          ).routeFixture;
          state.phase = 'ready';
          state.path = next;
        }, next);
        await expect(path).toHaveText(
          next === 'direct' ? '本次连接：直连' : '本次连接：TURN 中继',
        );
      }
      await page.getByRole('button', { name: '停止并清屏' }).click();
      await expect(path).toBeHidden();
      await expect(
        page.getByRole('region', { name: '公网远控测试' }).getByRole('status'),
      ).toContainText('本次会话已停止');
    },
  );
}
