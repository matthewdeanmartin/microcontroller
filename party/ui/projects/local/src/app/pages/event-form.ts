import { Component, computed, inject, input, OnInit, signal } from '@angular/core';
import { Router } from '@angular/router';
import {
  EventKind, EventView, fromLocalInput, KIND_INFO, Module, MODULE_LABEL, Question, SeriesView, Shift, Slot,
  TimeOption, toLocalInput,
} from 'party-core';
import { Api, problem } from '../api';
import { value } from './auth';

type Repeat = '' | 'weekly' | 'monthly-nth' | 'monthly-date';

/**
 * New event (`/groups/:ref/events/new`, `?series=1` for a recurring one) or
 * edit (`/events/:id/edit`, `?scope=following` for this and later ones).
 */
@Component({
  selector: 'party-event-form',
  template: `
    <section class="card narrow">
      <h1>{{ id() ? 'Edit event' : 'New event' }}</h1>
      @if (!id()) {
        <fieldset>
          <legend>What kind?</legend>
          <div class="chips">
            @for (k of kinds; track k) {
              <button type="button" class="chip" [class.on]="kind() === k" (click)="pickKind(k)">{{ info[k].icon }} {{ info[k].label }}</button>
            }
          </div>
        </fieldset>
      }
      @if (scope() === 'following') { <p class="status on">Changes apply to this and every later occurrence.</p> }
      <form (submit)="submit($event)">
        <label>Title <input maxlength="80" required [value]="title()" (input)="title.set(v($event))" /></label>
        <label>About <small>optional</small> <textarea rows="3" maxlength="4000" [value]="about()" (input)="about.set(v($event))"></textarea></label>
        @if (!has('when')) {
          <div class="row">
            <label>Starts <input type="datetime-local" required [value]="start()" (input)="start.set(v($event))" /></label>
            <label>Ends <small>optional</small> <input type="datetime-local" [value]="end()" (input)="end.set(v($event))" /></label>
          </div>
        }
        @if (!id() && repeatable()) {
          <fieldset>
            <legend>Repeat</legend>
            <select [value]="repeat()" (change)="repeat.set($any(v($event)))" aria-label="Repeat">
              <option value="">Once</option>
              <option value="weekly">Weekly</option>
              <option value="monthly-nth">Monthly, same weekday (e.g. first Friday)</option>
              <option value="monthly-date">Monthly, same date</option>
            </select>
            @if (repeat() === 'weekly') {
              <label>Every <input type="number" min="1" max="4" [value]="every()" (input)="every.set(+v($event))" /> week(s)</label>
            }
            @if (repeat()) {
              <label>Keep this many ahead <input type="number" min="1" max="8" [value]="window()" (input)="window.set(+v($event))" /></label>
            }
          </fieldset>
        }
        <fieldset>
          <legend>Where</legend>
          <div class="segmented small">
            @for (w of ['place', 'home', 'tbd']; track w) {
              <button type="button" [class.on]="whereKind() === w" (click)="whereKind.set($any(w))">{{ w === 'tbd' ? 'To be decided' : w === 'home' ? "Someone's home" : 'A place' }}</button>
            }
          </div>
          @if (whereKind() === 'place') {
            <label>Place <input maxlength="120" [value]="place()" (input)="place.set(v($event))" /></label>
          }
          @if (whereKind() === 'home') {
            <label>Whose home
              <select [value]="home()" (change)="home.set(+v($event))">
                <option value="0">Pick a household</option>
                @for (h of households(); track h.id) { <option [value]="h.id" [selected]="h.id === home()">{{ h.name }}</option> }
              </select></label>
          }
          @if (whereKind() !== 'tbd') {
            <label>Address <small>shown on this board only</small> <input maxlength="200" [value]="address()" (input)="address.set(v($event))" /></label>
          }
        </fieldset>
        <fieldset>
          <legend>Who and how many</legend>
          <label>Places <small>0 = no limit, counts guests too</small>
            <input type="number" min="0" max="2000" [value]="capacity()" (input)="capacity.set(+v($event))" /></label>
          <label>Guests per person
            <select [value]="plusOnes()" (change)="plusOnes.set($any(v($event)))">
              <option value="none">None</option>
              <option value="limited">Up to…</option>
              <option value="unlimited">Any number</option>
            </select></label>
          @if (plusOnes() === 'limited') {
            <label>Up to <input type="number" min="1" max="9" [value]="plusMax()" (input)="plusMax.set(+v($event))" /></label>
          }
          <label>Answers close <small>optional</small>
            <input type="datetime-local" [value]="rsvpBy()" (input)="rsvpBy.set(v($event))" /></label>
          <label>Remind people
            <select [value]="reminder()" (change)="reminder.set(+v($event))">
              <option value="0">No reminder</option>
              <option value="3600">An hour before</option>
              <option value="86400">The day before</option>
              <option value="604800">A week before</option>
            </select></label>
          <label class="switch"><input type="checkbox" [checked]="draft()" (change)="draft.set(!draft())" [disabled]="!!id() && !wasDraft()" /> Draft (only hosts see it)</label>
        </fieldset>
        <fieldset>
          <legend>Extras</legend>
          @for (m of optional; track m) {
            <label class="check"><input type="checkbox" [checked]="has(m)" (change)="flip(m)" /> {{ moduleLabel[m] }}</label>
          }
        </fieldset>
        @if (has('bring')) {
          <fieldset>
            <legend>Bring list</legend>
            @for (s of bring(); track $index) {
              <div class="row">
                <label>Item <input maxlength="40" [value]="s.name" (input)="s.name = v($event)" /></label>
                <label>How many <input type="number" min="1" max="99" [value]="s.count" (input)="s.count = +v($event)" /></label>
                <button type="button" class="secondary" (click)="bring.set(without(bring(), $index))">Remove</button>
              </div>
            }
            <button type="button" class="secondary" (click)="bring.set([...bring(), { id: 0, name: '', count: 1 }])">Add an item</button>
          </fieldset>
        }
        @if (has('diet')) {
          <label>Diet question <input maxlength="120" [value]="diet()" placeholder="Allergies or diet?" (input)="diet.set(v($event))" /></label>
        }
        @if (has('when')) {
          <fieldset>
            <legend>Times to choose from (2 to 6)</legend>
            @for (o of when(); track $index) {
              <div class="row">
                <label>Option {{ $index + 1 }} <input type="datetime-local" [value]="local(o.start)" (input)="o.start = secs(v($event))" /></label>
                <button type="button" class="secondary" (click)="when.set(without(when(), $index))">Remove</button>
              </div>
            }
            @if (when().length < 6) { <button type="button" class="secondary" (click)="when.set([...when(), { id: 0, start: 0, end: 0 }])">Add a time</button> }
          </fieldset>
        }
        @if (has('shifts')) {
          <fieldset>
            <legend>Shifts</legend>
            @for (s of shifts(); track $index) {
              <div class="row">
                <label>Shift <input maxlength="40" [value]="s.name" (input)="s.name = v($event)" /></label>
                <label>From <input type="datetime-local" [value]="local(s.start)" (input)="s.start = secs(v($event))" /></label>
                <label>To <input type="datetime-local" [value]="local(s.end)" (input)="s.end = secs(v($event))" /></label>
                <label>People <input type="number" min="1" max="99" [value]="s.count" (input)="s.count = +v($event)" /></label>
                <button type="button" class="secondary" (click)="shifts.set(without(shifts(), $index))">Remove</button>
              </div>
            }
            <button type="button" class="secondary" (click)="shifts.set([...shifts(), { id: 0, name: '', start: 0, end: 0, count: 1 }])">Add a shift</button>
          </fieldset>
        }
        @if (has('questions')) {
          <fieldset>
            <legend>Questions (up to 4)</legend>
            @for (q of questions(); track $index) {
              <div class="row">
                <label>Question <input maxlength="120" [value]="q.prompt" (input)="q.prompt = v($event)" /></label>
                <label>Answer
                  <select [value]="q.kind" (change)="q.kind = $any(v($event))">
                    <option value="yesno">Yes / no</option>
                    <option value="choice">Pick one</option>
                    <option value="text">Short text</option>
                  </select></label>
                @if (q.kind === 'choice') {
                  <label>Choices, comma-separated <input [value]="q.choices.join(', ')" (input)="q.choices = split(v($event))" /></label>
                }
                <button type="button" class="secondary" (click)="questions.set(without(questions(), $index))">Remove</button>
              </div>
            }
            @if (questions().length < 4) {
              <button type="button" class="secondary" (click)="questions.set([...questions(), { id: 0, kind: 'yesno', prompt: '', choices: [] }])">Add a question</button>
            }
          </fieldset>
        }
        @if (has('rides')) {
          <label>Rides note <input maxlength="200" [value]="ridesNote()" placeholder="Meet at the church lot" (input)="ridesNote.set(v($event))" /></label>
        }
        @if (has('cost')) {
          <label>Cost <input maxlength="200" [value]="cost()" placeholder="$10 for pizza, pay Sam" (input)="cost.set(v($event))" /></label>
        }
        @if (has('link')) {
          <label>Online link <input type="url" maxlength="300" [value]="link()" (input)="link.set(v($event))" /></label>
        }
        @if (error()) { <p class="error" role="alert">{{ error() }}</p> }
        <button type="submit" [disabled]="busy()">{{ id() ? 'Save' : repeat() ? 'Start the series' : 'Create' }}</button>
      </form>
    </section>
  `,
})
export class EventForm implements OnInit {
  private readonly api = inject(Api);
  private readonly router = inject(Router);
  /** Route parameters: the group to create in, or the event to edit. */
  readonly ref = input<string>('');
  readonly id = input<string>('');
  /** Query: `series=1` starts with repeating on; `scope=following`. */
  readonly series = input<string>('');
  readonly scope = input<string>('');

