import { describe, expect, it } from 'vitest';
import { freshness, PublicEvent, SnapshotSource, upcoming } from './snapshot';

const ev = (id: number, start: number, over: Partial<PublicEvent> = {}): PublicEvent => ({
  id, group: 1, title: `E${id}`, about: '', kind: 'generic', start, end: 0, area: '', status: 'scheduled',
  going: 0, capacity: 0, series: 0, ...over,
});

describe('freshness', () => {
  it('reads like a person would say it', () => {
    expect(freshness(0, 100)).toEqual({ label: 'never published', stale: true });
    expect(freshness(1000, 1010).label).toBe('updated just now');
    expect(freshness(1000, 1000 + 14 * 60)).toEqual({ label: 'updated 14 min ago', stale: false });
    expect(freshness(0 + 1, 3 * 86400).stale).toBe(true);
  });
});

describe('upcoming', () => {
  it('sorts by time, keeps polls last, drops what is over', () => {
    const now = 10_000;
    const list = upcoming([ev(1, 50_000), ev(2, 0), ev(3, 20_000), ev(4, 100, { end: 200 }), ev(5, 30_000, { group: 2 })], now);
    expect(list.map((e) => e.id)).toEqual([3, 5, 1, 2]);
    expect(upcoming(list, now, 2).map((e) => e.id)).toEqual([5]);
  });
});

describe('SnapshotSource', () => {
  it('reads the manifest, then the files at its generation', async () => {
    const asked: string[] = [];
    const files: Record<string, unknown> = {
      'data/manifest.json': { v: 1, generation: 7, created: 5, instance: 'party', bot: '', public: [] },
      'data/public/groups.json': { groups: [{ id: 1, name: 'Club' }] },
      'data/public/events.json': {},
      'data/public/people.json': { people: [] },
    };
    const fake = (async (url: string) => {
      asked.push(url);
      const path = url.replace('https://x.example/d/', '').split('?')[0];
      return { ok: path in files, status: path in files ? 200 : 404, json: async () => files[path] } as Response;
    }) as typeof fetch;
    const s = await new SnapshotSource('https://x.example/d/', fake).load();
    expect(s.manifest.generation).toBe(7);
    expect(s.groups[0].name).toBe('Club');
    expect(s.events).toEqual([]);
    expect(asked.filter((u) => u.includes('groups.json'))[0]).toContain('?v=7');
  });
});
