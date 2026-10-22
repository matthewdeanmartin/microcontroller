// What the board publishes to the companion site (api/src/publish.rs), and
// the data sources the companion site and its demo read it through.

export interface Manifest {
  v: number;
  generation: number;
  created: number;
  instance: string;
  bot: string;
  public: string[];
}

export interface PublicGroup {
  id: number;
  slug: string;
  name: string;
  about: string;
  kind: string;
  join: string;
  member_count: number;
  hashtag: string;
}

export interface PublicEvent {
  id: number;
  group: number;
  title: string;
  about: string;
  kind: string;
  start: number;
  end: number;
  /** A place's name, never an address. */
  area: string;
  status: string;
  going: number;
  capacity: number;
  series: number;
}

export interface PublicPerson {
  id: number;
  display_name: string;
  bio: string;
}

/** Everything the public pages show, from wherever it comes. */
export interface Snapshot {
  manifest: Manifest;
  groups: PublicGroup[];
  events: PublicEvent[];
  people: PublicPerson[];
}

export interface DataSource {
  load(): Promise<Snapshot>;
  /** The demo says so on every page. */
  readonly demo: boolean;
}

/** How old the data is, and whether to warn about it. */
export function freshness(created: number, now: number): { label: string; stale: boolean } {
  if (!created) return { label: 'never published', stale: true };
  const mins = Math.max(0, Math.round((now - created) / 60));
  const label =
    mins < 1 ? 'updated just now'
    : mins < 60 ? `updated ${mins} min ago`
    : mins < 48 * 60 ? `updated ${Math.round(mins / 60)} h ago`
    : `updated ${Math.round(mins / 1440)} days ago`;
  return { label, stale: mins > 24 * 60 };
}

/** Events of a group (or all), soonest first, without ones long over. */
export function upcoming(events: PublicEvent[], now: number, group?: number): PublicEvent[] {
  return events
    .filter((e) => group === undefined || e.group === group)
    .filter((e) => e.status !== 'cancelled' || e.start > now)
    .filter((e) => !e.start || (e.end || e.start + 3 * 3600) > now)
    .sort((a, b) => (a.start || Number.MAX_SAFE_INTEGER) - (b.start || Number.MAX_SAFE_INTEGER) || a.id - b.id);
}

/** Reads the published files from a base URL (cache-busted by generation). */
export class SnapshotSource implements DataSource {
  readonly demo = false;
  constructor(private readonly base: string, private readonly fetcher: typeof fetch = (u, i) => fetch(u, i)) {}

  private async json<T>(path: string, v: number | string): Promise<T> {
    const r = await this.fetcher(`${this.base}${path}?v=${v}`, { cache: 'no-cache' });
    if (!r.ok) throw new Error(`${path}: HTTP ${r.status}`);
    return (await r.json()) as T;
  }

  async load(): Promise<Snapshot> {
    const manifest = await this.json<Manifest>('data/manifest.json', Date.now());
    const g = manifest.generation;
    const [groups, events, people] = await Promise.all([
      this.json<{ groups?: PublicGroup[] }>('data/public/groups.json', g),
      this.json<{ events?: PublicEvent[] }>('data/public/events.json', g),
      this.json<{ people?: PublicPerson[] }>('data/public/people.json', g),
    ]);
    return { manifest, groups: groups.groups ?? [], events: events.events ?? [], people: people.people ?? [] };
  }
}
