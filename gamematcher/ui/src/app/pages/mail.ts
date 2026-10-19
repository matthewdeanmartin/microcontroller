import { Component, inject, OnDestroy, OnInit, signal } from '@angular/core';
import { Api, MailView, problem } from '../api';

/** Admin: sending email through Brevo. */
@Component({
  selector: 'gm-mail',
  template: `
    <section class="card">
      <h2>Email</h2>
      @if (mail(); as m) {
        @if (m.configured) {
          <p>Sending as <strong>{{ m.sender }}</strong> through Brevo@if (m.built_in) { <span class="aside"> (key from the firmware's build settings)</span> }.
            "Forgot PIN?" codes, invites typed on a plan, and plan news for everyone who hasn't turned it off.</p>
          <p class="aside">{{ m.sent }} sent · {{ m.sent_today }} of {{ m.daily_limit }} today
            @if (m.last_error) { · <span class="error">last error: {{ m.last_error }}</span> }</p>
          <div class="actions">
            <button type="button" class="secondary" (click)="call('POST', '/api/v1/admin/mail/test', undefined, 'Test email queued: check your inbox (and spam).')">Send me a test</button>
            <button type="button" class="secondary" (click)="remove()">Stop sending email</button>
          </div>
        } @else {
          <p>With a free <a href="https://app.brevo.com" target="_blank" rel="noopener">Brevo</a> account the board can email
            "forgot PIN?" codes, invites and plan news. Paste an <strong>API key</strong> (Brevo → SMTP &amp; API → API keys;
            it starts <code>xkeysib-</code>, not the SMTP key).</p>
          <form (submit)="setup($event)">
            <label>Brevo API key <input type="password" required autocomplete="off" (input)="key = v($event)" /></label>
            <div class="row">
              <label>From address <span class="aside">(optional: a verified sender; the first one otherwise)</span>
                <input type="email" (input)="sender = v($event)" /></label>
              <label>From name <input maxlength="40" [value]="name" (input)="name = v($event)" /></label>
            </div>
            <button type="submit" [disabled]="m.setup === 'working'">{{ m.setup === 'working' ? 'Checking the key…' : 'Turn on email' }}</button>
          </form>
          @if (m.setup.startsWith('failed')) { <p class="error" role="alert">{{ m.setup }}</p> }
        }
      }
      @if (notice()) { <p class="ok" role="status">{{ notice() }}</p> }
      @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
    </section>
  `,
})
export class Mail implements OnInit, OnDestroy {
  readonly api = inject(Api);
  readonly mail = signal<MailView | null>(null);
  readonly notice = signal('');
  readonly error = signal('');
  key = '';
  sender = '';
  name = 'Game Matcher';
  private timer = 0;

  ngOnInit(): Promise<void> {
    return this.call('GET', '/api/v1/admin/mail');
  }

  ngOnDestroy(): void {
    clearTimeout(this.timer);
  }

  v(event: Event): string {
    return (event.target as HTMLInputElement).value;
  }

  async call(method: string, path: string, body?: unknown, done = ''): Promise<void> {
    this.error.set('');
    this.notice.set('');
    try {
      const mail = await this.api.send<MailView>(method, path, body, 'MailView');
      this.mail.set(mail);
      if (done) this.notice.set(done);
      clearTimeout(this.timer);
      // The board checks the key in the background: watch until it's done.
      if (mail.setup === 'working') this.timer = setTimeout(() => this.call('GET', '/api/v1/admin/mail'), 1500);
      else if (mail.configured !== this.api.status()?.mail) await this.api.refresh();
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  setup(event: Event): Promise<void> {
    event.preventDefault();
    const body = { api_key: this.key, sender_email: this.sender, sender_name: this.name };
    this.key = '';
    return this.call('POST', '/api/v1/admin/mail', body);
  }

  async remove(): Promise<void> {
    if (!confirm('Stop sending email? People can still sign in with their PIN; queued email is dropped.')) return;
    await this.call('DELETE', '/api/v1/admin/mail');
  }
}
