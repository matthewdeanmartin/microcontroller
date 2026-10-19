import { Component, inject, input, OnDestroy, OnInit, signal } from '@angular/core';
import { Api, BotView, problem } from '../api';

/** Admin: the household's `@gamematcher` Mastodon bot. */
@Component({
  selector: 'gm-bot',
  template: `
    <section class="card">
      <h2>{{ slot() === 'public' ? 'Public Mastodon bot' : 'Household Mastodon bot' }}</h2>
      @if (bot(); as b) {
        @if (b.configured) {
          <p>Posting as <strong>{{ b.handle }}</strong> <span class="aside">({{ b.instance }})</span>.
            Plans whose host ticks "post to Mastodon" are announced, and so is their lock-in or cancellation.
            Comments can go to everyone coming as a direct message.</p>
          <div class="row">
            <label>Who sees posts
              <select [value]="b.visibility" (change)="settings({ visibility: v($event) })">
                <option value="unlisted">unlisted (followers and the link)</option>
                <option value="private">followers only</option>
                <option value="public">public</option>
              </select></label>
            <label>Daily summary at <input type="time" [value]="b.digest_time" (change)="settings({ digest_time: v($event) })" /></label>
          </div>
          <label class="switch"><input type="checkbox" [checked]="b.digest" (change)="settings({ digest: checked($event) })" />
            Post the day's plans every morning (skipped when nothing is planned)</label>
          <label class="switch"><input type="checkbox" [checked]="b.replies" (change)="settings({ replies: checked($event) })" />
            Accept RSVP replies from linked accounts on this instance</label>
          <p class="aside">Reply “in”, “maybe”, or “out” to a plan post, or mention this bot with “in 42” for plan #42.
            Link your account in Profile first. Older mentions are skipped when polling starts.
            Replies work away from home; the plan website still needs home-network access.</p>
          @if (b.reply_error) { <p class="error">{{ b.reply_error }}</p> }
          <div class="actions">
            <button type="button" class="secondary" (click)="call('POST', endpoint + '/digest')">Post a summary now</button>
            <button type="button" class="secondary" (click)="remove()">Stop using this bot</button>
          </div>
          <p class="aside">{{ b.posts }} {{ b.posts === 1 ? 'post' : 'posts' }}@if (b.last_post) { · last {{ when(b.last_post) }} }
            @if (b.last_error) { · <span class="error">last error: {{ b.last_error }}</span> }</p>
        } @else {
          <p>A bot account on your Mastodon instance can announce plans, DM comments to the people coming,
            and post a morning summary.</p>
          <div class="chips" role="tablist">
            <button type="button" class="chip" [class.on]="mode() === 'create'" (click)="mode.set('create')">Create one on mastomini</button>
            <button type="button" class="chip" [class.on]="mode() === 'token'" (click)="mode.set('token')">Use an access token</button>
          </div>
          <form (submit)="setup($event)">
            <label>Instance <input type="url" required [value]="draft.instance" (input)="draft.instance = v($event)" /></label>
            @if (mode() === 'create') {
              <div class="row">
                <label>Bot name <input required maxlength="30" [value]="draft.username" (input)="draft.username = v($event)" /></label>
                <label>Display name <input maxlength="30" [value]="draft.display_name" (input)="draft.display_name = v($event)" /></label>
              </div>
              <div class="row">
                <label>mastomini admin <input required autocomplete="username" (input)="draft.admin = v($event)" /></label>
                <label>Password <input type="password" required autocomplete="current-password" (input)="draft.password = v($event)" /></label>
              </div>
              <p class="aside">Used once to create the bot and its key; the board never stores it.</p>
            } @else {
              <label>Access token <span class="aside">(the account's Development → application, scopes write:statuses + read:accounts + read:notifications)</span>
                <input type="password" required (input)="draft.token = v($event)" /></label>
              <label>Certificate
                <select (change)="draft.anchor = v($event)"><option value="public">public (internet)</option><option value="household-ca">household CA</option></select></label>
            }
            <button type="submit" [disabled]="b.setup === 'working'">{{ b.setup === 'working' ? 'Setting up…' : 'Set up the bot' }}</button>
          </form>
          @if (b.setup.startsWith('failed')) { <p class="error" role="alert">{{ b.setup }}</p> }
        }
        @if (b.jobs.length) {
          <details>
            <summary>Recent posts</summary>
            <ul class="outbox">
              @for (j of b.jobs; track j.id) {
                <li><small [class.error]="j.state === 'failed'">{{ j.state }}</small> <small>{{ j.kind }}</small>
                  @if (j.url) { <a [href]="j.url" target="_blank" rel="noopener">{{ j.text }}</a> } @else { {{ j.text }} }
                  @if (j.error && j.state !== 'sent') { <span class="aside"> · {{ j.error }} (tried {{ j.attempts }}×)</span> }</li>
              }
            </ul>
          </details>
        }
      }
      @if (notice()) { <p class="ok" role="status">{{ notice() }}</p> }
      @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
    </section>
  `,
})
export class Bot implements OnInit, OnDestroy {
  readonly slot = input<'household' | 'public'>('household');
  get endpoint(): string { return this.slot() === 'public' ? '/api/v1/admin/public-bot' : '/api/v1/admin/bot'; }
  readonly api = inject(Api);
  readonly bot = signal<BotView | null>(null);
  readonly mode = signal<'create' | 'token'>('create');
  readonly notice = signal('');
  readonly error = signal('');
  draft = { instance: 'https://mastomini.local', username: 'gamematcher', display_name: 'Game Matcher', admin: '', password: '', token: '', anchor: 'public' };
  private timer = 0;

