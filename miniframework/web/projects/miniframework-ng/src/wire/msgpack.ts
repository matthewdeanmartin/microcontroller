// MessagePack decoder: maps become plain objects, arrays arrays.
import { Cursor, WireError } from './bytes';

const MAX_DEPTH = 64;

export function decodeMsgPack(bytes: Uint8Array): unknown {
  const c = new Cursor(bytes);
  const value = read(c, 0);
  if (c.pos !== bytes.length) throw new WireError('trailing MessagePack data');
  return value;
}

function read(c: Cursor, depth: number): unknown {
  if (depth > MAX_DEPTH) throw new WireError('nesting too deep');
  const b = c.u8();
  if (b <= 0x7f) return b;
  if (b >= 0xe0) return b - 0x100;
  if ((b & 0xf0) === 0x80) return map(c, b & 0x0f, depth);
  if ((b & 0xf0) === 0x90) return array(c, b & 0x0f, depth);
  if ((b & 0xe0) === 0xa0) return c.text(b & 0x1f);
  switch (b) {
    case 0xc0:
      return null;
    case 0xc2:
      return false;
    case 0xc3:
      return true;
    case 0xc4:
    case 0xc5:
    case 0xc6: {
      const n = c.uintBE(1 << (b - 0xc4));
      c.need(n);
      const out = c.bytes.slice(c.pos, c.pos + n);
      c.pos += n;
      return out;
    }
    case 0xc7:
    case 0xc8:
    case 0xc9: {
      const n = c.uintBE(1 << (b - 0xc7));
      c.need(n + 1);
      c.pos += n + 1;
      return null;
    }
    case 0xca: {
      c.need(4);
      const v = c.view.getFloat32(c.pos);
      c.pos += 4;
      return v;
    }
    case 0xcb: {
      c.need(8);
      const v = c.view.getFloat64(c.pos);
      c.pos += 8;
      return v;
    }
    case 0xcc:
      return c.uintBE(1);
    case 0xcd:
      return c.uintBE(2);
    case 0xce:
      return c.uintBE(4);
    case 0xcf:
      return c.uintBE(8);
    case 0xd0: {
      c.need(1);
      return c.view.getInt8(c.pos++);
    }
    case 0xd1: {
      c.need(2);
      const v = c.view.getInt16(c.pos);
      c.pos += 2;
      return v;
    }
    case 0xd2: {
      c.need(4);
      const v = c.view.getInt32(c.pos);
      c.pos += 4;
      return v;
    }
    case 0xd3: {
      c.need(8);
      const v = c.view.getInt32(c.pos) * 4294967296 + c.view.getUint32(c.pos + 4);
      c.pos += 8;
      return v;
    }
    case 0xd4:
    case 0xd5:
    case 0xd6:
    case 0xd7:
    case 0xd8: {
      const n = 1 + (1 << (b - 0xd4));
      c.need(n);
      c.pos += n;
      return null;
    }
    case 0xd9:
      return c.text(c.uintBE(1));
    case 0xda:
      return c.text(c.uintBE(2));
    case 0xdb:
      return c.text(c.uintBE(4));
    case 0xdc:
      return array(c, c.uintBE(2), depth);
    case 0xdd:
      return array(c, c.uintBE(4), depth);
    case 0xde:
      return map(c, c.uintBE(2), depth);
    case 0xdf:
      return map(c, c.uintBE(4), depth);
  }
  throw new WireError(`invalid MessagePack byte 0x${b.toString(16)}`);
}

function array(c: Cursor, n: number, depth: number): unknown[] {
  // Each element takes at least one byte: refuse impossible lengths early.
  c.need(n);
  const out = new Array<unknown>(n);
  for (let i = 0; i < n; i++) out[i] = read(c, depth + 1);
  return out;
}

function map(c: Cursor, n: number, depth: number): Record<string, unknown> {
  c.need(n * 2);
  const out: Record<string, unknown> = Object.create(null);
  for (let i = 0; i < n; i++) {
    const key = read(c, depth + 1);
    out[String(key)] = read(c, depth + 1);
  }
  return out;
}
