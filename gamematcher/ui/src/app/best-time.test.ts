import { describe, expect, it } from 'vitest';
import { PlanView } from './api';
import { rankTimes } from './best-time';

const plan = (): PlanView => ({
  host: 1,
  times: [1, 2, 3].map((id) => ({ id, start: id * 1000, end: 0, yes: [], if_needed: [], no: [] })),
  responses: [
    { account: 1, overall: 'in', times: [{ time: 1, answer: 'no' }, { time: 2, answer: 'yes' }, { time: 3, answer: 'yes' }] },
    { account: 2, overall: 'in', times: [{ time: 1, answer: 'yes' }, { time: 2, answer: 'if_needed' }, { time: 3, answer: 'yes' }] },
    { account: 3, overall: 'out', times: [{ time: 1, answer: 'yes' }, { time: 2, answer: 'yes' }] },
    { account: 4, overall: 'maybe', times: [{ time: 3, answer: 'yes' }] },
  ],
} as PlanView);

describe('best times', () => {
  it('ranks attendance and flexibility, putting host conflicts last', () => {
    const rows = rankTimes(plan());
    expect(rows.map((r) => r.time.id)).toEqual([3, 2, 1]);
    expect(rows[0].yes).toEqual([1, 2]);
    expect(rows[0].flexible).toEqual([4]);
    expect(rows[0].no).toEqual([3]);
    expect(rows[1].unknown).toEqual([4]);
    expect(rows[2].hostUnavailable).toBe(true);
  });
  it('does not turn missing answers into attendance', () => {
    const p = plan(); p.responses = [];
    const rows = rankTimes(p);
    expect(rows.every((r) => r.yes.length === 0 && r.unknown.includes(p.host))).toBe(true);
    expect(rankTimes(null)).toEqual([]);
  });
});
