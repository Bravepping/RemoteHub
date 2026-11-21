import { expect, test } from '@playwright/test';
test('real Edge stores relay-only transport policy without gathering or network probe', async ({
  page,
}) => {
  await page.goto('/');
  const policy = await page.evaluate(() => {
    // No local description, capture, gather or remote endpoint is used here.
    const pc = new RTCPeerConnection({
      iceTransportPolicy: 'relay',
      iceServers: [],
    });
    const result = pc.getConfiguration().iceTransportPolicy;
    pc.close();
    return result;
  });
  expect(policy).toBe('relay');
});
