import { Component, inject, OnInit, signal } from '@angular/core';
import { Router } from '@angular/router';
import { Api, FeedUrl, Me, problem } from '../api';

function value(event: Event): string {
  return (event.target as HTMLInputElement).value;
}

@Component({
  selector: 'gm-profile',
  template: `
    @if (api.me(); as me) {
      <section class="card narrow">
        <h1>Profile</h1>
        <p class="aside">Signed in as {{ me.email }} · {{ me.role }}</p>
        <form (submit)="rename($event)">
          <label>Your name <input maxlength="32" required [value]="me.display_name" (input)="name.set(v($event))" /></label>
          <button type="submit">Save name</button>
        </form>
        @if (saved()) { <p class="ok" role="status">{{ saved() }}</p> }
      </section>

      @if (api.status()?.mail && me.email) {
        <section class="card narrow">
          <h2>Email</h2>
          <label class="switch"><input type="checkbox" [checked]="me.email_notify" (change)="setEmail($event)" />
            Email me about plans I'm in (new plans for games I like, lock-ins, changes, reminders)</label>
        </section>
      }

      <section class="card narrow">
        <h2>Change PIN</h2>
        <form (submit)="changePin($event)">
          @if (me.has_pin) {
            <label>Current PIN
              <input type="password" inputmode="numeric" pattern="[0-9]*" autocomplete="current-password" maxlength="8" required (input)="current.set(v($event))" /></label>
          }
          <label>New PIN (4–8 digits)
            <input type="password" inputmode="numeric" pattern="[0-9]*" autocomplete="new-password" maxlength="8" required (input)="next.set(v($event))" /></label>
          <button type="submit">Change PIN</button>
        </form>
      </section>

      @if (me.mastodon || api.status()?.providers?.length) {
        <section class="card narrow">
          <h2>Mastodon</h2>
          @if (me.mastodon) {
            <p>Linked to <strong>{{ me.mastodon }}</strong>: you can sign in with it.</p>
            <button type="button" class="secondary" (click)="unlink()">Unlink</button>
            @if (!me.has_pin) { <p class="aside">Set a PIN above before unlinking, or you'd have no way in.</p> }
          } @else {
            <p>Link your Mastodon account to sign in with it instead of a PIN.</p>
            @for (p of api.status()?.providers; track p.id) {
              <button type="button" class="secondary" (click)="link(p.id)">Link {{ p.name }}</button>
            }
          }
        </section>
      }

      <section class="card narrow">
        <h2>Calendar</h2>
        <p>Subscribe to see plans you're in (or host) in your calendar app, updated as they change.</p>
        @if (feed()?.url) {
          <p class="feed">{{ feed()?.url }}</p>
          <div class="actions">
            <a class="button" [href]="feed()?.webcal">Subscribe</a>
            <button type="button" class="secondary" (click)="copy()">Copy link</button>
          </div>
          <p class="aside">Save it now: for privacy the board shows it only once.</p>
        } @else if (feed()?.enabled) {
          <p class="aside">Your calendar link is on. Lost it? Get a new one; the old link stops working.</p>
        }
        <div class="actions">
          <button type="button" class="secondary" (click)="newFeed()">{{ feed()?.enabled ? 'New link' : 'Get my calendar link' }}</button>
          @if (feed()?.enabled) { <button type="button" class="secondary" (click)="feedOff()">Turn off</button> }
        </div>
        <p class="aside">Works with calendar apps that fetch from your phone or computer on the home Wi-Fi (iPhone and Mac
          Calendar, Thunderbird, ICSx⁵ on Android). Google Calendar and Outlook.com fetch from their own servers and
          can't reach the board; use the 📅 link on a plan instead.</p>
      </section>

      <section class="card narrow">
        <h2>Devices</h2>
        <p>You're signed in on {{ me.sessions }} {{ me.sessions === 1 ? 'device' : 'devices' }}.</p>
        <button type="button" class="secondary" (click)="everywhere()">Sign out everywhere</button>
        <p class="aside">Want the padlock? <a href="/trust">Install the household certificate</a> on this device, then use
          <code>https://</code>.</p>
      </section>
      @if (error()) { <p class="error narrow" role="alert">{{ error() }}</p> }
    }
  `,
})
export class Profile implements OnInit {
  readonly api = inject(Api);
  private readonly router = inject(Router);
  readonly name = signal('');
  readonly current = signal('');
  readonly next = signal('');
  readonly saved = signal('');
  readonly error = signal('');
  readonly v = value;
  readonly feed = signal<FeedUrl | null>(null);

  async ngOnInit(): Promise<void> {
    this.feed.set(await this.api.get<FeedUrl>('/api/v1/me/feed', 'FeedUrl').catch(() => null));
  }

  link(provider: number): Promise<void> {
    return this.run(async () => {
      await this.api.startMastodon(provider, true);
      return '';
    });
  }

  unlink(): Promise<void> {
    return this.run(async () => {
      await this.api.send('DELETE', '/api/v1/me/mastodon', undefined, 'Done');
      await this.api.refresh();
      return 'Unlinked.';
    });
  }

  newFeed(): Promise<void> {
    return this.run(async () => {
      this.feed.set(await this.api.send<FeedUrl>('POST', '/api/v1/me/feed', undefined, 'FeedUrl'));
      return '';
    });
  }

  feedOff(): Promise<void> {
    return this.run(async () => {
      this.feed.set(await this.api.send<FeedUrl>('DELETE', '/api/v1/me/feed', undefined, 'FeedUrl'));
      return 'Calendar link turned off.';
    });
  }

  async copy(): Promise<void> {
    try {
      await navigator.clipboard.writeText(this.feed()?.url ?? '');
      this.saved.set('Copied.');
    } catch {
      this.saved.set('Copy it from the box above.');
    }
  }

  private async run(work: () => Promise<string>): Promise<void> {
    this.error.set('');
    this.saved.set('');
    try {
      this.saved.set(await work());
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  rename(event: Event): Promise<void> {
    event.preventDefault();
    return this.run(async () => {
      this.api.me.set(await this.api.send<Me>('PATCH', '/api/v1/me', { display_name: this.name() || this.api.me()?.display_name }, 'Me'));
      return 'Saved.';
    });
  }

  setEmail(event: Event): Promise<void> {
    const on = (event.target as HTMLInputElement).checked;
    return this.run(async () => {
      this.api.me.set(await this.api.send<Me>('PATCH', '/api/v1/me', { email_notify: on }, 'Me'));
      return on ? 'Plan emails on.' : 'Plan emails off.';
    });
  }

  changePin(event: Event): Promise<void> {
    event.preventDefault();
    return this.run(async () => {
      await this.api.send('PUT', '/api/v1/me/pin', { current: this.current(), new_pin: this.next() }, 'Done');
      (event.target as HTMLFormElement).reset();
      return 'PIN changed.';
    });
  }

  everywhere(): Promise<void> {
    return this.run(async () => {
      await this.api.signOut(true);
      await this.router.navigateByUrl('/signin');
      return '';
    });
  }
}
