//! The cards of the screen's rotation, drawn from a [`DisplayCard`]. Each
//! function draws the whole card in screen coordinates; the canvas keeps
//! only the rows of the strip being drawn. Layouts follow the mockups in
//! `spec/display/`.
use super::canvas::{Canvas, Rgb, H, W};
use super::clock::{relative, when, Local};
use super::text::{list_fit, squeeze, squeeze_line, Align, BODY, BOLD, HUGE, MID, SMALL, TITLE};
use crate::messages::DisplayCard;

pub const BG: Rgb = Rgb(16, 18, 24);
pub const PANEL: Rgb = Rgb(28, 31, 40);
pub const TEXT: Rgb = Rgb(238, 236, 230);
pub const MUTED: Rgb = Rgb(148, 152, 166);
pub const DIM: Rgb = Rgb(70, 74, 86);
pub const GOOD: Rgb = Rgb(111, 191, 143);
pub const WARN: Rgb = Rgb(242, 180, 76);
pub const HOT: Rgb = Rgb(240, 98, 84);

/// Everything a card needs besides its data.
pub struct Context {
    pub now: u64,
    /// UTC offset in seconds for `now` (the household zone).
    pub offset: i64,
    pub page: usize,
    pub pages: usize,
    pub online: bool,
    /// What the QR codes point at, e.g. `http://192.168.1.163` (an IP
    /// works on Android phones that can't resolve `.local`).
    pub origin: String,
    /// The name people type, e.g. `gamematcher.local`.
    pub name: String,
    /// The QR code for this card's link, if it has one: (size, modules).
    pub qr: Option<(i32, Vec<bool>)>,
}

/// The game's colour (same rule as the web UI's avatars).
pub fn accent(seed: u32) -> Rgb {
    Rgb::hue(seed, 0.6, 0.55)
}

fn avatar(c: &mut Canvas, cx: f32, cy: f32, r: f32, name: &str, id: u32, ring: Option<Rgb>) {
    if let Some(ring) = ring {
        c.circle(cx, cy, r + 2.0, ring);
    }
    c.circle(cx, cy, r, Rgb::hue(id, 0.55, 0.45));
    let initial: String = name
        .chars()
        .next()
        .map(|ch| ch.to_uppercase().collect())
        .unwrap_or_default();
    let face = if r >= 20.0 { &TITLE } else { &BOLD };
    c.text(
        face,
        cx as i32,
        (cy + face.ascent as f32 * 0.36) as i32,
        Align::Center,
        Rgb(255, 255, 255),
        &initial,
    );
}

/// Kind icons from primitives (die, cards, gamepad, d20, meeple).
pub fn icon(c: &mut Canvas, kind: &str, x: f32, y: f32, s: f32, color: Rgb) {
    match kind {
        "board" => {
            c.round_rect(
                x as i32,
                y as i32,
                s as i32,
                s as i32,
                (s / 5.0) as i32,
                color,
            );
            let p = s / 9.0;
            for (fx, fy) in [
                (0.27, 0.27),
                (0.73, 0.27),
                (0.5, 0.5),
                (0.27, 0.73),
                (0.73, 0.73),
            ] {
                c.circle(x + fx * s, y + fy * s, p, BG);
            }
        }
        "card" => {
            c.round_rect(
                (x + s * 0.12) as i32,
                y as i32,
                (s * 0.6) as i32,
                (s * 0.85) as i32,
                (s / 8.0) as i32,
                color.mix(BG, 0.45),
            );
            c.round_rect(
                (x + s * 0.32) as i32,
                (y + s * 0.15) as i32,
                (s * 0.6) as i32,
                (s * 0.85) as i32,
                (s / 8.0) as i32,
                color,
            );
        }
        "video" => {
            c.round_rect(
                x as i32,
                (y + s * 0.25) as i32,
                s as i32,
                (s * 0.55) as i32,
                (s / 4.0) as i32,
                color,
            );
            let mid = y + s * 0.52;
            c.rect(
                (x + s * 0.17) as i32,
                (mid - s * 0.04) as i32,
                (s * 0.2) as i32,
                (s * 0.08).max(2.0) as i32,
                BG,
            );
            c.rect(
                (x + s * 0.23) as i32,
                (mid - s * 0.1) as i32,
                (s * 0.08).max(2.0) as i32,
                (s * 0.2) as i32,
                BG,
            );
            for bx in [0.65, 0.8] {
                c.circle(x + s * bx, mid, s * 0.05, BG);
            }
        }
        "rpg" => {
            let pts: Vec<(f32, f32)> = (0..6)
                .map(|i| {
                    let a = (-90.0 + 60.0 * i as f32).to_radians();
                    (
                        x + s / 2.0 + s / 2.0 * a.cos(),
                        y + s / 2.0 + s / 2.0 * a.sin(),
                    )
                })
                .collect();
            c.polygon(&pts, color);
            c.text(
                &SMALL,
                (x + s / 2.0) as i32,
                (y + s / 2.0 + 4.0) as i32,
                Align::Center,
                BG,
                "20",
            );
        }
        _ => {
            // A meeple: head and body.
            c.circle(x + s / 2.0, y + s * 0.22, s * 0.18, color);
            let body = [
                (x + s * 0.15, y + s),
                (x + s * 0.3, y + s * 0.62),
                (x, y + s * 0.5),
                (x + s * 0.12, y + s * 0.38),
                (x + s * 0.88, y + s * 0.38),
                (x + s, y + s * 0.5),
                (x + s * 0.7, y + s * 0.62),
                (x + s * 0.85, y + s),
            ];
            c.polygon(&body, color);
        }
    }
}

