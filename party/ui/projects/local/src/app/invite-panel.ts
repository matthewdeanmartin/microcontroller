import { Component, inject, input, OnInit, signal } from '@angular/core';
import { day, InviteView } from 'party-core';
import { Api, problem } from './api';
import { value } from './pages/auth';

/**
 * Invite people to a group or event: by email, by Mastodon handle, as a
 * link to copy, or by adding someone who has no account (an email-only
 * relative). For organizers and hosts.
 */
@Component({
  selector: 'party-invite-panel',
  template: `
    <section class="card">
      <h2>Invite people</h2>
      <form (submit)="send($event)">
        <div class="row">
          <label>Email <small>optional</small> <input type="email" [value]="email()" (input)="email.set(v($event))" /></label>
          <label>Mastodon <small>optional</small> <input placeholder="@name@instance.social" [value]="handle()" (input)="handle.set(v($event))" /></label>
        </div>
        <div class="row">
          <label>Works for <select [value]="days()" (change)="days.set(+v($event))">
            <option value="7">a week</option><option value="14">two weeks</option><option value="30">a month</option></select></label>
          <label>Uses <input type="number" min="1" max="50" [value]="uses()" (input)="uses.set(+v($event))" /></label>
        </div>
        <button type="submit" [disabled]="busy()">Make an invitation</button>
      </form>
      @if (made(); as m) {
        <p class="ok" role="status">
          @if (m.emailed) { Emailed. } @if (m.messaged) { Sent by Mastodon DM. }
          Link (share it yourself, or show it on a phone at the house):</p>
        <p class="feed">{{ m.link }}</p>
        <button type="button" class="secondary" (click)="copy(m.link)">Copy link</button>
      }
      <details>
        <summary>Someone without an account (email-only)</summary>
        <form (submit)="addPerson($event)">
          <label>Name <input maxlength="40" required [value]="name()" (input)="name.set(v($event))" /></label>
          <label>Email <small>for invitations and calendar entries</small> <input type="email" [value]="personEmail()" (input)="personEmail.set(v($event))" /></label>
          <button type="submit" class="secondary" [disabled]="busy()">Add and invite</button>
        </form>
        <p class="aside">They get emails (with calendar invites for events) and can be answered for. They can claim the account later from their invitation.</p>
      </details>
      @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
      @if (invites().length) {
        <h3>Invitations</h3>
        <ul class="accounts">
          @for (i of invites(); track i.id) {
            <li [class.disabled]="i.status !== 'open'">
              <div>{{ i.email || i.handle || 'Link' }} <small>{{ i.status }} · until {{ day(i.expires) }} · {{ i.uses_left }} use(s) left</small></div>
              @if (i.status === 'open') { <div class="actions"><button type="button" class="secondary" (click)="revoke(i.id)">Withdraw</button></div> }
            </li>
          }
        </ul>
      }
    </section>
  `,
})
export class InvitePanel implements OnInit {
  private readonly api = inject(Api);
  readonly kind = input.required<'group' | 'event'>();
  readonly target = input.required<number>();
  readonly email = signal('');
  readonly handle = signal('');
  readonly days = signal(14);
  readonly uses = signal(1);
  readonly name = signal('');
  readonly personEmail = signal('');
  readonly made = signal<InviteView | null>(null);
  readonly invites = signal<InviteView[]>([]);
  readonly error = signal('');
  readonly busy = signal(false);
  readonly day = day;
  readonly v = value;

  ngOnInit(): void {
    void this.load();
  }

  private async load(): Promise<void> {
    try {
      const list = await this.api.get<{ invites: InviteView[] }>(`/api/v1/invitations?kind=${this.kind()}&target=${this.target()}`, 'InviteList');
      this.invites.set(list.invites.reverse());
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  private async run(f: () => Promise<void>): Promise<void> {
    this.busy.set(true);
    this.error.set('');
    try {
      await f();
      await this.load();
    } catch (e) {
      this.error.set(problem(e));
    } finally {
      this.busy.set(false);
    }
  }

  send(event: Event): Promise<void> {
    event.preventDefault();
    return this.run(async () => {
      const body = { target_kind: this.kind(), target: this.target(), email: this.email(), handle: this.handle(), person: 0, days: this.days(), uses: this.uses() };
      this.made.set(await this.api.send<InviteView>('POST', '/api/v1/invitations', body, 'InviteView'));
      this.email.set('');
      this.handle.set('');
    });
  }

  addPerson(event: Event): Promise<void> {
    event.preventDefault();
    return this.run(async () => {
      const p = await this.api.send<{ id: number }>('POST', '/api/v1/people', { display_name: this.name(), email: this.personEmail(), household: 0 }, 'PersonView');
      const body = { target_kind: this.kind(), target: this.target(), email: '', handle: '', person: p.id, days: 30, uses: 1 };
      this.made.set(await this.api.send<InviteView>('POST', '/api/v1/invitations', body, 'InviteView'));
      this.name.set('');
      this.personEmail.set('');
    });
  }

  revoke(id: number): Promise<void> {
    return this.run(() => this.api.send('POST', `/api/v1/invitations/${id}/revoke`, undefined, 'Done'));
  }

  copy(text: string): void {
    void navigator.clipboard?.writeText(text);
  }
}
