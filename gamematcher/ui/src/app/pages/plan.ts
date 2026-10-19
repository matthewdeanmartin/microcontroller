import { Component, computed, effect, inject, input, OnDestroy, OnInit, signal, untracked } from '@angular/core';
import { RouterLink } from '@angular/router';
import { Answer, Api, ApiError, Overall, PlanGame, PlanView, problem, UserSummary } from '../api';
import { players, when } from '../fmt';
import { Thread } from './thread';
import { Report } from './report';
import { rankTimes } from '../best-time';
import { canShare, planInvite, share, shareTargets } from '../invite';

const ANSWERS: { value: Answer; label: string }[] = [
  { value: 'yes', label: 'Yes' },
  { value: 'if_needed', label: 'If needed' },
  { value: 'no', label: 'No' },
];

/**
 * The plan screen. Everyone answers (in/maybe/out, each time, each game);
 * the board shows who said what and never picks. The host locks it in.
 */
@Component({
  selector: 'gm-plan',
  imports: [RouterLink, Thread, Report],
  template: `
    @if (plan(); as p) {
      <section class="card">
        <div class="bar">
          <h1>{{ heading() }}</h1>
          <span class="status" [class]="p.status">{{ statusText() }}</span>
        </div>
        <p class="hostline">
          <span class="host-chip"><span class="avatar small" [style.background]="colour(p.host)" aria-hidden="true">{{ p.host_name[0] }}</span>
            👑 {{ p.host_name }} is hosting</span>
          <span class="aside">{{ p.place }}@if (p.seats) { · {{ p.seats }} seats }</span>
        </p>
        @if (chosen(); as t) {
          <p class="locked">🎲 {{ when(t.start, t.end) }}@if (chosenGames()) { · {{ chosenGames() }} }</p>
        }
        @if (p.notes) { <p class="notes">{{ p.notes }}</p> }
        @if (p.times.length) {
          <a class="aside" [href]="'/api/v1/plans/' + p.id + '/calendar.ics'" download>📅 Add to my calendar</a>
        }
      </section>

      @if (open()) {
        <section class="card">
          <h2>Are you in?</h2>
          <div class="segmented" role="radiogroup" aria-label="Are you in?">
            @for (o of overalls; track o) {
              <button type="button" role="radio" [attr.aria-checked]="overall() === o" [class.on]="overall() === o" (click)="overall.set(o); changed()">
                {{ o === 'in' ? 'In' : o === 'maybe' ? 'Maybe' : 'Out' }}</button>
            }
          </div>
        </section>
      }

      @if (p.status === 'open' && p.times.length > 1) {
        <section class="card">
          <h2>Best times</h2>
          <p class="aside">Ranked by definite attendance, then flexibility. Missing answers are unknown; the host still chooses.</p>
          <ol class="answers">
            @for (r of rankings(); track r.time.id) {
              <li>
                <div class="what">{{ when(r.time.start, r.time.end) }}
                  <span class="aside">{{ r.yes.length }} yes · {{ r.flexible.length }} flexible · {{ r.unknown.length }} unanswered</span>
                  @if (r.no.length) { <span class="aside">Cannot make it: {{ names(r.no) }}</span> }
                  @if (r.hostUnavailable) { <span class="error">Host unavailable</span> }
                </div>
                @if (p.manageable) {
                  <button type="button" class="secondary" (click)="lockTime = r.time.id">{{ lockTime === r.time.id ? 'Chosen below' : 'Choose this time' }}</button>
                }
              </li>
            }
          </ol>
        </section>
      }
      @if (p.times.length) {
        <section class="card">
          <h2>{{ p.status === 'open' ? 'When can you do?' : 'When' }}</h2>
          <ul class="answers">
            @for (t of p.times; track t.id) {
              <li [class.chosen]="t.id === p.chosen_time">
                <div class="what">{{ when(t.start, t.end) }}
                  <span class="aside">
                    @if (t.yes.length) { ✔ {{ names(t.yes) }} }
                    @if (t.if_needed.length) { · if needed: {{ names(t.if_needed) }} }
                    @if (t.no.length) { · can't: {{ names(t.no) }} }
                  </span>
                </div>
                @if (open()) {
                  <div class="segmented small" role="radiogroup" [attr.aria-label]="'Can you do ' + when(t.start, t.end) + '?'">
                    @for (a of answers; track a.value) {
                      <button type="button" role="radio" [attr.aria-checked]="timeAnswer(t.id) === a.value"
                        [class.on]="timeAnswer(t.id) === a.value" (click)="setTime(t.id, a.value); changed()">{{ a.label }}</button>
                    }
                  </div>
                }
              </li>
            }
          </ul>
        </section>
      } @else {
        <section class="card"><p class="aside">No times yet: this one's a "who's up for it?". Say if you're in and what you'd play;
          the host will add times.</p></section>
      }

      <section class="card">
        <h2>{{ open() ? 'What would you play? (tick all)' : 'Games' }}</h2>
        <ul class="answers">
          @for (g of p.games; track g.id) {
            <li [class.dim]="tooMany(g) || tooFew(g)" [class.chosen]="p.chosen_games.includes(g.id)">
              @if (open()) {
                <label class="check"><input type="checkbox" [checked]="games().includes(g.id)" (change)="toggleGame(g.id); changed()" /> {{ g.title }}</label>
              } @else {
                <span>{{ g.title }}</span>
              }
              <span class="aside">
                @if (g.ticks.length) { {{ names(g.ticks) }} }
                @if (players(g.min_players, g.max_players)) { · {{ players(g.min_players, g.max_players) }} }
                @if (tooFew(g)) { · needs {{ g.min_players }}+ }
                @if (tooMany(g)) { · too many for it }
              </span>
            </li>
          }
        </ul>
      </section>

      @if (open()) {
        <section class="card">
          <label>Note <span class="aside">(optional)</span>
            <input maxlength="140" [value]="note()" (input)="note.set(v($event))" (change)="changed(0)" placeholder="only after 3, bringing snacks…" /></label>
          @if (saveError()) { <p class="error" role="alert">{{ saveError() }}</p> }
          <p class="aside saved" role="status">{{ busy() ? 'Saving…' : saved() }}</p>
        </section>
      }

      <section class="card">
        <h2>Who's answered</h2>
        <ul class="people">
          @for (r of p.responses; track r.account) {
            <li><strong>{{ r.name }}</strong> <small>{{ r.overall }}</small>@if (r.account === p.host) { <small class="host-tag">👑 host</small> }@if (r.note) { <span class="aside">“{{ r.note }}”</span> }</li>
          } @empty {
            <li class="aside">Nobody yet.</li>
          }
        </ul>
      </section>

      @if (open()) {
        <section class="card">
          <h2>Invite</h2>
          <p class="aside">Send it from your own email or messages: the link works on the home Wi-Fi.</p>
          <div class="actions">
            @for (t of targets(); track t.id) {
              <a class="button secondary" [href]="t.href" target="_blank" rel="noopener">{{ t.label }}</a>
            }
            @if (shareable) { <button type="button" class="secondary" (click)="shareInvite()">More…</button> }
            <button type="button" class="secondary" (click)="copyInvite()">Copy</button>
          </div>
          @if (copied()) { <p class="ok" role="status">{{ copied() }}</p> }
          @if (api.status()?.mail) {
            <form class="row" (submit)="emailInvite($event)">
              <label>Or let the board email it <input type="text" inputmode="email" placeholder="gran@example.com, uncle@example.org"
                [value]="inviteTo()" (input)="inviteTo.set($any($event.target).value)" /></label>
              <button type="submit" class="secondary" [disabled]="!inviteTo().trim()">Send</button>
            </form>
          }
        </section>
      }

      <section class="card">
        <a class="button secondary" routerLink="/plans/new" [queryParams]="{ copy: p.id }">Plan this again</a>
      </section>
      @if (p.series.length > 1) {
        <section class="card"><h2>Recurring series</h2><p class="aside">Each occurrence can be edited or cancelled independently.</p>
          <div class="actions">@for (id of p.series; track id; let i = $index) {
            <a [routerLink]="['/plans', id]">{{ id === p.id ? 'This plan' : 'Occurrence ' + (i + 1) }}</a>
          }</div>
        </section>
      }
      <gm-report [plan]="p" (saved)="show($event)" />
      <gm-thread [plan]="p.id" />

      @if (p.manageable) {
        <section class="card">
          <h2>Host</h2>
          @if (p.status === 'open' && p.times.length) {
            <form (submit)="lock($event)">
              <fieldset>
                <legend>Lock it in: when?</legend>
                @for (t of p.times; track t.id) {
                  <label class="check"><input type="radio" name="lock-time" [value]="t.id" [checked]="lockTime === t.id" (change)="lockTime = t.id" />
                    {{ when(t.start, t.end) }} <span class="aside">({{ t.yes.length }} yes, {{ t.if_needed.length }} if needed)</span></label>
                }
              </fieldset>
              <fieldset>
                <legend>Which games? <span class="aside">(none = decide on the night)</span></legend>
                @for (g of p.games; track g.id) {
                  <label class="check"><input type="checkbox" (change)="toggleLockGame(g.id)" /> {{ g.title }}
                    <span class="aside">({{ g.ticks.length }} would play)</span></label>
                }
              </fieldset>
              <button type="submit" [disabled]="!lockTime">Lock it in</button>
            </form>
          }
          <div class="actions">
            <a class="button secondary" [routerLink]="['/plans', p.id, 'edit']">Edit / reschedule</a>
            @if (p.status === 'on') { <button type="button" class="secondary" (click)="act('reopen')">Reopen</button> }
            <button type="button" class="secondary" (click)="cancel()">Cancel plan</button>
          </div>
          <form class="row" (submit)="handOver($event)">
            <label>Hand it over to
              <select (change)="handTo = +v($event)">
                <option value="0">pick someone…</option>
                @for (u of others(); track u.id) { <option [value]="u.id">{{ u.display_name }}</option> }
              </select></label>
            <button type="submit" class="secondary" [disabled]="!handTo">Hand over</button>
          </form>
          @if (hostError()) { <p class="error" role="alert">{{ hostError() }}</p> }
        </section>
      }
    } @else if (error()) {
      <p class="error card" role="alert">{{ error() }}</p>
    }
  `,
})
export class Plan implements OnInit, OnDestroy {
  constructor() {
    effect(() => {
      this.id();
      untracked(() => {
        clearTimeout(this.pending);
        this.lockTime = 0; this.lockGames = [];
        void this.load();
      });
    });
  }
  ngOnDestroy(): void { clearTimeout(this.pending); }
  readonly id = input.required<string>();
  readonly api = inject(Api);
  readonly when = when;
  readonly players = players;
  readonly answers = ANSWERS;
  readonly overalls: Overall[] = ['in', 'maybe', 'out'];
  readonly plan = signal<PlanView | null>(null);
  readonly users = signal<UserSummary[]>([]);
  readonly rankings = computed(() => rankTimes(this.plan()));
  readonly overall = signal<Overall | null>(null);
  readonly times = signal<Record<number, Answer>>({});
  readonly games = signal<number[]>([]);
  readonly note = signal('');
  readonly busy = signal(false);
  readonly error = signal('');
  readonly saveError = signal('');
  private pending?: ReturnType<typeof setTimeout>;

