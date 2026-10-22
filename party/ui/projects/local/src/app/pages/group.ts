import { Component, computed, inject, input, OnInit, signal } from '@angular/core';
import { Router, RouterLink } from '@angular/router';
import {
  colour, count, EventList as Events, EventSummary, GroupView, initials, JOIN_LABEL, MemberView, PersonList, ROLE_LABEL,
  VISIBILITY_LABEL,
} from 'party-core';
import { Api, ApiError, problem } from '../api';
import { EventList } from '../event-list';
import { InvitePanel } from '../invite-panel';
import { value } from './auth';

/** One group: about, join/leave, members, and organizer tools. */
@Component({
  selector: 'party-group',
  imports: [RouterLink, EventList, InvitePanel],
  template: `
    @if (error()) { <p class="error card" role="alert">{{ error() }}</p> }
    @if (group(); as g) {
      <section class="card">
        <div class="bar">
          <h1><span class="avatar" [style.background]="colour(g.id)" aria-hidden="true">{{ initials(g.name) }}</span> {{ g.name }}</h1>
          @if (g.can_manage) { <a class="button secondary" [routerLink]="['/groups', g.slug, 'edit']">Edit</a> }
        </div>
        @if (g.status === 'archived') { <p class="status">Archived</p> }
        @if (g.about) { <p class="notes">{{ g.about }}</p> }
        <dl class="facts">
          <dt>Members</dt><dd>{{ count(g.members, 'member') }}</dd>
          <dt>Visibility</dt><dd>{{ visibilityLabel[g.visibility] }}</dd>
          <dt>Joining</dt><dd>{{ joinLabel[g.join] }}</dd>
          @if (g.my_role) { <dt>You</dt><dd>{{ roleLabel[g.my_role] }}</dd> }
        </dl>
        <div class="actions">
          @if (!g.my_status && g.join !== 'invite' && g.status === 'active') {
            <button type="button" (click)="join()" [disabled]="busy()">{{ g.join === 'open' ? 'Join' : 'Ask to join' }}</button>
          }
          @if (g.my_status === 'requested') {
            <span class="status">You asked to join</span>
            <button type="button" class="secondary" (click)="leave()" [disabled]="busy()">Withdraw</button>
          }
          @if (g.my_status === 'active') {
            <button type="button" class="secondary" (click)="leave()" [disabled]="busy()">Leave</button>
          }
        </div>
      </section>

      @if (g.my_status === 'active' || g.visibility === 'public' || g.can_manage) {
        <section class="card">
          <div class="bar">
            <h2>Coming up</h2>
            @if (g.can_create_events) {
              <div class="actions">
                <a class="button" [routerLink]="['/groups', g.slug, 'events', 'new']">New event</a>
                <a class="button secondary" [routerLink]="['/groups', g.slug, 'events', 'new']" [queryParams]="{ series: 1 }">Repeating</a>
              </div>
            }
          </div>
          <party-event-list [events]="events()" empty="Nothing planned yet." />
        </section>
      }

      @if (members().length) {
        <section class="card">
          <h2>People</h2>
          <ul class="accounts">
            @for (m of active(); track m.person) {
              <li>
                <div><strong>{{ m.display_name }}</strong> <small>{{ roleLabel[m.role] }}</small></div>
                @if (g.can_manage && m.person !== api.me()?.id) {
                  <div class="actions">
                    @if (m.role === 'member') { <button type="button" class="secondary" (click)="set(m, { role: 'co-organizer' })">Make co-organizer</button> }
                    @if (m.role === 'co-organizer') { <button type="button" class="secondary" (click)="set(m, { role: 'member' })">Make member</button> }
                    @if (g.can_own && m.role !== 'organizer') {
                      <button type="button" class="secondary" (click)="set(m, { role: 'organizer' })">Make organizer</button>
                      <button type="button" class="secondary" (click)="transfer(m)">Hand over</button>
                    }
                    @if (m.role !== 'organizer' || g.can_own) {
                      <button type="button" class="secondary" (click)="set(m, { status: 'removed' })">Remove</button>
                    }
                  </div>
                }
              </li>
            }
          </ul>
        </section>
      }

      @if (g.can_manage) {
        @if (requests().length) {
          <section class="card">
            <h2>Asking to join</h2>
            <ul class="accounts">
              @for (m of requests(); track m.person) {
                <li>
                  <strong>{{ m.display_name }}</strong>
                  <div class="actions">
                    <button type="button" (click)="set(m, { status: 'active' })">Let them in</button>
                    <button type="button" class="secondary" (click)="set(m, { status: 'removed' })">Turn down</button>
                  </div>
                </li>
              }
            </ul>
          </section>
        }
        <party-invite-panel kind="group" [target]="g.id" />
        <section class="card">
          <h2>Add someone</h2>
          <form (submit)="find($event)" role="search">
            <label>Name <input type="search" (input)="q.set(v($event))" /></label>
          </form>
          <ul class="accounts">
            @for (p of found(); track p.id) {
              <li><strong>{{ p.display_name }}</strong>
                <div class="actions"><button type="button" (click)="add(p.id)">Add</button></div></li>
            }
          </ul>
        </section>
        @if (g.can_own) {
          <section class="card">
            <h2>Organizer</h2>
            <label class="switch"><input type="checkbox" [checked]="g.members_create_events" (change)="membersCreate(!g.members_create_events)" />
              Members can create events, not just organizers</label>
            <div class="actions">
              <button type="button" class="secondary" (click)="archive()">{{ g.status === 'archived' ? 'Restore' : 'Archive' }}</button>
              <button type="button" class="secondary" (click)="remove()">Delete group</button>
            </div>
          </section>
        }
      }
    }
  `,
})
export class Group implements OnInit {
  readonly api = inject(Api);
  private readonly router = inject(Router);
  readonly ref = input<string>('');
  readonly group = signal<GroupView | null>(null);
  readonly members = signal<MemberView[]>([]);
  readonly events = signal<EventSummary[]>([]);
  readonly active = computed(() => this.members().filter((m) => m.status === 'active'));
  readonly requests = computed(() => this.members().filter((m) => m.status === 'requested'));
  readonly found = signal<{ id: number; display_name: string }[]>([]);
  readonly q = signal('');
  readonly busy = signal(false);
  readonly error = signal('');
  readonly colour = colour;
  readonly initials = initials;
  readonly count = count;
  readonly roleLabel = ROLE_LABEL;
  readonly visibilityLabel = VISIBILITY_LABEL;
  readonly joinLabel = JOIN_LABEL;
  readonly v = value;