  readonly kind = signal<EventKind>('generic');
  readonly modules = signal<Module[]>(['headcount']);
  readonly title = signal('');
  readonly about = signal('');
  readonly start = signal('');
  readonly end = signal('');
  readonly whereKind = signal<'place' | 'home' | 'tbd'>('tbd');
  readonly place = signal('');
  readonly address = signal('');
  readonly home = signal(0);
  readonly capacity = signal(0);
  readonly plusOnes = signal<'none' | 'limited' | 'unlimited'>('unlimited');
  readonly plusMax = signal(2);
  readonly rsvpBy = signal('');
  readonly reminder = signal(0);
  readonly draft = signal(false);
  readonly wasDraft = signal(false);
  readonly bring = signal<Slot[]>([]);
  readonly diet = signal('');
  readonly when = signal<TimeOption[]>([]);
  readonly shifts = signal<Shift[]>([]);
  readonly questions = signal<Question[]>([]);
  readonly ridesNote = signal('');
  readonly cost = signal('');
  readonly link = signal('');
  readonly repeat = signal<Repeat>('');
  readonly every = signal(1);
  readonly window = signal(4);
  readonly households = signal<{ id: number; name: string }[]>([]);
  readonly error = signal('');
  readonly busy = signal(false);
  private rev = 0;
  private group = 0;
  readonly repeatable = computed(() => !this.has('when'));