/// A faint hex-tile texture behind a header (board games) or dots (others).
fn texture(c: &mut Canvas, h: i32, color: Rgb, kind: &str) {
    let faint = color.mix(BG, 0.82);
    if kind == "board" {
        let r = 9.0f32;
        let mut row = 0;
        let mut y = 0.0f32;
        while y < h as f32 + r {
            let mut x = if row % 2 == 1 { 0.0 } else { -r };
            while x < W as f32 + r {
                for i in 0..6 {
                    let a0 = (30.0 + 60.0 * i as f32).to_radians();
                    let a1 = (90.0 + 60.0 * i as f32).to_radians();
                    c.line(
                        x + r * a0.cos(),
                        y + r * a0.sin(),
                        x + r * a1.cos(),
                        y + r * a1.sin(),
                        faint,
                        0.9,
                    );
                }
                x += r * 2.0;
            }
            y += r * 1.5;
            row += 1;
        }
    } else {
        for y in (4..h).step_by(10) {
            for x in ((4 + (y / 10 % 2) * 5)..W).step_by(10) {
                c.rect(x, y, 2, 2, faint);
            }
        }
    }
}

pub fn status_bar(c: &mut Canvas, ctx: &Context, clock: bool) {
    if clock {
        let local = Local::at(ctx.now, ctx.offset);
        c.text(&SMALL, 8, H - 5, Align::Left, MUTED, &local.clock());
    }
    let bars = if ctx.online { MUTED } else { WARN };
    for i in 0..4 {
        let h = 3 + i * 2;
        c.rect(
            W - 22 + i * 4,
            H - 6 - h,
            2,
            h,
            if i < 3 { bars } else { DIM },
        );
    }
    if ctx.pages > 1 {
        let x0 = W / 2 - ctx.pages as i32 * 5;
        for i in 0..ctx.pages as i32 {
            let on = i as usize == ctx.page;
            c.circle(
                (x0 + i * 10 + 3) as f32,
                (H - 9) as f32,
                2.5,
                if on { TEXT } else { DIM },
            );
        }
    }
}

pub fn draw(c: &mut Canvas, card: &DisplayCard, ctx: &Context) {
    c.fill(BG);
    match card.kind.as_str() {
        "soon" => soon(c, card, ctx),
        "upcoming" => upcoming(c, card, ctx),
        "needs" | "deciding" => needs(c, card, ctx),
        "rsvp" => rsvp(c, card, ctx),
        "locked" | "cancelled" => event(c, card, ctx),
        _ => idle(c, ctx),
    }
    status_bar(c, ctx, card.kind != "idle");
}

