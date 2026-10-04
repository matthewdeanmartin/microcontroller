import { ChangeDetectionStrategy, Component, OnDestroy, OnInit, computed, inject, signal } from '@angular/core';
import { DecimalPipe } from '@angular/common';
import { formatBytes } from 'miniframework-ng';
import { Api } from '../api';
import { Chart, ChartSeries } from '../chart';
import { QueryResult, SeriesInfo, SeriesList, toColumns } from '../messages';
import { WireStatsLine } from '../wire-stats';
import { boardOf } from '../boards';

const RANGES = [
  { label: '15 min', ms: 15 * 60_000 },
  { label: '1 h', ms: 3_600_000 },
  { label: '6 h', ms: 6 * 3_600_000 },
  { label: '24 h', ms: 24 * 3_600_000 },
  { label: '3 d', ms: 3 * 86_400_000 },
  { label: '7 d', ms: 7 * 86_400_000 },
];

function load(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function keep(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* storage unavailable */
  }
}

@Component({
  selector: 'hm-dashboard',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [Chart, WireStatsLine, DecimalPipe],
  template: `
    <section class="toolbar">
      <label>Board
        <select aria-label="Board" [value]="board()" (change)="setBoard($any($event.target).value)">
          <option value="">All boards</option>
          @for (b of boards(); track b) { <option [value]="b">{{ b }}</option> }
        </select>
      </label>
      <div class="segmented" role="group" aria-label="Time range">
        @for (r of ranges; track r.ms) {
          <button [class.on]="range() === r.ms" (click)="setRange(r.ms)">{{ r.label }}</button>
        }
      </div>
      <label class="check"><input type="checkbox" [checked]="live()" (change)="setLive($any($event.target).checked)" /> refresh every 10 s</label>
      <button class="ghost" (click)="refresh()">Refresh</button>
    </section>

    @if (error()) {
      <p class="error">{{ error() }}</p>
    }

    <section class="card">
      @if (chart().length) {
        <hm-chart [series]="chart()" [range]="window()" />
        <p class="muted small">
          @for (s of shown(); track s.id) {
            <span class="pill">{{ s.kind === 'raw' ? 'raw' : 'buckets of ' + stepLabel(s.step) }} · {{ s.points }} points</span>
          }
        </p>
      } @else {
        <p class="empty">{{ selected().size ? 'No data in this range yet.' : 'Pick one or more series below to graph them.' }}</p>
      }
      <hm-wire-stats [stats]="api.recent()[0]" />
    </section>

    <section class="card">
      <div class="row between">
        <h2>Series</h2>
        <input class="search" type="search" placeholder="Filter" [value]="filter()" (input)="filter.set($any($event.target).value)" />
      </div>
      <p class="muted small">Choose up to eight metrics. Missing a board? Check its scrape result on Devices. System shows this collector's health.</p>
      @for (group of groups(); track group.board) {
        <h3>{{ group.board }}</h3>
        <ul class="series">
          @for (s of group.series; track s.id) {
            <li>
              <label>
                <input type="checkbox" [checked]="selected().has(s.id)" [disabled]="!selected().has(s.id) && selected().size >= 8" (change)="toggle(s.id)" />
                <span class="name">{{ s.field }}</span>
                @if (s.tags) {
                  <span class="muted">{{ s.tags }}</span>
                }
              </label>
              <span class="value">{{ s.last_v | number: '1.0-3' }}</span>
              <span class="muted small">{{ s.raw_points }} raw · {{ bytes(s.raw_bytes) }}</span>
            </li>
          }
        </ul>
      } @empty {
        <p class="empty">No series yet. Boards create them by writing (see Devices), and this board records its own health every 10 s once its clock is set.</p>
      }
    </section>
  `,
})
export class Dashboard implements OnInit, OnDestroy {
  readonly api = inject(Api);
  readonly ranges = RANGES;
  readonly list = signal<SeriesInfo[]>([]);
  readonly selected = signal<Set<number>>(new Set(JSON.parse(load('hm.selected') ?? '[]') as number[]));
  readonly range = signal(Number(load('hm.range')) || 3_600_000);
  readonly live = signal(load('hm.live') !== '0');
  readonly filter = signal('');
  readonly board = signal(load('hm.board') ?? '');
  readonly boards = computed(() => [...new Set(this.list().map(boardOf))].sort());
  readonly chart = signal<ChartSeries[]>([]);
  readonly window = signal<[number, number] | null>(null);
  readonly shown = signal<{ id: number; kind: string; step: number; points: number }[]>([]);
  readonly error = signal('');
  readonly bytes = formatBytes;
  private timer?: ReturnType<typeof setInterval>;
  private chartRequest = 0;

