import { Component, computed, inject, Injectable, input, OnInit, signal } from '@angular/core';
import { RouterLink, RouterOutlet } from '@angular/router';
import {
  count, DataSource, freshness, headline, KIND_INFO, PublicEvent, PublicGroup, Snapshot, SnapshotSource, upcoming, when,
} from 'party-core';
import { DemoSource } from './demo-data';
import { DATA_BASE, MODE } from './mode';

/** The published data, loaded once per visit. */
@Injectable({ providedIn: 'root' })
export class Data {
  readonly source: DataSource = MODE === 'demo' ? new DemoSource() : new SnapshotSource(DATA_BASE);
  readonly snapshot = signal<Snapshot | null>(null);
  readonly error = signal('');
  private loading: Promise<void> | null = null;

  ensure(): Promise<void> {
    this.loading ??= this.source
      .load()
      .then((s) => this.snapshot.set(s))
      .catch((e: unknown) => this.error.set(`Couldn't load the published data (${e instanceof Error ? e.message : e}).`));
    return this.loading;
  }

  group(ref: string): PublicGroup | undefined {
    return this.snapshot()?.groups.find((g) => g.slug === ref || String(g.id) === ref);
  }
}

function icon(kind: string): string {
  return KIND_INFO[kind as keyof typeof KIND_INFO]?.icon ?? '📅';
}

@Component({
  selector: 'party-event-row',
  imports: [RouterLink],
  template: `
    <span class="when">{{ e().start ? when(e().start, e().end) : 'Time being decided' }}</span>
    <a [routerLink]="['/event', e().id]">{{ icon(e().kind) }} {{ e().title }}</a>
    @if (e().status === 'cancelled') { <span class="status">cancelled</span> }
    <span class="aside">{{ headline(e().going, e().capacity) }}</span>
  `,
})
export class EventRow {
  readonly e = input.required<PublicEvent>();
  readonly when = when;
  readonly headline = headline;
  readonly icon = icon;
}

/** Public groups and what's coming up. */
@Component({
  selector: 'party-landing',
  imports: [RouterLink, EventRow],
  template: `
    @if (data.snapshot(); as s) {
      <section class="card">
        <h1>Coming up</h1>
        <ul class="events">
          @for (e of soon(); track e.id) { <li><party-event-row [e]="e" /></li> } @empty { <li class="aside">Nothing public is planned.</li> }
        </ul>
      </section>
      <section class="card">
        <h2>Groups</h2>
        <ul class="groups">
          @for (g of s.groups; track g.id) {
            <li><a [routerLink]="['/group', g.slug]">{{ g.name }}</a> <span class="aside">{{ count(g.member_count, 'member') }}</span></li>
          } @empty { <li class="aside">No public groups yet.</li> }
        </ul>
      </section>
    }
  `,
})
export class Landing implements OnInit {
  readonly data = inject(Data);
  readonly soon = computed(() => upcoming(this.data.snapshot()?.events ?? [], Math.floor(Date.now() / 1000)).slice(0, 12));
  readonly count = count;

  ngOnInit(): void {
    void this.data.ensure();
  }
}

@Component({
  selector: 'party-group-page',
  imports: [RouterLink, EventRow],
  template: `
    @if (group(); as g) {
      <section class="card">
        <h1>{{ g.name }}</h1>
        @if (g.about) { <p class="notes">{{ g.about }}</p> }
        <p class="aside">{{ count(g.member_count, 'member') }} · {{ g.join === 'open' ? 'anyone can join' : g.join === 'approval' ? 'ask to join' : 'by invitation' }}</p>
        @if (data.snapshot()?.manifest?.bot; as bot) {
          <p class="aside">To join from Mastodon, send {{ bot }} a private message: <code>join G{{ g.id }}</code></p>
        }
        <p class="aside"><a [href]="ics(g.id)">Subscribe to its calendar</a></p>
      </section>
      <section class="card">
        <h2>Coming up</h2>
        <ul class="events">
          @for (e of events(); track e.id) { <li><party-event-row [e]="e" /></li> } @empty { <li class="aside">Nothing planned.</li> }
        </ul>
      </section>
    } @else if (data.snapshot()) {
      <p class="card">There's no public group here by that name. <a routerLink="/">See all</a></p>
    }
  `,
})
export class GroupPage implements OnInit {
  readonly data = inject(Data);
  readonly ref = input<string>('');
  readonly group = computed(() => this.data.group(this.ref()));
  readonly events = computed(() => {
    const g = this.group();
    return g ? upcoming(this.data.snapshot()?.events ?? [], Math.floor(Date.now() / 1000), g.id) : [];
  });
  readonly count = count;

