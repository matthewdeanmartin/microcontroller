import { decodeCbor } from './cbor';
import { decodeMsgPack } from './msgpack';
import { decodeProtobuf } from './protobuf';
import { normalize, renameIntKeys, Schema } from './schema';

export { Schema, normalize, renameIntKeys } from './schema';
export type { SchemaDoc, MessageDoc, FieldDoc } from './schema';
export { decodeCbor } from './cbor';
export { decodeMsgPack } from './msgpack';
export { decodeProtobuf } from './protobuf';
export { WireError } from './bytes';

/** The response formats a miniframework server speaks (`?fmt=`). */
export type Format = 'json' | 'msgpack' | 'cbor' | 'cbor-int' | 'protobuf';

export const FORMATS: readonly Format[] = ['json', 'msgpack', 'cbor', 'cbor-int', 'protobuf'];

export const FORMAT_LABELS: Record<Format, string> = {
  json: 'JSON',
  msgpack: 'MessagePack',
  cbor: 'CBOR',
  'cbor-int': 'CBOR int keys',
  protobuf: 'Protobuf',
};

/** Formats that cannot be read without the schema. */
export function needsSchema(format: Format): boolean {
  return format === 'protobuf' || format === 'cbor-int';
}

const jsonText = new TextDecoder('utf-8');

/**
 * Bytes → value, without filling defaults. `message` names the top-level
 * message type (needed for protobuf and integer-keyed CBOR).
 */
export function decodeRaw(format: Format, bytes: Uint8Array, message?: string, schema?: Schema): unknown {
  switch (format) {
    case 'json':
      return JSON.parse(jsonText.decode(bytes));
    case 'msgpack':
      return decodeMsgPack(bytes);
    case 'cbor':
      return decodeCbor(bytes);
    case 'cbor-int':
      if (!message || !schema) throw new Error('cbor-int needs the message name and schema');
      return renameIntKeys(decodeCbor(bytes), message, schema);
    case 'protobuf':
      if (!message || !schema) throw new Error('protobuf needs the message name and schema');
      return decodeProtobuf(bytes, message, schema);
  }
}

/** Decode and give every format the same shape (missing fields filled). */
export function decode<T>(format: Format, bytes: Uint8Array, message: string, schema: Schema): T {
  return normalize<T>(decodeRaw(format, bytes, message, schema), message, schema);
}
