import { ChangeDetectionStrategy, Component, inject, signal } from '@angular/core';
import { RouterLink, RouterLinkActive, RouterOutlet } from '@angular/router';
import { FORMATS, FORMAT_LABELS, Format } from 'miniframework-ng';
import { Api } from './api';

@Component({
  selector: 'hm-root',
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [RouterOutlet, RouterLink, RouterLinkActive],
  template: `
    <header>
      <a class="brand" routerLink="/">housemetrics</a>
      <nav>
        <a routerLink="/" routerLinkActive="on" [routerLinkActiveOptions]="{ exact: true }">Dashboard</a>
        <a routerLink="/system" routerLinkActive="on">System</a>
        <a routerLink="/lab" routerLinkActive="on">Format lab</a>
        <a routerLink="/devices" routerLinkActive="on">Devices</a>
      </nav>
      <div class="wire-choice">
        <label>
          Format
          <select [value]="api.format()" (change)="api.setFormat($any($event.target).value)">
            @for (f of formats; track f) {
              <option [value]="f">{{ labels[f] }}</option>
            }
          </select>
        </label>
        <label class="check"><input type="checkbox" [checked]="api.gzip()" (change)="api.setGzip($any($event.target).checked)" /> gzip</label>
      </div>
    </header>
    <main>
      <router-outlet />
    </main>
    <footer>
      <span>Talking to <b>{{ api.base() || 'this site' }}</b></span>
      @if (editing()) {
        <form (submit)="saveBase($event, base.value)">
          <input #base [value]="api.base()" placeholder="https://housemetrics.local (blank: this site)" />
          <button>Save</button>
        </form>
      } @else {
        <button class="link" (click)="editing.set(true)">change</button>
      }
      <a [href]="api.url('/trust')" target="_blank" rel="noopener">Trust this board's certificate</a>
      <a [href]="api.url('/api/v1/schema.proto')" target="_blank" rel="noopener">Schema (.proto)</a>
    </footer>
  `,
})
export class App {
  readonly api = inject(Api);
  readonly formats = FORMATS;
  readonly labels: Record<Format, string> = FORMAT_LABELS;
  readonly editing = signal(false);

  saveBase(event: Event, value: string): void {
    event.preventDefault();
    this.api.setBase(value);
    this.editing.set(false);
    location.reload();
  }
}
