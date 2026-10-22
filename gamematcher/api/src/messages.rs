//! Every message Game Matcher sends, accepts or stores. Tags are protobuf
//! field numbers and CBOR integer keys: add fields with new numbers, never
//! renumber. Stored messages (`Stored*`) are the flash records' schema.
use miniframework::message;

// ---- API -----------------------------------------------------------------

message! {
    /// `POST /api/v1/statuses` (Mastodon): the fields we send.
    pub struct StatusPost {
        1 status: String,
        /// `public`, `unlisted`, `private` or `direct`.
        2 visibility: String,
    }
}

message! {
    /// One queued or sent bot post, for the admin page.
    pub struct OutboxEntry {
        1 id: u32,
        /// `post` or `dm`.
        2 kind: String,
        /// The first 80 characters.
        3 text: String,
        /// `queued`, `sent` or `failed`.
        4 state: String,
        5 attempts: u32,
        6 error: String,
        7 created: u64,
        /// The status on the instance, once sent.
        8 url: String,
        9 next_at: u64,
    }
}

message! {
    /// `GET /api/v1/admin/bot`.
    pub struct BotView {
        1 configured: bool,
        2 handle: String,
        3 instance: String,
        4 visibility: String,
        /// Post the day's plans every morning.
        5 digest: bool,
        /// Local "HH:MM".
        6 digest_time: String,
        7 posts: u32,
        8 last_post: u64,
        9 last_error: String,
        /// The last setup: "", "working", "ready: @handle" or "failed: why".
        10 setup: String,
        /// Newest first.
        11 jobs: Vec<OutboxEntry>,
        12 replies: bool,
        13 last_poll: u64,
        14 reply_error: String,
    }
}

message! {
    /// `POST /api/v1/admin/bot`: create a bot on a mastomini (admin
    /// username + password, used once and never stored) or use an
    /// existing account's access token.
    pub struct BotSetup {
        1 instance: String,
        2 token: String,
        /// For a token: `household-ca` or `public` (default).
        3 anchor: String,
        4 admin: String,
        5 password: String,
        6 username: String,
        7 display_name: String,
    }
}

message! {
    /// `GET /api/v1/admin/mail`.
    pub struct MailView {
        1 configured: bool,
        /// `Name <address>` mail is sent from.
        2 sender: String,
        3 sent: u32,
        4 sent_today: u32,
        5 daily_limit: u32,
        6 last_error: String,
        /// The last setup: "", "working", "ready: ..." or "failed: why".
        7 setup: String,
        /// Configured from the firmware's build settings.
        8 built_in: bool,
    }
}

message! {
    /// `POST /api/v1/admin/mail`: a Brevo API key (`xkeysib-...`); the
    /// sender defaults to the account's first verified one.
    pub struct MailSetup {
        1 api_key: String,
        2 sender_email: String,
        3 sender_name: String,
    }
}

message! {
    /// `POST /api/v1/plans/{id}/invite` and `POST /api/v1/invite`.
    pub struct InviteEmails {
        1 emails: Vec<String>,
    }
}

message! {
    pub struct Sent {
        1 sent: u32,
    }
}

message! {
    /// `POST /api/v1/auth/pin-reset`.
    pub struct PinResetStart {
        1 email: String,
    }
}

message! {
    /// `POST /api/v1/auth/pin-reset/confirm`.
    pub struct PinResetConfirm {
        1 email: String,
        2 code: String,
        3 pin: String,
    }
}

message! {
    /// `PATCH /api/v1/admin/bot`.
    pub struct BotSettings {
        1 visibility: Option<String>,
        2 digest: Option<bool>,
        3 digest_time: Option<String>,
        4 replies: Option<bool>,
    }
}

message! {
    /// `GET /api/v1/status`: public, drives the sign-in and setup pages.
    pub struct Status {
        1 app: String,
        2 version: String,
        3 build: String,
        /// No account exists yet: the next visitor claims admin.
        4 setup_needed: bool,
        5 signup_open: bool,
        /// The board's clock has been set by SNTP since boot.
        6 clock_synced: bool,
        /// A setup code is compiled in and `/api/v1/setup` needs it.
        7 setup_code_required: bool,
        /// Arrived over TLS.
        8 secure: bool,
        /// "Sign in with ..." buttons (empty until setup is done).
        9 providers: Vec<ProviderButton>,
        /// The household's Mastodon bot (`@gamematcher@host`), "" when none:
        /// the plan form offers "post to Mastodon" and threads offer DMs.
        10 bot: String,
        /// The board can send email (invites, "forgot PIN", notifications).
        11 mail: bool,
        12 public_bot: String,
    }
}

