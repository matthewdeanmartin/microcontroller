import { ChangeDetectionStrategy, Component, WritableSignal, computed, inject, signal } from '@angular/core';
import { DecimalPipe } from '@angular/common';
import { FORMATS, FORMAT_LABELS, Format, SysInfo, WireStats, formatBytes, median } from 'miniframework-ng';
import { Api } from '../api';
import { SeriesList } from '../messages';

interface Payload {
  id: string;
  label: string;
  message: string;
  /** Path for this run; series payloads need a series id. */
  path: (seriesId: number) => string;
  needsSeries?: boolean;
}

const PAYLOADS: Payload[] = [
  { id: 'sys', label: 'System info (small object)', message: 'SysInfo', path: () => '/api/v1/sys' },
  { id: 'rows10', label: '10 rows', message: 'Rows', path: () => '/api/v1/bench/rows?n=10' },
  { id: 'rows100', label: '100 rows', message: 'Rows', path: () => '/api/v1/bench/rows?n=100' },
  { id: 'rows1000', label: '1000 rows', message: 'Rows', path: () => '/api/v1/bench/rows?n=1000' },
  { id: 'cols1000', label: '1000 rows as columns', message: 'RowColumns', path: () => '/api/v1/bench/rows?n=1000&shape=columns' },
  { id: 'raw', label: 'Series: raw points as rows', message: 'QueryResult', path: (id) => `/api/v1/query?ids=${id}&from=0&raw=1&limit=2000`, needsSeries: true },
  { id: 'rawcols', label: 'Series: raw points as columns', message: 'QueryResult', path: (id) => `/api/v1/query?ids=${id}&from=0&raw=1&limit=2000&shape=columns`, needsSeries: true },
];

interface Sample {
  payload: string;
  format: Format;
  gzip: boolean;
  stats: WireStats;
}

export interface ResultRow {
  payload: string;
  format: Format;
  gzip: boolean;
  n: number;
  bodyBytes: number;
  rawBytes: number;
  server: number;
  serverEnc: number;
  serverGz: number;
  ttfb: number;
  fetch: number;
  decode: number;
  total: number;
  connects: number;
}

