// Small presentation helpers shared by the local app, the companion site
// and the demo. Pure functions, unit-tested in format.test.ts.
import type { GroupKind, GroupRole, JoinPolicy, Visibility } from './models';

/** Up to two initials for an avatar: "Sunday Dinners" → "SD", "sam" → "S". */
export function initials(name: string): string {
  const words = name.trim().split(/\s+/).filter(Boolean);
  const letters = words.slice(0, 2).map((w) => [...w][0] ?? '');
  return letters.join('').toUpperCase() || '?';
}

/** A stable colour for an id, readable behind white text. */
export function colour(id: number): string {
  const hue = (Math.imul(id + 1, 2654435761) >>> 0) % 360;
  return `hsl(${hue} 45% 38%)`;
}

export const VISIBILITY_LABEL: Record<Visibility, string> = {
  public: 'Public: anyone can find it',
  unlisted: 'Unlisted: people with the link can see it',
  private: 'Private: only members know it exists',
};

export const JOIN_LABEL: Record<JoinPolicy, string> = {
  open: 'Anyone who can see it can join',
  approval: 'People ask; an organizer approves',
  invite: 'By invitation only',
};

export const KIND_LABEL: Record<GroupKind, string> = {
  club: 'Club (regular meetups)',
  circle: 'Circle (family or friends)',
  'one-off': 'One-off (a single event)',
};

export const ROLE_LABEL: Record<GroupRole, string> = {
  organizer: 'Organizer',
  'co-organizer': 'Co-organizer',
  member: 'Member',
};

/** "1 member", "12 members". */
export function count(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** A date from Unix seconds, or "" while the board's clock was unset. */
export function day(secs: number): string {
  if (!secs) return '';
  return new Date(secs * 1000).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}
