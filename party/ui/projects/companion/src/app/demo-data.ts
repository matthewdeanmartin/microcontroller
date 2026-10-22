import { DataSource, PublicEvent, Snapshot } from 'party-core';

/**
 * A made-up instance for the demo: three public groups and their coming
 * events, dated relative to today so it never goes stale. No real people.
 */
export function demoSnapshot(now = Math.floor(Date.now() / 1000)): Snapshot {
  const day = 86_400;
  const at = (days: number, hour: number) => {
    const d = new Date((now + days * day) * 1000);
    d.setHours(hour, 0, 0, 0);
    return Math.floor(d.getTime() / 1000);
  };
  const ev = (id: number, group: number, title: string, start: number, hours: number, over: Partial<PublicEvent> = {}): PublicEvent => ({
    id, group, title, about: '', kind: 'generic', start, end: start + hours * 3600, area: '', status: 'scheduled',
    going: 0, capacity: 0, series: 0, ...over,
  });
  return {
    manifest: { v: 1, generation: 42, created: now - 9 * 60, instance: 'party (demo)', bot: '@party@example.social', public: [] },
    groups: [
      { id: 1, slug: 'game-club', name: 'Friday Game Club', about: 'Board games most Fridays. Beginners welcome; we teach.', kind: 'club', join: 'open', member_count: 14, hashtag: '' },
      { id: 2, slug: 'riverside-walkers', name: 'Riverside Walkers', about: 'An easy 5 km by the river, coffee after.', kind: 'club', join: 'open', member_count: 23, hashtag: '' },
      { id: 3, slug: 'maple-street', name: 'Maple Street Neighbours', about: 'Street parties, a tool library and the odd working bee.', kind: 'circle', join: 'approval', member_count: 31, hashtag: '' },
    ],
    events: [
      ev(101, 1, 'Game night: Wingspan and friends', at(2, 19), 3, { kind: 'meeting', going: 9, capacity: 12, series: 7, area: 'The back room at Hops & Co' }),
      ev(102, 1, 'Game night', at(9, 19), 3, { kind: 'meeting', going: 4, capacity: 12, series: 7, area: 'The back room at Hops & Co' }),
      ev(103, 2, 'Saturday river walk', at(3, 9), 2, { kind: 'outing', going: 11, area: 'Riverside Park, north gate' }),
      ev(104, 3, 'Street party', at(16, 14), 5, { kind: 'potluck', going: 37, about: 'Bring a dish to share. The road is closed 13:00-20:00.' }),
      ev(105, 3, 'Tool library working bee', at(5, 10), 3, { kind: 'workday', going: 6, area: 'Community garden shed' }),
      ev(106, 3, 'When suits for the winter meeting?', 0, 0, { kind: 'poll', going: 12 }),
    ],
    people: [
      { id: 1, display_name: 'Alex (organizer)', bio: 'Runs the game club.' },
      { id: 2, display_name: 'Priya', bio: 'Walks, every Saturday.' },
    ],
  };
}

export class DemoSource implements DataSource {
  readonly demo = true;

  async load(): Promise<Snapshot> {
    return demoSnapshot();
  }
}
