// TypeScript shapes of housemetrics' messages (apps/housemetrics/src/messages.rs).
// Field names match the Rust `message!` declarations exactly.

export interface SeriesInfo {
  id: number;
  key: string;
  measurement: string;
  tags: string;
  field: string;
  raw_points: number;
  first_t: number;
  last_t: number;
  last_v: number;
  raw_bytes: number;
  total: number;
  raw_from: number;
}

export interface StoreStats {
  series: number;
  max_series: number;
  blocks_used: number;
  blocks_total: number;
  block_bytes: number;
  raw_points: number;
  bytes_per_point: number;
  rollup_secs: number;
  rollup_slots: number;
  accepted: number;
  rejected: number;
  evicted_blocks: number;
  capacity_bytes: number;
}

export interface SeriesList {
  series: SeriesInfo[];
  store: StoreStats;
}

export interface Point {
  t: number;
  v: number;
}

export interface Bucket {
  t: number;
  min: number;
  max: number;
  avg: number;
  n: number;
}

export interface SeriesData {
  id: number;
  key: string;
  kind: 'raw' | 'buckets';
  from: number;
  to: number;
  step: number;
  points: Point[];
  buckets: Bucket[];
  t0: number;
  dt: number[];
  v: number[];
  min: number[];
  max: number[];
  avg: number[];
  n: number[];
  next: number | null;
}

export interface QueryResult {
  series: SeriesData[];
}

export interface Device {
  id: number;
  name: string;
  created: number;
  prefix: string;
  last_seen: number;
  writes: number;
}

export interface DeviceList {
  devices: Device[];
}

export interface DeviceToken {
  device: Device;
  token: string;
}

export interface Target {
  id: number;
  name: string;
  url: string;
  every_s: number;
  last_ok: number;
  last_error: string;
  last_ms: number;
  samples: number;
}

export interface TargetList {
  targets: Target[];
}

/** One series' samples as parallel arrays, whatever shape the server sent. */
export interface Columns {
  t: number[];
  v: number[];
  min?: number[];
  max?: number[];
}

export function toColumns(s: SeriesData): Columns {
  if (s.kind === 'raw') {
    if (s.points.length) return { t: s.points.map((p) => p.t), v: s.points.map((p) => p.v) };
    return { t: undelta(s.t0, s.dt), v: s.v };
  }
  if (s.buckets.length) {
    return {
      t: s.buckets.map((b) => b.t),
      v: s.buckets.map((b) => b.avg),
      min: s.buckets.map((b) => b.min),
      max: s.buckets.map((b) => b.max),
    };
  }
  return { t: undelta(s.t0, s.dt), v: s.avg, min: s.min, max: s.max };
}

function undelta(t0: number, dt: number[]): number[] {
  const out = new Array<number>(dt.length);
  let t = t0;
  for (let i = 0; i < dt.length; i++) {
    t += dt[i];
    out[i] = t;
  }
  return out;
}
