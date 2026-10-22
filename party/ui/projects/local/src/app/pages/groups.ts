import { Component, inject, OnInit, signal } from '@angular/core';
import { RouterLink } from '@angular/router';
import { colour, count, GroupList, GroupView, initials } from 'party-core';
import { Api, problem } from '../api';
import { value } from './auth';

/** Search the groups I may see; a page at a time. */
@Component({
  selector: 'party-groups',
  imports: [RouterLink],
  template: `
    <div class="bar">
      <h1>Groups</h1>
      <a class="button" routerLink="/groups/new">Start a group</a>
    </div>
    <form class="card" role="search" (submit)="search($event)">
      <label>Find a group <input type="search" placeholder="Name or description" (input)="q.set(v($event))" /></label>
      <label class="switch"><input type="checkbox" [checked]="mine()" (change)="toggleMine()" /> Only mine</label>
    </form>
    @if (error()) { <p class="error card" role="alert">{{ error() }}</p> }
    <section class="card">
      @if (groups().length) {
        <ul class="groups">
          @for (g of groups(); track g.id) {
            <li>
              <span class="avatar" [style.background]="colour(g.id)" aria-hidden="true">{{ initials(g.name) }}</span>
              <a [routerLink]="['/groups', g.slug]">{{ g.name }}</a>
              <span class="aside">{{ count(g.members, 'member') }} · {{ g.visibility }}</span>
              @if (g.status === 'archived') { <span class="status">archived</span> }
            </li>
          }
        </ul>
        @if (next()) { <button type="button" class="secondary" (click)="more()" [disabled]="busy()">More</button> }
      } @else if (!busy()) {
        <p class="aside">No groups match.</p>
      }
    </section>
  `,
})
export class Groups implements OnInit {
  private readonly api = inject(Api);
  readonly q = signal('');
  readonly mine = signal(false);
  readonly groups = signal<GroupView[]>([]);
  readonly next = signal(0);
  readonly busy = signal(false);
  readonly error = signal('');
  readonly colour = colour;
  readonly initials = initials;
  readonly count = count;
  readonly v = value;

  ngOnInit(): void {
    void this.load(0);
  }

  search(event: Event): void {
    event.preventDefault();
    void this.load(0);
  }

  toggleMine(): void {
    this.mine.update((m) => !m);
    void this.load(0);
  }

  more(): void {
    void this.load(this.next());
  }

  private async load(after: number): Promise<void> {
    this.busy.set(true);
    this.error.set('');
    const params = new URLSearchParams({ q: this.q(), after: String(after) });
    if (this.mine()) params.set('mine', '1');
    try {
      const page = await this.api.get<GroupList>(`/api/v1/groups?${params}`, 'GroupList');
      this.groups.update((g) => (after ? [...g, ...page.groups] : page.groups));
      this.next.set(page.next);
    } catch (e) {
      this.error.set(problem(e));
    } finally {
      this.busy.set(false);
    }
  }
}