message! {
    pub struct ProviderButton {
        1 id: u32,
        2 name: String,
    }
}

message! {
    /// `POST /api/v1/setup` and `POST /api/v1/auth/signup`.
    pub struct NewAccount {
        1 email: String,
        2 display_name: String,
        3 pin: String,
        /// Only for `/setup` when the build has a setup code.
        4 setup_code: String,
    }
}

message! {
    /// `POST /api/v1/auth/pin`.
    pub struct PinSignIn {
        1 email: String,
        2 pin: String,
    }
}

message! {
    /// The signed-in user's own view.
    pub struct Me {
        1 id: u32,
        2 email: String,
        3 display_name: String,
        /// `admin` or `member`.
        4 role: String,
        5 has_pin: bool,
        /// Signed-in browsers for this account.
        6 sessions: u32,
        7 created: u64,
        /// Linked Mastodon account (`@user@host`), or empty.
        8 mastodon: String,
        /// Plan news by email (on unless turned off; needs an email address).
        9 email_notify: bool,
    }
}

message! {
    /// `PATCH /api/v1/me`: fields left out are unchanged.
    pub struct MeUpdate {
        1 display_name: Option<String>,
        2 email_notify: Option<bool>,
    }
}

message! {
    /// `PUT /api/v1/me/pin`.
    pub struct PinChange {
        /// Empty only while the account has no PIN.
        1 current: String,
        2 new_pin: String,
    }
}

message! {
    /// What every signed-in user may see about another.
    pub struct UserSummary {
        1 id: u32,
        2 display_name: String,
        3 role: String,
    }
}

message! {
    pub struct UserList {
        1 users: Vec<UserSummary>,
    }
}

message! {
    /// The admin's view of an account.
    pub struct AdminUser {
        1 id: u32,
        2 email: String,
        3 display_name: String,
        4 role: String,
        5 disabled: bool,
        6 has_pin: bool,
        7 sessions: u32,
        8 created: u64,
        9 last_seen: u64,
        /// PIN sign-in is locked for this many more seconds.
        10 locked_secs: u32,
        11 mastodon: String,
    }
}

message! {
    pub struct AdminUserList {
        1 users: Vec<AdminUser>,
    }
}

message! {
    /// `PATCH /api/v1/admin/users/{id}`: fields left out are unchanged.
    pub struct AdminUserUpdate {
        /// `admin` or `member`.
        1 role: Option<String>,
        2 disabled: Option<bool>,
        /// A temporary PIN the admin tells the person.
        3 pin: Option<String>,
        /// End every session of this account.
        4 sign_out: Option<bool>,
        5 display_name: Option<String>,
    }
}

message! {
    pub struct Settings {
        1 signup_open: bool,
    }
}

message! {
    /// `PATCH /api/v1/admin/settings`.
    pub struct SettingsUpdate {
        1 signup_open: Option<bool>,
    }
}

message! {
    /// `GET /api/v1/admin/storage`.
    pub struct StorageInfo {
        1 generation: u64,
        2 snapshot_bytes: u32,
        3 commits: u64,
        4 accounts: u32,
        5 max_accounts: u32,
        6 sessions: u32,
        7 max_sessions: u32,
        /// The slot boot loaded from, and whether the other one was valid.
        8 loaded_slot: u32,
        9 fallback_used: bool,
        /// Records on flash (rows, sessions, meta, index).
        10 records: u32,
        /// Bytes the last commit wrote (journal included).
        11 last_commit_bytes: u32,
        /// Largest commit since boot.
        12 max_commit_bytes: u32,
        /// Records that failed to load (left on flash, not served).
        13 quarantined: Vec<String>,
        /// This boot converted the old snapshot slots to records.
        14 migrated: bool,
    }
}

message! {
    /// A body-less success.
    pub struct Done {
        1 ok: bool,
    }
}