  readonly kinds = Object.keys(KIND_INFO) as EventKind[];
  readonly info = KIND_INFO;
  readonly moduleLabel = MODULE_LABEL;
  readonly optional: Module[] = ['bring', 'diet', 'rides', 'when', 'shifts', 'questions', 'cost', 'link'];
  readonly v = value;
  readonly local = toLocalInput;
  readonly secs = fromLocalInput;

  has(m: Module): boolean {
    return this.modules().includes(m);
  }

  flip(m: Module): void {
    this.modules.update((ms) => (ms.includes(m) ? ms.filter((x) => x !== m) : [...ms, m]));
    if (m === 'when' && this.has('when')) this.repeat.set('');
  }

  pickKind(k: EventKind): void {
    this.kind.set(k);
    this.modules.set([...KIND_INFO[k].modules]);
    const plus: Record<EventKind, 'none' | 'limited' | 'unlimited'> = {
      generic: 'unlimited', party: 'limited', dinner: 'none', potluck: 'limited', outing: 'limited',
      meeting: 'unlimited', workday: 'unlimited', poll: 'unlimited',
    };
    this.plusOnes.set(plus[k]);
    if (k === 'outing') this.whereKind.set('place');
    if (k === 'meeting') this.repeat.set('weekly');
  }

  without<T>(list: T[], i: number): T[] {
    return list.filter((_, j) => j !== i);
  }

  split(raw: string): string[] {
    return raw.split(',').map((s) => s.trim()).filter(Boolean);
  }

