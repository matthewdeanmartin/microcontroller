import { Component, inject, OnInit, signal } from '@angular/core';
import { BotView, day, InboxEntry, JobView, MailView, Provider, PublishView } from 'party-core';
import { Api, problem } from '../api';
import { value } from './auth';

/**
 * Admin → channels: email (Brevo), the Mastodon bot, sign-in instances,
 * publication to the companion site, and what went out. Secrets are
 * write-only: the board shows their last four characters.
 */
@Component({
  selector: 'party-channels',
  template: `
    <h1>Channels</h1>
    @if (error()) { <p class="error card" role="alert">{{ error() }}</p> }

    <section class="card">
      <h2>Email (Brevo)</h2>
      @if (mail(); as m) {
        <p>State: <strong>{{ m.state }}</strong>@if (m.sender_email) { · from {{ m.sender_email }} }@if (m.key_hint) { · key {{ m.key_hint }} }
          @if (m.state === 'ready') { · {{ m.sent_today }}/{{ m.daily_cap }} today }</p>
        @if (m.error) { <p class="error">{{ m.error }}</p> }
      }
      <form (submit)="saveMail($event)">
        <label>API key <small>xkeysib-…, from Brevo → SMTP &amp; API</small> <input autocomplete="off" (input)="mailKey.set(v($event))" /></label>
        <label>Sender <small>a verified Brevo sender; empty = the first</small> <input type="email" (input)="mailFrom.set(v($event))" /></label>
        <div class="actions">
          <button type="submit" [disabled]="!mailKey()">Save and check</button>
          @if (mail()?.state !== 'off') { <button type="button" class="secondary" (click)="remove('mail')">Turn off</button> }
        </div>
      </form>
    </section>

    <section class="card">
      <h2>Mastodon bot</h2>
      @if (bot(); as b) {
        <p>State: <strong>{{ b.state }}</strong>@if (b.handle) { · {{ b.handle }} }@if (b.token_hint) { · token {{ b.token_hint }} }
          @if (b.last_poll) { · last checked {{ day(b.last_poll) }} }</p>
        @if (b.error) { <p class="error">{{ b.error }}</p> }
      }
      <form (submit)="saveBot($event)">
        <label>Instance <input placeholder="mastodon.social" [value]="botInstance()" (input)="botInstance.set(v($event))" /></label>
        <label>Access token <small>from the bot account's Development page: read:notifications, read:statuses, write:statuses</small>
          <input autocomplete="off" (input)="botToken.set(v($event))" /></label>
        <label class="switch"><input type="checkbox" [checked]="botCommands()" (change)="botCommands.set(!botCommands())" /> Answer commands sent by DM</label>
        <label>Check every <input type="number" min="30" max="600" [value]="botPoll()" (input)="botPoll.set(+v($event))" /> seconds</label>
        <div class="actions">
          <button type="submit">Save and check</button>
          @if (bot()?.state !== 'off') { <button type="button" class="secondary" (click)="remove('bot')">Turn off</button> }
        </div>
      </form>
      @if (inbox().length) {
        <h3>Recent commands</h3>
        <ul class="outbox">
          @for (e of inbox(); track $index) { <li><small>{{ e.from }}</small> {{ e.text }} → {{ e.result }}</li> }
        </ul>
      }
    </section>

    <section class="card">
      <h2>Sign in with Mastodon</h2>
      <ul class="accounts">
        @for (p of providers(); track p.id) {
          <li><div><strong>{{ p.name }}</strong> <small>{{ p.base }} · {{ p.trust }}</small></div>
            <div class="actions"><button type="button" class="secondary" (click)="deleteProvider(p.id)">Remove</button></div></li>
        }
      </ul>
      <form class="row" (submit)="addProvider($event)">
        <label>Instance <input placeholder="mastodon.social" [value]="providerBase()" (input)="providerBase.set(v($event))" /></label>
        <label>Certificate <select (change)="providerTrust.set(v($event))"><option value="public">public</option><option value="household">household CA</option></select></label>
        <button type="submit" class="secondary">Add</button>
      </form>
    </section>

    <section class="card">
      <h2>Companion site (GitHub Pages)</h2>
      @if (publish(); as p) {
        <p>State: <strong>{{ p.state }}</strong>@if (p.repo) { · {{ p.repo }} ({{ p.branch }}) }@if (p.last_published) { · published {{ day(p.last_published) }}, generation {{ p.generation }}, {{ p.files }} files }</p>
        @if (p.last_error) { <p class="error">{{ p.last_error }}</p> }
      }
      <form (submit)="savePublish($event)">
        <label>Data repo <small>owner/name, e.g. ourparty/party-data</small> <input [value]="repo()" (input)="repo.set(v($event))" /></label>
        <label>Branch <input [value]="branch()" (input)="branch.set(v($event))" /></label>
        <label>Token <small>fine-grained, Contents read/write on that repo only</small> <input autocomplete="off" (input)="ghToken.set(v($event))" /></label>
        <label>Site address <small>https://ourparty.github.io/party-site/</small> <input [value]="site()" (input)="site.set(v($event))" /></label>
        <label>Publish at most every <input type="number" min="10" max="1440" [value]="every()" (input)="every.set(+v($event))" /> minutes</label>
        <div class="actions">
          <button type="submit">Save</button>
          @if (publish()?.state !== 'off') {
            <button type="button" class="secondary" (click)="publishNow()">Publish now</button>
            <button type="button" class="secondary" (click)="remove('publish')">Turn off</button>
          }
        </div>
      </form>
      <p class="aside">Only public groups' public events are published, without addresses. See the install guide for the GitHub side.</p>
    </section>

    <section class="card">
      <h2>Delivery</h2>
      @if (dropped()) { <p class="error">{{ dropped() }} notification(s) dropped: the queue was full.</p> }
      <ul class="outbox">
        @for (j of jobs(); track j.id) {
          <li><small>{{ day(j.created) }} · {{ j.kind }} · {{ j.state }}</small> {{ j.to }}: {{ j.subject }}
            @if (j.error) { <span class="error">{{ j.error }}</span> }
            @if (j.state === 'failed') { <button type="button" class="link" (click)="retry(j.id)">retry</button> }</li>
        } @empty { <li class="aside">Nothing sent yet.</li> }
      </ul>
    </section>
  `,
})
export class Channels implements OnInit {
  private readonly api = inject(Api);
  readonly mail = signal<MailView | null>(null);
  readonly bot = signal<BotView | null>(null);
  readonly publish = signal<PublishView | null>(null);
  readonly providers = signal<Provider[]>([]);
  readonly jobs = signal<JobView[]>([]);
  readonly dropped = signal(0);
  readonly inbox = signal<InboxEntry[]>([]);
  readonly mailKey = signal('');
  readonly mailFrom = signal('');
  readonly botInstance = signal('mastodon.social');
  readonly botToken = signal('');
  readonly botCommands = signal(true);
  readonly botPoll = signal(60);
  readonly providerBase = signal('');
  readonly providerTrust = signal('public');
  readonly repo = signal('');
  readonly branch = signal('main');
  readonly ghToken = signal('');
  readonly site = signal('');
  readonly every = signal(10);
  readonly error = signal('');
  readonly day = day;
  readonly v = value;