message! {
    /// A game as the catalog shows it.
    pub struct GameView {
        1 id: u32,
        2 title: String,
        3 kind: String,
        4 min_players: u32,
        5 max_players: u32,
        6 minutes: u32,
        7 notes: String,
        8 added_by: u32,
        9 added_by_name: String,
        10 archived: bool,
        /// How many people are into it.
        11 interested: u32,
        /// The caller is into it.
        12 mine: bool,
        /// The caller may edit it (added it, or admin).
        13 editable: bool,
    }
}

message! {
    pub struct GameList {
        1 games: Vec<GameView>,
    }
}

message! {
    pub struct GameDetail {
        1 game: GameView,
        2 interested: Vec<UserSummary>,
        /// Plans that include it, newest first.
        3 plans: Vec<PlanSummary>,
    }
}

message! {
    /// `POST /api/v1/games` and `PATCH /api/v1/games/{id}` (fields left out
    /// are unchanged on PATCH).
    pub struct GameInput {
        1 title: Option<String>,
        2 kind: Option<String>,
        3 min_players: Option<u32>,
        4 max_players: Option<u32>,
        5 minutes: Option<u32>,
        6 notes: Option<String>,
        7 archived: Option<bool>,
    }
}

message! {
    pub struct Merge {
        1 into: u32,
    }
}

message! {
    /// A time option with who can make it.
    pub struct PlanTime {
        1 id: u32,
        2 start: u64,
        3 end: u64,
        4 yes: Vec<u32>,
        5 if_needed: Vec<u32>,
        6 no: Vec<u32>,
    }
}

message! {
    /// A candidate game with who would play it.
    pub struct PlanGame {
        1 id: u32,
        2 title: String,
        3 kind: String,
        4 min_players: u32,
        5 max_players: u32,
        6 minutes: u32,
        7 archived: bool,
        8 ticks: Vec<u32>,
    }
}

message! {
    pub struct ResponseView {
        1 account: u32,
        2 name: String,
        3 overall: String,
        4 times: Vec<TimeAnswer>,
        5 games: Vec<u32>,
        6 note: String,
        7 updated: u64,
    }
}

message! {
    /// One plan, everything the plan screen shows.
    pub struct PlanView {
        1 id: u32,
        2 host: u32,
        3 host_name: String,
        4 title: String,
        5 notes: String,
        6 place: String,
        7 seats: u32,
        8 status: String,
        9 games: Vec<PlanGame>,
        10 times: Vec<PlanTime>,
        11 responses: Vec<ResponseView>,
        12 chosen_time: Option<u32>,
        13 chosen_games: Vec<u32>,
        14 created: u64,
        15 updated: u64,
        /// The caller may edit, lock, cancel or hand it over.
        16 manageable: bool,
        /// The household bot posts its news.
        17 announced: bool,
        18 announced_public: bool,
        19 series: Vec<u32>,
        20 report: Option<PostGameReport>,
        21 reportable: bool,
    }
}

message! {
    /// A plan in a list.
    pub struct PlanSummary {
        1 id: u32,
        2 title: String,
        3 status: String,
        4 host_name: String,
        5 games: Vec<String>,
        /// The chosen time if locked in, else the earliest option (0 = none).
        6 start: u64,
        7 end: u64,
        8 times: u32,
        9 in_count: u32,
        10 maybe_count: u32,
        /// The caller has answered.
        11 answered: bool,
        12 seats: u32,
        13 updated: u64,
    }
}

message! {
    pub struct PlanList {
        1 plans: Vec<PlanSummary>,
    }
}

message! {
    /// A time option in a plan being written: `id` 0 is a new one; an
    /// existing id with a different start or end becomes a new option.
    pub struct TimeInput {
        1 id: u32,
        2 start: u64,
        3 end: u64,
    }
}

message! {
    /// `POST /api/v1/plans` and `PATCH /api/v1/plans/{id}` (fields left out
    /// are unchanged on PATCH; games and times only while open).
    pub struct PlanInput {
        1 title: Option<String>,
        2 notes: Option<String>,
        3 place: Option<String>,
        4 seats: Option<u32>,
        5 games: Option<Vec<u32>>,
        6 times: Option<Vec<TimeInput>>,
        /// Post it from the household bot (and its lock-in / cancellation).
        7 announce: Option<bool>,
        /// Move the locked-in time atomically and notify attendees.
        8 reschedule: Option<TimeInput>,
        9 repeat: Option<RepeatInput>,
        10 announce_public: Option<bool>,
    }
}

