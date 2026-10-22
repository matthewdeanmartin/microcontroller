import { Component, inject, input, OnInit, signal } from '@angular/core';
import { Router } from '@angular/router';
import { GroupInput, GroupKind, GroupView, JOIN_LABEL, JoinPolicy, KIND_LABEL, Visibility, VISIBILITY_LABEL } from 'party-core';
import { Api, problem } from '../api';
import { value } from './auth';

/** Start a group, or edit one (`/groups/:ref/edit`). */
@Component({
  selector: 'party-group-form',
  template: `
    <section class="card narrow">
      <h1>{{ ref() ? 'Edit group' : 'Start a group' }}</h1>
      <form (submit)="submit($event)">
        <label>Name <input maxlength="60" required [value]="name()" (input)="name.set(v($event))" /></label>
        <label>About <small>optional</small>
          <textarea rows="4" maxlength="2000" [value]="about()" (input)="about.set(v($event))"></textarea></label>
        <fieldset>
          <legend>Kind</legend>
          @for (k of kinds; track k) {
            <label class="check"><input type="radio" name="kind" [checked]="kind() === k" (change)="kind.set(k)" /> {{ kindLabel[k] }}</label>
          }
        </fieldset>
        <fieldset>
          <legend>Who can see it</legend>
          @for (o of visibilities; track o) {
            <label class="check"><input type="radio" name="visibility" [checked]="visibility() === o" (change)="visibility.set(o)" /> {{ visibilityLabel[o] }}</label>
          }
        </fieldset>
        <fieldset>
          <legend>Who can join</legend>
          @for (o of joins; track o) {
            <label class="check"><input type="radio" name="join" [checked]="join() === o" (change)="join.set(o)" /> {{ joinLabel[o] }}</label>
          }
        </fieldset>
        @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
        <button type="submit" [disabled]="busy()">{{ ref() ? 'Save' : 'Start it' }}</button>
      </form>
    </section>
  `,
})
export class GroupForm implements OnInit {
  private readonly api = inject(Api);
  private readonly router = inject(Router);
  /** Route parameter: editing this group (id or slug). */
  readonly ref = input<string>('');
  readonly name = signal('');
  readonly about = signal('');
  readonly kind = signal<GroupKind>('circle');
  readonly visibility = signal<Visibility>('unlisted');
  readonly join = signal<JoinPolicy>('approval');
  readonly error = signal('');
  readonly busy = signal(false);
  private rev = 0;
  readonly kinds = Object.keys(KIND_LABEL) as GroupKind[];
  readonly visibilities = Object.keys(VISIBILITY_LABEL) as Visibility[];
  readonly joins = Object.keys(JOIN_LABEL) as JoinPolicy[];
  readonly kindLabel = KIND_LABEL;
  readonly visibilityLabel = VISIBILITY_LABEL;
  readonly joinLabel = JOIN_LABEL;
  readonly v = value;

  async ngOnInit(): Promise<void> {
    if (!this.ref()) return;
    try {
      const g = await this.api.get<GroupView>(`/api/v1/groups/${this.ref()}`, 'GroupView');
      this.name.set(g.name);
      this.about.set(g.about);
      this.kind.set(g.kind);
      this.visibility.set(g.visibility);
      this.join.set(g.join);
      this.rev = g.rev;
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  async submit(event: Event): Promise<void> {
    event.preventDefault();
    this.busy.set(true);
    this.error.set('');
    const body: GroupInput = {
      name: this.name(),
      about: this.about(),
      kind: this.kind(),
      visibility: this.visibility(),
      join: this.join(),
    };
    try {
      const g = this.ref()
        ? await this.api.send<GroupView>('PATCH', `/api/v1/groups/${this.ref()}`, { ...body, rev: this.rev }, 'GroupView')
        : await this.api.send<GroupView>('POST', '/api/v1/groups', body, 'GroupView');
      await this.router.navigate(['/groups', g.slug]);
    } catch (e) {
      this.error.set(problem(e));
    } finally {
      this.busy.set(false);
    }
  }
}