fn people_row(c: &mut Canvas, card: &DisplayCard, x: i32, y: i32) -> i32 {
    let mut cx = x + 12;
    let shown = if card.people.len() > 6 {
        5
    } else {
        card.people.len()
    };
    for p in card.people.iter().take(shown) {
        let ring = match p.answer.as_str() {
            "in" => GOOD,
            "maybe" => WARN,
            _ => DIM,
        };
        avatar(c, cx as f32, y as f32, 11.0, &p.name, p.id, Some(ring));
        cx += 26;
    }
    if card.people.len() > shown {
        c.circle(cx as f32, y as f32, 11.0, PANEL);
        let more = format!("+{}", card.people.len() - shown);
        c.text(&SMALL, cx, y + 4, Align::Center, MUTED, &more);
        cx += 26;
    }
    cx - 12
}

fn upcoming(c: &mut Canvas, card: &DisplayCard, ctx: &Context) {
    let color = accent(card.accent);
    c.vgradient(0, 0, W, 58, color.mix(BG, 0.55), BG);
    texture(c, 58, color, &card.game_kind);
    icon(c, &card.game_kind, 12.0, 12.0, 34.0, color);
    c.text(&SMALL, 56, 20, Align::Left, color, "UP NEXT");
    let title = squeeze(&card.title, W - 66, 34, &[&TITLE, &MID, &BOLD]);
    c.block(&title, 56, 24, Align::Left, TEXT);
    let when = when(card.start, ctx.now, ctx.offset);
    let chip = BOLD.width(&when) + 20;
    c.round_rect(12, 66, chip, 21, 10, PANEL);
    c.text(&BOLD, 12 + chip / 2, 81, Align::Center, TEXT, &when);
    c.text(
        &BODY,
        22 + chip,
        81,
        Align::Left,
        MUTED,
        &relative(card.start, ctx.now),
    );
    let games: Vec<&str> = card.games.iter().map(String::as_str).collect();
    let line = squeeze(&games.join(" · "), W - 24, 20, &[&BOLD, &BODY, &SMALL]);
    let line = if line.lines.last().is_some_and(|l| l.ends_with('…')) {
        // Too many games for one line even small: list what fits, "+N".
        list_fit(&BOLD, &games, W - 24)
    } else {
        line.lines.join(" ")
    };
    c.text(&BOLD, 12, 108, Align::Left, TEXT, &line);
    let end = people_row(c, card, 12, 135);
    let mut summary = format!("{} in", card.count_in);
    if card.count_maybe > 0 {
        summary.push_str(&format!(", {} maybe", card.count_maybe));
    }
    c.text(&BODY, end + 16, 133, Align::Left, MUTED, &summary);
    if card.seats > 0 {
        c.text(
            &SMALL,
            end + 16,
            148,
            Align::Left,
            MUTED,
            &format!("seats: {}", card.seats),
        );
    }
}

fn soon(c: &mut Canvas, card: &DisplayCard, ctx: &Context) {
    c.vgradient(0, 0, W, H - 18, HOT.mix(BG, 0.7), BG);
    c.text(&BOLD, W / 2, 18, Align::Center, WARN, "GAME STARTING SOON!");
    let left = card.start.saturating_sub(ctx.now).div_ceil(60);
    let (cx, cy, r) = (W as f32 / 2.0, 68.0, 42.0);
    c.arc(cx, cy, r, 4.0, 0.0, 360.0, WARN.mix(BG, 0.75));
    c.arc(
        cx,
        cy,
        r,
        4.0,
        0.0,
        360.0 * (left.min(120) as f32 / 120.0),
        WARN,
    );
    c.text(&HUGE, W / 2, 80, Align::Center, TEXT, &left.to_string());
    c.text(
        &SMALL,
        W / 2,
        98,
        Align::Center,
        MUTED,
        if left == 1 { "minute" } else { "min" },
    );
    let color = accent(card.accent);
    icon(c, &card.game_kind, 16.0, 117.0, 24.0, color);
    let game = card
        .games
        .first()
        .cloned()
        .unwrap_or_else(|| card.title.clone());
    let fitted = squeeze(&game, W - 60, 24, &[&TITLE, &MID, &BOLD]);
    c.text(fitted.face, 48, 136, Align::Left, TEXT, &fitted.lines[0]);
    // Who's in matters most, then where, then the note: each gets what's left.
    let names: Vec<&str> = card
        .people
        .iter()
        .filter(|p| p.answer == "in")
        .map(|p| p.name.as_str())
        .collect();
    let room = W - 60;
    let who = list_fit(&SMALL, &names, room);
    let mut line = who.clone();
    for extra in [card.place.as_str(), card.note.as_str()] {
        if extra.is_empty() {
            continue;
        }
        let candidate = if line.is_empty() {
            extra.to_string()
        } else {
            format!("{line} · {extra}")
        };
        if SMALL.width(&candidate) <= room {
            line = candidate;
        }
    }
    c.text(&SMALL, 48, 152, Align::Left, MUTED, &line);
}