message! {
    /// `PUT /api/v1/plans/{id}/response`: replaces the caller's response.
    pub struct ResponseInput {
        1 overall: String,
        2 times: Vec<TimeAnswer>,
        3 games: Vec<u32>,
        4 note: String,
    }
}

message! {
    /// `POST /api/v1/plans/{id}/lock`.
    pub struct LockIn {
        1 time: u32,
        /// Empty: decide on the night.
        2 games: Vec<u32>,
    }
}

message! {
    /// `POST /api/v1/plans/{id}/host`.
    pub struct HandOver {
        1 user: u32,
    }
}

message! {
    pub struct CommentView {
        1 id: u32,
        2 author: u32,
        3 author_name: String,
        4 text: String,
        5 created: u64,
        /// 0 = never edited.
        6 edited: u64,
        7 mine: bool,
        /// The caller may delete it (author or admin).
        8 deletable: bool,
    }
}

message! {
    pub struct CommentList {
        1 comments: Vec<CommentView>,
        /// More comments after the last one here (`?after=<last id>`).
        2 more: bool,
        /// The thread accepts new comments.
        3 open: bool,
        /// Mastodon DMs queued for the comment just posted.
        4 sent_dms: u32,
    }
}

message! {
    pub struct CommentInput {
        1 text: String,
        /// Also DM it (from the household bot) to everyone coming who has a
        /// Mastodon account the bot can reach. Only on create.
        2 dm: bool,
    }
}

message! {
    pub struct NotificationView {
        1 id: u32,
        /// `new_plan`, `locked`, `cancelled`, `comments`, `responses`, `reminder`, `host`.
        2 kind: String,
        3 plan: u32,
        4 plan_title: String,
        /// Coalesced events (e.g. 3 new comments).
        5 count: u32,
        /// Who did the latest of them.
        6 actor_name: String,
        7 created: u64,
        8 read: bool,
    }
}

message! {
    pub struct NotificationList {
        1 notifications: Vec<NotificationView>,
    }
}

message! {
    /// `POST /api/v1/notifications/read`: everything up to and including
    /// `up_to`, or all when 0.
    pub struct MarkRead {
        1 up_to: u32,
    }
}

message! {
    /// `GET /api/v1/me/summary`: the one endpoint the UI polls.
    pub struct Summary {
        1 unread: u32,
        /// Open plans the caller hasn't answered.
        2 awaiting: u32,
        /// Newest notification id (the UI refreshes lists when it changes).
        3 latest: u32,
    }
}

message! {
    /// A calendar feed URL, shown once when (re)issued.
    pub struct FeedUrl {
        1 url: String,
        /// The same as webcal:// for one-tap subscribe.
        2 webcal: String,
        3 enabled: bool,
    }
}

message! {
    /// A Mastodon instance people may sign in with (admin view).
    pub struct ProviderView {
        1 id: u32,
        2 name: String,
        /// `https://mastodon.social`
        3 base: String,
        /// `household` (anyone on it) or `invited` (only listed handles).
        4 trust: String,
        /// `household-ca` or `public`: what vouches for its certificate.
        5 anchor: String,
        /// `@user@host` handles allowed when trust is `invited`.
        6 invites: Vec<String>,
    }
}

message! {
    pub struct ProviderList {
        1 providers: Vec<ProviderView>,
    }
}

message! {
    /// `POST /api/v1/admin/providers`, `PATCH .../{id}`: left out = unchanged.
    pub struct ProviderInput {
        1 name: Option<String>,
        2 base: Option<String>,
        3 trust: Option<String>,
        4 anchor: Option<String>,
        5 invites: Option<Vec<String>>,
    }
}

message! {
    /// `POST /api/v1/auth/oauth/start`.
    pub struct OAuthStart {
        1 provider: u32,
        /// Link to the signed-in account instead of signing in.
        2 link: bool,
    }
}

message! {
    pub struct OAuthTicket {
        1 ticket: String,
    }
}

message! {
    /// `GET /api/v1/auth/oauth/{ticket}`.
    pub struct OAuthStatus {
        /// `waiting`, `redirect`, `done` or `failed`.
        1 state: String,
        /// For `redirect`: where to send the browser.
        2 location: String,
        3 message: String,
        /// For `done`: `signed_in` or `linked`.
        4 result: String,
    }
}

