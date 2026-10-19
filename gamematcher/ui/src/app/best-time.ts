import { PlanTime, PlanView } from './api';

export interface RankedTime {
  time: PlanTime;
  yes: number[];
  flexible: number[];
  no: number[];
  unknown: number[];
  hostUnavailable: boolean;
}

export function rankTimes(plan: PlanView | null): RankedTime[] {
  if (!plan) return [];
  const people = [...new Set([plan.host, ...plan.responses.map((r) => r.account)])];
  return plan.times.map((time): RankedTime => {
    const row: RankedTime = { time, yes: [], flexible: [], no: [], unknown: [], hostUnavailable: false };
    for (const id of people) {
      const response = plan.responses.find((r) => r.account === id);
      const answer = response?.times.find((a) => a.time === time.id)?.answer;
      if (response?.overall === 'out' || answer === 'no') row.no.push(id);
      else if (answer === 'yes' && response?.overall === 'in') row.yes.push(id);
      else if (answer === 'yes' || answer === 'if_needed') row.flexible.push(id);
      else row.unknown.push(id);
    }
    row.hostUnavailable = row.no.includes(plan.host);
    return row;
  }).sort((a, b) =>
    Number(a.hostUnavailable) - Number(b.hostUnavailable)
    || b.yes.length - a.yes.length
    || b.flexible.length - a.flexible.length
    || a.no.length - b.no.length
    || a.time.start - b.time.start);
}
