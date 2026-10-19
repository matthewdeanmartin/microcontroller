import { Component, computed, inject, input, OnInit, signal } from '@angular/core';
import { Router, RouterLink } from '@angular/router';
import { Api, GameView, PlanInput, PlanView, problem } from '../api';
import { next, toInputs, toSeconds } from '../fmt';
import { nextTime } from '../plan-time';

interface TimeRow {
  /** Existing option's id (kept if unchanged), or 0 for a new one. */
  id: number;
  date: string;
  start: string;
  end: string;
}

function value(event: Event): string {
  return (event.target as HTMLInputElement).value;
}

/** New plan, or editing one (`/plans/:id/edit`). */
@Component({
  selector: 'gm-plan-form',
  imports: [RouterLink],
  template: `
    <section class="card">
      <h1>{{ planId() ? 'Edit / reschedule plan' : copy() ? 'Plan this again' : 'New plan' }}</h1>
      <form (submit)="submit($event)">
        <fieldset [disabled]="locked()">
          <legend>What could we play? <span class="aside">(1–8)</span></legend>
          @if (locked()) { <p class="aside">Reopen the plan to change its games. The confirmed time can be rescheduled below.</p> }
          <input type="search" placeholder="Find a game" aria-label="Find a game" (input)="query.set(v($event))" />
          <ul class="pick">
            @for (g of shown(); track g.id) {
              <li><label class="check"><input type="checkbox" [checked]="picked().includes(g.id)" (change)="toggle(g.id)" /> {{ g.title }}
                @if (g.mine) { <span aria-label="you're into it">★</span> }</label></li>
            } @empty {
              <li class="aside">No match. <a routerLink="/games">Add it to the catalog</a> first.</li>
            }
          </ul>
          @if (picked().length) {
            <p class="aside">Picked: {{ pickedTitles() }}</p>
          }
        </fieldset>

        <fieldset>
          <legend>When? <span class="aside">(optional, up to 6 options; leave empty for "who's up for it?")</span></legend>
          @if (!locked()) {
          <div class="chips">
            <button type="button" class="chip" (click)="quick('tonight')">Tonight 7 pm</button>
            <button type="button" class="chip" (click)="quick('sat')">Sat 2–6 pm</button>
            <button type="button" class="chip" (click)="quick('sun')">Sun 1 pm</button>
          </div>
          }
          @if (locked()) { <p class="aside">Change the confirmed time below. Saving sends updates to attendees.</p> }
          @for (t of times(); track $index; let i = $index) {
            <div class="row time">
              <label>Day <input type="date" required [value]="t.date" (input)="set(i, 'date', v($event))" /></label>
              <label>From <input type="time" required [value]="t.start" (input)="set(i, 'start', v($event))" /></label>
              <label>Until <input type="time" [value]="t.end" (input)="set(i, 'end', v($event))" /></label>
              @if (!locked()) { <button type="button" class="secondary" (click)="removeTime(i)" [attr.aria-label]="'Remove time ' + (i + 1)">✕</button> }
            </div>
          }
          @if (!locked() && times().length < 6) {
            <button type="button" class="secondary" (click)="addTime()">Add a time</button>
          }
        </fieldset>

        @if (!planId()) {
          <fieldset>
            <legend>Repeat this plan</legend>
            <label>Frequency <select [value]="repeat" (change)="repeat = v($event)">
              <option value="">Just once</option><option value="weekly">Weekly</option>
              <option value="fortnightly">Every two weeks</option><option value="monthly">Monthly</option>
            </select></label>
            @if (repeat) {
              <label>Total occurrences <input type="number" min="2" max="12" required [value]="repeatCount" (input)="repeatCount = +v($event)" /></label>
              <p class="aside">Creates separate plans in the household time zone. Edit or cancel any date as an exception.
                Monthly dates use the last day when a month is shorter. Everyone RSVPs separately for each occurrence.</p>
            }
          </fieldset>
        }
        <label>Title <span class="aside">(optional)</span> <input maxlength="48" [value]="title" (input)="title = v($event)" placeholder="Game night" /></label>
        <div class="row">
          <label>Where <input maxlength="64" [value]="place" (input)="place = v($event)" /></label>
          <label>Seats <span class="aside">(optional)</span> <input type="number" min="0" max="99" [value]="seats || ''" (input)="seats = +v($event)" /></label>
        </div>
        <label>Notes <textarea maxlength="500" rows="3" (input)="notes = v($event)">{{ notes }}</textarea></label>
        @if (api.status()?.bot; as bot) {
          <label class="switch"><input type="checkbox" [checked]="announce" (change)="announce = $any($event.target).checked" />
            Post it to Mastodon as {{ bot }}@if (!announced) { <span class="aside"> (and when it's locked in)</span> }</label>
        }
        @if (api.status()?.public_bot; as bot) {
          <label class="switch"><input type="checkbox" [checked]="announcePublic" (change)="announcePublic = $any($event.target).checked" />
            Also post this plan to {{ bot }} (outside the household network)</label>
        }
        @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
        <button type="submit" [disabled]="busy()">{{ planId() ? 'Save and send updates' : 'Post plan' }}</button>
      </form>
    </section>
  `,
})
export class PlanForm implements OnInit {
  /** Route param when editing. */
  readonly id = input<string>();
  /** `?game=` to start with one game picked. */
  readonly game = input<string>();
  readonly copy = input<string>();
  readonly api = inject(Api);
  private readonly router = inject(Router);
  readonly v = value;
  readonly games = signal<GameView[]>([]);
  readonly picked = signal<number[]>([]);
  readonly times = signal<TimeRow[]>([]);
  readonly query = signal('');
  readonly locked = signal(false);
  readonly error = signal('');
  readonly busy = signal(false);
  readonly planId = computed(() => Number(this.id() ?? 0));
  title = '';
  place = 'Home';
  seats = 0;
  notes = '';
  announce = false;
  announcePublic = false;
  repeat = "";
  repeatCount = 4;
  /** Already posted: unticking only stops the follow-ups. */
  announced = false;