  async ngOnInit(): Promise<void> {
    this.api
      .get<{ households: { id: number; name: string }[] }>('/api/v1/households', 'HouseholdList')
      .then((h) => this.households.set(h.households))
      .catch(() => undefined);
    if (this.series()) this.repeat.set('weekly');
    if (!this.id()) {
      try {
        const g = await this.api.get<{ id: number }>(`/api/v1/groups/${this.ref()}`, 'GroupView');
        this.group = g.id;
      } catch (e) {
        this.error.set(problem(e));
      }
      return;
    }
    try {
      const v = await this.api.get<EventView>(`/api/v1/events/${this.id()}`, 'EventView');
      const e = v.event;
      this.rev = e.rev;
      this.kind.set(e.kind);
      this.modules.set(e.modules);
      this.title.set(e.title);
      this.about.set(e.about);
      this.start.set(toLocalInput(e.start));
      this.end.set(toLocalInput(e.end));
      this.whereKind.set(e.where_kind);
      this.place.set(e.place);
      this.address.set(e.address);
      this.home.set(e.home);
      this.capacity.set(e.capacity);
      this.plusOnes.set(e.plus_ones);
      this.plusMax.set(e.plus_max || 2);
      this.rsvpBy.set(toLocalInput(e.rsvp_by));
      this.reminder.set(e.reminder_secs);
      this.draft.set(e.status === 'draft');
      this.wasDraft.set(e.status === 'draft');
      this.bring.set(e.bring.map((s) => ({ ...s })));
      this.diet.set(e.diet_prompt);
      this.when.set(e.when.map((o) => ({ ...o })));
      this.shifts.set(e.shifts.map((s) => ({ ...s })));
      this.questions.set(e.questions.map((q) => ({ ...q, choices: [...q.choices] })));
      this.ridesNote.set(e.rides_note);
      this.cost.set(e.cost);
      this.link.set(e.link);
    } catch (e) {
      this.error.set(problem(e));
    }
  }

  private body(): Record<string, unknown> {
    const polling = this.has('when');
    return {
      kind: this.id() ? undefined : this.kind(),
      title: this.title(),
      about: this.about(),
      start: polling ? 0 : fromLocalInput(this.start()),
      end: polling ? 0 : fromLocalInput(this.end()),
      where_kind: this.whereKind(),
      place: this.place(),
      address: this.address(),
      home: this.home(),
      capacity: this.capacity(),
      plus_ones: this.plusOnes(),
      plus_max: this.plusOnes() === 'limited' ? this.plusMax() : 0,
      rsvp_by: fromLocalInput(this.rsvpBy()),
      reminder_secs: this.reminder(),
      status: this.draft() ? 'draft' : 'scheduled',
      modules: this.modules(),
      bring: this.has('bring') ? this.bring().filter((s) => s.name.trim()) : undefined,
      diet_prompt: this.diet(),
      when: polling ? this.when().filter((o) => o.start) : undefined,
      shifts: this.has('shifts') ? this.shifts().filter((s) => s.name.trim()) : undefined,
      questions: this.has('questions') ? this.questions().filter((q) => q.prompt.trim()) : undefined,
      rides_note: this.ridesNote(),
      cost: this.cost(),
      link: this.link(),
    };
  }

  async submit(event: Event): Promise<void> {
    event.preventDefault();
    this.busy.set(true);
    this.error.set('');
    try {
      if (this.id()) {
        const scope = this.scope() === 'following' ? '?scope=following' : '';
        const v = await this.api.send<EventView>('PATCH', `/api/v1/events/${this.id()}${scope}`, { ...this.body(), rev: this.rev }, 'EventView');
        await this.router.navigate(['/events', v.event.id]);
      } else if (this.repeat()) {
        const s = await this.api.send<SeriesView>(
          'POST',
          `/api/v1/groups/${this.group}/series`,
          { event: this.body(), rule: this.repeat(), every: this.every(), window: this.window() },
          'SeriesView',
        );
        const first = s.upcoming[0];
        await this.router.navigate(first ? ['/events', first.id] : ['/groups', this.ref()]);
      } else {
        const v = await this.api.send<EventView>('POST', `/api/v1/groups/${this.group}/events`, this.body(), 'EventView');
        await this.router.navigate(['/events', v.event.id]);
      }
    } catch (e) {
      this.error.set(problem(e));
    } finally {
      this.busy.set(false);
    }
  }
}
