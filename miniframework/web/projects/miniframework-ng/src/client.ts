// A measuring API client: every call reports what it cost.
import { decodeRaw, Format, normalize, Schema, SchemaDoc } from './wire';

export interface WireStats {
  url: string;
  format: Format;
  gzip: boolean;
  status: number;
  /** Bytes on the wire for the body (compressed if gzipped). */
  bodyBytes: number;
  /** Body bytes before gzip. */
  rawBytes: number;
  /** Server: handler, encode and gzip time in ms (Server-Timing). */
  serverApp: number;
  serverEnc: number;
  serverGz: number;
  /** Request sent → first byte back (ms). */
  ttfb: number;
  /** Fetch start → last byte (ms), including any connection setup. */
  fetchMs: number;
  /** TCP + TLS setup inside this fetch (ms); 0 on a reused connection. */
  connectMs: number;
  /** Bytes → objects (ms), measured in this page. */
  decodeMs: number;
  /** Filling defaults so every format has the same shape (ms). */
  normalizeMs: number;
}

export interface Measured<T> {
  data: T;
  stats: WireStats;
}

export interface CallOptions {
  format?: Format;
  gzip?: boolean;
  /** Top-level message name, for decoding binary formats. */
  message: string;
  /** Bearer token / admin password. */
  auth?: string;
  signal?: AbortSignal;
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

let sequence = 0;

/** Reads `name;dur=1.23` entries from a Server-Timing header. */
export function serverTiming(header: string | null): Record<string, number> {
  const out: Record<string, number> = {};
  for (const part of (header ?? '').split(',')) {
    const [name, ...params] = part.trim().split(';');
    const dur = params.map((p) => p.trim()).find((p) => p.startsWith('dur='));
    if (name && dur) out[name] = Number(dur.slice(4));
  }
  return out;
}

async function resourceEntry(url: string): Promise<PerformanceResourceTiming | undefined> {
  for (let i = 0; i < 5; i++) {
    const entries = performance.getEntriesByName(url, 'resource') as PerformanceResourceTiming[];
    if (entries.length) return entries[entries.length - 1];
    await new Promise((r) => setTimeout(r, 0));
  }
  return undefined;
}

export class MfClient {
  /** '' for the page's own origin, or e.g. `https://housemetrics.local`. */
  base = '';
  private schemaPromise?: Promise<Schema>;

  constructor(base = '') {
    this.base = base.replace(/\/$/, '');
    if (typeof performance !== 'undefined' && 'setResourceTimingBufferSize' in performance) {
      performance.setResourceTimingBufferSize(2000);
    }
  }

  setBase(base: string): void {
    this.base = base.replace(/\/$/, '');
    this.schemaPromise = undefined;
  }

  /** The server's schema (fetched once, as JSON). */
  schema(): Promise<Schema> {
    this.schemaPromise ??= fetch(`${this.base}/api/v1/schema`)
      .then((r) => {
        if (!r.ok) throw new ApiError(r.status, 'schema', 'Could not load the API schema');
        return r.json() as Promise<SchemaDoc>;
      })
      .then((doc) => new Schema(doc))
      .catch((e) => {
        this.schemaPromise = undefined;
        throw e;
      });
    return this.schemaPromise;
  }

  async get<T>(path: string, options: CallOptions): Promise<Measured<T>> {
    return this.call<T>('GET', path, undefined, options);
  }

  /** Sends `body` as JSON; the reply is decoded like `get`. */
  async send<T>(method: string, path: string, body: unknown, options: CallOptions): Promise<Measured<T>> {
    return this.call<T>(method, path, body, options);
  }

  private async call<T>(method: string, path: string, body: unknown, options: CallOptions): Promise<Measured<T>> {
    const format = options.format ?? 'json';
    const schema = await this.schema();
    const url = new URL(`${this.base}${path}`, location.href);
    url.searchParams.set('fmt', format);
    if (options.gzip) url.searchParams.set('gz', '1');
    // A unique URL per call, so its Resource Timing entry is unambiguous.
    url.searchParams.set('_', String(++sequence));
    const headers: Record<string, string> = {};
    if (options.auth) headers['Authorization'] = `Bearer ${options.auth}`;
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    const started = performance.now();
    const response = await fetch(url.href, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: options.signal,
      cache: 'no-store',
    });
    const buffer = new Uint8Array(await response.arrayBuffer());
    const ended = performance.now();
    const entry = await resourceEntry(url.href);
    const timing = serverTiming(response.headers.get('Server-Timing'));
    const contentType = response.headers.get('Content-Type') ?? '';
    // Errors are ErrorBody in the negotiated format (or JSON for some).
    const replyFormat = (response.headers.get('X-Wire-Format') as Format | null) ?? (contentType.startsWith('application/json') ? 'json' : format);
    const isError = !response.ok;
    const message = isError ? 'ErrorBody' : options.message;
    let decoded: unknown = null;
    let decodeMs = 0;
    let normalizeMs = 0;
    if (buffer.length > 0 && response.status !== 204) {
      const t0 = performance.now();
      decoded = decodeRaw(replyFormat, buffer, message, schema);
      const t1 = performance.now();
      decoded = normalize(decoded, message, schema);
      normalizeMs = performance.now() - t1;
      decodeMs = t1 - t0;
    }
    if (isError) {
      const e = (decoded ?? {}) as { error?: string; message?: string };
      throw new ApiError(response.status, e.error ?? 'http_error', e.message ?? response.statusText);
    }
    const connectMs = entry && entry.connectEnd > entry.connectStart ? entry.connectEnd - entry.connectStart : 0;
    const stats: WireStats = {
      url: url.href,
      format: replyFormat,
      gzip: response.headers.get('Content-Encoding') === 'gzip' || !!response.headers.get('X-Raw-Length'),
      status: response.status,
      bodyBytes: entry?.encodedBodySize || Number(response.headers.get('Content-Length')) || buffer.length,
      rawBytes: Number(response.headers.get('X-Raw-Length')) || buffer.length,
      serverApp: timing['app'] ?? 0,
      serverEnc: timing['enc'] ?? 0,
      serverGz: timing['gz'] ?? 0,
      ttfb: entry && entry.responseStart > 0 ? entry.responseStart - entry.requestStart : 0,
      fetchMs: entry && entry.responseEnd > 0 ? entry.responseEnd - entry.startTime : ended - started,
      connectMs,
      decodeMs,
      normalizeMs,
    };
    if (performance.getEntriesByType('resource').length > 1500) performance.clearResourceTimings();
    return { data: decoded as T, stats };
  }
}