message! {
    /// Someone on a screen card, with their answer (`in`, `maybe`, `out`).
    pub struct DisplayPerson {
        1 id: u32,
        2 name: String,
        3 answer: String,
    }
}

message! {
    /// One card of the screen's rotation (`spec/07-display.md`). The board
    /// draws these, and `GET /api/v1/display` serves them for remote displays.
    pub struct DisplayCard {
        /// `soon`, `upcoming`, `needs`, `deciding`, `rsvp`, `locked`,
        /// `cancelled` or `idle`.
        1 kind: String,
        2 plan: u32,
        3 title: String,
        4 games: Vec<String>,
        /// The headline game's kind (`board`, `card`, `video`, `rpg`, `other`).
        5 game_kind: String,
        /// Colour seed: the headline game's id (or the person's, for `rsvp`).
        6 accent: u32,
        /// The chosen time, or the first option.
        7 start: u64,
        8 end: u64,
        /// All options' starts (`needs`, `deciding`).
        9 times: Vec<u64>,
        10 people: Vec<DisplayPerson>,
        11 place: String,
        12 note: String,
        13 count_in: u32,
        14 count_maybe: u32,
        /// Players still needed for the headline game's minimum.
        15 needed: u32,
        16 seats: u32,
        /// Who did it (`rsvp`, `locked`, `cancelled`, `needs`).
        17 actor: String,
        18 actor_id: u32,
        19 answer: String,
        /// When it happened (Unix seconds).
        20 at: u64,
    }
}

message! {
    pub struct DisplayDeck {
        1 cards: Vec<DisplayCard>,
        2 generated: u64,
    }
}

// ---- Snapshot --------------------------------------------------------------

message! {
    pub struct StoredAccount {
        1 id: u32,
        /// Lower-cased login name; mailed only by the board itself
        /// (notifications, PIN reset codes), never shown to other members.
        2 email: String,
        3 display_name: String,
        /// `pbkdf2-sha256$<rounds>$<salt hex>$<hash hex>`, or empty.
        4 pin: String,
        5 admin: bool,
        6 disabled: bool,
        7 created: u64,
        8 last_seen: u64,
        /// Games this person is into (game ids).
        9 interests: Vec<u32>,
        /// Newest first, at most `Limits::notifications`.
        10 notifications: Vec<StoredNotification>,
        /// SHA-256 (hex) of the personal calendar feed token; empty = none.
        11 feed_hash: String,
        12 mastodon: Option<StoredIdentity>,
        /// Turned plan emails off.
        13 email_off: bool,
        14 last_house_reply: String,
        15 last_public_reply: String,
    }
}

message! {
    pub struct StoredSession {
        /// SHA-256 of the cookie token, hex.
        1 hash: String,
        2 account: u32,
        3 created: u64,
        4 last_seen: u64,
    }
}

message! {
    /// The `meta` record: everything that isn't a row (`spec/sprint-10-storage.md`).
    pub struct StoredMeta {
        1 schema: u32,
        2 next_account: u32,
        3 signup_closed: bool,
        4 next_game: u32,
        5 next_plan: u32,
        6 next_notification: u32,
        7 house_feed_hash: String,
        8 next_provider: u32,
    }
}

message! {
    /// The `idx` record: which rows exist, in order. Written only when a row
    /// is added or removed.
    pub struct StoredIndex {
        1 accounts: Vec<u32>,
        2 games: Vec<u32>,
        3 plans: Vec<u32>,
        4 providers: Vec<u32>,
        /// Accounts with an `s<id>` sessions record.
        5 sessions: Vec<u32>,
    }
}

message! {
    /// One account's sessions (`s<id>`).
    pub struct StoredSessions {
        1 sessions: Vec<StoredSession>,
    }
}

message! {
    /// The whole durable state as schema 1 kept it (two snapshot slots).
    /// Read only to migrate to records.
    pub struct StoredState {
        1 schema: u32,
        2 next_account: u32,
        3 accounts: Vec<StoredAccount>,
        4 sessions: Vec<StoredSession>,
        /// Default (false) means open sign-up: on unless the admin closes it.
        5 signup_closed: bool,
        /// Commits ever made (flash-wear accounting).
        6 commits: u64,
        7 games: Vec<StoredGame>,
        8 next_game: u32,
        /// Open and locked-in plans, then the done/cancelled ring.
        9 plans: Vec<StoredPlan>,
        10 next_plan: u32,
        11 next_notification: u32,
        /// SHA-256 (hex) of the household calendar feed token; empty = off.
        12 house_feed_hash: String,
        13 providers: Vec<StoredProvider>,
        14 next_provider: u32,
    }
}

