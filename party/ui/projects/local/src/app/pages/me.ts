import { Component, inject, OnInit, signal } from '@angular/core';
import { Router } from '@angular/router';
import { Discoverable, Me } from 'party-core';
import { Api, problem } from '../api';
import { value } from './auth';

const FINDABLE: Record<Discoverable, string> = {
  nobody: 'Nobody (only admins)',
  members: 'People I share a group or household with',
  public: 'Anyone on this board',
};

@Component({
  selector: 'party-me',
  template: `
    @if (api.me(); as me) {
      <section class="card narrow">
        <h1>Your profile</h1>
        <form (submit)="save($event)">
          <label>Name <small>also your sign-in name</small>
            <input maxlength="40" required [value]="me.display_name" (input)="name.set(v($event))" /></label>
          <label>Email <small>optional, never shown to others</small>
            <input type="email" [value]="me.email" (input)="email.set(v($event))" /></label>
          <label>About you <small>optional</small>
            <textarea rows="3" maxlength="280" [value]="me.bio" (input)="bio.set(v($event))"></textarea></label>
          <fieldset>
            <legend>Who can find you</legend>
            @for (o of options; track o) {
              <label class="check"><input type="radio" name="findable" [checked]="(findable() || me.discoverable) === o" (change)="findable.set(o)" /> {{ findableLabel[o] }}</label>
            }
          </fieldset>
          @if (me.household_name) { <p class="aside">Household: {{ me.household_name }} (an admin sets this).</p> }
          @if (saved()) { <p class="ok" role="status">Saved.</p> }
          @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
          <button type="submit">Save</button>
        </form>
      </section>
      <section class="card narrow">
        <h2>Change PIN</h2>
        <form (submit)="changePin($event)">
          <label>Current PIN <input type="password" inputmode="numeric" autocomplete="current-password" maxlength="8" (input)="current.set(v($event))" /></label>
          <label>New PIN <input type="password" inputmode="numeric" autocomplete="new-password" maxlength="8" required (input)="pin.set(v($event))" /></label>
          @if (pinDone()) { <p class="ok" role="status">PIN changed.</p> }
          @if (pinError()) { <p class="error" role="alert">{{ pinError() }}</p> }
          <button type="submit" class="secondary">Change PIN</button>
        </form>
      </section>
      <section class="card narrow">
        <h2>Messages</h2>
        <label class="switch"><input type="checkbox" [checked]="me.email_notify" [disabled]="!me.email" (change)="notify({ email_notify: !me.email_notify })" />
          Email me about invitations, changes and reminders @if (!me.email) { <small>(add an email first)</small> }</label>
        <label class="switch"><input type="checkbox" [checked]="me.dm_notify" [disabled]="!me.identities.length" (change)="notify({ dm_notify: !me.dm_notify })" />
          Mastodon messages from the bot @if (!me.identities.length) { <small>(link an account first)</small> }</label>
      </section>
      <section class="card narrow">
        <h2>Mastodon</h2>
        @for (h of me.identities; track h) {
          <p>{{ h }} <button type="button" class="link" (click)="unlink(h)">unlink</button></p>
        } @empty {
          <p class="aside">Link a Mastodon account to sign in with it and to get and answer messages from the bot.</p>
        }
        @for (p of api.status()?.providers; track p.id) {
          <button type="button" class="secondary wide" (click)="link(p.id)">Link a {{ p.name }} account</button>
        }
      </section>
      <section class="card narrow">
        <h2>Your personal link</h2>
        <p class="aside">Opens your events on the companion site from anywhere. Anyone with it can see what you can, so keep it to yourself.</p>
        @if (me.personal_link) {
          <p class="feed">{{ me.personal_link }}</p>
          <button type="button" class="secondary" (click)="personal(true)">Make a new one (the old one stops)</button>
        } @else {
          <button type="button" class="secondary" (click)="personal(false)">Make my link</button>
        }
      </section>
      <section class="card narrow">
        <h2>Calendar</h2>
        <p class="aside">A feed of everything you're going to, for a calendar app on the home Wi-Fi. Calendars in the
          cloud (Google, iCloud) can't reach the board; download single events instead.</p>
        @if (feed()) { <p class="feed">{{ feed() }}</p> }
        <button type="button" class="secondary" (click)="newFeed()">{{ feed() ? 'Make a new link (the old one stops)' : 'Make my calendar link' }}</button>
      </section>
      <section class="card narrow">
        <h2>Devices</h2>
        <p class="aside">Signed in on {{ me.sessions }} device(s).</p>
        <button type="button" class="secondary" (click)="everywhere()">Sign out everywhere</button>
      </section>
    }
  `,
})
export class Profile implements OnInit {
  readonly api = inject(Api);
  private readonly router = inject(Router);
  readonly name = signal<string | null>(null);
  readonly email = signal<string | null>(null);
  readonly bio = signal<string | null>(null);
  readonly findable = signal<Discoverable | null>(null);
  readonly saved = signal(false);
  readonly error = signal('');
  readonly current = signal('');
  readonly pin = signal('');
  readonly pinDone = signal(false);
  readonly pinError = signal('');
  readonly feed = signal('');
  readonly options = Object.keys(FINDABLE) as Discoverable[];
  readonly findableLabel = FINDABLE;
  readonly v = value;

  async ngOnInit(): Promise<void> {
    await this.api.refresh().catch(() => undefined);
  }

  async save(event: Event): Promise<void> {
    event.preventDefault();
    this.saved.set(false);
    this.error.set('');
    const body: Record<string, string> = {};
    if (this.name() !== null) body['display_name'] = this.name()!;
    if (this.email() !== null) body['email'] = this.email()!;
    if (this.bio() !== null) body['bio'] = this.bio()!;
    if (this.findable() !== null) body['discoverable'] = this.findable()!;
    try {
      this.api.me.set(await this.api.send<Me>('PATCH', '/api/v1/me', body, 'Me'));
      this.saved.set(true);
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  async changePin(event: Event): Promise<void> {
    event.preventDefault();
    this.pinDone.set(false);
    this.pinError.set('');
    try {
      await this.api.send('PUT', '/api/v1/me/pin', { current: this.current(), pin: this.pin() }, 'Done');
      this.pinDone.set(true);
    } catch (e) {
      this.pinError.set(problem(e));
    }
  }

  async notify(change: { email_notify?: boolean; dm_notify?: boolean }): Promise<void> {
    try {
      this.api.me.set(await this.api.send<Me>('PATCH', '/api/v1/me', change, 'Me'));
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  async link(provider: number): Promise<void> {
    try {
      await this.api.startMastodon(provider, 'link');
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  async unlink(handle: string): Promise<void> {
    try {
      this.api.me.set(await this.api.send<Me>('DELETE', `/api/v1/me/identities?handle=${encodeURIComponent(handle)}`, undefined, 'Me'));
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  async personal(rotate: boolean): Promise<void> {
    try {
      const me = await this.api.send<Me>('POST', `/api/v1/me/link${rotate ? '?rotate=1' : ''}`, undefined, 'Me');
      this.api.me.set(me);
      if (!me.personal_link) this.error.set('Your key is made; the link appears once an admin sets up the companion site.');
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  async newFeed(): Promise<void> {
    try {
      const r = await this.api.send<{ path: string }>('POST', '/api/v1/me/calendar', undefined, 'FeedUrl');
      this.feed.set(`${location.origin}${r.path}`);
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  async everywhere(): Promise<void> {
    await this.api.signOut(true).catch(() => undefined);
    await this.router.navigateByUrl('/signin');
  }
}
