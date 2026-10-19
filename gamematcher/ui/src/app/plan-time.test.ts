import { describe, expect, it } from 'vitest';
import { nextTime } from './plan-time';

describe('time option defaults', () => {
  it('advances the previous start and end, rolling into the next year', () => {
    expect(nextTime({ id: 7, date: '2026-12-31', start: '23:30', end: '01:00' }))
      .toEqual({ id: 0, date: '2027-01-01', start: '00:30', end: '02:00' });
  });
  it('keeps an unspecified end empty and follows successive options', () => {
    const first = { id: 0, date: '2026-10-18', start: '19:00', end: '' };
    expect(nextTime(nextTime(first))).toEqual({ ...first, start: '21:00' });
  });
});
