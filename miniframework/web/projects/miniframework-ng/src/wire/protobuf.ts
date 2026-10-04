// Schema-driven protobuf (proto3) decoder. Integers up to 2^53 are exact
// (timestamps in ms are about 2^41); larger ones lose precision, as JSON's
// numbers would.
import { utf8, WireError } from './bytes';
import { IndexedMessage, Schema } from './schema';

const VARINT = 0;
const FIXED64 = 1;
const LEN = 2;
const FIXED32 = 5;
const MAX_DEPTH = 64;

class Reader {
  pos = 0;
  readonly view: DataView;
  constructor(readonly bytes: Uint8Array) {
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  varint(): number {
    let result = 0;
    let scale = 1;
    for (let i = 0; i < 10; i++) {
      if (this.pos >= this.bytes.length) throw new WireError('input ended early');
      const b = this.bytes[this.pos++];
      result += (b & 0x7f) * scale;
      if (b < 0x80) return result;
      scale *= 128;
    }
    throw new WireError('varint too long');
  }

  skip(wt: number): void {
    switch (wt) {
      case VARINT:
        this.varint();
        return;
      case FIXED64:
        this.pos += 8;
        break;
      case FIXED32:
        this.pos += 4;
        break;
      case LEN:
        this.pos += this.varint();
        break;
      default:
        throw new WireError(`unsupported wire type ${wt}`);
    }
    if (this.pos > this.bytes.length) throw new WireError('input ended early');
  }
}

const PACKABLE = new Set(['uint32', 'uint64', 'sint32', 'sint64', 'double', 'float', 'bool']);

function scalar(r: Reader, kind: string, wt: number): unknown {
  switch (kind) {
    case 'uint32':
    case 'uint64':
      return r.varint();
    case 'sint32':
    case 'sint64': {
      const v = r.varint();
      return v % 2 === 0 ? v / 2 : -(v + 1) / 2;
    }
    case 'bool':
      return r.varint() !== 0;
    case 'double':
    case 'float': {
      const wide = wt === FIXED64 || (wt === -1 && kind === 'double');
      const n = wide ? 8 : 4;
      if (r.pos + n > r.bytes.length) throw new WireError('input ended early');
      const v = wide ? r.view.getFloat64(r.pos, true) : r.view.getFloat32(r.pos, true);
      r.pos += n;
      return v;
    }
    case 'string': {
      const n = r.varint();
      if (r.pos + n > r.bytes.length) throw new WireError('input ended early');
      const s = utf8(r.bytes, r.pos, r.pos + n);
      r.pos += n;
      return s;
    }
  }
  throw new WireError(`unknown field kind ${kind}`);
}

function message(r: Reader, end: number, m: IndexedMessage, schema: Schema, depth: number): Record<string, unknown> {
  if (depth > MAX_DEPTH) throw new WireError('nesting too deep');
  const out: Record<string, unknown> = {};
  while (r.pos < end) {
    const key = r.varint();
    const tag = Math.floor(key / 8);
    const wt = key % 8;
    const field = m.byTag.get(tag);
    if (!field) {
      r.skip(wt);
      continue;
    }
    let value: unknown;
    if (field.kind === 'message') {
      const len = r.varint();
      const stop = r.pos + len;
      if (stop > end) throw new WireError('submessage overruns its parent');
      value = message(r, stop, schema.get(field.message), schema, depth + 1);
      r.pos = stop;
    } else if (field.repeated && wt === LEN && PACKABLE.has(field.kind)) {
      const len = r.varint();
      const stop = r.pos + len;
      if (stop > end) throw new WireError('packed run overruns its message');
      const list = (out[field.name] as unknown[] | undefined) ?? [];
      while (r.pos < stop) list.push(scalar(r, field.kind, -1));
      out[field.name] = list;
      continue;
    } else {
      value = scalar(r, field.kind, wt);
    }
    if (field.repeated) {
      const list = (out[field.name] as unknown[] | undefined) ?? [];
      list.push(value);
      out[field.name] = list;
    } else {
      out[field.name] = value;
    }
  }
  if (r.pos !== end) throw new WireError('message overran its length');
  return out;
}

export function decodeProtobuf(bytes: Uint8Array, messageName: string, schema: Schema): Record<string, unknown> {
  const r = new Reader(bytes);
  return message(r, bytes.length, schema.get(messageName), schema, 0);
}
