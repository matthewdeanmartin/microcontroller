// CBOR (RFC 8949) decoder. Maps become plain objects; integer keys become
// their decimal string (use the schema to rename them, see schema.ts).
import { Cursor, half, WireError } from './bytes';

const MAX_DEPTH = 64;
const BREAK = Symbol('break');

export function decodeCbor(bytes: Uint8Array): unknown {
  const c = new Cursor(bytes);
  const v = read(c, 0);
  if (v === BREAK) throw new WireError('unexpected CBOR break');
  return v;
}

/** The argument of a head; -1 for indefinite length. */
function argument(c: Cursor, info: number): number {
  if (info < 24) return info;
  switch (info) {
    case 24:
      return c.uintBE(1);
    case 25:
      return c.uintBE(2);
    case 26:
      return c.uintBE(4);
    case 27:
      return c.uintBE(8);
    case 31:
      return -1;
  }
  throw new WireError('reserved CBOR head');
}

function read(c: Cursor, depth: number): unknown {
  if (depth > MAX_DEPTH) throw new WireError('nesting too deep');
  const b = c.u8();
  const major = b >> 5;
  const info = b & 31;
  if (major === 7) {
    switch (info) {
      case 20:
        return false;
      case 21:
        return true;
      case 22:
      case 23:
        return null;
      case 25:
        return half(c.uintBE(2));
      case 26: {
        c.need(4);
        const v = c.view.getFloat32(c.pos);
        c.pos += 4;
        return v;
      }
      case 27: {
        c.need(8);
        const v = c.view.getFloat64(c.pos);
        c.pos += 8;
        return v;
      }
      case 31:
        return BREAK;
      default:
        if (info === 24) c.u8();
        return null;
    }
  }
  const n = argument(c, info);
  switch (major) {
    case 0:
      return n;
    case 1:
      return -1 - n;
    case 2:
    case 3: {
      if (n < 0) {
        const parts: unknown[] = [];
        for (;;) {
          const part = read(c, depth + 1);
          if (part === BREAK) break;
          parts.push(part);
        }
        return major === 3 ? parts.join('') : null;
      }
      if (major === 3) return c.text(n);
      c.need(n);
      const out = c.bytes.slice(c.pos, c.pos + n);
      c.pos += n;
      return out;
    }
    case 4: {
      const out: unknown[] = [];
      if (n < 0) {
        for (;;) {
          const v = read(c, depth + 1);
          if (v === BREAK) return out;
          out.push(v);
        }
      }
      c.need(n);
      for (let i = 0; i < n; i++) out.push(read(c, depth + 1));
      return out;
    }
    case 5: {
      const out: Record<string, unknown> = {};
      if (n < 0) {
        for (;;) {
          const k = read(c, depth + 1);
          if (k === BREAK) return out;
          out[String(k)] = read(c, depth + 1);
        }
      }
      c.need(n * 2);
      for (let i = 0; i < n; i++) {
        const k = read(c, depth + 1);
        out[String(k)] = read(c, depth + 1);
      }
      return out;
    }
    default:
      // Major 6: a semantic tag; return the tagged value as is.
      return read(c, depth + 1);
  }
}
