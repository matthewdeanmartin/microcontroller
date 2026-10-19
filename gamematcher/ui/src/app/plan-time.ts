import { toInputs, toSeconds } from './fmt';

export interface TimeRow {
  id: number;
  date: string;
  start: string;
  end: string;
}

/** Follow the last option by an hour, including across midnight. */
export function nextTime(previous?: TimeRow, now = Date.now() / 1000): TimeRow {
  const start = previous ? toSeconds(previous.date, previous.start) : 0;
  if (!start || !Number.isFinite(start)) {
    return { id: 0, date: toInputs(now).date, start: '19:00', end: '' };
  }
  const next = toInputs(start + 3600);
  const end = previous?.end ? toInputs(toSeconds(previous.date, previous.end) + 3600).time : '';
  return { id: 0, date: next.date, start: next.time, end };
}
