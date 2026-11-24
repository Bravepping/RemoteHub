export type PublicInputState =
  'view_only' | 'pending' | 'allowed' | 'denied' | 'closed';

export type PublicControlEvent =
  | { kind: 'mouse_move'; x_norm: number; y_norm: number }
  | { kind: 'mouse_button'; button: number; down: boolean }
  | { kind: 'key'; key: string; down: boolean };

const physicalKey =
  /^(Key[A-Z]|Digit[0-9]|Space|Enter|Backspace|Tab|Delete|Arrow(Left|Right|Up|Down)|Home|End|Shift(Left|Right)|Control(Left|Right)|Period|Comma|Slash|Semicolon|Quote|Minus|Equal)$/;
export const validPublicKey = (key: string) => physicalKey.test(key);

export function publicControlEvent(
  value: PublicControlEvent,
): string | undefined {
  switch (value.kind) {
    case 'mouse_move': {
      const { x_norm, y_norm } = value;
      if (
        !Number.isFinite(x_norm) ||
        x_norm < 0 ||
        x_norm > 1 ||
        !Number.isFinite(y_norm) ||
        y_norm < 0 ||
        y_norm > 1
      )
        return undefined;
      return JSON.stringify({ kind: 'mouse_move', x_norm, y_norm });
    }
    case 'mouse_button': {
      const { button, down } = value;
      if (
        !Number.isInteger(button) ||
        button < 0 ||
        button > 2 ||
        typeof down !== 'boolean'
      )
        return undefined;
      return JSON.stringify({ kind: 'mouse_button', button, down });
    }
    case 'key': {
      const { key, down } = value;
      if (!validPublicKey(key) || typeof down !== 'boolean') return undefined;
      return JSON.stringify({ kind: 'key', key, down });
    }
  }
}

export function normalizedPublicPoint(
  box: Pick<DOMRect, 'left' | 'top' | 'width' | 'height'>,
  frameWidth: number,
  frameHeight: number,
  x: number,
  y: number,
): { x_norm: number; y_norm: number } | undefined {
  if (box.width <= 0 || box.height <= 0 || frameWidth <= 0 || frameHeight <= 0)
    return undefined;
  const scale = Math.min(box.width / frameWidth, box.height / frameHeight);
  const w = frameWidth * scale;
  const h = frameHeight * scale;
  const x_norm = (x - box.left - (box.width - w) / 2) / w;
  const y_norm = (y - box.top - (box.height - h) / 2) / h;
  if (
    !Number.isFinite(x_norm) ||
    x_norm < 0 ||
    x_norm > 1 ||
    !Number.isFinite(y_norm) ||
    y_norm < 0 ||
    y_norm > 1
  )
    return undefined;
  return { x_norm, y_norm };
}
