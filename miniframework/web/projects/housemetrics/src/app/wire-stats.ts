import { ChangeDetectionStrategy, Component, input } from '@angular/core';
import { DecimalPipe } from '@angular/common';
import { FORMAT_LABELS, WireStats, formatBytes } from 'miniframework-ng';

/** One line saying what the last request cost. */
@Component({
  selector: 'hm-wire-stats',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [DecimalPipe],
  template: `
    @if (stats(); as s) {
      <p class="wire">
        <span class="tag">{{ label(s) }}{{ s.gzip ? ' + gzip' : '' }}</span>
        <span title="Body bytes on the wire (before gzip: {{ bytes(s.rawBytes) }})">{{ bytes(s.bodyBytes) }}</span>
        <span title="Server: handler / encode / gzip">server {{ s.serverApp + s.serverEnc + s.serverGz | number: '1.1-2' }} ms
          (enc {{ s.serverEnc | number: '1.2-2' }})</span>
        @if (s.connectMs > 0) {
          <span class="warn" title="This request opened a new connection">connect {{ s.connectMs | number: '1.0-0' }} ms</span>
        }
        <span title="Request sent to first byte">TTFB {{ s.ttfb | number: '1.0-0' }} ms</span>
        <span title="Fetch start to last byte">fetch {{ s.fetchMs | number: '1.0-0' }} ms</span>
        <span title="Bytes to objects in this browser">decode {{ s.decodeMs | number: '1.2-2' }} ms</span>
      </p>
    }
  `,
})
export class WireStatsLine {
  readonly stats = input<WireStats | undefined>();
  bytes = formatBytes;
  label(s: WireStats): string {
    return FORMAT_LABELS[s.format] ?? s.format;
  }
}
