# JSON API v1

The API serves JSON only; the existing Angular / `nanacoin_web` client is hosted separately. Desktop uses loopback HTTP on port 8080 and firmware uses HTTPS. CORS allows configured exact origins, with GET/POST/PATCH/OPTIONS and Authorization, Content-Type and Idempotency-Key headers. Request bodies are limited to 1,024 bytes.

## Provision and login

`GET /api/v1/status` is public. On an empty journal, `POST /api/v1/provision` takes:

```json
{"household_name":"Home","username":"nana","display_name":"Nana","password":"1234"}
```

Provisioning is allowed once. Login follows the existing client's PKCE flow:

1. Generate a random verifier of 43–128 PKCE characters. Its S256 challenge is unpadded base64url of SHA-256(verifier).
2. POST `/api/v1/auth/authorize` with username, password, code_challenge, code_challenge_method set to S256, and redirect_uri. Receive a code.
3. POST `/api/v1/auth/token` with code, code_verifier and the same redirect_uri. Receive access_token, token_type Bearer, expires_in 28800 and a user object.
4. Send `Authorization: Bearer <session-token>` on authenticated requests. POST `/api/v1/auth/logout` to revoke it.

Codes expire after 60 seconds and are consumed on redemption, including unsuccessful redemption. Sessions expire after eight hours and do not survive restart. These are temporary session credentials, not permanent account tokens. Passwords are hashed on the server; HTTP clients cannot inject credential verifiers.

## Existing client adapter

IDs use user-1, account-1, listing-5 and tx-6 forms. Roles are nana/user and member statuses ACTIVE/DISABLED. All paths below have prefix `/api/v1`.

| Endpoint | Purpose |
|---|---|
| GET `/me`, `/users` | Current user and members |
| POST `/users` | Nana creates member with username, display_name, password, optional role/grant |
| PATCH `/users/user-N` | Name/password; Nana can also change role/status or other members |
| GET `/transactions`, `/transactions/tx-N` | Recent ledger and individual transaction |
| GET `/accounts/account-N`, `/accounts/account-N/transactions` | Own balance/history, or any account for Nana; `account-N-usd` selects dollars |
| POST `/transfers` | Transfer with to, amount, memo |
| POST `/admin/issue`, `/admin/retire` | Nana issuance/retirement |
| POST `/transactions/tx-N/reverse` | Nana reversal with reason |
| GET/POST `/listings` | Query/create listings |
| GET `/listings/listing-N` | Read one listing |
| PATCH `/listings/listing-N` | Edit title, description, price |
| POST `/listings/listing-N/purchase`, `/listings/listing-N/cancel` | Purchase/cancel |
| GET/PATCH `/admin/config` | household_name, initial_grant, currency, offer_settles_after (seconds; zero restores 48 hours) |
| GET `/offers`, `/offers/offer-N` | Visible offers, newest first, or one visible offer |
| POST `/listings/listing-N/offers` | Propose amount and optional message (140 UTF-8 bytes); returns 201 |
| POST `/offers/offer-N/accept` | Owner accepts; requires Idempotency-Key; returns 201 |
| POST `/offers/offer-N/unaccept` | Either party or Nana undoes before deadline, with reason (140 UTF-8 bytes) and Idempotency-Key |
| POST `/offers/offer-N/decline` | Listing owner or Nana declines |
| POST `/offers/offer-N/withdraw` | Offerer or Nana withdraws |
| POST `/admin/issue-usd` | Nana issues cents to a coin account ID; fields to, cents, reason; keyed |
| GET/POST `/quotes` | Best-rate-first book; create with side BID/ASK, cents_per_coin, coins, optional expires_at |
| GET `/quotes/quote-N` | Quote, wallet parties, rate, timestamps and live status |
| POST `/quotes/quote-N/take` | Keyed, atomic coin/cash exchange; returns quote, coin_transaction, cash_transaction |
| POST `/quotes/quote-N/cancel` | Maker or Nana cancels |

See `src/client.rs` and `src/client/offers.rs` for bounded schemas and response views. Offer views include id, listing, listing_title, offerer, offerer_name, amount, message, status, created_at, updated_at, reversible, and (after acceptance) settled_tx and settles_at. Accept/unaccept return `{ "offer": {...}, "transaction": {...} }`. Statuses are OPEN, ACCEPTED, SETTLED, DECLINED, WITHDRAWN and REVERSED. Settlement is computed from the persisted deadline; GET never writes a settlement event. `/state` is Nana-only and omits private offers; use the visibility-filtered offer routes. `/transactions` is Nana-only; individual transactions require participation or Nana. `/users` hides other members' balances from ordinary users. Member and listing timestamps persist. Listings retain kind (item/service/currency), currency and minor_units, and include buyer_name when sold.

