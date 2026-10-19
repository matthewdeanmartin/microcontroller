import { Component, computed, inject, OnInit, signal } from '@angular/core';
import { RouterLink } from '@angular/router';
import { Api, PlanSummary, problem } from '../api';
import { when } from '../fmt';
import { mailto, siteInvite } from '../invite';

/** Home: what needs your answer, what's on, who's up for what. */
@Component({
  selector: 'gm-home',
  imports: [RouterLink],
  template: `
    <div class="bar">
      <h1>Hi, {{ api.me()?.display_name }}</h1>
      <a class="button" routerLink="/plans/new">New plan</a>
    </div>
    @if (error()) {
      <p class="error" role="alert">{{ error() }}</p>
    }

    @if (needsAnswer().length) {
      <section class="card">
        <h2>Needs your answer</h2>
        <ul class="plans">
          @for (p of needsAnswer(); track p.id) {
            <li>
              <a [routerLink]="['/plans', p.id]">{{ title(p) }}</a>
              <span class="aside">{{ p.times > 1 ? p.times + ' times to choose from' : when(p.start, p.end) }} ·
                by {{ p.host_name }}</span>
            </li>
          }
        </ul>
      </section>
    }

    <section class="card">
      <h2>Coming up</h2>
      @if (comingUp().length) {
        <ul class="plans">
          @for (p of comingUp(); track p.id) {
            <li>
              <a [routerLink]="['/plans', p.id]">{{ title(p) }}</a>
              <span class="aside">{{ when(p.start, p.end) }} · {{ p.in_count }} in@if (p.maybe_count) {, {{ p.maybe_count }} maybe}</span>
            </li>
          }
        </ul>
      } @else {
        <p class="aside">Nothing locked in yet. Plans that are still being decided are below.</p>
      }
    </section>

    @if (deciding().length) {
      <section class="card">
        <h2>Being decided</h2>
        <ul class="plans">
          @for (p of deciding(); track p.id) {
            <li>
              <a [routerLink]="['/plans', p.id]">{{ title(p) }}</a>
              <span class="aside">{{ p.times ? when(p.start, p.end) + (p.times > 1 ? ' (+' + (p.times - 1) + ' more)' : '') : 'when? no times yet' }}
                · {{ p.in_count }} in</span>
            </li>
          }
        </ul>
      </section>
    }

    @if (past().length) {
      <section class="card">
        <h2>Recently</h2>
        <ul class="plans">
          @for (p of past(); track p.id) {
            <li>
              <a [routerLink]="['/plans', p.id]">{{ title(p) }}</a>
              <span class="aside">{{ p.status === 'cancelled' ? 'cancelled' : when(p.start) }}</span>
            </li>
          }
        </ul>
      </section>
    }
    <p class="aside"><a [href]="joinMail()">✉️ Invite someone to Game Matcher</a></p>
    <p class="aside"><a routerLink="/games">Browse the games</a> and star the ones you're into: new plans list your starred games first.</p>
  `,
})
export class Home implements OnInit {
  readonly api = inject(Api);
  readonly active = signal<PlanSummary[]>([]);
  readonly past = signal<PlanSummary[]>([]);
  readonly error = signal('');
  readonly when = when;
  readonly joinMail = computed(() => mailto(siteInvite(globalThis.location.origin, this.api.me()?.display_name ?? 'Someone')));

  readonly needsAnswer = computed(() => this.active().filter((p) => p.status === 'open' && !p.answered));
  readonly comingUp = computed(() => this.active().filter((p) => p.status === 'on'));
  readonly deciding = computed(() => this.active().filter((p) => p.status === 'open' && p.answered));

  async ngOnInit(): Promise<void> {
    try {
      const [active, past] = await Promise.all([
        this.api.get<{ plans: PlanSummary[] }>('/api/v1/plans', 'PlanList'),
        this.api.get<{ plans: PlanSummary[] }>('/api/v1/plans?status=done,cancelled', 'PlanList'),
      ]);
      this.active.set(active.plans);
      this.past.set(past.plans.slice(0, 5));
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  title(p: PlanSummary): string {
    return p.title || (p.games.length === 1 ? p.games[0] : 'Game night: ' + p.games.join(', '));
  }
}
