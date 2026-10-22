# Installing party's outside world

The board works on its own at `http://party.local`. This guide adds what
reaches people outside the house, in the order the sprints built it:
email (Brevo), a Mastodon bot, and the companion site on GitHub Pages. Each
part is optional. None of them opens the board to the internet: it only ever
makes outbound connections.

Secrets are pasted into the board's **Admin → Email, bot and companion
site** page. The board keeps them, never shows them back (only their last
four characters), and isn't encrypted at rest. Anyone with the board and a
USB cable could read them.

## 1. Email (Brevo)

1. Make a free Brevo account. Under **Senders**, add and verify the address
   the board sends from.
2. Under **SMTP & API → API keys**, create an **API key** (`xkeysib-…`). An
   SMTP key (`xsmtpsib-…`) won't work.
3. Paste it on the board. It checks the key and finds the sender: the state
   goes `checking` → `ready`.

The board sends invitations, changes, reminders and cancellations, with
calendar invitations attached. At most 100 a day (Brevo's free plan allows
300). Replies go to the host's own email, not the board.

## 2. The Mastodon bot

1. Make an account for the bot on an instance (mastodon.social to start),
   and tick "This is a bot account" in its profile.
2. In its **Preferences → Development**, create an application with the
   scopes `read:notifications read:statuses write:statuses`, and copy its
   access token.
3. Paste the instance and token on the board. It checks them and shows the
   bot's handle.

People link their own Mastodon accounts on their profile (Me → Mastodon). Then
they can message the bot privately (`going E12`, `events`, `help`). The first
check only finds where "now" is: nothing sent before the bot was set up runs.

To let people sign in with Mastodon, add their instances under **Sign in
with Mastodon**.

## 3. The companion site (GitHub Pages)

Everything published is public, history included, so the board only
publishes public groups' public events, without addresses
(`spec/05-companion-site.md`).

1. **Make a free GitHub organization** for it (e.g. `ourparty`). Its sites
   live at `https://ourparty.github.io/`, an origin no other site of yours
   shares. That matters later, for personal links kept in the browser.
2. In it, make two public repositories:
   - `party-data`: leave it empty. The board writes it.
   - `party-site`: the site itself (step 5).
3. Make a **fine-grained personal access token**: resource owner the
   organization, repository access **only `party-data`**, permission
   **Contents: read and write**. Give it an expiry you'll remember. When it
   runs out, publication stops and the board's admin page says so.
4. On the board: data repo `ourparty/party-data`, branch `main`, the token,
   and the site's address `https://ourparty.github.io/party-site/`. Then
   **Publish now**. The first publication creates the branch.
5. In `party-data` → Settings → Pages: deploy from the branch `main`, folder
   `/` (root).
6. The site: run `make site demo` here, then copy `ui/dist/companion/browser/*`
   to the root of `party-site`, and `ui/dist/demo/browser/*` to `party-site/demo/`.
   Copy `deploy/party-site/.github/` in too, and push. Its workflow publishes
   the repo to Pages (Settings → Pages → Source: GitHub Actions).

The site reads `../party-data/data/…` on the same origin. After a change on
the board, expect the site to catch up within about 20 minutes: the board
publishes at most every 10, and Pages caches for about 10.