  readonly shown = computed(() => {
    const q = this.query().trim().toLowerCase();
    const list = this.games().filter((g) => !q || g.title.toLowerCase().includes(q));
    // Your starred games first, then the rest; picked ones always visible.
    return list.sort((a, b) => Number(b.mine) - Number(a.mine) || a.title.localeCompare(b.title));
  });
  readonly pickedTitles = computed(() =>
    this.picked()
      .map((id) => this.games().find((g) => g.id === id)?.title ?? '?')
      .join(', '),
  );

  async ngOnInit(): Promise<void> {
    try {
      this.games.set((await this.api.get<{ games: GameView[] }>('/api/v1/games', 'GameList')).games);
      if (this.planId() || this.copy()) {
        const plan = await this.api.get<PlanView>(`/api/v1/plans/${this.planId() || Number(this.copy())}`, 'PlanView');
        this.title = plan.title;
        this.place = plan.place;
        this.seats = plan.seats;
        this.notes = plan.notes;
        this.announce = this.announced = !this.copy() && plan.announced;
        this.announcePublic = !this.copy() && plan.announced_public;
        this.locked.set(!this.copy() && plan.status === 'on');
        this.picked.set(plan.games.map((g) => g.id));
        this.times.set(
          (this.copy() ? [] : plan.times.filter((t) => !this.locked() || t.id === plan.chosen_time)).map((t) => {
            const s = toInputs(t.start);
            return { id: t.id, date: s.date, start: s.time, end: t.end ? toInputs(t.end).time : '' };
          }),
        );
      } else if (this.game()) {
        this.picked.set([Number(this.game())]);
      }
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  toggle(id: number): void {
    this.picked.update((p) => (p.includes(id) ? p.filter((x) => x !== id) : p.length < 8 ? [...p, id] : p));
  }

  addTime(row: Partial<TimeRow> = {}): void {
    if (this.times().length >= 6) return;
    this.times.update((t) => [...t, { ...nextTime(t.at(-1)), ...row }]);
  }

  removeTime(i: number): void {
    this.times.update((t) => t.filter((_, j) => j !== i));
  }

  set(i: number, field: 'date' | 'start' | 'end', value: string): void {
    // Changing a time makes it a new option (answers to the old one go).
    this.times.update((t) => t.map((row, j) => (j === i ? { ...row, [field]: value } : row)));
  }

  quick(which: 'tonight' | 'sat' | 'sun'): void {
    if (this.times().length >= 6) return;
    const at = (d: Date) => toInputs(d.getTime() / 1000).date;
    if (which === 'tonight') this.addTime({ date: at(new Date()), start: '19:00' });
    if (which === 'sat') this.addTime({ date: at(next(6)), start: '14:00', end: '18:00' });
    if (which === 'sun') this.addTime({ date: at(next(0)), start: '13:00' });
  }

  async submit(event: Event): Promise<void> {
    event.preventDefault();
    this.error.set('');
    if (!this.picked().length) return this.error.set('Pick at least one game.');
    const times = this.times().map((t) => {
      const start = toSeconds(t.date, t.start);
      let end = t.end ? toSeconds(t.date, t.end) : 0;
      if (end && end <= start) end += 86_400; // "8 pm until 1 am"
      return { id: t.id, start, end };
    });
    const body: PlanInput = { title: this.title, place: this.place, seats: this.seats || 0, notes: this.notes };
    if (this.api.status()?.bot) body.announce = this.announce;
    if (this.api.status()?.public_bot) body.announce_public = this.announcePublic;
    if (!this.planId() && this.repeat) body.repeat = { frequency: this.repeat, count: this.repeatCount };
    if (this.locked()) {
      body.reschedule = times[0];
    } else {
      body.games = this.picked();
      body.times = times;
    }
    this.busy.set(true);
    try {
      const plan = this.planId()
        ? await this.api.send<PlanView>('PATCH', `/api/v1/plans/${this.planId() || Number(this.copy())}`, body, 'PlanView')
        : await this.api.send<PlanView>('POST', '/api/v1/plans', body, 'PlanView');
      await this.router.navigate(['/plans', plan.id]);
    } catch (e) {
      this.error.set(problem(e));
    } finally {
      this.busy.set(false);
    }
  }
}