  /** Answers save as you go: a moment after the last tap. */
  changed(delay = 400): void {
    clearTimeout(this.pending);
    this.pending = setTimeout(() => void this.save(), delay);
  }

  colour(id: number): string {
    return `hsl(${(id * 137) % 360} 55% 45%)`;
  }
  readonly saved = signal('');
  readonly hostError = signal('');
  readonly copied = signal('');
  readonly inviteTo = signal('');
  readonly shareable = canShare();
  readonly invite = computed(() => {
    const p = this.plan();
    return p ? planInvite(p, globalThis.location.origin, this.api.me()?.display_name ?? 'Someone') : null;
  });
  readonly targets = computed(() => {
    const i = this.invite();
    return i ? shareTargets(i, this.mastodonHome()) : [];
  });
  lockTime = 0;
  lockGames: number[] = [];
  handTo = 0;

  readonly open = computed(() => {
    const s = this.plan()?.status;
    return s === 'open' || s === 'on';
  });
  readonly heading = computed(() => {
    const p = this.plan();
    if (!p) return '';
    return p.title || (p.games.length === 1 ? p.games[0].title : 'Game night');
  });
  readonly statusText = computed(
    () => ({ open: 'deciding', on: 'on!', done: 'played', cancelled: 'cancelled' })[this.plan()?.status ?? 'open'],
  );
  readonly chosen = computed(() => {
    const p = this.plan();
    return p?.times.find((t) => t.id === p.chosen_time) ?? null;
  });
  readonly chosenGames = computed(() => {
    const p = this.plan();
    return p ? p.games.filter((g) => p.chosen_games.includes(g.id)).map((g) => g.title).join(', ') : '';
  });
  /** People who said "in": the count the player-range hints use. */
  readonly inCount = computed(() => this.plan()?.responses.filter((r) => r.overall === 'in').length ?? 0);
  readonly others = computed(() => this.users().filter((u) => u.id !== this.plan()?.host));

