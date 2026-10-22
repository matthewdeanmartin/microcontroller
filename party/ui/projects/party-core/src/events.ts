// Events, answers, series and notifications (api/src/messages.rs), plus the
// pure helpers the event pages share. Unit-tested in events.test.ts.

export type EventKind = 'generic' | 'party' | 'dinner' | 'potluck' | 'outing' | 'meeting' | 'workday' | 'poll';
export type Module = 'headcount' | 'bring' | 'diet' | 'rides' | 'when' | 'shifts' | 'questions' | 'cost' | 'link';
export type EventStatus = 'draft' | 'scheduled' | 'cancelled' | 'done';
export type RsvpStatus = 'going' | 'maybe' | 'no' | 'waitlist';

export interface Slot { id: number; name: string; count: number }
export interface Question { id: number; kind: 'yesno' | 'choice' | 'text'; prompt: string; choices: string[] }
export interface TimeOption { id: number; start: number; end: number }
export interface Shift { id: number; name: string; start: number; end: number; count: number }
export interface Answer { question: number; value: string }

export interface StoredEvent {
  id: number;
  group: number;
  series: number;
  kind: EventKind;
  title: string;
  about: string;
  hosts: number[];
  start: number;
  end: number;
  where_kind: 'place' | 'home' | 'tbd';
  place: string;
  address: string;
  map: string;
  home: number;
  visibility: 'group' | 'invite-only' | 'public';
  invitees: number[];
  capacity: number;
  rsvp_by: number;
  plus_ones: 'none' | 'limited' | 'unlimited';
  plus_max: number;
  status: EventStatus;
  rev: number;
  modules: Module[];
  bring: Slot[];
  diet_prompt: string;
  questions: Question[];
  cost: string;
  rides_note: string;
  when: TimeOption[];
  shifts: Shift[];
  link: string;
  reminder_secs: number;
  occurrence: number;
}

export interface SlotCount { id: number; count: number; names: string[] }

export interface RsvpView {
  person: number;
  display_name: string;
  status: RsvpStatus;
  plus: number;
  kids: number;
  names: string;
  bring: number[];
  bring_other: string;
  diet: string;
  answers: Answer[];
  ride: '' | 'need' | 'offer' | 'own';
  seats: number;
  when: number[];
  shifts: number[];
  note: string;
  by_name: string;
  via: string;
  updated: number;
  over_policy: boolean;
}

export interface EventView {
  event: StoredEvent;
  group_name: string;
  group_slug: string;
  host_names: string[];
  heads: number;
  going: number;
  maybe: number;
  no: number;
  waitlist: number;
  mine?: RsvpView | null;
  can_edit: boolean;
  can_answer: boolean;
  answer_for: { id: number; display_name: string }[];
  claimed: SlotCount[];
  votes: SlotCount[];
  taken: SlotCount[];
  locked: boolean;
}

export interface EventSummary {
  id: number;
  group: number;
  group_name: string;
  title: string;
  kind: EventKind;
  start: number;
  end: number;
  status: EventStatus;
  heads: number;
  capacity: number;
  my_status: RsvpStatus | '';
  series: number;
  polling: boolean;
}

export interface EventList { events: EventSummary[]; next: number }

export interface RsvpInput {
  status: 'going' | 'maybe' | 'no';
  plus?: number;
  kids?: number;
  names?: string;
  bring?: number[];
  bring_other?: string;
  diet?: string;
  answers?: Answer[];
  ride?: string;
  seats?: number;
  when?: number[];
  shifts?: number[];
  note?: string;
  rev_seen: number;
}

export interface RideBoard {
  needs: { person: number; display_name: string; seats: number }[];
  offers: { person: number; display_name: string; seats: number }[];
  spare: number;
}

export interface SeriesView {
  id: number;
  group: number;
  rule: string;
  every: number;
  nth: number;
  until: number;
  count: number;
  window: number;
  status: string;
  summary: string;
  regular: boolean;
  regulars: number;
  upcoming: EventSummary[];
  can_edit: boolean;
}

export interface NoticeView {
  id: number;
  kind: string;
  event: number;
  event_title: string;
  group: number;
  group_name: string;
  actor_name: string;
  created: number;
  read: boolean;
  count: number;
}

export interface Summary { unread: number; awaiting: number; latest: number }

