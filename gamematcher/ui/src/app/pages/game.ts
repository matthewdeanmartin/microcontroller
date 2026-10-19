import { Component, inject, input, OnInit, signal } from '@angular/core';
import { Router, RouterLink } from '@angular/router';
import { Api, GameDetail, GameView, Kind, KINDS, problem } from '../api';
import { players, when } from '../fmt';

function value(event: Event): string {
  return (event.target as HTMLInputElement).value;
}

/** One game: who's into it, its plans, and (for its adder or an admin) editing. */
@Component({
  selector: 'gm-game',
  imports: [RouterLink],
  template: `
    @if (detail(); as d) {
      <section class="card">
        <div class="bar">
          <h1>{{ d.game.title }} @if (d.game.archived) { <small>archived</small> }</h1>
          <button type="button" class="secondary" [attr.aria-pressed]="d.game.mine" (click)="star()">
            {{ d.game.mine ? '★ Into it' : '☆ I\\'m into this' }}</button>
        </div>
        <p class="aside">{{ d.game.kind }}@if (players(d.game.min_players, d.game.max_players)) { · {{ players(d.game.min_players, d.game.max_players) }} }
          @if (d.game.minutes) { · about {{ d.game.minutes }} min } · added by {{ d.game.added_by_name }}</p>
        @if (d.game.notes) { <p>{{ d.game.notes }}</p> }
        <p>
          @if (d.interested.length) {
            Into it: @for (u of d.interested; track u.id; let last = $last) { {{ u.display_name }}@if (!last) {, } }
          } @else {
            <span class="aside">Nobody has starred it yet.</span>
          }
        </p>
        <a class="button" routerLink="/plans/new" [queryParams]="{ game: d.game.id }">Plan to play it</a>
      </section>

      @if (d.plans.length) {
        <section class="card">
          <h2>Plans</h2>
          <ul class="plans">
            @for (p of d.plans; track p.id) {
              <li><a [routerLink]="['/plans', p.id]">{{ p.title || p.games.join(', ') }}</a>
                <span class="aside">{{ p.status }} · {{ when(p.start, p.end) }}</span></li>
            }
          </ul>
        </section>
      }

      @if (d.game.editable) {
        <section class="card">
          <h2>Edit</h2>
          <form (submit)="save($event)">
            <label>Title <input maxlength="48" required [value]="d.game.title" (input)="edit.title = v($event)" /></label>
            <div class="row">
              <label>Kind
                <select (change)="edit.kind = $any(v($event))">
                  @for (k of kinds; track k) { <option [value]="k" [selected]="k === d.game.kind">{{ k }}</option> }
                </select></label>
              <label>Players <span class="pair">
                <input type="number" min="0" max="99" aria-label="Fewest players" [value]="d.game.min_players || ''" (input)="edit.min_players = +v($event)" />–
                <input type="number" min="0" max="99" aria-label="Most players" [value]="d.game.max_players || ''" (input)="edit.max_players = +v($event)" /></span></label>
              <label>Minutes <input type="number" min="0" max="1440" [value]="d.game.minutes || ''" (input)="edit.minutes = +v($event)" /></label>
            </div>
            <label>Notes <input maxlength="280" [value]="d.game.notes" (input)="edit.notes = v($event)" /></label>
            <div class="actions">
              <button type="submit">Save</button>
              <button type="button" class="secondary" (click)="archive(!d.game.archived)">{{ d.game.archived ? 'Unarchive' : 'Archive' }}</button>
            </div>
          </form>
        </section>
      }

      @if (api.me()?.role === 'admin') {
        <section class="card">
          <h2>Admin</h2>
          <form class="row" (submit)="merge($event)">
            <label>Merge this duplicate into
              <select (change)="mergeInto = +v($event)">
                <option value="0">pick a game…</option>
                @for (g of others(); track g.id) { <option [value]="g.id">{{ g.title }}</option> }
              </select></label>
            <button type="submit" class="secondary" [disabled]="!mergeInto">Merge</button>
          </form>
          <button type="button" class="secondary" (click)="remove()">Delete game</button>
          <p class="aside">A game some plan uses is archived instead of deleted.</p>
        </section>
      }
    }
    @if (notice()) { <p class="ok" role="status">{{ notice() }}</p> }
    @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
  `,
})
export class Game implements OnInit {
  readonly id = input.required<string>();
  readonly api = inject(Api);
  private readonly router = inject(Router);
  readonly kinds = KINDS;
  readonly players = players;
  readonly when = when;
  readonly v = value;
  readonly detail = signal<GameDetail | null>(null);
  readonly others = signal<GameView[]>([]);
  readonly notice = signal('');
  readonly error = signal('');
  edit: { title?: string; kind?: Kind; min_players?: number; max_players?: number; minutes?: number; notes?: string } = {};
  mergeInto = 0;

  async ngOnInit(): Promise<void> {
    await this.load();
    if (this.api.me()?.role === 'admin') {
      const all = await this.api.get<{ games: GameView[] }>('/api/v1/games?archived=1', 'GameList').catch(() => ({ games: [] }));
      this.others.set(all.games.filter((g) => g.id !== +this.id()));
    }
  }

  private async load(): Promise<void> {
    try {
      this.detail.set(await this.api.get<GameDetail>(`/api/v1/games/${this.id()}`, 'GameDetail'));
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  private async run(work: () => Promise<unknown>, done: string): Promise<void> {
    this.error.set('');
    this.notice.set('');
    try {
      await work();
      this.notice.set(done);
      await this.load();
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  star(): Promise<void> {
    const mine = this.detail()?.game.mine;
    return this.run(() => this.api.send(mine ? 'DELETE' : 'PUT', `/api/v1/games/${this.id()}/interest`, undefined, 'GameView'), '');
  }

  save(event: Event): Promise<void> {
    event.preventDefault();
    return this.run(() => this.api.send('PATCH', `/api/v1/games/${this.id()}`, this.edit, 'GameView'), 'Saved.');
  }

  archive(archived: boolean): Promise<void> {
    return this.run(() => this.api.send('PATCH', `/api/v1/games/${this.id()}`, { archived }, 'GameView'), archived ? 'Archived.' : 'Back in the catalog.');
  }

  async merge(event: Event): Promise<void> {
    event.preventDefault();
    if (!this.mergeInto || !confirm('Merge this game into the one you picked? Plans and stars move over.')) return;
    try {
      await this.api.send('POST', `/api/v1/admin/games/${this.id()}/merge`, { into: this.mergeInto }, 'Done');
      await this.router.navigate(['/games', this.mergeInto]);
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  async remove(): Promise<void> {
    if (!confirm('Delete this game?')) return;
    try {
      const done = await this.api.send<{ ok: boolean }>('DELETE', `/api/v1/admin/games/${this.id()}`, undefined, 'Done');
      if (done.ok) await this.router.navigateByUrl('/games');
      else {
        this.notice.set('A plan uses it, so it was archived instead.');
        await this.load();
      }
    } catch (e) {
      this.error.set(problem(e));
    }
  }
}
