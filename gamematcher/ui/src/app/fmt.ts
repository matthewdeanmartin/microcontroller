// Dates in the browser's zone (the household's zone: play is in the house).

const day = new Intl.DateTimeFormat([], { weekday: 'short', month: 'short', day: 'numeric' });
const clock = new Intl.DateTimeFormat([], { hour: 'numeric', minute: '2-digit' });

function sameDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
}

/** "Tonight", "Tomorrow", or "Sat, Oct 24". */
export function dayName(seconds: number, now = new Date()): string {
  const d = new Date(seconds * 1000);
  if (sameDay(d, now)) return d.getHours() >= 17 ? 'Tonight' : 'Today';
  const tomorrow = new Date(now);
  tomorrow.setDate(now.getDate() + 1);
  if (sameDay(d, tomorrow)) return 'Tomorrow';
  return day.format(d);
}

/** "Sat, Oct 24, 2:00 PM–6:00 PM" or "Tonight 7:00 PM". */
export function when(start: number, end = 0, now = new Date()): string {
  if (!start) return 'Time to be decided';
  const from = `${dayName(start, now)} ${clock.format(new Date(start * 1000))}`;
  return end ? `${from}–${clock.format(new Date(end * 1000))}` : from;
}

/** "2–4 players", "2+ players", "" when unknown. */
export function players(min: number, max: number): string {
  if (min && max) return min === max ? `${min} players` : `${min}–${max} players`;
  if (min) return `${min}+ players`;
  if (max) return `up to ${max} players`;
  return '';
}

/** Local date + time inputs → Unix seconds. */
export function toSeconds(date: string, time: string): number {
  if (!date || !time) return 0;
  return Math.round(new Date(`${date}T${time}`).getTime() / 1000);
}

/** Unix seconds → values for <input type=date> and <input type=time>. */
export function toInputs(seconds: number): { date: string; time: string } {
  const d = new Date(seconds * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return {
    date: `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`,
    time: `${pad(d.getHours())}:${pad(d.getMinutes())}`,
  };
}

/** The next date (today included) that falls on `weekday` (0 = Sunday). */
export function next(weekday: number, now = new Date()): Date {
  const d = new Date(now);
  d.setDate(now.getDate() + ((weekday - now.getDay() + 7) % 7));
  return d;
}
