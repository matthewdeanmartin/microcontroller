import { SeriesInfo } from './messages';

/** Tags use Influx escaping; configured source names stay stable across IP changes. */
export function boardOf(series: SeriesInfo): string {
  const tags = new Map<string, string>();
  for (const match of series.tags.matchAll(/(?:^|,)([^=,]+)=((?:\\.|[^,])*)/g)) {
    tags.set(match[1], match[2].replace(/\\(.)/g, '$1'));
  }
  return tags.get('source') || tags.get('host') || tags.get('app') || series.measurement;
}
