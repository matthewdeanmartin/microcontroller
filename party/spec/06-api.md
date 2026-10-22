# 06 — REST API (board, `party.local`)

Conventions are the framework's and Game Matcher's: `/api/v1`, JSON by
default (other formats via `?fmt=`), schema at `/api/v1/schema`, errors as
`{"error": "<code>", "message": "…", "detail": {…}}`, cursor pagination
(`?after=<id>&limit=≤50`), `rev` on every mutable resource with `If-Match` or
a `rev` body field (stale → 409 `stale`), bounds → 409 `full`, same-origin guard
on unsafe methods, rate limits on auth and search.

External-channel commands don't use REST. They enter the service layer from the
inbox worker. REST is the local app's API only.

| Resource | Endpoints |
|---|---|
| Setup | `GET /setup`, `POST /setup` (first admin) |
| Auth | `POST /auth/pin`, `POST /auth/signup`, `POST /auth/logout`, `POST /auth/logout-all`, `GET /auth/me`, `POST /auth/pin-reset`, `POST /auth/pin-reset/confirm` |
| OAuth | `POST /auth/oauth/start {provider, purpose: signin|link|redeem}`, `GET /auth/oauth/ticket/{id}`, `GET /auth/callback` |
| Me | `GET /me/summary` (bell, polled), `PATCH /me`, `GET /me/link` (QR + link), `POST /me/link/rotate`, `DELETE /me/identities/{i}` |
| People | `GET /people?q=`, `GET /people/{id}` (fields by discoverability) |
| Households | `GET/POST /households`, `PATCH/DELETE /households/{id}`, `PUT /households/{id}/members/{person}` |
| Groups | `GET /groups?q=&mine=`, `POST /groups`, `GET/PATCH/DELETE /groups/{id}` |
| Membership | `GET /groups/{id}/members`, `POST /groups/{id}/join`, `POST /groups/{id}/leave`, `PUT /groups/{id}/members/{person}` (role/status), `POST /groups/{id}/transfer` |
| Series | `POST /groups/{id}/series`, `GET/PATCH/DELETE /series/{id}`, `PUT /series/{id}/regular` |
| Events | `GET /events?group=&from=&to=&mine=`, `POST /groups/{id}/events`, `GET/PATCH/DELETE /events/{id}`, `POST /events/{id}/cancel`, `POST /events/{id}/lock {when option}` |
| RSVPs | `GET /events/{id}/rsvps`, `PUT /events/{id}/rsvps/me`, `PUT /events/{id}/rsvps/{person}` (household member or host, records `by`), `GET /events/{id}/rides` (hosts) |
| Invitations | `POST /invitations`, `GET /invitations?target=`, `GET /invitations/{token}` (preview), `POST /invitations/{token}/redeem`, `POST /invitations/{id}/revoke` |
| Calendar | `GET /events/{id}.ics`, `GET /cal/{token}.ics`, `POST /me/cal/rotate` |
| Admin | `/admin/people`, `/admin/groups`, `/admin/settings`, `/admin/audit`, `/admin/storage` |
| Channels | `GET/PUT /admin/bot`, `GET/PUT /admin/mail`, `GET/PUT /admin/llm`, `GET/PUT /admin/relay`, `GET /admin/delivery`, `POST /admin/delivery/{id}/retry`, `GET /admin/inbox` (recent commands and results) |
| Publication | `GET/PUT /admin/publish` (repo, token, cadence), `POST /admin/publish/now`, `GET /sync/status` (public: last publish, generation, bot last poll) |
| Health | framework `/api/v1/sys`, `/.well-known/ops/health` (app checks: store, bot poll age, publish age, outbound backlog) |

Admin channel settings that hold secrets accept them write-only: `GET` shows
"set, ends in …4f2a, expires …" and never the value.
