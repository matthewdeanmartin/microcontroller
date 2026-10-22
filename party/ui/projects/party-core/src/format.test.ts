import { describe, expect, it } from 'vitest';
import { colour, count, day, initials } from './format';

describe('initials', () => {
  it('takes the first letters of two words', () => {
    expect(initials('Sunday Dinners')).toBe('SD');
    expect(initials('  sam ')).toBe('S');
    expect(initials('Book club for grown-ups')).toBe('BC');
  });
  it('copes with empty names and emoji', () => {
    expect(initials('')).toBe('?');
    expect(initials('🎉 Party')).toBe('🎉P');
  });
});

describe('colour', () => {
  it('is stable per id and differs between neighbours', () => {
    expect(colour(7)).toBe(colour(7));
    expect(colour(7)).not.toBe(colour(8));
    expect(colour(0)).toMatch(/^hsl\(\d+ 45% 38%\)$/);
  });
});

describe('count and day', () => {
  it('pluralises', () => {
    expect(count(1, 'member')).toBe('1 member');
    expect(count(3, 'member')).toBe('3 members');
    expect(count(2, 'person', 'people')).toBe('2 people');
  });
  it('shows nothing for an unset clock', () => {
    expect(day(0)).toBe('');
    expect(day(1_793_000_000)).not.toBe('');
  });
});
