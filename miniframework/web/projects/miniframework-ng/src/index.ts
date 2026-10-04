// miniframework-ng: the browser half of miniframework.
//
// - wire/: decoders for every format a miniframework server speaks, driven
//   by the server's own schema (GET /api/v1/schema).
// - client.ts: a fetch-based client that reports bytes, server encode time,
//   TTFB, connection setup and decode time for every call.
// - sys.ts: types for the built-in /api/v1/sys system information.
export * from './wire';
export * from './client';
export * from './sys';
export * from './stats';
