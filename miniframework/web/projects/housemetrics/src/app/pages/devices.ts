import { ChangeDetectionStrategy, Component, OnInit, computed, inject, signal } from '@angular/core';
import { DatePipe } from '@angular/common';
import { ApiError } from 'miniframework-ng';
import { Api } from '../api';
import { Device, DeviceList, DeviceToken, Target, TargetList } from '../messages';

@Component({
  selector: 'hm-devices',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [DatePipe],
  template: `
    @if (!signedIn()) {
      <section class="card narrow">
        <h2>Admin</h2>
        <p class="muted">Managing devices and scrape targets needs the admin password set when the firmware was built. On the board it is only accepted over HTTPS.</p>
        <form class="row" (submit)="signIn($event, pw.value)">
          <input #pw type="password" autocomplete="current-password" placeholder="Admin password" />
          <button class="primary">Sign in</button>
        </form>
        @if (error()) {
          <p class="error">{{ error() }}</p>
        }
      </section>
    } @else {
      <section class="card">
        <div class="row between">
          <h2>Devices</h2>
          <button class="ghost" (click)="signOut()">Sign out</button>
        </div>
        <p class="muted">Each board that pushes metrics gets its own token. Revoking one stops only that board.</p>
        <form class="row" (submit)="create($event, name.value); name.value = ''">
          <input #name placeholder="Device name, e.g. attic-s2" maxlength="40" />
          <button class="primary">Add device</button>
        </form>
        @if (error()) {
          <p class="error">{{ error() }}</p>
        }
        @if (fresh(); as f) {
          <div class="callout">
            <p><b>Token for {{ f.device.name }}</b> (shown once; copy it now):</p>
            <p><code class="token">{{ f.token }}</code> <button class="ghost" (click)="copy(f.token)">Copy</button></p>
            <p class="small">From any shell (Git Bash, Linux, macOS):</p>
            <pre>{{ curlExample() }}</pre>
            <p class="small">From MicroPython:</p>
            <pre>{{ micropythonExample() }}</pre>
          </div>
        }
        <table>
          <thead><tr><th>Name</th><th>Token</th><th>Created</th><th>Last write</th><th class="num">Writes</th><th></th></tr></thead>
          <tbody>
            @for (d of devices(); track d.id) {
              <tr>
                <td>{{ d.name }}</td>
                <td><code>{{ d.prefix }}…</code></td>
                <td>{{ d.created ? (d.created | date: 'short') : '–' }}</td>
                <td>{{ d.last_seen ? (d.last_seen | date: 'short') : 'not since boot' }}</td>
                <td class="num">{{ d.writes }}</td>
                <td><button class="ghost danger" (click)="revoke(d)">Revoke</button></td>
              </tr>
            } @empty {
              <tr><td colspan="6" class="muted">No devices yet.</td></tr>
            }
          </tbody>
        </table>
      </section>

      <section class="card">
        <h2>Scrape targets</h2>
        <p class="muted">The board can also fetch metrics itself: from another miniframework board's <code>/metrics</code> (Influx lines), or any JSON endpoint such as NanaCoin's <code>/api/v1/diag</code> (numeric fields become series).</p>
        <form class="row wrap" (submit)="addTarget($event, tname.value, turl.value, +tevery.value); tname.value = ''; turl.value = ''">
          <input #tname placeholder="Name, e.g. nanacoin-s2" />
          <input #turl class="grow" placeholder="http://nanacoin-s2.local/api/v1/diag" />
          <label>every <input #tevery type="number" min="5" max="3600" value="10" class="short" /> s</label>
          <button class="primary">Add target</button>
        </form>
        <table>
          <thead><tr><th>Name</th><th>URL</th><th class="num">Every</th><th>Last success</th><th class="num">Took</th><th class="num">Samples</th><th></th></tr></thead>
          <tbody>
            @for (t of targets(); track t.id) {
              <tr>
                <td>{{ t.name }}</td>
                <td><code>{{ t.url }}</code>@if (t.last_error) {<br /><span class="error small">{{ t.last_error }}</span>}</td>
                <td class="num">{{ t.every_s }} s</td>
                <td>{{ t.last_ok ? (t.last_ok | date: 'mediumTime') : '–' }}</td>
                <td class="num">{{ t.last_ms }} ms</td>
                <td class="num">{{ t.samples }}</td>
                <td><button class="ghost danger" (click)="removeTarget(t)">Remove</button></td>
              </tr>
            } @empty {
              <tr><td colspan="7" class="muted">No scrape targets.</td></tr>
            }
          </tbody>
        </table>
      </section>
    }
  `,
})
export class Devices implements OnInit {
  private readonly api = inject(Api);
  readonly signedIn = signal(false);
  readonly devices = signal<Device[]>([]);
  readonly targets = signal<Target[]>([]);
  readonly fresh = signal<DeviceToken | null>(null);
  readonly error = signal('');