  async ngOnInit(): Promise<void> {
    this.api
      .get<{ users: UserSummary[] }>('/api/v1/users', 'UserList')
      .then((u) => this.users.set(u.users))
      .catch(() => undefined);
  }

  private async load(): Promise<void> {
    const id = this.id();
    this.plan.set(null); this.error.set('');
    try {
      const plan = await this.api.get<PlanView>(`/api/v1/plans/${id}`, 'PlanView');
      if (this.id() === id) this.show(plan);
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  /** Shows a plan and fills the form with my current answer. */
  show(p: PlanView): void {
    this.plan.set(p);
    const mine = p.responses.find((r) => r.account === this.api.me()?.id);
    this.overall.set(mine?.overall ?? null);
    this.times.set(Object.fromEntries((mine?.times ?? []).map((a) => [a.time, a.answer])));
    this.games.set(mine?.games ?? []);
    this.note.set(mine?.note ?? '');
  }

  v(event: Event): string {
    return (event.target as HTMLInputElement).value;
  }

  names(ids: number[]): string {
    const p = this.plan();
    return ids
      .map((id) => p?.responses.find((r) => r.account === id)?.name ?? this.users().find((u) => u.id === id)?.display_name ?? (id === p?.host ? p.host_name : '?'))
      .join(', ');
  }

  timeAnswer(id: number): Answer | undefined {
    return this.times()[id];
  }

  setTime(id: number, answer: Answer): void {
    this.times.update((t) => ({ ...t, [id]: answer }));
  }

  toggleGame(id: number): void {
    this.games.update((g) => (g.includes(id) ? g.filter((x) => x !== id) : [...g, id]));
  }

  toggleLockGame(id: number): void {
    this.lockGames = this.lockGames.includes(id) ? this.lockGames.filter((x) => x !== id) : [...this.lockGames, id];
  }

  tooFew(g: PlanGame): boolean {
    return this.inCount() > 0 && g.min_players > 0 && this.inCount() < g.min_players;
  }

  tooMany(g: PlanGame): boolean {
    return g.max_players > 0 && this.inCount() > g.max_players;
  }

  /** Post from the person's own instance if linked, else mastodon.social. */
  private mastodonHome(): string {
    const host = this.api.me()?.mastodon.split('@')[2];
    return host ? `https://${host}` : 'https://mastodon.social';
  }

  async shareInvite(): Promise<void> {
    const i = this.invite();
    if (i) await share(i).catch(() => undefined);
  }

  async emailInvite(event: Event): Promise<void> {
    event.preventDefault();
    const p = this.plan();
    if (!p) return;
    try {
      const r = await this.api.send<{ sent: number }>('POST', `/api/v1/plans/${p.id}/invite`, { emails: [this.inviteTo()] }, 'Sent');
      this.copied.set(r.sent ? `Sent ${r.sent} ${r.sent === 1 ? 'invite' : 'invites'}.` : "Today's email allowance is used up; use your own email instead.");
      if (r.sent) this.inviteTo.set('');
    } catch (e) {
      this.copied.set(problem(e));
    }
  }

  async copyInvite(): Promise<void> {
    const i = this.invite();
    if (!i) return;
    try {
      await navigator.clipboard.writeText(`${i.subject}

${i.body}`);
      this.copied.set('Copied. Paste it into a message.');
    } catch {
      this.copied.set("Your browser wouldn't copy; use Email instead.");
    }
  }

  async save(): Promise<void> {
    // Answering a time or a game before In/Maybe/Out: a yes means in.
    if (!this.overall()) this.overall.set(Object.values(this.times()).includes('yes') ? 'in' : 'maybe');
    const planId = this.id();
    this.busy.set(true);
    this.saveError.set('');
    this.saved.set('');
    const body = {
      overall: this.overall(),
      times: Object.entries(this.times()).map(([time, answer]) => ({ time: Number(time), answer })),
      games: this.games(),
      note: this.note(),
    };
    try {
      // Refresh counts and names only: taps made while this was in flight
      // stay in the form and go out with the next save.
      const updated = await this.api.send<PlanView>('PUT', `/api/v1/plans/${planId}/response`, body, 'PlanView');
      if (this.id() !== planId) return;
      this.plan.set(updated);
      this.saved.set('Saved ✓');
    } catch (e) {
      if (e instanceof ApiError && e.code === 'plan_changed') {
        await this.load();
        this.saveError.set('The host changed the plan while you were answering. Here it is now; please answer again.');
      } else {
        this.saveError.set(problem(e));
      }
    } finally {
      this.busy.set(false);
    }
  }

  private async host(work: () => Promise<PlanView>): Promise<void> {
    this.hostError.set('');
    try {
      this.show(await work());
    } catch (e) {
      this.hostError.set(problem(e));
    }
  }

  lock(event: Event): Promise<void> {
    event.preventDefault();
    return this.host(() =>
      this.api.send<PlanView>('POST', `/api/v1/plans/${this.id()}/lock`, { time: this.lockTime, games: this.lockGames }, 'PlanView'),
    );
  }

  act(action: 'reopen' | 'cancel'): Promise<void> {
    return this.host(() => this.api.send<PlanView>('POST', `/api/v1/plans/${this.id()}/${action}`, undefined, 'PlanView'));
  }

  cancel(): Promise<void> {
    return confirm('Cancel this plan for everyone?') ? this.act('cancel') : Promise.resolve();
  }

  handOver(event: Event): Promise<void> {
    event.preventDefault();
    return this.host(() => this.api.send<PlanView>('POST', `/api/v1/plans/${this.id()}/host`, { user: this.handTo }, 'PlanView'));
  }
}