Only the owner may accept, including when Nana is another member. SELL debits the offerer; BUY debits the owner. Acceptance checks both accounts and funds and closes the listing. Decline/withdraw move no money. At the exact deadline, unaccept refuses even for Nana. Within the window it allows a correction overdraft and atomically reverses payment, marks the offer REVERSED and reopens the listing. It works even after the original payment leaves recent history. Manual Nana reversals remain separate and prevent duplicate refunds. Reusing an acceptance key after unaccept returns its original acceptance receipt while retained; it does not accept again.

Money requests supply an `Idempotency-Key` of 1–80 bytes. Retry with the same key and command after a timeout. Per-member receipts persist through restart and intervening commands; different commands with the same key conflict. A retry requiring a transaction outside the recent window may report stale_request; it never repeats the movement.

## Typed command API

Nana-only `GET /api/v1/state` returns member, state and storage_failed fields, excluding credential verifiers and internal retry fingerprints. `POST /api/v1/commands` accepts an externally tagged Rust command:

```json
{"request_id":2,"command":{"issue":{"to":1,"amount":25,"memo":"Chores"}}}
```

Variants include issue, retire, transfer, list, update_listing, cancel, buy, reverse, make_offer, accept_offer, unaccept_offer, decline_offer, withdraw_offer issue_usd, post_quote, take_quote, cancel_quote, and Nana-only configure. Offer commands use a typed OfferId represented as an integer. Timestamps are server-owned event metadata, not command fields. Credential creation/update/migration variants are rejected here; use dedicated endpoints. Obsolete add_member exists for legacy journal replay only and cannot be submitted over HTTP.

The typed endpoint uses per-member monotonic request_id values: start at last_request + 1. The same most-recent ID and command returns the original sequence receipt with replayed=true. A changed command conflicts; an older ID is stale. Failed commands do not advance the watermark. Do not assign a fresh ID to an uncertain operation. The adapter's durable Idempotency-Key index provides stronger retry history than this last-request contract.

Creation/money endpoints return 201; updates, cancellation and unaccept return 200; logout returns 204 without a body. Ledger pages default to 100 globally or 50 per account, cap at 100, and read from the 365-record ring. Listings are newest first, with validated status filters.

Quote rates are 1–10,000 cents/coin and sizes 1–100,000 coins; there are 16 slots. Proposals do not reserve funds; taking validates both currencies and active parties. Timed quote operations require the same valid clock as offers. Expired quotes display EXPIRED and can be recycled. Transaction IDs are opaque: forex cash-leg IDs use the disjoint event-sequence + 4096 range so existing Rust event/transaction IDs remain compatible. Use response order and timestamps, not numeric ID sorting.

## Diagnostics

`GET /api/v1/diag` needs no authentication and takes no ledger lock, so it
answers while a write is in flight. It is sampled every two seconds by a task
pinned to core 0, away from the HTTPS workers and the ledger on core 1.

```json
{"uptime_seconds":977,"free_heap":159683,"largest_free_block":63488,
 "minimum_free_heap":80599,"psram_free":7359116,"samples":480,
 "sampler_core":0,"http_core":1}
```

Sizes are bytes of internal (DMA-capable) heap, except `psram_free`.
`largest_free_block` is the fragmentation signal: it holding steady while
`free_heap` moves means the heap is not fragmenting. `minimum_free_heap` is the
low-water mark since boot and never recovers, by design. Firmware only; the
desktop server does not serve this route.

## Errors and limits

Errors contain error and message fields. Statuses include 400 invalid input/overflow, 401 authentication failure, 403 forbidden/disabled, 404 not found, 409 conflict/stale request/insufficient funds, 413 oversized body, 429 login rate limit, 503 storage/availability failure and 507 capacity exhaustion.

Offer-specific errors include self_deal (400), offer_closed, listing_closed and offer_settled (409). An invalid or unsynchronized settlement clock returns unavailable (503). Timed mutations never reset a persisted deadline on restart.

Every movement balances debit and credit, including issuance account zero. Ordinary spending cannot overdraw. Corrections can make balances negative, matching TinyGo; they append opposite postings and never rewrite history. Journal and recent-history limits are explicit in README.md; retention is bounded.
