// Shared helpers for the binary decoders.

const textDecoder = new TextDecoder('utf-8', { fatal: true });

/**
 * UTF-8 to string. Short ASCII strings (most keys and tag values) are
 * decoded by hand: TextDecoder's per-call overhead dominates for them.
 */
export function utf8(bytes: Uint8Array, start: number, end: number): string {
  if (end - start <= 24) {
    let out = '';
    for (let i = start; i < end; i++) {
      const c = bytes[i];
      if (c >= 0x80) return textDecoder.decode(bytes.subarray(start, end));
      out += String.fromCharCode(c);
    }
    return out;
  }
  return textDecoder.decode(bytes.subarray(start, end));
}

export class WireError extends Error {}

/** A cursor over a byte array with a DataView for fixed-width numbers. */
export class Cursor {
  pos = 0;
  readonly view: DataView;

  constructor(readonly bytes: Uint8Array) {
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  need(n: number): void {
    if (!Number.isSafeInteger(n) || n < 0 || n > this.bytes.length - this.pos) {
      throw new WireError('invalid or truncated length');
    }
  }

  u8(): number {
    this.need(1);
    return this.bytes[this.pos++];
  }

  /** Big-endian unsigned integer of 1, 2, 4 or 8 bytes, as a Number. */
  uintBE(n: number): number {
    if (![1, 2, 4, 8].includes(n)) throw new WireError('invalid integer width');
    this.need(n);
    const v = this.view;
    const p = this.pos;
    this.pos += n;
    switch (n) {
      case 1:
        return v.getUint8(p);
      case 2:
        return v.getUint16(p);
      case 4:
        return v.getUint32(p);
      default:
        return v.getUint32(p) * 4294967296 + v.getUint32(p + 4);
    }
  }

  text(n: number): string {
    this.need(n);
    const s = utf8(this.bytes, this.pos, this.pos + n);
    this.pos += n;
    return s;
  }
}

/** IEEE 754 half precision. */
export function half(bits: number): number {
  const sign = bits & 0x8000 ? -1 : 1;
  const exp = (bits >> 10) & 0x1f;
  const mant = bits & 0x3ff;
  if (exp === 0) return sign * mant * 2 ** -24;
  if (exp === 31) return mant ? NaN : sign * Infinity;
  return sign * (1 + mant / 1024) * 2 ** (exp - 15);
}