  ngOnInit(): Promise<void> {
    if (this.slot() === 'public') {
      this.mode.set('token');
      this.draft.instance = 'https://mastodon.social';
    }
    return this.call('GET', this.endpoint);
  }

  ngOnDestroy(): void {
    clearTimeout(this.timer);
  }

  v(event: Event): string {
    return (event.target as HTMLInputElement).value;
  }

  checked(event: Event): boolean {
    return (event.target as HTMLInputElement).checked;
  }

  async call(method: string, path: string, body?: unknown, done = ''): Promise<void> {
    this.error.set('');
    this.notice.set('');
    try {
      const bot = await this.api.send<BotView>(method, path, body, 'BotView');
      this.bot.set(bot);
      if (done) this.notice.set(done);
      else if (path.endsWith('/digest')) this.notice.set('Queued; it goes out in a moment.');
      // The board sets the bot up in the background: watch until it's done.
      clearTimeout(this.timer);
      if (bot.setup === 'working') this.timer = setTimeout(() => this.call('GET', this.endpoint, undefined, ''), 1500);
      else if (method === 'GET' && bot.configured && !(this.slot() === 'public' ? this.api.status()?.public_bot : this.api.status()?.bot)) await this.api.refresh();
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  setup(event: Event): Promise<void> {
    event.preventDefault();
    const d = this.draft;
    const body =
      this.mode() === 'create'
        ? { instance: d.instance, username: d.username, display_name: d.display_name, admin: d.admin, password: d.password }
        : { instance: d.instance, token: d.token, anchor: d.anchor };
    this.draft.password = '';
    return this.call('POST', this.endpoint, body);
  }

  settings(change: Record<string, unknown>): Promise<void> {
    return this.call('PATCH', this.endpoint, change, 'Saved.');
  }

  async remove(): Promise<void> {
    if (!confirm('Stop posting from this bot? The account stays on the instance; queued posts are dropped.')) return;
    await this.call('DELETE', this.endpoint);
    await this.api.refresh();
  }

  when(seconds: number): string {
    return new Date(seconds * 1000).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' });
  }
}
