import { Component, computed, inject, OnInit, signal } from '@angular/core';
import { RouterLink } from '@angular/router';
import { Api, ApiError, GameView, Kind, KINDS, problem } from '../api';
import { players } from '../fmt';

function value(event: Event): string {
  return (event.target as HTMLInputElement).value;
}

/** The catalog: search, filter, star what you're into, add a game. */
@Component({
  selector: 'gm-games',
  imports: [RouterLink],
  template: `
    <div class="bar">
      <h1>Games</h1>
      <button type="button" (click)="adding.set(!adding())">{{ adding() ? 'Close' : 'Add a game' }}</button>
    </div>

    @if (adding()) {
      <section class="card">
        <h2>Add a game</h2>
        <form (submit)="add($event)">
          <label>Title <input maxlength="48" required [value]="draft.title" (input)="draft.title = v($event)" /></label>
          <div class="row">
            <label>Kind
              <select (change)="draft.kind = $any(v($event))">
                @for (k of kinds; track k) { <option [value]="k" [selected]="k === draft.kind">{{ k }}</option> }
              </select></label>
            <label>Players <span class="pair">
              <input type="number" min="1" max="99" inputmode="numeric" aria-label="Fewest players" (input)="draft.min = +v($event)" />–
              <input type="number" min="1" max="99" inputmode="numeric" aria-label="Most players" (input)="draft.max = +v($event)" /></span></label>
            <label>Minutes <input type="number" min="0" max="1440" inputmode="numeric" (input)="draft.minutes = +v($event)" /></label>
          </div>
          <label>Notes <input maxlength="280" placeholder="needs the expansion box, 2 copies…" (input)="draft.notes = v($event)" /></label>
          @if (addError()) {
            <p class="error" role="alert">{{ addError() }}
              @if (existing()) { <a [routerLink]="['/games', existing()]">Open it</a> }</p>
          }
          <button type="submit">Add</button>
        </form>
      </section>
    }

    <section class="card">
      <input type="search" placeholder="Search" aria-label="Search games" (input)="query.set(v($event))" />
      <div class="chips" role="group" aria-label="Kind">
        <button type="button" class="chip" [class.on]="!kind()" (click)="kind.set(null)">all</button>
        @for (k of kinds; track k) {
          <button type="button" class="chip" [class.on]="kind() === k" (click)="kind.set(k)">{{ k }}</button>
        }
      </div>
      @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
      <ul class="games">
        @for (g of shown(); track g.id) {
          <li>
            <button type="button" class="star" [attr.aria-pressed]="g.mine" [attr.aria-label]="(g.mine ? 'Not into ' : 'Into ') + g.title"
              (click)="star(g)">{{ g.mine ? '★' : '☆' }}</button>
            <a [routerLink]="['/games', g.id]">{{ g.title }}</a>
            <span class="aside">{{ g.kind }}@if (players(g.min_players, g.max_players)) { · {{ players(g.min_players, g.max_players) }} }
              @if (g.interested) { · {{ g.interested }} into it }</span>
          </li>
        } @empty {
          <li class="aside">No games yet. Add the first one.</li>
        }
      </ul>
    </section>
  `,
})
export class Games implements OnInit {
  readonly api = inject(Api);
  readonly kinds = KINDS;
  readonly players = players;
  readonly v = value;
  readonly games = signal<GameView[]>([]);
  readonly query = signal('');
  readonly kind = signal<Kind | null>(null);
  readonly adding = signal(false);
  readonly error = signal('');
  readonly addError = signal('');
  readonly existing = signal<number | null>(null);
  draft = { title: '', kind: 'board' as Kind, min: 0, max: 0, minutes: 0, notes: '' };

  readonly shown = computed(() => {
    const q = this.query().trim().toLowerCase();
    return this.games().filter((g) => (!q || g.title.toLowerCase().includes(q)) && (!this.kind() || g.kind === this.kind()));
  });

  async ngOnInit(): Promise<void> {
    await this.load();
  }

  private async load(): Promise<void> {
    try {
      this.games.set((await this.api.get<{ games: GameView[] }>('/api/v1/games', 'GameList')).games);
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  async add(event: Event): Promise<void> {
    event.preventDefault();
    this.addError.set('');
    this.existing.set(null);
    const d = this.draft;
    try {
      await this.api.send<GameView>(
        'POST',
        '/api/v1/games',
        { title: d.title, kind: d.kind, min_players: d.min || 0, max_players: d.max || 0, minutes: d.minutes || 0, notes: d.notes },
        'GameView',
      );
      (event.target as HTMLFormElement).reset();
      this.draft = { title: '', kind: 'board', min: 0, max: 0, minutes: 0, notes: '' };
      this.adding.set(false);
      await this.load();
    } catch (e) {
      this.addError.set(problem(e));
      if (e instanceof ApiError && e.code === 'duplicate_title') {
        const match = this.games().find((g) => g.title.toLowerCase().replace(/[^a-z0-9]/g, '') === d.title.toLowerCase().replace(/[^a-z0-9]/g, ''));
        this.existing.set(match?.id ?? null);
      }
    }
  }

  async star(g: GameView): Promise<void> {
    try {
      const updated = await this.api.send<GameView>(g.mine ? 'DELETE' : 'PUT', `/api/v1/games/${g.id}/interest`, undefined, 'GameView');
      this.games.update((list) => list.map((x) => (x.id === g.id ? updated : x)));
    } catch (e) {
      this.error.set(problem(e));
    }
  }
}