@Component({
  selector: 'hm-lab',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [DecimalPipe],
  template: `
    <section class="card">
      <h2>Format lab</h2>
      <p class="muted">
        Fetches the same data in every format, interleaved, and reports medians. It answers one question:
        <b>does the serialization format matter next to everything else a request costs?</b>
        Requests run one at a time over the browser's kept-alive connection, so a TLS handshake only shows up when the browser opens a new one.
      </p>
      <div class="lab-controls">
        <fieldset>
          <legend>Payloads</legend>
          @for (p of payloads; track p.id) {
            <label class="check"><input type="checkbox" [checked]="chosen().has(p.id)" (change)="flip(chosen, p.id)" /> {{ p.label }}</label>
          }
        </fieldset>
        <fieldset>
          <legend>Formats</legend>
          @for (f of formats; track f) {
            <label class="check"><input type="checkbox" [checked]="formatsOn().has(f)" (change)="flip(formatsOn, f)" /> {{ labels[f] }}</label>
          }
        </fieldset>
        <fieldset>
          <legend>Run</legend>
          <label class="check"><input type="radio" name="gz" [checked]="gz() === 'off'" (change)="gz.set('off')" /> no gzip</label>
          <label class="check"><input type="radio" name="gz" [checked]="gz() === 'on'" (change)="gz.set('on')" /> gzip only</label>
          <label class="check"><input type="radio" name="gz" [checked]="gz() === 'both'" (change)="gz.set('both')" /> both</label>
          <label>Repeats <input type="number" min="1" max="50" [value]="repeats()" (change)="repeats.set(+$any($event.target).value || 5)" /></label>
          <div class="row">
            @if (running()) {
              <button (click)="stop()">Stop</button>
            } @else {
              <button class="primary" (click)="run()">Run</button>
            }
            <button class="ghost" [disabled]="!rows().length" (click)="download()">Download results</button>
          </div>
        </fieldset>
      </div>
      @if (running()) {
        <div class="meter"><span [style.width.%]="progress()"></span></div>
        <p class="muted small">{{ status() }}</p>
      }
      @if (error()) {
        <p class="error">{{ error() }}</p>
      }
    </section>

    @if (verdicts().length) {
      <section class="card">
        <h2>Verdict</h2>
        @if (handshake()) {
          <p>For scale: one full TLS handshake on this server averages <b>{{ handshake() }} ms</b>{{ handshakeNote() }}.</p>
        }
        <ul class="verdicts">
          @for (v of verdicts(); track v.payload) {
            <li>
              <b>{{ v.label }}:</b> fastest {{ v.best }} at {{ v.bestMs | number: '1.0-1' }} ms, slowest {{ v.worst }} at {{ v.worstMs | number: '1.0-1' }} ms
              (spread <b>{{ v.worstMs - v.bestMs | number: '1.0-1' }} ms</b>). Smallest {{ v.smallest }} at {{ bytes(v.minBytes) }}, largest {{ bytes(v.maxBytes) }}.
              @if (handshake()) {
                <span class="muted">The whole format spread is {{ (100 * (v.worstMs - v.bestMs)) / handshake() | number: '1.0-0' }}% of one handshake.</span>
              }
            </li>
          }
        </ul>
      </section>
    }

    @if (rows().length) {
      <section class="card wide">
        <h2>Results <span class="muted small">(medians of {{ repeats() }} runs; ms)</span></h2>
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th>Payload</th><th>Format</th><th class="num">Wire</th><th class="num">Raw</th>
                <th class="num" title="Server encode (+ gzip)">Encode</th><th class="num">TTFB</th><th class="num">Fetch</th>
                <th class="num">Decode</th><th class="num">Total</th><th>Where the time goes</th>
              </tr>
            </thead>
            <tbody>
              @for (r of rows(); track $index) {
                <tr [class.group]="r.format === firstFormat() && !r.gzip">
                  <td>{{ payloadLabel(r.payload) }}</td>
                  <td>{{ labels[r.format] }}{{ r.gzip ? ' + gz' : '' }}</td>
                  <td class="num">{{ bytes(r.bodyBytes) }}</td>
                  <td class="num muted">{{ bytes(r.rawBytes) }}</td>
                  <td class="num">{{ r.serverEnc + r.serverGz | number: '1.2-2' }}</td>
                  <td class="num">{{ r.ttfb | number: '1.1-1' }}</td>
                  <td class="num">{{ r.fetch | number: '1.1-1' }}</td>
                  <td class="num">{{ r.decode | number: '1.2-2' }}</td>
                  <td class="num"><b>{{ r.total | number: '1.1-1' }}</b></td>
                  <td class="bar-cell">
                    <div class="bar" [title]="'server ' + (r.server | number: '1.1-2') + ' ms, network ' + (r.fetch - r.server | number: '1.1-1') + ' ms, decode ' + (r.decode | number: '1.2-2') + ' ms'">
                      <span class="seg server" [style.width.%]="share(r, r.server)"></span>
                      <span class="seg net" [style.width.%]="share(r, r.fetch - r.server)"></span>
                      <span class="seg decode" [style.width.%]="share(r, r.decode)"></span>
                    </div>
                  </td>
                </tr>
              }
            </tbody>
          </table>
        </div>
        <p class="legend small"><span class="seg server"></span> server (handler + encode + gzip) <span class="seg net"></span> network and waiting <span class="seg decode"></span> decode in this browser</p>
      </section>
    }
  `,
})
export class Lab {
  private readonly api = inject(Api);
  readonly payloads = PAYLOADS;
  readonly formats = FORMATS;
  readonly labels = FORMAT_LABELS;
  readonly bytes = formatBytes;
  readonly chosen = signal(new Set(['sys', 'rows100', 'rows1000', 'cols1000', 'raw', 'rawcols']));
  readonly formatsOn = signal(new Set<string>(FORMATS));
  readonly gz = signal<'off' | 'on' | 'both'>('both');
  readonly repeats = signal(5);
  readonly running = signal(false);
  readonly progress = signal(0);
  readonly status = signal('');
  readonly error = signal('');
  readonly samples = signal<Sample[]>([]);
  readonly sys = signal<SysInfo | null>(null);
  private abort?: AbortController;

  readonly handshake = computed(() => this.sys()?.net.handshake_ms_avg ?? 0);
  readonly handshakeNote = computed(() => {
    const s = this.sys();
    return s ? ` (${s.net.tls_handshakes} measured on ${s.chip})` : '';
  });

  readonly rows = computed<ResultRow[]>(() => {
    const groups = new Map<string, Sample[]>();
    for (const s of this.samples()) {
      const key = `${s.payload}|${s.format}|${s.gzip}`;
      const g = groups.get(key) ?? [];
      g.push(s);
      groups.set(key, g);
    }
    const order = (p: string) => PAYLOADS.findIndex((x) => x.id === p);
    return [...groups.values()]
      .map((g) => {
        const m = (f: (w: WireStats) => number) => median(g.map((s) => f(s.stats)));
        const fetch = m((w) => w.fetchMs);
        const decode = m((w) => w.decodeMs);
        return {
          payload: g[0].payload,
          format: g[0].format,
          gzip: g[0].gzip,
          n: g.length,
          bodyBytes: m((w) => w.bodyBytes),
          rawBytes: m((w) => w.rawBytes),
          server: m((w) => w.serverApp + w.serverEnc + w.serverGz),
          serverEnc: m((w) => w.serverEnc),
          serverGz: m((w) => w.serverGz),
          ttfb: m((w) => w.ttfb),
          fetch,
          decode,
          total: median(g.map((s) => s.stats.fetchMs + s.stats.decodeMs)),
          connects: g.filter((s) => s.stats.connectMs > 0).length,
        } satisfies ResultRow;
      })
      .sort(
        (a, b) =>
          order(a.payload) - order(b.payload) ||
          FORMATS.indexOf(a.format) - FORMATS.indexOf(b.format) ||
          Number(a.gzip) - Number(b.gzip),
      );
  });

