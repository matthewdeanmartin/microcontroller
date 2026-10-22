import { describe, expect, it } from 'vitest';
import { blankAnswer, fromLocalInput, guestLimit, headline, KIND_INFO, StoredEvent, toggle, toLocalInput, when } from './events';

const event = (over: Partial<StoredEvent> = {}): StoredEvent =>
  ({
    id: 1, group: 1, series: 0, kind: 'potluck', title: 'T', about: '', hosts: [1], start: 1_893_456_000, end: 0,
    where_kind: 'tbd', place: '', address: '', map: '', home: 0, visibility: 'group', invitees: [], capacity: 0,
    rsvp_by: 0, plus_ones: 'limited', plus_max: 2, status: 'scheduled', rev: 3, modules: ['headcount', 'bring'],
    bring: [], diet_prompt: '', questions: [], cost: '', rides_note: '', when: [], shifts: [], link: '',
    reminder_secs: 0, occurrence: 0, ...over,
  }) as StoredEvent;

describe('datetime-local round trip', () => {
  it('returns the same instant', () => {
    const secs = 1_893_456_000;
    expect(fromLocalInput(toLocalInput(secs))).toBe(secs);
    expect(toLocalInput(0)).toBe('');
    expect(fromLocalInput('')).toBe(0);
    expect(fromLocalInput('not a date')).toBe(0);
  });
});

describe('labels', () => {
  it('formats times and headcounts', () => {
    expect(when(0)).toBe('');
    expect(when(1_893_456_000, 1_893_456_000 + 3 * 3600)).toContain('–');
    expect(headline(4, 10)).toBe('4 of 10 places taken');
    expect(headline(3, 0, 2)).toBe('3 going · 2 maybe');
  });
  it('mirrors the server presets', () => {
    expect(KIND_INFO.potluck.modules).toEqual(['headcount', 'bring', 'diet']);
    expect(KIND_INFO.poll.modules).toContain('when');
  });
});

describe('answers', () => {
  it('limits guests by the rule', () => {
    expect(guestLimit(event({ plus_ones: 'none' }))).toBe(0);
    expect(guestLimit(event())).toBe(2);
    expect(guestLimit(event({ plus_ones: 'unlimited' }))).toBe(20);
  });
  it('starts a form from the current answer and the event rev', () => {
    const blank = blankAnswer(event());
    expect(blank.status).toBe('going');
    expect(blank.rev_seen).toBe(3);
    const mine = blankAnswer(event(), {
      person: 2, display_name: 'A', status: 'maybe', plus: 1, kids: 0, names: 'Bo', bring: [4], bring_other: '',
      diet: '', answers: [], ride: '', seats: 0, when: [], shifts: [], note: 'late', by_name: '', via: 'web',
      updated: 0, over_policy: false,
    });
    expect([mine.status, mine.plus, mine.bring, mine.note]).toEqual(['maybe', 1, [4], 'late']);
  });
  it('toggles ids in order', () => {
    expect(toggle([3, 1], 2)).toEqual([1, 2, 3]);
    expect(toggle([1, 2], 2)).toEqual([1]);
  });
});
