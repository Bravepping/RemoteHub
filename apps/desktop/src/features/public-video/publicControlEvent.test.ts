import { expect, it } from 'vitest';
import {
  normalizedPublicPoint,
  publicControlEvent,
} from './publicControlEvent';

it('normalizes the presented region and rejects letterbox bars', () => {
  const box = { left: 16, top: 20, width: 160, height: 160 };
  expect(normalizedPublicPoint(box, 160, 90, 96, 100)).toEqual({
    x_norm: 0.5,
    y_norm: 0.5,
  });
  expect(normalizedPublicPoint(box, 160, 90, 96, 30)).toBeUndefined();
  expect(normalizedPublicPoint(box, 0, 90, 96, 100)).toBeUndefined();
  expect(normalizedPublicPoint(box, 160, 90, Infinity, 100)).toBeUndefined();
});

it('serializes exact Rust event fields with no extra metadata or invalid inputs', () => {
  const extra = {
    kind: 'mouse_button',
    button: 1,
    down: true,
    seq: 2,
  } as const;
  expect(publicControlEvent(extra)).toBe(
    '{"kind":"mouse_button","button":1,"down":true}',
  );
  expect(publicControlEvent({ kind: 'mouse_move', x_norm: 1, y_norm: 0 })).toBe(
    '{"kind":"mouse_move","x_norm":1,"y_norm":0}',
  );
  expect(
    publicControlEvent({ kind: 'mouse_move', x_norm: -1, y_norm: 0 }),
  ).toBeUndefined();
  expect(
    publicControlEvent({ kind: 'key', key: 'Escape', down: true }),
  ).toBeUndefined();
  expect(
    publicControlEvent({ kind: 'mouse_button', button: 3, down: true }),
  ).toBeUndefined();
});
