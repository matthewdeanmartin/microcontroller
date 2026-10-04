import { ChangeDetectionStrategy, Component, OnDestroy, OnInit, inject, signal } from '@angular/core';
import { DatePipe, DecimalPipe } from '@angular/common';
import { LogLine, LogPage, SysInfo, formatBytes, formatDuration } from 'miniframework-ng';
import { Api } from '../api';
import { StoreStats } from '../messages';
import { WireStatsLine } from '../wire-stats';

@Component({
  selector: 'hm-system',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [DatePipe, DecimalPipe, WireStatsLine],
  template: `
    @if (error()) {
      <p class="error">{{ error() }}</p>
    }
    @if (sys(); as s) {
      <section class="grid">
        <article class="card">
          <h2>Board</h2>
          <dl>
            <dt>App</dt><dd>{{ s.app }} {{ s.version }} <span class="muted">build {{ s.build }}</span></dd>
            <dt>Host</dt><dd>{{ s.host }}</dd>
            <dt>Chip</dt><dd>{{ s.chip }} · {{ s.cores }} core{{ s.cores === 1 ? '' : 's' }}{{ s.cpu_mhz ? ' · ' + s.cpu_mhz + ' MHz' : '' }}</dd>
            <dt>Platform</dt><dd>{{ s.platform }} <span class="muted">{{ s.sdk }}</span></dd>
            @if (s.status) {
              <dt>Status light</dt><dd>{{ s.status }}</dd>
            }
            <dt>Up</dt><dd>{{ duration(s.uptime_ms) }} <span class="muted">(last reset: {{ s.reset_reason }})</span></dd>
            <dt>Clock</dt><dd>{{ s.wall_ms ? (s.wall_ms | date: 'medium') : 'not set yet' }}</dd>
            @if (s.temp_c !== null) {
              <dt>Chip temp</dt><dd>{{ s.temp_c | number: '1.1-1' }} °C</dd>
            }
            @if (s.flash_bytes) {
              <dt>Flash</dt><dd>{{ bytes(s.flash_bytes) }}</dd>
            }
          </dl>
        </article>
        <article class="card">
          <h2>Memory</h2>
          @if (s.heap.internal_total) {
            <p class="meter-label">Internal RAM <span>{{ bytes(s.heap.internal_free) }} free of {{ bytes(s.heap.internal_total) }}</span></p>
            <div class="meter"><span [style.width.%]="used(s.heap.internal_free, s.heap.internal_total)"></span></div>
            <p class="muted small">lowest ever {{ bytes(s.heap.internal_min) }} · largest block {{ bytes(s.heap.internal_largest) }}</p>
          }
          @if (s.heap.psram_total) {
            <p class="meter-label">PSRAM <span>{{ bytes(s.heap.psram_free) }} free of {{ bytes(s.heap.psram_total) }}</span></p>
            <div class="meter"><span [style.width.%]="used(s.heap.psram_free, s.heap.psram_total)"></span></div>
            <p class="muted small">lowest ever {{ bytes(s.heap.psram_min) }} · largest block {{ bytes(s.heap.psram_largest) }}</p>
          }
          @if (!s.heap.internal_total && !s.heap.psram_total) {
            <p class="muted">The desktop build does not report memory.</p>
          }
        </article>
        <article class="card">
          <h2>Network</h2>
          <dl>
            @if (s.wifi.ssid) {
              <dt>Wi-Fi</dt><dd>{{ s.wifi.ssid }} · {{ s.wifi.rssi }} dBm · channel {{ s.wifi.channel }}</dd>
              <dt>Address</dt><dd>{{ s.wifi.ip }} <span class="muted">{{ s.wifi.mac }}</span></dd>
            }
            <dt>Requests</dt><dd>{{ s.net.requests | number }} <span class="muted">({{ s.net.errors | number }} errors)</span></dd>
            <dt>Sent</dt><dd>{{ bytes(s.net.kib_out * 1024) }}</dd>
            <dt>Connections</dt><dd>{{ s.net.tls_open }}/{{ s.net.tls_slots }} HTTPS · {{ s.net.http_open }}/{{ s.net.http_slots }} HTTP · {{ s.net.rejected }} turned away</dd>
            <dt>TLS handshakes</dt><dd>{{ s.net.tls_handshakes | number }} · last {{ s.net.handshake_ms_last }} ms · avg {{ s.net.handshake_ms_avg }} ms · {{ s.net.tls_failures }} failed</dd>
          </dl>
        </article>
        @if (store(); as st) {
          <article class="card">
            <h2>Metrics store</h2>
            <p class="meter-label">Raw blocks <span>{{ st.blocks_used }} of {{ st.blocks_total }} ({{ bytes(st.block_bytes) }} each)</span></p>
            <div class="meter"><span [style.width.%]="(100 * st.blocks_used) / st.blocks_total"></span></div>
            <dl>
              <dt>Series</dt><dd>{{ st.series }} of {{ st.max_series }}</dd>
              <dt>Raw points</dt><dd>{{ st.raw_points | number }} at {{ st.bytes_per_point | number: '1.1-2' }} bytes each</dd>
              <dt>Rollups</dt><dd>{{ st.rollup_slots }} × {{ st.rollup_secs / 60 }} min per series ({{ duration(st.rollup_slots * st.rollup_secs * 1000) }})</dd>
              <dt>Written</dt><dd>{{ st.accepted | number }} accepted · {{ st.rejected | number }} rejected · {{ st.evicted_blocks | number }} blocks evicted</dd>
              <dt>Capacity</dt><dd>{{ bytes(st.capacity_bytes) }} when full</dd>
            </dl>
          </article>
        }
      </section>
      <section class="card log-card">
        <div class="row between">
          <h2>Board log</h2>
          <a class="small" [href]="api.url('/api/v1/log.txt')" target="_blank" rel="noopener">as text</a>
        </div>
        @if (previous(); as p) {
          <details class="previous" open>
            <summary>The previous boot ended with <b>{{ p.reason }}</b>. Its last lines:</summary>
            <pre>{{ p.text }}</pre>
          </details>
        }
        @if (dropped()) {
          <p class="muted small">{{ dropped() }} older lines no longer held.</p>
        }
        <div class="log">
          @for (l of log(); track l.seq) {
            <div class="line" [class.err]="l.level === 1" [class.warn]="l.level === 2">
              <span class="t">{{ l.t / 1000 | number: '1.3-3' }}</span>
              <span class="lv">{{ levels[l.level] ?? '?' }}</span>
              <span class="tx">{{ l.text }}</span>
            </div>
          } @empty {
            <p class="muted">No log lines yet.</p>
          }
        </div>
      </section>
      <hm-wire-stats [stats]="api.recent()[0]" />
    }
  `,
})
export class System implements OnInit, OnDestroy {
  readonly api = inject(Api);
  readonly sys = signal<SysInfo | null>(null);
  readonly store = signal<StoreStats | null>(null);
  readonly log = signal<LogLine[]>([]);
  readonly previous = signal<{ reason: string; text: string } | null>(null);
  readonly dropped = signal(0);
  readonly levels: Record<number, string> = { 1: 'E', 2: 'W', 3: 'I', 4: 'D', 5: 'V' };
  private after = 0;
  private lastUptime = 0;
  readonly error = signal('');
  readonly bytes = formatBytes;
  readonly duration = formatDuration;
  private timer?: ReturnType<typeof setInterval>;

