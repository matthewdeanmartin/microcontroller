import { Component, inject, OnInit, signal } from '@angular/core';
import { colour, initials, PersonList, PersonView } from 'party-core';
import { Api, problem } from '../api';
import { value } from './auth';

/** People I may find: those who share a group with me, or chose "public". */
@Component({
  selector: 'party-people',
  template: `
    <h1>People</h1>
    <form class="card" role="search" (submit)="search($event)">
      <label>Find someone <input type="search" (input)="q.set(v($event))" /></label>
      <p class="aside">You see people you share a group or household with, and anyone who chose to be findable.</p>
    </form>
    @if (error()) { <p class="error card" role="alert">{{ error() }}</p> }
    <section class="card">
      <ul class="people">
        @for (p of people(); track p.id) {
          <li>
            <span class="avatar" [style.background]="colour(p.id)" aria-hidden="true">{{ initials(p.display_name) }}</span>
            <span><strong>{{ p.display_name }}</strong>
              @if (p.household_name) { <small>{{ p.household_name }}</small> }
              @if (p.bio) { <br /><span class="aside">{{ p.bio }}</span> }</span>
          </li>
        }
      </ul>
      @if (next()) { <button type="button" class="secondary" (click)="load(next())">More</button> }
    </section>
  `,
})
export class People implements OnInit {
  private readonly api = inject(Api);
  readonly q = signal('');
  readonly people = signal<PersonView[]>([]);
  readonly next = signal(0);
  readonly error = signal('');
  readonly colour = colour;
  readonly initials = initials;
  readonly v = value;

  ngOnInit(): void {
    void this.load(0);
  }

  search(event: Event): void {
    event.preventDefault();
    void this.load(0);
  }

  async load(after: number): Promise<void> {
    this.error.set('');
    const params = new URLSearchParams({ q: this.q(), after: String(after) });
    try {
      const page = await this.api.get<PersonList>(`/api/v1/people?${params}`, 'PersonList');
      this.people.update((p) => (after ? [...p, ...page.people] : page.people));
      this.next.set(page.next);
    } catch (e) {
      this.error.set(problem(e));
    }
  }
}