  readonly groups = computed(() => {
    const f = this.filter().toLowerCase();
    const map = new Map<string, SeriesInfo[]>();
    for (const s of this.list()) {
      const board = boardOf(s);
      if (this.board() && board !== this.board()) continue;
      if (f && !s.key.toLowerCase().includes(f)) continue;
      const g = map.get(board) ?? [];
      g.push(s);
      map.set(board, g);
    }
    return [...map.entries()].map(([board, series]) => ({ board, series }));
  });

  setBoard(board: string): void {
    this.board.set(board);
    keep('hm.board', board);
    const allowed = new Set(this.list().filter(s => !board || boardOf(s) === board).map(s => s.id));
    const kept = new Set([...this.selected()].filter(id => allowed.has(id)).slice(0, 8));
    this.selected.set(kept);
    keep('hm.selected', JSON.stringify([...kept]));
    void this.loadChart();
  }

  ngOnInit(): void {
    void this.refresh();
    this.timer = setInterval(() => this.live() && this.refresh(), 10_000);
  }

  ngOnDestroy(): void {
    clearInterval(this.timer);
  }

  setRange(ms: number): void {
    this.range.set(ms);
    keep('hm.range', String(ms));
    void this.loadChart();
  }

  setLive(on: boolean): void {
    this.live.set(on);
    keep('hm.live', on ? '1' : '0');
  }

  toggle(id: number): void {
    const next = new Set(this.selected());
    if (next.has(id)) next.delete(id);
    else if (next.size < 8) next.add(id);
    this.selected.set(next);
    keep('hm.selected', JSON.stringify([...next]));
    void this.loadChart();
  }

  stepLabel(ms: number): string {
    if (ms < 60_000) return `${ms / 1000} s`;
    if (ms < 3_600_000) return `${ms / 60_000} min`;
    return `${ms / 3_600_000} h`;
  }

  async refresh(): Promise<void> {
    try {
      const list = await this.api.get<SeriesList>('/api/v1/series', 'SeriesList');
      this.list.set(list.series);
      const live = new Set(list.series.map((s) => s.id));
      const kept = new Set([...this.selected()].filter((id) => live.has(id)));
      if (kept.size !== this.selected().size) this.selected.set(kept);
      await this.loadChart();
      this.error.set('');
    } catch (e) {
      this.error.set(`Could not reach the server: ${(e as Error).message}`);
    }
  }

  async loadChart(): Promise<void> {
    const request = ++this.chartRequest;
    const visible = new Set(this.list().filter(s => !this.board() || boardOf(s) === this.board()).map(s => s.id));
    const ids = [...this.selected()].filter(id => visible.has(id)).slice(0, 8);
    if (!ids.length) {
      this.chart.set([]);
      this.shown.set([]);
      return;
    }
    const from = Date.now() - this.range();
    // About one point per two pixels of chart width.
    const max = Math.max(200, Math.min(2000, Math.round(window.innerWidth / 2)));
    const result = await this.api.get<QueryResult>(
      `/api/v1/query?ids=${ids.join(',')}&from=${from}&max=${max}&shape=columns`,
      'QueryResult',
    );
    if (request !== this.chartRequest) return;
    const names = new Map(this.list().map((s) => [s.id, s]));
    this.window.set([from, Date.now()]);
    this.chart.set(
      result.series
        .map((s) => ({ label: labelOf(names.get(s.id)), data: toColumns(s) }))
        .filter((s) => s.data.t.length > 0),
    );
    this.shown.set(
      result.series.map((s) => ({ id: s.id, kind: s.kind, step: s.step, points: toColumns(s).t.length })),
    );
  }
}

function labelOf(s?: SeriesInfo): string {
  if (!s) return '?';
  return s.tags ? `${s.measurement} ${s.field} (${s.tags})` : `${s.measurement} ${s.field}`;
}