message! {
    /// A Mastodon account linked to a local one: (host, subject) is the key.
    pub struct StoredIdentity {
        1 host: String,
        /// The account id on that instance (stable across renames).
        2 subject: String,
        /// `@user@host` as of the last sign-in, for display.
        3 handle: String,
    }
}

message! {
    pub struct StoredProvider {
        1 id: u32,
        2 name: String,
        3 base: String,
        4 trust: String,
        5 anchor: String,
        6 invites: Vec<String>,
    }
}

message! {
    pub struct StoredNotification {
        1 id: u32,
        2 kind: String,
        3 plan: u32,
        4 count: u32,
        5 actor: u32,
        6 created: u64,
        7 read: bool,
    }
}

message! {
    pub struct StoredComment {
        1 id: u32,
        2 author: u32,
        3 text: String,
        4 created: u64,
        5 edited: u64,
    }
}

message! {
    /// One plan's comment thread, its own Kv blob.
    pub struct StoredThread {
        1 comments: Vec<StoredComment>,
    }
}

message! {
    pub struct StoredGame {
        1 id: u32,
        2 title: String,
        /// `board`, `card`, `video`, `rpg` or `other`.
        3 kind: String,
        /// 0 = not given.
        4 min_players: u32,
        5 max_players: u32,
        6 minutes: u32,
        7 notes: String,
        8 added_by: u32,
        9 created: u64,
        10 archived: bool,
    }
}

message! {
    /// One proposed time: a start, and an end for a window ("Sat 2–6").
    /// Ids are stable within a plan; an edited time gets a new id, so a
    /// response to the old one is detectably stale.
    pub struct StoredTime {
        1 id: u32,
        /// Unix seconds.
        2 start: u64,
        /// 0 = no end given.
        3 end: u64,
    }
}

message! {
    pub struct TimeAnswer {
        /// A `StoredTime::id` of the plan.
        1 time: u32,
        /// `yes`, `if_needed` or `no`.
        2 answer: String,
    }
}

message! {
    pub struct StoredResponse {
        1 account: u32,
        /// `in`, `maybe` or `out`.
        2 overall: String,
        3 times: Vec<TimeAnswer>,
        /// Games this person would play.
        4 games: Vec<u32>,
        5 note: String,
        6 updated: u64,
    }
}

message! {
    pub struct StoredPlan {
        1 id: u32,
        2 host: u32,
        3 title: String,
        4 notes: String,
        5 games: Vec<u32>,
        6 times: Vec<StoredTime>,
        7 place: String,
        /// 0 = no limit given.
        8 seats: u32,
        /// `open`, `on`, `done` or `cancelled`.
        9 status: String,
        /// Set when locked in: a time id of this plan.
        10 chosen_time: Option<u32>,
        11 chosen_games: Vec<u32>,
        12 responses: Vec<StoredResponse>,
        13 created: u64,
        14 updated: u64,
        /// Edits since creation (the calendar's SEQUENCE).
        15 edits: u32,
        16 next_time: u32,
        /// Comments in this plan's thread blob (`c<id>` in the Kv).
        17 comments: u32,
        18 next_comment: u32,
        /// The 15-minutes-before reminder went out.
        19 reminded: bool,
        /// When it became done or cancelled (comments close 7 days later).
        20 finished: u64,
        /// The household bot posts its news (create, lock-in, cancel).
        21 announce: bool,
        22 announce_public: bool,
        23 series: u32,
        24 report: Option<PostGameReport>,
    }
}

message! {
    pub struct RepeatInput {
        /// weekly, fortnightly or monthly. Count includes the first plan.
        1 frequency: String,
        2 count: u32,
    }
}
message! {
    pub struct GameResult {
        1 game: u32,
        /// Empty for cooperative play / no winner; several for a tie or team.
        2 winners: Vec<u32>,
    }
}
message! {
    pub struct PostGameReport {
        1 summary: String,
        2 games: Vec<GameResult>,
    }
}
message! {
    pub struct DeliveryStatus {
        1 household: BotView,
        2 public: BotView,
        3 mail: MailView,
    }
}
