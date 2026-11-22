import { expect, it } from 'vitest';
import { pageFromHash, pageLink, pages } from './navigation';
it('resolves only the five local route names', () => {
  for (const p of pages) expect(pageFromHash(pageLink(p.id))).toBe(p.id);
  for (const hash of [
    '',
    '#/missing',
    '#/connect/extra',
    '#/connect?pair=fixture',
    '#javascript:fixture',
    '#/settings%00',
  ])
    expect(pageFromHash(hash)).toBe('home');
});