  readonly firstFormat = computed(() => FORMATS.find((f) => this.formatsOn().has(f)));

  readonly verdicts = computed(() => {
    const by = new Map<string, ResultRow[]>();
    for (const r of this.rows()) by.set(r.payload, [...(by.get(r.payload) ?? []), r]);
    return [...by.entries()].map(([payload, rs]) => {
      const name = (r: ResultRow) => `${FORMAT_LABELS[r.format]}${r.gzip ? ' + gzip' : ''}`;
      const best = rs.reduce((a, b) => (b.total < a.total ? b : a));
      const worst = rs.reduce((a, b) => (b.total > a.total ? b : a));
      const small = rs.reduce((a, b) => (b.bodyBytes < a.bodyBytes ? b : a));
      return {
        payload,
        label: this.payloadLabel(payload),
        best: name(best),
        bestMs: best.total,
        worst: name(worst),
        worstMs: worst.total,
        smallest: name(small),
        minBytes: small.bodyBytes,
        maxBytes: Math.max(...rs.map((r) => r.bodyBytes)),
      };
    });
  });

  flip(set: WritableSignal<Set<string>>, id: string): void {
    const next = new Set(set());
    if (next.has(id)) next.delete(id);
    else next.add(id);
    set.set(next);
  }

  payloadLabel(id: string): string {
    return PAYLOADS.find((p) => p.id === id)?.label ?? id;
  }

  share(r: ResultRow, part: number): number {
    const groupMax = Math.max(...this.rows().filter((x) => x.payload === r.payload).map((x) => x.total));
    return groupMax > 0 ? Math.max(0, (100 * part) / groupMax) : 0;
  }

  stop(): void {
    this.abort?.abort();
  }

  async run(): Promise<void> {
    this.error.set('');
    this.samples.set([]);
    this.running.set(true);
    this.abort = new AbortController();
    const signal = this.abort.signal;
    try {
      let seriesId = -1;
      const payloads = PAYLOADS.filter((p) => this.chosen().has(p.id));
      if (payloads.some((p) => p.needsSeries)) {
        const list = (await this.api.measure<SeriesList>('/api/v1/series', { message: 'SeriesList' })).data;
        const biggest = [...list.series].sort((a, b) => b.raw_points - a.raw_points)[0];
        seriesId = biggest?.id ?? -1;
      }
      const runnable = payloads.filter((p) => !p.needsSeries || seriesId >= 0);
      if (runnable.length < payloads.length) {
        this.error.set('No series with data yet, so the series payloads were skipped.');
      }
      const formats = FORMATS.filter((f) => this.formatsOn().has(f));
      const gzips = this.gz() === 'both' ? [false, true] : [this.gz() === 'on'];
      const warmup = 1;
      const rounds = warmup + this.repeats();
      const total = rounds * runnable.length * formats.length * gzips.length;
      let done = 0;
      for (let round = 0; round < rounds; round++) {
        for (const p of runnable) {
          for (const format of formats) {
            for (const gzip of gzips) {
              if (signal.aborted) return;
              this.status.set(`${round < warmup ? 'Warm-up' : `Round ${round}`}: ${p.label}, ${FORMAT_LABELS[format]}${gzip ? ' + gzip' : ''}`);
              const m = await this.api.measure(p.path(seriesId), { format, gzip, message: p.message, signal });
              if (round >= warmup) {
                this.samples.update((s) => [...s, { payload: p.id, format, gzip, stats: m.stats }]);
              }
              this.progress.set((100 * ++done) / total);
            }
          }
        }
      }
      this.sys.set((await this.api.measure<SysInfo>('/api/v1/sys', { message: 'SysInfo' })).data);
    } catch (e) {
      if ((e as Error).name !== 'AbortError') this.error.set(`Run failed: ${(e as Error).message}`);
    } finally {
      this.running.set(false);
    }
  }

  download(): void {
    const body = JSON.stringify(
      { when: new Date().toISOString(), server: this.api.base() || location.origin, sys: this.sys(), rows: this.rows(), samples: this.samples() },
      null,
      2,
    );
    const a = document.createElement('a');
    a.href = URL.createObjectURL(new Blob([body], { type: 'application/json' }));
    a.download = `format-lab-${Date.now()}.json`;
    a.click();
    URL.revokeObjectURL(a.href);
  }
}