/// The QR code's module size: as big as fits beside the text (the screen is
/// small and phones are held at arm's length).
fn qr_module(size: i32) -> i32 {
    if (size + 2) * 4 <= H - 28 {
        4
    } else if (size + 2) * 3 <= H - 28 {
        3
    } else {
        2
    }
}

fn needs(c: &mut Canvas, card: &DisplayCard, ctx: &Context) {
    let color = accent(card.accent);
    // The QR code is the way in from a phone: big, on the right, full height.
    let qr = ctx.qr.as_ref().map(|(size, modules)| {
        let module = qr_module(*size);
        let full = (size + 2) * module;
        (size, modules, module, full)
    });
    let qr_w = qr.map_or(0, |(_, _, _, full)| full + 8);
    if let Some((size, modules, module, full)) = qr {
        let x = W - 6 - full;
        let y = (H - 18 - full) / 2;
        c.qr(
            x,
            y,
            *size,
            module,
            |mx, my| modules[(my * size + mx) as usize],
            BG,
            TEXT,
        );
    }
    let right = W - 6 - qr_w; // the text column ends here
    let room = right - 12;
    icon(c, &card.game_kind, 12.0, 10.0, 24.0, color);
    let label = if card.kind == "needs" {
        "NEEDS PLAYERS"
    } else {
        "NEW PLAN"
    };
    c.text(&SMALL, 42, 21, Align::Left, color, label);
    c.text(
        &SMALL,
        42,
        34,
        Align::Left,
        MUTED,
        &SMALL.fit(&format!("by {}", card.actor), right - 42),
    );
    let title = squeeze(&card.title, room, 44, &[&TITLE, &MID, &BOLD, &BODY]);
    let mut top = 40 + c.block(&title, 12, 40, Align::Left, TEXT);
    let times = match card.times.len() {
        0 => "When? No times yet".to_string(),
        1 => when(card.times[0], ctx.now, ctx.offset),
        n => format!(
            "{} or {} more",
            when(card.times[0], ctx.now, ctx.offset),
            n - 1
        ),
    };
    let times = squeeze_line(&times, room, 2, &[&BODY, &SMALL]);
    top += c.block(&times, 12, top + 2, Align::Left, TEXT) + 6;
    if card.kind == "needs" && card.needed > 0 {
        let total = (card.count_in + card.needed).min(6) as i32;
        for i in 0..total {
            let fill = if (i as u32) < card.count_in {
                GOOD
            } else {
                DIM
            };
            c.round_rect(12 + i * 15, top, 11, 11, 3, fill);
        }
        let more = if card.needed == 1 {
            "1 more to play".into()
        } else {
            format!("{} more to play", card.needed)
        };
        let x = 16 + total * 15;
        let more = squeeze(&more, right - x, 16, &[&BOLD, &SMALL]);
        c.text(more.face, x, top + 10, Align::Left, WARN, &more.lines[0]);
    } else {
        let s = format!("{} in, {} maybe so far", card.count_in, card.count_maybe);
        c.text(
            &SMALL,
            12,
            top + 10,
            Align::Left,
            MUTED,
            &SMALL.fit(&s, room),
        );
    }
    c.text(
        &SMALL,
        12,
        H - 22,
        Align::Left,
        MUTED,
        &SMALL.fit(&format!("scan or open {}", ctx.name), room),
    );
}

