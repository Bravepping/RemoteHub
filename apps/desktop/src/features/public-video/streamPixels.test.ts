import { describe, expect, it, vi } from 'vitest';
import { checkQuadrants } from '../../../stream-display/pixels';

// Use uniform tiles and verify the checker requests each quadrant exactly once.
function check(samples: number[][]) {
  const getImageData = vi.fn(() => {
    const sample = samples.shift()!;
    return {
      data: new Uint8ClampedArray(
        Array.from({ length: 9 }, () => sample).flat(),
      ),
    } as ImageData;
  });
  return { result: checkQuadrants({ getImageData }), getImageData };
}

const gray = (levels: number[]) =>
  levels.map((level) => [level, level, level, 255]);

describe('signed stream test pixel readback', () => {
  it('accepts the fixture in either limited or converted RGB range', () => {
    const rgb = check(gray([23, 90, 160, 235]));
    expect(rgb.result).toEqual({
      pass: true,
      rgba: '23/23/23/255 · 90/90/90/255 · 160/160/160/255 · 235/235/235/255',
    });
    expect(rgb.getImageData.mock.calls).toEqual([
      [7, 7, 3, 3],
      [23, 7, 3, 3],
      [7, 23, 3, 3],
      [23, 23, 3, 3],
    ]);
    expect(check(gray([36, 94, 154, 218])).result.pass).toBe(true);
  });
  it('does not pass a black, reversed, tinted, or transparent image', () => {
    expect(check(gray([0, 0, 0, 0])).result).toEqual({
      pass: false,
      rgba: '0/0/0/255 · 0/0/0/255 · 0/0/0/255 · 0/0/0/255',
    });
    expect(check(gray([218, 154, 94, 36])).result.pass).toBe(false);
    const tinted = gray([36, 94, 154, 218]);
    tinted[0][0] = 64;
    expect(check(tinted).result.pass).toBe(false);
    const transparent = gray([36, 94, 154, 218]);
    transparent[0][3] = 0;
    expect(check(transparent).result.pass).toBe(false);
  });
});
