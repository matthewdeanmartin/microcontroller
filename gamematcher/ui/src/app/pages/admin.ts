import { Component, inject, OnInit, signal } from '@angular/core';
import { AdminUser, Api, FeedUrl, problem, ProviderView, StorageInfo } from '../api';
import { Bot } from './bot';
import { Mail } from './mail';

/** Admin: accounts, sign-up, storage. Loaded only when an admin opens it. */
@Component({
  selector: 'gm-admin',
  imports: [Bot, Mail],
  template: `
    <section class="card">
      <h1>Admin</h1>
      <label class="switch">
        <input type="checkbox" [checked]="signupOpen()" (change)="setSignup($event)" />
        Anyone on the Wi-Fi can make an account (open sign-up)
      </label>
    </section>

    <section class="card">
      <h2>Sign in with Mastodon</h2>
      <p>Instances people may sign in with. <strong>Household</strong>: anyone with an account there (your own
        instance). <strong>Invited</strong>: only the handles you list (a public instance).</p>
      <ul class="accounts">
        @for (p of providers(); track p.id) {
          <li>
            <div><strong>{{ p.name }}</strong> <small>{{ p.trust }}</small> <small>{{ p.anchor === 'public' ? 'public certificate' : 'household CA' }}</small>
              <div class="aside">{{ p.base }}</div></div>
            <label>Invited handles <span class="aside">(one per line, like @bob@mastodon.social)</span>
              <textarea rows="2" [value]="p.invites.join('\n')" (change)="saveInvites(p, $event)"></textarea></label>
            <div class="actions">
              <button type="button" class="secondary" (click)="setTrust(p, p.trust === 'household' ? 'invited' : 'household')">
                Make {{ p.trust === 'household' ? 'invited-only' : 'household' }}</button>
              <button type="button" class="secondary" (click)="removeProvider(p)">Remove</button>
            </div>
          </li>
        } @empty {
          <li class="aside">None yet.</li>
        }
      </ul>
      <form class="row" (submit)="addProvider($event)">
        <label>Instance <input type="url" placeholder="https://mastodon.social" required (input)="draft.base = v($event)" /></label>
        <label>Name <input maxlength="32" placeholder="optional" (input)="draft.name = v($event)" /></label>
        <label>Who
          <select (change)="draft.trust = $any(v($event))"><option value="invited">invited only</option><option value="household">household</option></select></label>
        <label>Certificate
          <select (change)="draft.anchor = $any(v($event))"><option value="public">public (internet)</option><option value="household-ca">household CA</option></select></label>
        <button type="submit" class="secondary">Add</button>
      </form>
    </section>

    <gm-mail />

    <gm-bot />
    <gm-bot slot="public" />

    <section class="card">
      <h2>Household calendar</h2>
      <p>One feed with every locked-in plan, for a shared family calendar or a kitchen screen.</p>
      @if (house()?.url) {
        <p class="feed">{{ house()?.url }}</p>
        <p class="aside">Shown once. Anyone with this link can see the plans.</p>
      }
      <div class="actions">
        <button type="button" class="secondary" (click)="houseFeed('POST')">{{ house()?.enabled ? 'New link' : 'Turn on' }}</button>
        @if (house()?.enabled) { <button type="button" class="secondary" (click)="houseFeed('DELETE')">Turn off</button> }
      </div>
    </section>

    <section class="card">
      <h2>People</h2>
      <ul class="accounts">
        @for (u of users(); track u.id) {
          <li [class.disabled]="u.disabled">
            <div>
              <strong>{{ u.display_name }}</strong> <small>{{ u.role }}</small>
              @if (u.disabled) { <small>disabled</small> }
              @if (u.locked_secs > 0) { <small>PIN locked {{ u.locked_secs }} s</small> }
              <div class="aside">{{ u.email || u.mastodon }}@if (u.email && u.mastodon) { · {{ u.mastodon }} } · {{ u.sessions }} {{ u.sessions === 1 ? 'device' : 'devices' }} · seen {{ when(u.last_seen) }}</div>
            </div>
            <div class="actions">
              <button type="button" class="secondary" (click)="resetPin(u)">Reset PIN</button>
              <button type="button" class="secondary" (click)="patch(u, { role: u.role === 'admin' ? 'member' : 'admin' })">
                {{ u.role === 'admin' ? 'Make member' : 'Make admin' }}</button>
              <button type="button" class="secondary" (click)="patch(u, { disabled: !u.disabled })">{{ u.disabled ? 'Enable' : 'Disable' }}</button>
              <button type="button" class="secondary" (click)="patch(u, { sign_out: true })" [disabled]="u.sessions === 0">Sign out</button>
            </div>
          </li>
        }
      </ul>
      @if (notice()) { <p class="ok" role="status">{{ notice() }}</p> }
      @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
    </section>

    @if (storage(); as s) {
      <section class="card">
        <h2>Board</h2>
        <dl class="facts">
          <dt>Accounts</dt><dd>{{ s.accounts }} / {{ s.max_accounts }}</dd>
          <dt>Signed-in devices</dt><dd>{{ s.sessions }} / {{ s.max_sessions }}</dd>
          <dt>Saved state</dt><dd>{{ s.snapshot_bytes }} bytes, generation {{ s.generation }}, {{ s.commits }} commits</dd>
          @if (s.fallback_used) { <dt>Warning</dt><dd class="error">The newest save was damaged; the board started from the one before.</dd> }
        </dl>
        <p class="aside"><a href="/api/v1/sys?fmt=json">System details</a> · <a href="/api/v1/log.txt">Log</a></p>
      </section>
    }
  `,
})
export class Admin implements OnInit {
  readonly api = inject(Api);
  readonly users = signal<AdminUser[]>([]);
  readonly storage = signal<StorageInfo | null>(null);
  readonly signupOpen = signal(true);
  readonly house = signal<FeedUrl | null>(null);
  readonly providers = signal<ProviderView[]>([]);
  draft: { base: string; name: string; trust: 'household' | 'invited'; anchor: 'public' | 'household-ca' } = {
    base: '',
    name: '',
    trust: 'invited',
    anchor: 'public',
  };
  readonly notice = signal('');
  readonly error = signal('');