  ngOnInit(): void {
    void this.load();
  }

  private path(rest = ''): string {
    return `/api/v1/groups/${this.group()?.id ?? this.ref()}${rest}`;
  }

  private async load(): Promise<void> {
    try {
      this.group.set(await this.api.get<GroupView>(this.path(), 'GroupView'));
      const list = await this.api.get<Events>(`/api/v1/events?group=${this.group()?.id}`, 'EventList').catch(() => null);
      this.events.set(list?.events ?? []);
      try {
        this.members.set((await this.api.get<{ members: MemberView[] }>(this.path('/members'), 'MemberList')).members);
      } catch (e) {
        // Not a member of a non-public group: the list is theirs to see only.
        if (!(e instanceof ApiError && e.status === 403)) throw e;
        this.members.set([]);
      }
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  private async act(run: () => Promise<unknown>): Promise<void> {
    this.busy.set(true);
    this.error.set('');
    try {
      await run();
      await this.load();
    } catch (e) {
      this.error.set(problem(e));
    } finally {
      this.busy.set(false);
    }
  }

  join(): Promise<void> {
    return this.act(() => this.api.send('POST', this.path('/join'), undefined, 'GroupView'));
  }

  leave(): Promise<void> {
    return this.act(() => this.api.send('POST', this.path('/leave'), undefined, 'GroupView'));
  }

  set(m: MemberView, change: { role?: string; status?: string }): Promise<void> {
    return this.act(() => this.api.send('PUT', this.path(`/members/${m.person}`), change, 'MemberList'));
  }

  add(person: number): Promise<void> {
    this.found.set([]);
    return this.act(() => this.api.send('PUT', this.path(`/members/${person}`), {}, 'MemberList'));
  }

  transfer(m: MemberView): Promise<void> {
    if (!confirm(`Make ${m.display_name} the organizer? You'll become a co-organizer.`)) return Promise.resolve();
    return this.act(() => this.api.send('POST', this.path('/transfer'), { to: m.person }, 'GroupView'));
  }

  membersCreate(on: boolean): Promise<void> {
    const g = this.group();
    if (!g) return Promise.resolve();
    return this.act(() => this.api.send('PATCH', this.path(), { members_create_events: on, rev: g.rev }, 'GroupView'));
  }

  archive(): Promise<void> {
    const g = this.group();
    if (!g) return Promise.resolve();
    const status = g.status === 'archived' ? 'active' : 'archived';
    return this.act(() => this.api.send('PATCH', this.path(), { status, rev: g.rev }, 'GroupView'));
  }

  async remove(): Promise<void> {
    if (!confirm('Delete this group for everyone? This cannot be undone.')) return;
    await this.act(() => this.api.send('DELETE', this.path(), undefined, 'Done'));
    if (!this.error()) await this.router.navigateByUrl('/groups');
  }

  async find(event: Event): Promise<void> {
    event.preventDefault();
    const here = new Set(this.members().filter((m) => m.status === 'active').map((m) => m.person));
    try {
      const page = await this.api.get<PersonList>(`/api/v1/people?q=${encodeURIComponent(this.q())}`, 'PersonList');
      this.found.set(page.people.filter((p) => !here.has(p.id)));
    } catch (e) {
      this.error.set(problem(e));
    }
  }
}
