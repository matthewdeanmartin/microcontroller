// The server's message schema (GET /api/v1/schema), indexed for decoding.

export interface FieldDoc {
  tag: number;
  name: string;
  /** proto3 scalar type name, or "message". */
  kind: string;
  message: string;
  repeated: boolean;
  optional: boolean;
  doc: string;
}

export interface MessageDoc {
  name: string;
  doc: string;
  fields: FieldDoc[];
}

export interface SchemaDoc {
  messages: MessageDoc[];
}

export interface IndexedMessage {
  doc: MessageDoc;
  byTag: Map<number, FieldDoc>;
  byName: Map<string, FieldDoc>;
}

export class Schema {
  readonly messages = new Map<string, IndexedMessage>();

  constructor(doc: SchemaDoc) {
    for (const m of doc.messages) {
      this.messages.set(m.name, {
        doc: m,
        byTag: new Map(m.fields.map((f) => [f.tag, f])),
        byName: new Map(m.fields.map((f) => [f.name, f])),
      });
    }
  }

  get(name: string): IndexedMessage {
    const m = this.messages.get(name);
    if (!m) throw new Error(`The server's schema has no message ${name}`);
    return m;
  }
}

/** Integer-keyed CBOR → field names, recursively. */
export function renameIntKeys(value: unknown, message: string, schema: Schema): unknown {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return value;
  const m = schema.get(message);
  const out: Record<string, unknown> = {};
  for (const [key, v] of Object.entries(value as Record<string, unknown>)) {
    const field = m.byTag.get(Number(key)) ?? m.byName.get(key);
    if (!field) continue;
    if (field.kind === 'message' && v !== null) {
      out[field.name] = Array.isArray(v)
        ? v.map((item) => renameIntKeys(item, field.message, schema))
        : renameIntKeys(v, field.message, schema);
    } else {
      out[field.name] = v;
    }
  }
  return out;
}

function defaultOf(field: FieldDoc, schema: Schema): unknown {
  if (field.repeated) return [];
  if (field.optional) return null;
  switch (field.kind) {
    case 'string':
      return '';
    case 'bool':
      return false;
    case 'message':
      return normalize({}, field.message, schema);
    default:
      return 0;
  }
}

/**
 * Fills fields a format left out (protobuf omits defaults; streamed views
 * write only the fields they use) so every format yields the same shape.
 */
export function normalize<T = unknown>(value: unknown, message: string, schema: Schema): T {
  const m = schema.get(message);
  const obj = (value ?? {}) as Record<string, unknown>;
  for (const field of m.doc.fields) {
    const v = obj[field.name];
    if (v === undefined) {
      obj[field.name] = defaultOf(field, schema);
    } else if (field.kind === 'message' && v !== null) {
      if (Array.isArray(v)) {
        for (const item of v) normalize(item, field.message, schema);
      } else {
        normalize(v, field.message, schema);
      }
    }
  }
  return obj as T;
}