  ics(id: number): string {
    return `${DATA_BASE}data/ics/g${id}.ics`;
  }

  ngOnInit(): void {
    void this.data.ensure();
  }
}

@Component({
  selector: 'party-event-page',
  imports: [RouterLink],
  template: `
    @if (event(); as e) {
      <section class="card">
        <h1>{{ icon(e.kind) }} {{ e.title }}</h1>
        @if (group(); as g) { <p><a [routerLink]="['/group', g.slug]">{{ g.name }}</a></p> }
        @if (e.status === 'cancelled') { <p class="status">Cancelled</p> }
        <dl class="facts">
          <dt>When</dt><dd>{{ e.start ? when(e.start, e.end) : 'Being decided' }}</dd>
          @if (e.area) { <dt>Where</dt><dd>{{ e.area }}</dd> }
          <dt>Coming</dt><dd>{{ headline(e.going, e.capacity) }}</dd>
        </dl>
        @if (e.about) { <p class="notes">{{ e.about }}</p> }
        @if (data.snapshot()?.manifest?.bot; as bot) {
          <p class="aside">Members answer from Mastodon with a private message to {{ bot }}: <code>going E{{ e.id }}</code>, <code>maybe E{{ e.id }}</code> or <code>no E{{ e.id }}</code>.</p>
        }
      </section>
    } @else if (data.snapshot()) {
      <p class="card">That event isn't public (or is over). <a routerLink="/">See what's coming up</a></p>
    }
  `,
})
export class EventPage implements OnInit {
  readonly data = inject(Data);
  readonly id = input<string>('');
  readonly event = computed(() => this.data.snapshot()?.events.find((e) => String(e.id) === this.id()));
  readonly group = computed(() => this.data.snapshot()?.groups.find((g) => g.id === this.event()?.group));
  readonly when = when;
  readonly headline = headline;
  readonly icon = icon;

  ngOnInit(): void {
    void this.data.ensure();
  }
}

/**
 * The companion site on GitHub Pages (spec/05-companion-site.md): what the
 * board last published. Sealed data (Sprint 9) and answering through
 * Mastodon (Sprint 10) come later.
 */
@Component({
  selector: 'party-companion',
  imports: [RouterLink, RouterOutlet],
  template: `
    <header>
      <a class="brand" routerLink="/">🎉 {{ data.snapshot()?.manifest?.instance || 'party' }}</a>
      @if (data.source.demo) { <span class="status on">demo: made-up data</span> }
    </header>
    <main>
      @if (data.error()) { <p class="error card" role="alert">{{ data.error() }}</p> }
      @if (fresh(); as f) {
        <p class="aside" [class.error]="f.stale">{{ f.label }}@if (f.stale) { : this may be out of date }</p>
      }
      <router-outlet />
      @if (!data.source.demo) {
        <p class="aside">At the house? Open <a href="http://party.local/">party.local</a> on the home Wi-Fi for everything.</p>
      }
    </main>
  `,
})
export class App implements OnInit {
  readonly data = inject(Data);
  readonly fresh = computed(() => {
    const m = this.data.snapshot()?.manifest;
    return m ? freshness(m.created, Math.floor(Date.now() / 1000)) : null;
  });

  ngOnInit(): void {
    void this.data.ensure();
  }
}