  ngOnInit(): void {
    void this.load();
    this.timer = setInterval(() => this.load(), 3000);
  }

  ngOnDestroy(): void {
    clearInterval(this.timer);
  }

  /** Only lines newer than the last ones seen (a reboot resets `after`). */
  private async loadLog(): Promise<void> {
    const page = await this.api.get<LogPage>(`/api/v1/log?after=${this.after}&limit=300`, 'LogPage');
    this.after = page.next;
    this.dropped.set(page.dropped);
    this.previous.set(page.previous ? { reason: page.previous_reason, text: page.previous } : null);
    if (page.lines.length) this.log.update((old) => [...old, ...page.lines].slice(-300));
  }

  used(free: number, total: number): number {
    return total ? (100 * (total - free)) / total : 0;
  }

  async load(): Promise<void> {
    try {
      const sys = await this.api.get<SysInfo>('/api/v1/sys', 'SysInfo');
      if (sys.uptime_ms < this.lastUptime) {
        // Rebooted: its log starts again at line 1.
        this.after = 0;
        this.log.set([]);
      }
      this.lastUptime = sys.uptime_ms;
      this.sys.set(sys);
      this.store.set(await this.api.get<StoreStats>('/api/v1/store', 'StoreStats'));
      await this.loadLog();
      this.error.set('');
    } catch (e) {
      this.error.set(`Could not reach the server: ${(e as Error).message}`);
    }
  }
}
