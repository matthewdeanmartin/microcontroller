# 03 — Domain: games, plans, comments, notifications, calendar

## Games

The catalog is shared and **anyone can add to it** (owner decision 11).

| Field | Notes |
|---|---|
| `id` | u16 slot |
| `title` | 1–48 chars. Unique after normalization (case, whitespace, punctuation folded), so "Catan" and "catan " collide and the UI offers the existing one |
| `kind` | `board`, `card`, `video`, `rpg`, `other` |
| `players` | min–max, 1–99. Optional |
| `minutes` | typical length. Optional |
| `notes` | ≤ 280 chars: "needs the expansion box", "PC only, 2 copies" |
| `added_by`, `created` | |
| `archived` | hidden from pickers. Old plans still show the title |

Editing: whoever added it, or admin. Delete is admin-only and becomes archive if
any plan references the game. Admin can **merge** a duplicate into another game
(plans and interests move over).

### Interests

Each account has an interest bitmap over game slots ("I'm into this"). Effects:

- a new plan that includes a game you're interested in notifies you;
- the game page lists who's interested, which is useful before proposing.

No notification levels beyond on/off in v1.

## Plans

A **plan** is the single event type (owner decision 7). With one candidate game it's
a session, with several it's a party. The UI says "Game night" vs "Catan"
based on the count, but the data doesn't distinguish them.

| Field | Notes |
|---|---|
| `id`, `host`, `created`, `updated` | |
| `title` | optional. Defaults to the game name or "Game night" |
| `notes` | ≤ 500 chars |
| `games` | 1–8 candidate games |
| `times` | 0–6 **time options**. Each has a stable id, a start, and an optional end ("Sat 2pm–6pm" = a window). An edited time gets a new id |
| `where` | free text, ≤ 64 chars. Default "Home" |
| `seats` | optional capacity, shown only as a hint |
| `status` | see below |
| `chosen_time`, `chosen_games` | set when the host locks it in |

A plan with **zero** time options is a "who's up for…?" shout: someone wants to play
a game and doesn't know when yet. Times can be added later.

### Responses (the negotiation)

Each person has one response per plan, which they can change at any time while the plan is open:

| Part | Values |
|---|---|
| overall | `in`, `maybe`, `out` (or no response) |
| per time option | `yes`, `if_needed`, `no` ("yeah I can do that") |
| per game | ticked / not ticked: "I'd play this" (owner decision 8) |
| note | ≤ 140 chars, e.g. "only after 3" |

The board shows **counts and names**, nothing more: per time, who said yes and
who said if-needed; per game, who ticked it. If a game's player range can't fit
the current `in` count, the game is shown greyed with "needs 3+". That's a hint,
not a rule. **There's no recommended time and no winner calculation** (owner decision 9).
Anyone can propose another time option or game on someone else's plan, as a
comment the host can accept with one tap.

### Status

```
open ──(host locks in)──▶ on ──(chosen time passes + 6 h)──▶ done
  │                        │
  └────(host/admin)────────┴──▶ cancelled
```

- `open`: collecting responses. Times and games can be edited by the host.
  Removing an option clears responses to it.
- `on`: host picked one time (and optionally one or more of the games, possibly
  left open until the night). Responses can still change, people back out.
  Editing the chosen time notifies everyone who's `in`/`maybe`.
- `done`: automatic, from the board clock. Read-only except comments for 7 days.
- `cancelled`: read-only. Responders are notified.

The host can hand the plan to someone else. Admin can do anything the host can.

### Visibility

All plans are visible to all signed-in users. A household doesn't need private
plans; see [open-questions.md](open-questions.md). Nothing is visible signed
out except the setup/sign-in page and the calendar feed (token-gated).

## Comments

A flat thread per plan, stored on the board (owner decision 12).

- ≤ 500 chars plain text. Rendered as text, never HTML. URLs are linkified by the UI.
- Author can edit (marked "edited") and delete. Admin can delete any.
- Chronological. The API pages by id.
- Posting a comment notifies the host and everyone who responded `in`/`maybe`,
  except the author. Notifications for one plan are coalesced
  ("3 new comments on Game night").

## Notifications

In-app is the only channel that always exists. Mastodon DM is optional per user
([08](08-mastodon.md)). The LCD shows a summary ([07](07-display.md)).