  async ngOnInit(): Promise<void> {
    await this.load();
  }

  private async load(): Promise<void> {
    try {
      const [users, settings, storage] = await Promise.all([
        this.api.get<{ users: AdminUser[] }>('/api/v1/admin/users', 'AdminUserList'),
        this.api.get<{ signup_open: boolean }>('/api/v1/admin/settings', 'Settings'),
        this.api.get<StorageInfo>('/api/v1/admin/storage', 'StorageInfo'),
      ]);
      this.users.set(users.users);
      this.signupOpen.set(settings.signup_open);
      this.storage.set(storage);
      this.house.set(await this.api.get<FeedUrl>('/api/v1/admin/house-feed', 'FeedUrl'));
      this.providers.set((await this.api.get<{ providers: ProviderView[] }>('/api/v1/admin/providers', 'ProviderList')).providers);
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  async patch(u: AdminUser, change: Record<string, unknown>, done = 'Saved.'): Promise<void> {
    this.error.set('');
    this.notice.set('');
    try {
      await this.api.send('PATCH', `/api/v1/admin/users/${u.id}`, change, 'Done');
      this.notice.set(done);
      await this.load();
      await this.api.refresh();
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  v(event: Event): string {
    return (event.target as HTMLInputElement).value;
  }

  private async providerCall(method: string, path: string, body?: unknown): Promise<void> {
    this.error.set('');
    try {
      const list = await this.api.send<{ providers: ProviderView[] }>(method, path, body, 'ProviderList');
      this.providers.set(list.providers);
      await this.api.refresh();
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  async addProvider(event: Event): Promise<void> {
    event.preventDefault();
    const d = this.draft;
    await this.providerCall('POST', '/api/v1/admin/providers', { base: d.base, name: d.name || undefined, trust: d.trust, anchor: d.anchor });
    if (!this.error()) {
      (event.target as HTMLFormElement).reset();
      this.draft = { base: '', name: '', trust: 'invited', anchor: 'public' };
    }
  }

  saveInvites(p: ProviderView, event: Event): Promise<void> {
    const invites = (event.target as HTMLTextAreaElement).value.split(/[\n,]+/).map((h) => h.trim()).filter(Boolean);
    return this.providerCall('PATCH', `/api/v1/admin/providers/${p.id}`, { invites });
  }

  setTrust(p: ProviderView, trust: string): Promise<void> {
    return this.providerCall('PATCH', `/api/v1/admin/providers/${p.id}`, { trust });
  }

  removeProvider(p: ProviderView): Promise<void> {
    if (!confirm(`Stop allowing sign-in with ${p.name}? Linked accounts keep working with their PIN.`)) return Promise.resolve();
    return this.providerCall('DELETE', `/api/v1/admin/providers/${p.id}`);
  }

  async houseFeed(method: 'POST' | 'DELETE'): Promise<void> {
    try {
      this.house.set(await this.api.send<FeedUrl>(method, '/api/v1/admin/house-feed', undefined, 'FeedUrl'));
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  resetPin(u: AdminUser): Promise<void> {
    const pin = prompt(`New PIN for ${u.display_name} (4–8 digits). Tell them in person.`);
    if (!pin) return Promise.resolve();
    return this.patch(u, { pin }, `${u.display_name}'s PIN was reset.`);
  }

  async setSignup(event: Event): Promise<void> {
    const open = (event.target as HTMLInputElement).checked;
    try {
      const s = await this.api.send<{ signup_open: boolean }>('PATCH', '/api/v1/admin/settings', { signup_open: open }, 'Settings');
      this.signupOpen.set(s.signup_open);
      await this.api.refresh();
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  when(seconds: number): string {
    return seconds ? new Date(seconds * 1000).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' }) : '—';
  }
}
