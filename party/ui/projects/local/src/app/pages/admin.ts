import { Component, inject, OnInit, signal } from '@angular/core';
import { RouterLink } from '@angular/router';
import { AdminPerson, AuditEntry, day, HouseholdView, Settings, StorageInfo } from 'party-core';
import { Api, problem } from '../api';
import { value } from './auth';

/** People, households, settings, storage and the audit log. */
@Component({
  selector: 'party-admin',
  imports: [RouterLink],
  template: `
    <div class="bar">
      <h1>Admin</h1>
      <a class="button secondary" routerLink="/admin/channels">Email, bot and companion site</a>
    </div>
    @if (error()) { <p class="error card" role="alert">{{ error() }}</p> }

    @if (settings(); as s) {
      <section class="card">
        <h2>Settings</h2>
        <label class="switch"><input type="checkbox" [checked]="s.signup_open" (change)="setting({ signup_open: !s.signup_open })" />
          Anyone on the Wi-Fi can make an account</label>
        <label class="switch"><input type="checkbox" [checked]="s.groups_admin_only" (change)="setting({ groups_admin_only: !s.groups_admin_only })" />
          Only admins can start groups</label>
        <form class="row" (submit)="saveZone($event)">
          <label>Time zone <small>POSIX rule; now UTC{{ offset() }}</small>
            <input [value]="zone()" (input)="zone.set(v($event))" /></label>
          <button type="submit" class="secondary">Save</button>
        </form>
      </section>
    }

    <section class="card">
      <h2>People</h2>
      <ul class="accounts">
        @for (p of people(); track p.id) {
          <li [class.disabled]="p.status !== 'active'">
            <div><strong>{{ p.display_name }}</strong>
              <small>{{ p.role }}{{ p.status !== 'active' ? ' · ' + p.status : '' }}{{ p.email ? ' · ' + p.email : '' }}</small>
              @if (p.locked_secs) { <small class="error">locked {{ p.locked_secs }} s</small> }</div>
            <div class="actions">
              <select [value]="p.household" (change)="household(p, v($event))" aria-label="Household">
                <option value="0">No household</option>
                @for (h of households(); track h.id) { <option [value]="h.id" [selected]="h.id === p.household">{{ h.name }}</option> }
              </select>
              @if (p.id !== api.me()?.id) {
                <button type="button" class="secondary" (click)="update(p, { role: p.role === 'admin' ? 'member' : 'admin' })">{{ p.role === 'admin' ? 'Make member' : 'Make admin' }}</button>
                <button type="button" class="secondary" (click)="update(p, { disabled: p.status === 'active' })">{{ p.status === 'active' ? 'Disable' : 'Enable' }}</button>
              }
              <button type="button" class="secondary" (click)="resetPin(p)">Set PIN</button>
            </div>
          </li>
        }
      </ul>
      @if (nextPeople()) { <button type="button" class="secondary" (click)="loadPeople(nextPeople())">More</button> }
    </section>

    <section class="card">
      <h2>Households</h2>
      <ul class="accounts">
        @for (h of households(); track h.id) {
          <li>
            <div><strong>{{ h.name }}</strong> <small>{{ h.members.length }} people</small></div>
            <div class="actions">
              <button type="button" class="secondary" (click)="rename(h)">Rename</button>
              <button type="button" class="secondary" (click)="deleteHousehold(h)">Delete</button>
            </div>
          </li>
        }
      </ul>
      <form class="row" (submit)="addHousehold($event)">
        <label>New household <input maxlength="60" [value]="newHousehold()" (input)="newHousehold.set(v($event))" /></label>
        <button type="submit">Add</button>
      </form>
    </section>

    @if (storage(); as s) {
      <section class="card">
        <h2>Board</h2>
        <dl class="facts">
          <dt>People</dt><dd>{{ s.people }} / {{ s.max_people }}</dd>
          <dt>Groups</dt><dd>{{ s.groups }} / {{ s.max_groups }}</dd>
          <dt>Households</dt><dd>{{ s.households }} / {{ s.max_households }}</dd>
          <dt>Signed-in devices</dt><dd>{{ s.sessions }} / {{ s.max_sessions }}</dd>
          <dt>Saved state</dt><dd>{{ s.records }} records, {{ s.commits }} commits; the last wrote {{ s.last_commit_bytes }} bytes (largest {{ s.max_commit_bytes }})</dd>
          @if (s.quarantined.length) { <dt>Warning</dt><dd class="error">{{ s.quarantined.length }} saved item(s) were damaged and are not shown: {{ s.quarantined.join('; ') }}</dd> }
        </dl>
        <p class="aside"><a href="/api/v1/sys?fmt=json">System details</a> · <a href="/api/v1/log.txt">Log</a></p>
      </section>
    }

    <section class="card">
      <h2>Recent admin actions</h2>
      <ul class="outbox">
        @for (e of audit(); track e.seq) {
          <li><small>{{ day(e.at) }}</small> {{ name(e.actor) }}: {{ e.action }} <span class="aside">{{ e.detail }}</span></li>
        }
      </ul>
    </section>
  `,
})
export class Admin implements OnInit {
  readonly api = inject(Api);
  readonly settings = signal<Settings | null>(null);
  readonly people = signal<AdminPerson[]>([]);
  readonly nextPeople = signal(0);
  readonly households = signal<HouseholdView[]>([]);
  readonly storage = signal<StorageInfo | null>(null);
  readonly audit = signal<AuditEntry[]>([]);
  readonly newHousehold = signal('');
  readonly zone = signal('');
  readonly offset = signal('');
  readonly error = signal('');
  readonly day = day;
  readonly v = value;