export const KIND_INFO: Record<EventKind, { label: string; icon: string; modules: Module[] }> = {
  generic: { label: 'Get-together', icon: '📅', modules: ['headcount'] },
  party: { label: 'Party', icon: '🎉', modules: ['headcount', 'bring', 'diet'] },
  dinner: { label: 'Dinner', icon: '🍽️', modules: ['headcount', 'diet'] },
  potluck: { label: 'Potluck', icon: '🥘', modules: ['headcount', 'bring', 'diet'] },
  outing: { label: 'Outing', icon: '🚗', modules: ['headcount', 'rides', 'cost'] },
  meeting: { label: 'Meeting', icon: '🗒️', modules: ['headcount', 'questions'] },
  workday: { label: 'Workday', icon: '🛠️', modules: ['headcount', 'shifts', 'bring'] },
  poll: { label: 'When works?', icon: '🗳️', modules: ['headcount', 'when'] },
};

export const MODULE_LABEL: Record<Module, string> = {
  headcount: 'Headcount and guests',
  bring: 'Bring list',
  diet: 'Allergies or diet',
  rides: 'Rides',
  when: 'Time poll',
  shifts: 'Shifts',
  questions: 'Questions',
  cost: 'Cost',
  link: 'Online link',
};

export const STATUS_LABEL: Record<RsvpStatus, string> = {
  going: 'Going',
  maybe: 'Maybe',
  no: "Can't come",
  waitlist: 'On the waitlist',
};

export const NOTICE_TEXT: Record<string, string> = {
  invited: 'invited you to',
  changed: 'changed',
  cancelled: 'cancelled',
  promoted: 'You moved off the waitlist for',
  reminder: 'Coming up:',
  request: 'asked to join',
  answered_for: 'answered for you:',
};

/** Unix seconds → the value of an `<input type="datetime-local">` in the browser's zone. */
export function toLocalInput(secs: number): string {
  if (!secs) return '';
  const d = new Date(secs * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** The reverse: "" → 0. */
export function fromLocalInput(value: string): number {
  if (!value) return 0;
  const t = new Date(value).getTime();
  return Number.isNaN(t) ? 0 : Math.round(t / 1000);
}

/** "Sat 26 Oct, 18:00–21:00" (or "18:00" with no end, "" for no time). */
export function when(start: number, end = 0): string {
  if (!start) return '';
  const s = new Date(start * 1000);
  const day = s.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
  const time = (d: Date) => d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  if (!end || end <= start) return `${day}, ${time(s)}`;
  const e = new Date(end * 1000);
  const sameDay = e.toDateString() === s.toDateString();
  return sameDay ? `${day}, ${time(s)}–${time(e)}` : `${day}, ${time(s)} – ${e.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' })} ${time(e)}`;
}

/** "4 going · 2 maybe" / "4 of 10 places". */
export function headline(heads: number, capacity: number, maybe = 0): string {
  const going = capacity ? `${heads} of ${capacity} places taken` : `${heads} going`;
  return maybe ? `${going} · ${maybe} maybe` : going;
}

/** Most guests one answer may bring under the event's rule. */
export function guestLimit(e: Pick<StoredEvent, 'plus_ones' | 'plus_max'>): number {
  if (e.plus_ones === 'none') return 0;
  if (e.plus_ones === 'limited') return e.plus_max;
  return 20;
}

/** The form a person fills in for an event, from its modules. */
export function blankAnswer(e: StoredEvent, mine?: RsvpView | null): RsvpInput {
  return {
    // Someone on the waitlist still wants to go.
    status: mine && mine.status !== 'waitlist' ? mine.status : 'going',
    plus: mine?.plus ?? 0,
    kids: mine?.kids ?? 0,
    names: mine?.names ?? '',
    bring: mine ? [...mine.bring] : [],
    bring_other: mine?.bring_other ?? '',
    diet: mine?.diet ?? '',
    answers: mine ? mine.answers.map((a) => ({ ...a })) : [],
    ride: mine?.ride ?? '',
    seats: mine?.seats ?? 0,
    when: mine ? [...mine.when] : [],
    shifts: mine ? [...mine.shifts] : [],
    note: mine?.note ?? '',
    rev_seen: e.rev,
  };
}

/** Toggles `id` in a list (for checkboxes). */
export function toggle(list: number[], id: number): number[] {
  return list.includes(id) ? list.filter((x) => x !== id) : [...list, id].sort((a, b) => a - b);
}