  readonly host = computed(() => this.api.base() || location.origin);

  readonly curlExample = computed(() => {
    const token = this.fresh()?.token ?? 'TOKEN';
    return `curl -H "Authorization: Bearer ${token}" \\
  --data-binary "temp,room=attic value=21.5" \\
  "${this.host()}/api/v1/write"`;
  });

  readonly micropythonExample = computed(() => {
    const token = this.fresh()?.token ?? 'TOKEN';
    const http = this.host().replace(/^https:/, 'http:');
    return `import urequests
line = "temp,room=attic value=%.1f" % reading
urequests.post("${http}/api/v1/write", data=line,
               headers={"Authorization": "Bearer ${token}"}).close()`;
  });

  ngOnInit(): void {
    if (this.api.admin()) void this.load();
  }

  private fail(e: unknown): void {
    if (e instanceof ApiError && (e.status === 401 || e.code === 'https_required')) {
      this.signedIn.set(false);
    }
    this.error.set((e as Error).message);
  }

  async signIn(event: Event, password: string): Promise<void> {
    event.preventDefault();
    this.api.setAdmin(password);
    await this.load();
  }

  signOut(): void {
    this.api.setAdmin('');
    this.signedIn.set(false);
    this.fresh.set(null);
  }

  async load(): Promise<void> {
    try {
      const list = await this.api.get<DeviceList>('/api/v1/devices', 'DeviceList');
      this.devices.set(list.devices);
      this.targets.set((await this.api.get<TargetList>('/api/v1/scrapes', 'TargetList')).targets);
      this.signedIn.set(true);
      this.error.set('');
    } catch (e) {
      this.fail(e);
    }
  }

  async create(event: Event, name: string): Promise<void> {
    event.preventDefault();
    try {
      this.fresh.set(await this.api.send<DeviceToken>('POST', '/api/v1/devices', { name }, 'DeviceToken'));
      await this.load();
    } catch (e) {
      this.fail(e);
    }
  }

  async revoke(d: Device): Promise<void> {
    if (!confirm(`Revoke ${d.name}'s token? That board can no longer write.`)) return;
    try {
      await this.api.send('DELETE', `/api/v1/devices/${d.id}`, undefined, 'ErrorBody');
      await this.load();
    } catch (e) {
      this.fail(e);
    }
  }

  async addTarget(event: Event, name: string, url: string, every_s: number): Promise<void> {
    event.preventDefault();
    try {
      await this.api.send('POST', '/api/v1/scrapes', { name, url, every_s }, 'Target');
      await this.load();
    } catch (e) {
      this.fail(e);
    }
  }

  async removeTarget(t: Target): Promise<void> {
    try {
      await this.api.send('DELETE', `/api/v1/scrapes/${t.id}`, undefined, 'ErrorBody');
      await this.load();
    } catch (e) {
      this.fail(e);
    }
  }

  copy(text: string): void {
    void navigator.clipboard?.writeText(text);
  }
}