| Trigger | Who |
|---|---|
| New plan including a game you're interested in | interested users, except host |
| Plan locked in, time changed, cancelled | everyone who responded `in`/`maybe` |
| New comment(s) | host + `in`/`maybe` responders, coalesced per plan |
| Someone responded to your plan | host, coalesced |
| Reminder: locked plan starts in 15 minutes | host + `in` responders (only if the board clock is synced) |

Stored per user as a bounded ring ([05](05-storage-limits.md)): kind, plan id,
count, created, read flag. Each notification is deduplicated on `(user, kind, plan)` while unread, so the same
event never produces more than one unread notification. Plan updates and reminders are
exceptions: a rescheduled event must send fresh updates and another reminder even
when the previous notification is unread.

The UI polls `GET /api/v1/me/summary` (unread count, plans awaiting my response)
**at most once per 60 s, and only while the tab is visible**, plus on focus.
No push, no SSE, no WebSockets in v1.

## Calendar

Owner decision 13: both downloads and a feed.

- `GET /api/v1/plans/{id}/calendar.ics`: one VEVENT for an `on` plan (or one tentative VEVENT
  per time option for an `open` plan). Signed-in only.
- `GET /cal/{feed_token}.ics`: the user's feed with every `on` plan they're
  `in`/`maybe` for, plus (setting) all `on` plans. No cookie needed. The token is the
  credential. Rotating it in the profile invalidates the old URL.
- `GET /cal/house/{house_token}.ics`: optional household feed of all `on`
  plans. Admin enables and rotates it.

Times are UTC in the file (`DTSTART:…Z`). UIDs are `plan-{id}@gamematcher.local`,
stable so updates replace instead of duplicate. `SEQUENCE` = edit count.

**Caveat in the UI text:** cloud calendar services (Google Calendar web
subscriptions, Outlook.com) fetch feeds from their own servers and can never
reach `gamematcher.local`. Feeds work with calendar apps that fetch from the
device on the Wi-Fi: iOS/macOS Calendar, Thunderbird, DAVx⁵/ICSx⁵ on Android.
Downloads (`.ics` file import) work everywhere.

## Time

- One household time zone, a POSIX TZ string set at build time
  (`GAMEMATCHER_TZ`, e.g. `CST6CDT,M3.2.0,M11.1.0`). The board uses it only
  for the LCD and "Saturday 2pm" text in Mastodon posts. Browsers display
  UTC instants in their local zone, which is the same zone in this house.
- Stored as UTC seconds (u32, good until 2106).
- **No clock (SNTP not yet synced since boot):** the API and UI work. New
  plans get times from the browser, which has a clock. `done` transitions,
  reminders and the LCD's "in 2 h" wait for sync. `/api/v1/status`
  reports `clock: unsynced`. No RTC hardware.

### Editing and repeating plans

The host can edit an open plan or reschedule the confirmed time of a locked plan.
PATCH accepts `reschedule: {start, end}` for a locked plan; validation and the move
are atomic. The plan stays locked with its chosen games, answers to the moved time
are removed, calendar sequence advances, and the reminder resets. Changes to times,
games, title, place, seats or notes notify engaged participants.

“Plan this again” opens a new-plan form with the games, title, place, seats and notes
copied. Dates, responses and announcement consent are not copied. Each additional
time defaults to one hour after the preceding option, rolling dates at midnight.

### Recurring series and post-game reports

POST /api/v1/plans accepts repeat: {frequency, count}, where frequency is weekly,
fortnightly, or monthly, and count is 2–12 including the first plan. All dates and
capacity are validated before committing the series. Occurrences have independent
IDs, responses, reminders, and status; PlanView.series links surviving dates.
This is a finite series, not an indefinitely running scheduler.

PUT /api/v1/plans/{id}/report accepts {summary, games: [{game, winners: [account]}]}.
Only the host/admin can write after the confirmed start, or when already done.
Summary is limited to 1,000 characters and eight distinct games from the plan.
Winners must be host/attendees; an empty list means cooperative/no winner.
Saving an ongoing plan marks it done. Reports use the plan's normal read access.

Best-time ranking treats out/no as unavailable, in+yes as definite, maybe+yes
and if-needed as flexible. Missing answers stay unknown. Host conflicts sort
last, then definite attendance, flexibility, fewer declines, and time order.
