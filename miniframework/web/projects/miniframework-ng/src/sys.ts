// The built-in system information every miniframework server publishes.

export interface Heap {
  internal_free: number;
  internal_min: number;
  internal_largest: number;
  internal_total: number;
  psram_free: number;
  psram_min: number;
  psram_largest: number;
  psram_total: number;
}

export interface Wifi {
  ssid: string;
  rssi: number;
  channel: number;
  ip: string;
  mac: string;
}

export interface Net {
  requests: number;
  errors: number;
  kib_out: number;
  tls_handshakes: number;
  tls_failures: number;
  handshake_ms_last: number;
  handshake_ms_avg: number;
  tls_open: number;
  http_open: number;
  rejected: number;
  tls_slots: number;
  http_slots: number;
}

export interface SysInfo {
  app: string;
  version: string;
  build: string;
  host: string;
  platform: string;
  chip: string;
  cores: number;
  cpu_mhz: number;
  uptime_ms: number;
  wall_ms: number;
  reset_reason: string;
  heap: Heap;
  wifi: Wifi;
  net: Net;
  flash_bytes: number;
  temp_c: number | null;
  sdk: string;
  /** What the status light shows (booting, connecting, healthy, ...); '' without one. */
  status: string;
}

/** `1.5 KiB`, `320 B`. */
export function formatBytes(n: number): string {
  if (!Number.isFinite(n)) return '–';
  if (n < 1024) return `${Math.round(n)} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KiB`;
  return `${(n / 1024 / 1024).toFixed(1)} MiB`;
}

/** `3 d 4 h`, `12 min`, `40 s`. */
export function formatDuration(ms: number): string {
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  if (h < 48) return `${h} h ${m % 60} min`;
  return `${Math.floor(h / 24)} d ${h % 24} h`;
}

/** GET /api/v1/log: the board's own log (see logbuf.rs). */
export interface LogLine {
  seq: number;
  /** ms since boot */
  t: number;
  /** 1 error, 2 warn, 3 info, 4 debug, 5 verbose */
  level: number;
  text: string;
}

export interface LogPage {
  lines: LogLine[];
  next: number;
  dropped: number;
  previous: string;
  previous_reason: string;
  capacity: number;
}
