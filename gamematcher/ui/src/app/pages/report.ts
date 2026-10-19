import { Component, computed, inject, input, OnChanges, output, signal } from '@angular/core';
import { Api, PlanView, PostGameReport, problem } from '../api';

@Component({
  selector: 'gm-report',
  template: `
    @if (plan().report; as report) {
      <section class="card">
        <h2>Post-game report</h2>
        <p class="notes">{{ report.summary }}</p>
        <ul>@for (result of report.games; track result.game) {
          <li><strong>{{ gameName(result.game) }}</strong> —
            {{ result.winners.length ? 'Winner' + (result.winners.length > 1 ? 's' : '') + ': ' + names(result.winners) : 'Cooperative / no winner' }}</li>
        }</ul>
      </section>
    }
    @if (canReport()) {
      <section class="card">
        <details [open]="!plan().report">
          <summary>{{ plan().report ? 'Edit post-game report' : 'Write a post-game report' }}</summary>
          <form (submit)="save($event)">
            <label>How did it go?
              <textarea maxlength="1000" rows="4" [value]="summary" (input)="summary = $any($event.target).value"></textarea>
            </label>
            <p class="aside">Select the games played and their winners. Select several people for a tie or team; leave winners empty for cooperative play or no winner.</p>
            @for (g of plan().games; track g.id) {
              <fieldset>
                <legend><label class="check"><input type="checkbox" [checked]="played().includes(g.id)" (change)="toggleGame(g.id)" />{{ g.title }}</label></legend>
                @if (played().includes(g.id)) {
                  @for (person of people(); track person.id) {
                    <label class="check"><input type="checkbox" [checked]="(winners()[g.id] || []).includes(person.id)"
                      (change)="toggleWinner(g.id, person.id)" />{{ person.name }} won</label>
                  }
                }
              </fieldset>
            }
            <p class="aside">Saving marks this occurrence as played. Other dates in its series are unaffected.</p>
            @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
            <button type="submit" [disabled]="busy()">Save report</button>
            @if (notice()) { <p class="ok" role="status">{{ notice() }}</p> }
          </form>
        </details>
      </section>
    }
  `,
})
export class Report implements OnChanges {
  readonly plan = input.required<PlanView>();
  readonly saved = output<PlanView>();
  readonly api = inject(Api);
  readonly played = signal<number[]>([]);
  readonly winners = signal<Record<number, number[]>>({});
  readonly busy = signal(false);
  readonly error = signal('');
  readonly notice = signal('');
  summary = '';
  private loaded = '';
  readonly canReport = computed(() => {
    const p = this.plan();
    return p.reportable && (p.status === 'done' || p.times.some((t) => t.id === p.chosen_time && t.start <= Date.now() / 1000));
  });
  readonly people = computed(() => {
    const p = this.plan();
    return [{ id: p.host, name: p.host_name }, ...p.responses
      .filter((r) => r.account !== p.host && r.overall === 'in')
      .map((r) => ({ id: r.account, name: r.name }))];
  });

  ngOnChanges(): void {
    const p = this.plan();
    const key = JSON.stringify([p.id, p.report]);
    if (key === this.loaded) return;
    this.loaded = key;
    this.summary = p.report?.summary ?? '';
    this.played.set(p.report?.games.map((r) => r.game) ?? p.chosen_games);
    this.winners.set(Object.fromEntries(p.report?.games.map((r) => [r.game, r.winners]) ?? []));
    this.error.set('');
  }
  gameName(id: number): string { return this.plan().games.find((g) => g.id === id)?.title ?? 'Game'; }
  names(ids: number[]): string { return ids.map((id) => this.people().find((p) => p.id === id)?.name ?? 'Former attendee').join(', '); }
  toggleGame(id: number): void {
    this.played.update((list) => list.includes(id) ? list.filter((g) => g !== id) : [...list, id]);
  }
  toggleWinner(game: number, person: number): void {
    this.winners.update((all) => {
      const list = all[game] ?? [];
      return { ...all, [game]: list.includes(person) ? list.filter((id) => id !== person) : [...list, person] };
    });
  }
  async save(event: Event): Promise<void> {
    event.preventDefault();
    this.busy.set(true); this.error.set(''); this.notice.set('');
    const body: PostGameReport = { summary: this.summary, games: this.played().map((game) => ({ game, winners: this.winners()[game] ?? [] })) };
    try {
      this.saved.emit(await this.api.send<PlanView>('PUT', `/api/v1/plans/${this.plan().id}/report`, body, 'PlanView'));
      this.notice.set('Report saved.');
    } catch (e) { this.error.set(problem(e)); }
    finally { this.busy.set(false); }
  }
}
