import { Component, inject, input, OnInit, signal } from '@angular/core';
import { Api, CommentList, CommentView, problem } from '../api';
import { dayName } from '../fmt';

/** A plan's flat comment thread. Plain text only. */
@Component({
  selector: 'gm-thread',
  template: `
    <section class="card">
      <h2>Comments</h2>
      <ul class="comments">
        @for (c of comments(); track c.id) {
          <li>
            <div class="meta"><strong>{{ c.author_name }}</strong>
              <span class="aside">{{ when(c.created) }}@if (c.edited) { · edited }</span>
              @if (c.mine && open()) { <button type="button" class="secondary" (click)="startEdit(c)">Edit</button> }
              @if (c.deletable) { <button type="button" class="secondary" (click)="remove(c)">Delete</button> }
            </div>
            @if (editing() === c.id) {
              <form (submit)="saveEdit($event, c)">
                <textarea maxlength="500" rows="3" [value]="c.text" (input)="draft = v($event)" aria-label="Edit comment"></textarea>
                <div class="actions">
                  <button type="submit">Save</button>
                  <button type="button" class="secondary" (click)="editing.set(0)">Cancel</button>
                </div>
              </form>
            } @else {
              <div class="text">{{ c.text }}</div>
            }
          </li>
        } @empty {
          <li class="aside">No comments yet.</li>
        }
      </ul>
      @if (more()) { <button type="button" class="secondary" (click)="loadMore()">Show more</button> }
      @if (open()) {
        <form (submit)="post($event)">
          <label>Add a comment
            <textarea maxlength="500" rows="2" [value]="text()" (input)="text.set(v($event))" placeholder="arriving at 3, who has the expansion?"></textarea></label>
          @if (api.status()?.bot) {
            <label class="switch"><input type="checkbox" [checked]="dm()" (change)="dm.set($any($event.target).checked)" />
              Also send it as a Mastodon DM to everyone coming</label>
          }
          <button type="submit" [disabled]="!text().trim() || busy()">Post</button>
        </form>
        @if (sent()) { <p class="ok" role="status">{{ sent() }}</p> }
      } @else {
        <p class="aside">Comments are closed for this plan.</p>
      }
      @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
    </section>
  `,
})
export class Thread implements OnInit {
  readonly plan = input.required<number>();
  readonly api = inject(Api);
  readonly comments = signal<CommentView[]>([]);
  readonly more = signal(false);
  readonly open = signal(true);
  readonly text = signal('');
  readonly dm = signal(false);
  readonly sent = signal('');
  readonly editing = signal(0);
  readonly busy = signal(false);
  readonly error = signal('');
  draft = '';

  ngOnInit(): Promise<void> {
    return this.load(0, false);
  }

  v(event: Event): string {
    return (event.target as HTMLTextAreaElement).value;
  }

  when(seconds: number): string {
    return `${dayName(seconds)} ${new Date(seconds * 1000).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })}`;
  }

  private show(page: CommentList, append: boolean): void {
    this.comments.update((list) => (append ? [...list, ...page.comments] : page.comments));
    this.more.set(page.more);
    this.open.set(page.open);
  }

  private async load(after: number, append: boolean): Promise<void> {
    try {
      this.show(await this.api.get<CommentList>(`/api/v1/plans/${this.plan()}/comments?after=${after}`, 'CommentList'), append);
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  loadMore(): Promise<void> {
    const last = this.comments().at(-1)?.id ?? 0;
    return this.load(last, true);
  }

  async post(event: Event): Promise<void> {
    event.preventDefault();
    this.busy.set(true);
    this.error.set('');
    try {
      const dm = this.dm();
      const page = await this.api.send<CommentList>('POST', `/api/v1/plans/${this.plan()}/comments`, { text: this.text(), dm }, 'CommentList');
      this.sent.set(
        !dm ? '' : page.sent_dms ? `Sent as a DM to ${page.sent_dms} ${page.sent_dms === 1 ? 'person' : 'people'}.` : "Nobody coming has a Mastodon account the bot can reach.",
      );
      this.text.set('');
      this.dm.set(false);
      await this.reloadAll();
    } catch (e) {
      this.error.set(problem(e));
    } finally {
      this.busy.set(false);
    }
  }

  /** Reloads every page currently shown (a thread is at most 64). */
  private async reloadAll(): Promise<void> {
    let after = 0;
    let all: CommentView[] = [];
    for (;;) {
      const page = await this.api.get<CommentList>(`/api/v1/plans/${this.plan()}/comments?after=${after}`, 'CommentList');
      all = [...all, ...page.comments];
      this.open.set(page.open);
      if (!page.more || !page.comments.length) break;
      after = page.comments.at(-1)!.id;
    }
    this.comments.set(all);
    this.more.set(false);
  }

  startEdit(c: CommentView): void {
    this.draft = c.text;
    this.editing.set(c.id);
  }

  async saveEdit(event: Event, c: CommentView): Promise<void> {
    event.preventDefault();
    try {
      await this.api.send('PATCH', `/api/v1/plans/${this.plan()}/comments/${c.id}`, { text: this.draft }, 'Done');
      this.editing.set(0);
      await this.reloadAll();
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  async remove(c: CommentView): Promise<void> {
    if (!confirm('Delete this comment?')) return;
    try {
      await this.api.send('DELETE', `/api/v1/plans/${this.plan()}/comments/${c.id}`, undefined, 'Done');
      this.comments.update((list) => list.filter((x) => x.id !== c.id));
    } catch (e) {
      this.error.set(problem(e));
    }
  }
}