fn rsvp(c: &mut Canvas, card: &DisplayCard, ctx: &Context) {
    let ring = if card.answer == "in" { GOOD } else { WARN };
    avatar(c, 52.0, 62.0, 34.0, &card.actor, card.actor_id, Some(ring));
    let headline = if card.answer == "in" {
        format!("{} is in!", card.actor)
    } else {
        format!("{}: maybe", card.actor)
    };
    let room = W - 110;
    let head = squeeze(&headline, room, 26, &[&TITLE, &MID, &BOLD]);
    c.text(head.face, 100, 46, Align::Left, TEXT, &head.lines[0]);
    // The rest of the column (y 54..112) goes to the plan, the games and the
    // note, each squeezed to keep every word if it can.
    let mut top = 54;
    let plan = squeeze(&format!("for {}", card.title), room, 34, &[&BODY, &SMALL]);
    top += c.block(&plan, 100, top, Align::Left, MUTED);
    if !card.games.is_empty() && top < 96 {
        let games: Vec<&str> = card.games.iter().map(String::as_str).collect();
        let line = format!(
            "would play: {}",
            list_fit(&SMALL, &games, room - SMALL.width("would play: "))
        );
        c.text(&SMALL, 100, top + SMALL.ascent, Align::Left, TEXT, &line);
        top += SMALL.line as i32;
    }
    if !card.note.is_empty() && top < 104 {
        let note = squeeze(
            &format!("“{}”", card.note),
            room,
            112 - top,
            &[&BODY, &SMALL],
        );
        c.block(&note, 100, top, Align::Left, MUTED);
    }
    c.round_rect(16, 116, W - 32, 21, 10, PANEL);
    let mut tail = format!("{} · {} in", relative(card.at, ctx.now), card.count_in);
    if card.count_maybe > 0 {
        tail.push_str(&format!(", {} maybe", card.count_maybe));
    }
    c.text(&SMALL, W / 2, 131, Align::Center, MUTED, &tail);
}

fn event(c: &mut Canvas, card: &DisplayCard, ctx: &Context) {
    let color = if card.kind == "locked" {
        accent(card.accent)
    } else {
        HOT
    };
    c.vgradient(0, 0, W, 70, color.mix(BG, 0.6), BG);
    icon(c, &card.game_kind, 14.0, 16.0, 34.0, color);
    let label = if card.kind == "locked" {
        "IT'S ON!"
    } else {
        "CANCELLED"
    };
    c.text(&SMALL, 58, 26, Align::Left, color, label);
    let title = squeeze(&card.title, W - 68, 40, &[&TITLE, &MID, &BOLD]);
    c.block(&title, 58, 30, Align::Left, TEXT);
    if card.kind == "locked" {
        c.text(
            &BOLD,
            14,
            92,
            Align::Left,
            TEXT,
            &when(card.start, ctx.now, ctx.offset),
        );
        let games: Vec<&str> = card.games.iter().map(String::as_str).collect();
        c.text(
            &BODY,
            14,
            112,
            Align::Left,
            TEXT,
            &list_fit(&BODY, &games, W - 28),
        );
    }
    c.text(
        &SMALL,
        14,
        140,
        Align::Left,
        MUTED,
        &format!("{} · {}", card.actor, relative(card.at, ctx.now)),
    );
}

fn idle(c: &mut Canvas, ctx: &Context) {
    let local = Local::at(ctx.now, ctx.offset);
    let digits = local.clock_digits();
    let w = HUGE.width(&digits);
    c.text(&HUGE, W / 2 - 12, 66, Align::Center, TEXT, &digits);
    c.text(
        &BOLD,
        W / 2 - 12 + w / 2 + 4,
        66,
        Align::Left,
        MUTED,
        local.am_pm(),
    );
    c.text(&BODY, W / 2, 92, Align::Center, MUTED, &local.long_date());
    c.text(
        &BOLD,
        W / 2,
        118,
        Align::Center,
        TEXT,
        "Nothing planned yet. Up for a game?",
    );
    c.text(&BOLD, W / 2, 140, Align::Center, GOOD, &ctx.name);
    let ip = ctx
        .origin
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    if ip != ctx.name {
        c.text(&SMALL, W / 2, 160, Align::Center, MUTED, ip);
    }
}
