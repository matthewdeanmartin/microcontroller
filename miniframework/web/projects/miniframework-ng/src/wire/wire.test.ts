// The TypeScript decoders read exactly what the Rust encoders write.
// Fixtures: `make fixtures` (cargo run --example fixtures).
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decode, decodeRaw, FORMATS, Schema, SchemaDoc } from './index';
import { decodeCbor } from './cbor';
import { half } from './bytes';

const dir = resolve(dirname(fileURLToPath(import.meta.url)), '../../fixtures');
const schema = new Schema(JSON.parse(readFileSync(resolve(dir, 'schema.json'), 'utf8')) as SchemaDoc);
const bytes = (format: string) => new Uint8Array(readFileSync(resolve(dir, `fixture.${format}.bin`)));

describe('wire formats', () => {
  const expected = decode<Record<string, unknown>>('json', bytes('json'), 'Fixture', schema);

  it('JSON fixture has the values the Rust side wrote', () => {
    expect(expected['id']).toBe(300);
    expect(expected['name']).toBe('attic "loft" é ✓ \u{1F321}');
    expect(expected['delta']).toBe(-123456789);
    expect((expected['points'] as unknown[]).length).toBe(50);
    expect(expected['missing']).toBeNull();
    expect(expected['deltas']).toEqual([-1, 1, -70000, -2147483648, 9007199254740991]);
  });

  for (const format of FORMATS) {
    it(`${format} decodes to the same value as JSON`, () => {
      const value = decode(format, bytes(format), 'Fixture', schema);
      expect(value).toEqual(expected);
    });
  }

  it('protobuf leaves out defaults until normalized', () => {
    const raw = decodeRaw('protobuf', bytes('protobuf'), 'Fixture', schema) as Record<string, unknown>;
    expect(raw['off']).toBeUndefined();
    expect(raw['zero']).toBeUndefined();
    expect(raw['maybe']).toBe(0);
  });

  it('rejects truncated input instead of inventing data', () => {
    for (const format of FORMATS) {
      const b = bytes(format);
      expect(() => decode(format, b.subarray(0, b.length >> 1), 'Fixture', schema)).toThrow();
    }
  });

  it('CBOR half floats and indefinite lengths', () => {
    expect(half(0x3c00)).toBe(1);
    expect(half(0xc000)).toBe(-2);
    expect(half(0x7c00)).toBe(Infinity);
    // {_ "a": [_ 1, 2]} with indefinite map and array.
    expect(decodeCbor(new Uint8Array([0xbf, 0x61, 0x61, 0x9f, 1, 2, 0xff, 0xff]))).toEqual({ a: [1, 2] });
  });
});