  async ngOnInit(): Promise<void> {
    await this.reload();
  }

  name(id: number): string {
    if (!id) return 'the board';
    return this.people().find((p) => p.id === id)?.display_name ?? `#${id}`;
  }

  private async reload(): Promise<void> {
    try {
      const [settings, households, storage, audit] = await Promise.all([
        this.api.get<Settings>('/api/v1/admin/settings', 'Settings'),
        this.api.get<{ households: HouseholdView[] }>('/api/v1/households', 'HouseholdList'),
        this.api.get<StorageInfo>('/api/v1/admin/storage', 'StorageInfo'),
        this.api.get<{ entries: AuditEntry[] }>('/api/v1/admin/audit?limit=50', 'AuditList'),
      ]);
      this.settings.set(settings);
      this.households.set(households.households);
      this.storage.set(storage);
      this.audit.set(audit.entries);
      const zone = await this.api.get<{ zone: string; offset_secs: number }>('/api/v1/admin/zone', 'ZoneSetting');
      this.zone.set(zone.zone);
      const h = zone.offset_secs / 3600;
      this.offset.set(`${h >= 0 ? '+' : ''}${h}`);
      await this.loadPeople(0);
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  async loadPeople(after: number): Promise<void> {
    const page = await this.api.get<{ people: AdminPerson[]; next: number }>(`/api/v1/admin/people?after=${after}`, 'AdminPersonList');
    this.people.update((p) => (after ? [...p, ...page.people] : page.people));
    this.nextPeople.set(page.next);
  }

  private async act(run: () => Promise<unknown>): Promise<void> {
    this.error.set('');
    try {
      await run();
      await this.reload();
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  saveZone(event: Event): Promise<void> {
    event.preventDefault();
    return this.act(() => this.api.send('PATCH', '/api/v1/admin/zone', { zone: this.zone(), offset_secs: 0 }, 'ZoneSetting'));
  }

  setting(change: Partial<Settings>): Promise<void> {
    return this.act(() => this.api.send('PATCH', '/api/v1/admin/settings', change, 'Settings'));
  }

  update(p: AdminPerson, change: Record<string, unknown>): Promise<void> {
    return this.act(() => this.api.send('PATCH', `/api/v1/admin/people/${p.id}`, change, 'Done'));
  }

  household(p: AdminPerson, raw: string): Promise<void> {
    return this.update(p, { household: Number(raw) });
  }

  resetPin(p: AdminPerson): Promise<void> {
    const pin = prompt(`A new PIN for ${p.display_name} (4–8 digits). Tell them; they can change it later.`);
    if (!pin) return Promise.resolve();
    return this.update(p, { pin });
  }

  addHousehold(event: Event): Promise<void> {
    event.preventDefault();
    const name = this.newHousehold();
    this.newHousehold.set('');
    return this.act(() => this.api.send('POST', '/api/v1/households', { name }, 'HouseholdView'));
  }

  rename(h: HouseholdView): Promise<void> {
    const name = prompt('New name', h.name);
    if (!name) return Promise.resolve();
    return this.act(() => this.api.send('PATCH', `/api/v1/households/${h.id}`, { name }, 'Done'));
  }

  deleteHousehold(h: HouseholdView): Promise<void> {
    if (!confirm(`Delete ${h.name}? Its people stay, without a household.`)) return Promise.resolve();
    return this.act(() => this.api.send('DELETE', `/api/v1/households/${h.id}`, undefined, 'Done'));
  }
}