  async ngOnInit(): Promise<void> {
    await this.load();
    const b = this.bot();
    if (b && b.state !== 'off') {
      this.botInstance.set(b.instance);
      this.botCommands.set(b.commands);
      this.botPoll.set(b.poll_secs);
    }
    const p = this.publish();
    if (p && p.state !== 'off') {
      this.repo.set(p.repo);
      this.branch.set(p.branch);
      this.site.set(p.site_url);
      this.every.set(p.every_minutes);
    }
  }

  private async load(): Promise<void> {
    try {
      const [mail, bot, publish, providers, delivery, inbox] = await Promise.all([
        this.api.get<MailView>('/api/v1/admin/mail', 'MailView'),
        this.api.get<BotView>('/api/v1/admin/bot', 'BotView'),
        this.api.get<PublishView>('/api/v1/admin/publish', 'PublishView'),
        this.api.get<{ providers: Provider[] }>('/api/v1/admin/providers', 'ProviderList'),
        this.api.get<{ jobs: JobView[]; dropped: number }>('/api/v1/admin/delivery', 'Delivery'),
        this.api.get<{ entries: InboxEntry[] }>('/api/v1/admin/inbox', 'InboxLog'),
      ]);
      this.mail.set(mail);
      this.bot.set(bot);
      this.publish.set(publish);
      this.providers.set(providers.providers);
      this.jobs.set(delivery.jobs);
      this.dropped.set(delivery.dropped);
      this.inbox.set(inbox.entries);
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  private async act(f: () => Promise<unknown>): Promise<void> {
    this.error.set('');
    try {
      await f();
      await this.load();
      // The board checks keys on its worker: look again shortly.
      setTimeout(() => void this.load(), 3000);
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  saveMail(e: Event): Promise<void> {
    e.preventDefault();
    return this.act(() => this.api.send('PUT', '/api/v1/admin/mail', { api_key: this.mailKey(), sender_email: this.mailFrom(), sender_name: 'party' }, 'MailView'));
  }

  saveBot(e: Event): Promise<void> {
    e.preventDefault();
    const body = { instance: this.botInstance(), token: this.botToken(), trust: 'public', commands: this.botCommands(), poll_secs: this.botPoll() };
    return this.act(() => this.api.send('PUT', '/api/v1/admin/bot', body, 'BotView'));
  }

  savePublish(e: Event): Promise<void> {
    e.preventDefault();
    const body = { repo: this.repo(), branch: this.branch(), token: this.ghToken(), site_url: this.site(), every_minutes: this.every() };
    return this.act(() => this.api.send('PUT', '/api/v1/admin/publish', body, 'PublishView'));
  }

  publishNow(): Promise<void> {
    return this.act(() => this.api.send('POST', '/api/v1/admin/publish/now', undefined, 'PublishView'));
  }

  addProvider(e: Event): Promise<void> {
    e.preventDefault();
    return this.act(() => this.api.send('POST', '/api/v1/admin/providers', { name: '', base: this.providerBase(), trust: this.providerTrust() }, 'ProviderList'));
  }

  deleteProvider(id: number): Promise<void> {
    return this.act(() => this.api.send('DELETE', `/api/v1/admin/providers/${id}`, undefined, 'Done'));
  }

  remove(what: 'mail' | 'bot' | 'publish'): Promise<void> {
    if (!confirm(`Turn ${what} off? Its settings are forgotten.`)) return Promise.resolve();
    return this.act(() => this.api.send('DELETE', `/api/v1/admin/${what}`, undefined, 'Done'));
  }

  retry(id: number): Promise<void> {
    return this.act(() => this.api.send('POST', `/api/v1/admin/delivery/${id}/retry`, undefined, 'Done'));
  }
}
