# 04 — API

Framework conventions apply ([RECIPES.md](../../miniframework/docs/RECIPES.md)):
`message!` types, any of the five wire formats via `?fmt=` / `Accept`,
`ErrorBody` errors, `Server-Timing` on every response, `/api/v1/schema`.
The UI uses JSON unless the format lab says otherwise.

Auth: the `gm_session` cookie. `Origin` is checked on every non-GET ([02](02-accounts-auth.md#sessions)).
Every endpoint enforces permissions itself and never relies on the UI to hide things.

Errors: `{ "code": "plan_closed", "message": "…" }`, using HTTP status
400/401/403/404/409/413/429/503. 503 with `Retry-After` when an outbound job slot is busy.

## Public (no session)

| Method | Path | Notes |
|---|---|---|
| GET | `/api/v1/status` | app version, `setup_needed`, `signup_open`, clock state, providers for the sign-in page (name + id only) |
| POST | `/api/v1/setup` | first-run admin `{email, display_name, pin}`. 409 once any account exists |
| POST | `/api/v1/auth/pin` | `{email, pin}` → sets cookie. 429 when rate limited |
| POST | `/api/v1/auth/signup` | `{email, display_name, pin}`. 403 when sign-up is closed |
| GET | `/auth/mastodon/{provider}` | starts OAuth (302 to the instance) |
| GET | `/auth/callback` | OAuth return. 302 to `/` or `/link-result` |
| GET | `/cal/{token}.ics`, `/cal/house/{token}.ics` | feeds |
| GET | `/api/v1/display` | LCD/remote-display summary, see [07](07-display.md). No personal data beyond first names |
| — | `/trust`, `/ca`, `/api/v1/sys`, `/metrics` | framework built-ins |

## Signed in

| Method | Path | Notes |
|---|---|---|
| POST | `/api/v1/auth/logout` | `?all=1` for every session |
| GET | `/api/v1/me` | profile, interests, linked identity, notify settings |
| PATCH | `/api/v1/me` | display name, notify settings |
| PUT | `/api/v1/me/pin` | `{current, new}` (current omitted only if no PIN set) |
| GET/POST/DELETE | `/api/v1/me/feed` | feed on? / new URL (shown once; the old one dies) / off |
| DELETE | `/api/v1/me/mastodon` | unlink |
| GET | `/auth/mastodon/{provider}?link=1` | link to current account |
| GET | `/api/v1/me/summary` | unread count + plans awaiting my response. **The** polling endpoint |
| GET | `/api/v1/notifications` | `?before=` paging |
| POST | `/api/v1/notifications/read` | `{up_to}` or `{ids}` |
| GET | `/api/v1/users` | id, display name, role only |
| GET | `/api/v1/games` | `?archived=1` |
| POST | `/api/v1/games` | 409 `duplicate_title` returns the existing id |
| GET/PATCH | `/api/v1/games/{id}` | PATCH: adder or admin |
| PUT/DELETE | `/api/v1/games/{id}/interest` | |
| GET | `/api/v1/plans` | `?status=open,on` (default), `done`, `cancelled`, `?before=` |
| POST | `/api/v1/plans` | |
| GET | `/api/v1/plans/{id}` | plan + all responses + counts |
| PATCH | `/api/v1/plans/{id}` | host/admin. Editing times/games, notes, seats, where |
| POST | `/api/v1/plans/{id}/lock` | `{time, games?}` → `on` |
| POST | `/api/v1/plans/{id}/reopen` | `on` → `open` |
| POST | `/api/v1/plans/{id}/cancel` | |
| POST | `/api/v1/plans/{id}/host` | hand over `{user}` |
| PUT | `/api/v1/plans/{id}/response` | the caller's whole response, replaced atomically |
| GET | `/api/v1/plans/{id}/calendar.ics` | download (one event, or one tentative event per time while open) |
| GET/POST | `/api/v1/plans/{id}/comments` | `?after=<id>`, 25 per page (`more` says if there are more) |
| PATCH/DELETE | `/api/v1/plans/{id}/comments/{cid}` | author; DELETE also admin |

## Admin

| Method | Path | Notes |
|---|---|---|
| GET/PATCH | `/api/v1/admin/settings` | open sign-up |
| GET/POST/DELETE | `/api/v1/admin/house-feed` | household feed on? / new URL / off |
| POST/PATCH | `/api/v1/admin/users/{id}` | role, status, reset PIN, sign out, unlink |
| POST | `/api/v1/admin/games/{id}/merge` | `{into}` |
| DELETE | `/api/v1/admin/games/{id}` | archive if referenced |
| GET/POST/PATCH/DELETE | `/api/v1/admin/providers[/{id}]` | Mastodon instances, trust, invite lists |
| GET/PUT | `/api/v1/admin/bot` | [08](08-mastodon.md) |
| GET | `/api/v1/admin/outbound` | queue entries, failures, retry |
| GET | `/api/v1/admin/storage` | slot generation, bytes used per collection vs bound |

## Example: respond to a plan

```http
PUT /api/v1/plans/12/response
Content-Type: application/json
Origin: http://gamematcher.local

{ "overall": "in",
  "times": [ { "time": 4, "answer": "yes" }, { "time": 5, "answer": "if_needed" } ],
  "games": [3, 17],
  "note": "can bring snacks" }
```

Each time option has an id that is stable within the plan. Editing a time's
start or end gives it a **new** id and drops answers to the old one. A response
naming an id (or game) the plan no longer has gets 409 `plan_changed`, so the
UI re-fetches. This stops a "yes" from attaching to a time the host just
edited. (The first draft used index + rev; ids do the same with less
bookkeeping.)
