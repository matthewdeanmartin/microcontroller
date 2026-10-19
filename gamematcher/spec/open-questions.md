# Open questions and assumptions

Written October 17, 2026. Confirm or correct the open items and record the answers below.

| # | Question | Status |
|---|---|---|
| 1 | **Which board?** The S2 was ruled out for TLS/OAuth memory, but the C6 being repaved has *less* RAM (no PSRAM). Minicloud saw HTTPS crash it. | **Answered by measurement:** the C6 passed the gate with HTTP + HTTPS/h2 + outbound HTTPS ([c6-gate.md](measurements/c6-gate.md)). |
| 2 | "Authenticating by email": an email + PIN login with no mail sent, or emailed magic links? | Assumed: email is a login name only. No SMTP in v1. |
| 3 | Guests: full accounts or a throwaway pass? | **Answered:** full accounts, open sign-up **on by default**, admin can switch it off. |
| 4 | Private plans? | Assumed: all plans visible to all signed-in users. |
| 5 | Can anyone add time options/games to someone else's plan directly? | Assumed: no, they suggest in a comment and the host accepts. |
| 6 | Mastodon bot and DMs in v1? | Assumed: yes, as Sprint 8, entirely optional. |
| 7 | First-run setup code? | Assumed: off by default, optional build-time code. |
| 8 | Does mastodon.social accept an `http://*.local` redirect URI? | **Answered:** the owner reports Mastodon is permissive (localhost:4200 works). Confirmed in Sprint 6. |
| 9 | Minicloud's data before the repave? | **Answered:** ephemeral, not kept. Repave authorized. |
| 10 | Where does the code live? | **Answered:** `microcontroller/gamematcher/` in the monorepo until it outgrows it. |

## Decisions log

| Date | Decision |
|---|---|
| 2026-10-17 | Owner decisions 1–17 in [README.md](README.md). |
| 2026-10-17 | Storage is NVS blobs via the framework `Kv` (two snapshot slots + per-plan comment blobs), not raw partition writes. |
| 2026-10-17 | PIN rate limits: per account and board-wide. The framework has no client IP to key on. |
| 2026-10-17 | Gate passed on the C6 after fixing a framework livelock (serving task vs TLS handoff on one core). PINs use 1,000 PBKDF2 rounds (99 ms on the C6). |
| 2026-10-17 | The household CA's critical Name Constraints stopped mbedTLS from parsing it. Owner decision: reissue the CA certificate (same key and subject) with the constraints non-critical. Done the same day; no redeploys needed. |
