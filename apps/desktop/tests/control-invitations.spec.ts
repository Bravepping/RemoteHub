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
                  return {
                    id: 'a'.repeat(32),
                    role: 'host',
                    expires_at: 1900000000,
                  };
                default:
                  return {
                    id: 'b'.repeat(32),
                    role: 'viewer',
                    expires_at: 1900000001,
                  };
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
  const open = panel.getByRole('button', { name: '打开本轮邀请' });
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
  await start.click();
  await expect(panel.getByRole('status')).toContainText('本次会话已停止');
  expect(await starts()).toEqual([
    { role: 'host', invitationId: 'a'.repeat(32) },
    { role: 'viewer', invitationId: 'b'.repeat(32) },
  ]);
});
