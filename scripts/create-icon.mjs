import { mkdir, writeFile } from 'node:fs/promises';

// A small code-native application glyph; no external artwork or fonts.
const size = 32;
const dib = Buffer.alloc(40 + size * size * 4 + size * 4);
dib.writeUInt32LE(40, 0);
dib.writeInt32LE(size, 4);
dib.writeInt32LE(size * 2, 8);
dib.writeUInt16LE(1, 12);
dib.writeUInt16LE(32, 14);
for (let y = 0; y < size; y++) {
  for (let x = 0; x < size; x++) {
    const border =
      (x >= 6 && x <= 25 && (y === 7 || y === 21)) ||
      (y >= 7 && y <= 21 && (x === 6 || x === 25)) ||
      (y === 25 && x >= 11 && x <= 20) ||
      (x >= 15 && x <= 17 && y >= 22 && y <= 25);
    const offset = 40 + ((size - 1 - y) * size + x) * 4;
    dib.set(border ? [255, 255, 255, 255] : [235, 99, 37, 255], offset);
  }
}
const ico = Buffer.alloc(22);
ico.writeUInt16LE(1, 2);
ico.writeUInt16LE(1, 4);
ico[6] = size;
ico[7] = size;
ico.writeUInt16LE(1, 10);
ico.writeUInt16LE(32, 12);
ico.writeUInt32LE(dib.length, 14);
ico.writeUInt32LE(22, 18);
await mkdir('apps/desktop/src-tauri/icons', { recursive: true });
await writeFile(
  'apps/desktop/src-tauri/icons/icon.ico',
  Buffer.concat([ico, dib]),
);
