import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  OnDestroy,
  afterNextRender,
  effect,
  input,
  viewChild,
} from '@angular/core';
import uPlot from 'uplot';
import { Columns } from './messages';

export interface ChartSeries {
  label: string;
  data: Columns;
}

const PALETTE = ['#2a7de1', '#d9622b', '#1f9d6b', '#9b59b6', '#c49a00', '#d6336c', '#0f9ab4', '#6b7280'];

/**
 * A time-series chart (uPlot: draws 100k points without breaking a sweat).
 * One series with min/max gets a shaded band; several series share the
 * time axis (missing timestamps are gaps).
 */
@Component({
  selector: 'hm-chart',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `<div class="chart" #host></div>`,
  styles: `
    :host { display: block; }
    .chart { width: 100%; min-height: 320px; }
  `,
})
export class Chart implements OnDestroy {
  readonly series = input.required<ChartSeries[]>();
  readonly height = input(320);
  /** Time axis range in ms ([from, to]); otherwise fitted to the data. */
  readonly range = input<[number, number] | null>(null);
  private readonly host = viewChild.required<ElementRef<HTMLDivElement>>('host');
  private plot?: uPlot;
  private resize?: ResizeObserver;
  private ready = false;

  constructor() {
    afterNextRender(() => {
      this.ready = true;
      this.resize = new ResizeObserver(() => this.plot?.setSize({ width: this.width(), height: this.height() }));
      this.resize.observe(this.host().nativeElement);
      this.draw(this.series());
    });
    effect(() => {
      const series = this.series();
      if (this.ready) this.draw(series);
    });
  }

  private width(): number {
    return Math.max(300, this.host().nativeElement.clientWidth);
  }

  private draw(series: ChartSeries[]): void {
    this.plot?.destroy();
    this.plot = undefined;
    if (!series.length) return;
    const dark = matchMedia('(prefers-color-scheme: dark)').matches;
    const axis = { stroke: dark ? '#a5a39b' : '#5d5b55', grid: { stroke: dark ? '#2c2b28' : '#ebe8e1' }, ticks: { stroke: dark ? '#2c2b28' : '#ebe8e1' } };
    const band = series.length === 1 && series[0].data.min && series[0].data.max;
    let data: uPlot.AlignedData;
    const opts: uPlot.Options = {
      width: this.width(),
      height: this.height(),
      cursor: { drag: { x: true, y: false } },
      scales: { x: this.range() ? { time: true, range: [this.range()![0] / 1000, this.range()![1] / 1000] } : { time: true } },
      axes: [axis, { ...axis, size: 60 }],
      legend: { show: true },
      series: [{}],
    };
    if (band) {
      const s = series[0].data;
      data = [s.t.map((t) => t / 1000), s.v, s.min!, s.max!];
      const color = PALETTE[0];
      opts.series.push(
        { label: `${series[0].label} (avg)`, stroke: color, width: 1.5, spanGaps: false },
        { label: 'min', stroke: color + '55', width: 1 },
        { label: 'max', stroke: color + '55', width: 1 },
      );
      opts.bands = [{ series: [3, 2], fill: color + '22' }];
    } else {
      const tables = series.map((s) => [s.data.t.map((t) => t / 1000), s.data.v] as uPlot.AlignedData);
      data = tables.length === 1 ? tables[0] : uPlot.join(tables);
      series.forEach((s, i) =>
        opts.series.push({ label: s.label, stroke: PALETTE[i % PALETTE.length], width: 1.5, spanGaps: true }),
      );
    }
    this.plot = new uPlot(opts, data, this.host().nativeElement);
  }

  ngOnDestroy(): void {
    this.resize?.disconnect();
    this.plot?.destroy();
  }
}
