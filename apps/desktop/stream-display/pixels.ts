const centers = [
  [8, 8],
  [24, 8],
  [8, 24],
  [24, 24],
] as const;

export function checkQuadrants(
  context: Pick<CanvasRenderingContext2D, 'getImageData'>,
) {
  const samples = centers.map(([x, y]) => {
    const rgba = context.getImageData(x - 1, y - 1, 3, 3).data;
    const totals = [0, 0, 0, 0];
    let gray = true;
    for (let i = 0; i < rgba.length; i += 4) {
      const [r, g, b, a] = [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]];
      if (a !== 255 || Math.max(r, g, b) - Math.min(r, g, b) > 14) gray = false;
      for (let channel = 0; channel < 4; channel++)
        totals[channel] += rgba[i + channel];
    }
    const mean = totals.map((sum) => Math.round(sum / 9));
    return { mean, gray, shade: (mean[0] + mean[1] + mean[2]) / 3 };
  });
  const pass = samples.every(
    ({ gray, shade }, i) =>
      gray && (i === 0 || shade - samples[i - 1].shade >= 12),
  );
  return { pass, rgba: samples.map(({ mean }) => mean.join('/')).join(' · ') };
}
