import { describe, expect, it } from 'vitest';
import { boardOf } from './boards';
import { SeriesInfo } from './messages';

describe('board identity', () => {
  const series = (tags: string) => ({ tags, measurement: 'board' } as SeriesInfo);
  it('uses the configured source even if the scraped host is an IP', () => {
    expect(boardOf(series('app=mastomini,host=192.168.1.161,source=mastomini'))).toBe('mastomini');
  });
  it('recognizes existing board series and escaped tags', () => {
    expect(boardOf(series('app=mastomini-bots,host=mastomini-bots.local'))).toBe('mastomini-bots.local');
    expect(boardOf(series('host=up\\,stairs'))).toBe('up,stairs');
    expect(boardOf(series(''))).toBe('board');
  });
});
