//! The board's screen: cards drawn strip by strip (`spec/07-display.md`).
//! Pure Rust and platform-free, so the desktop renders exactly what the
//! board shows (previews, tests, `/api/v1/display/frame.bmp`).
pub mod canvas;
pub mod cards;
pub mod clock;
pub mod deck;
pub mod text;

use crate::messages::DisplayCard;
use canvas::{Canvas, H, W};
use cards::Context;
use std::io::{self, Read};

/// Rows per strip: 320 × 24 × 2 bytes = 15 KB, the panel driver's DMA buffer.
pub const STRIP: i32 = 24;

/// One card, ready to draw.
pub struct Frame {
    card: DisplayCard,
    ctx: Context,
}

impl Frame {
    pub fn new(card: DisplayCard, ctx: Context) -> Frame {
        let mut ctx = ctx;
        if matches!(card.kind.as_str(), "needs" | "deciding") {
            let url = format!("{}/plans/{}", ctx.origin.trim_end_matches('/'), card.plan);
            if let Ok(qr) = qrcodegen::QrCode::encode_text(&url, qrcodegen::QrCodeEcc::Low) {
                let size = qr.size();
                let modules = (0..size * size)
                    .map(|i| qr.get_module(i % size, i / size))
                    .collect();
                ctx.qr = Some((size, modules));
            }
        }
        Frame { card, ctx }
    }

    /// Draws rows `top..top + rows` into `px` (`rows` × 320 pixels).
    pub fn draw_strip(&self, px: &mut [u16], top: i32, rows: i32) {
        let mut canvas = Canvas::new(px, top, rows);
        cards::draw(&mut canvas, &self.card, &self.ctx);
    }

    /// Calls `send(top, rows, pixels)` for each strip, top to bottom.
    pub fn draw<E>(
        &self,
        buf: &mut [u16],
        mut send: impl FnMut(i32, i32, &[u16]) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut top = 0;
        while top < H {
            let rows = STRIP.min(H - top);
            let px = &mut buf[..(rows * W) as usize];
            self.draw_strip(px, top, rows);
            send(top, rows, px)?;
            top += rows;
        }
        Ok(())
    }
}

/// The frame as a 24-bit top-down BMP, rendered a strip at a time as it is
/// read (the board never holds the 165 KB image, only one 15 KB strip).
pub struct BmpStream {
    frame: Frame,
    header: Vec<u8>,
    sent_header: usize,
    next_top: i32,
    rows: Vec<u8>,
    pos: usize,
    strip: Vec<u16>,
}

pub const BMP_LEN: u64 = 54 + (W as u64 * 3) * H as u64;

impl BmpStream {
    pub fn new(frame: Frame) -> BmpStream {
        let mut h = Vec::with_capacity(54);
        h.extend_from_slice(b"BM");
        h.extend_from_slice(&(BMP_LEN as u32).to_le_bytes());
        h.extend_from_slice(&[0, 0, 0, 0]);
        h.extend_from_slice(&54u32.to_le_bytes());
        h.extend_from_slice(&40u32.to_le_bytes());
        h.extend_from_slice(&(W).to_le_bytes());
        h.extend_from_slice(&(-H).to_le_bytes()); // negative: top-down rows
        h.extend_from_slice(&1u16.to_le_bytes());
        h.extend_from_slice(&24u16.to_le_bytes());
        h.extend_from_slice(&[0; 24]);
        BmpStream {
            frame,
            header: h,
            sent_header: 0,
            next_top: 0,
            rows: Vec::new(),
            pos: 0,
            strip: vec![0; (W * STRIP) as usize],
        }
    }
}

impl Read for BmpStream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.sent_header < self.header.len() {
            let n = out.len().min(self.header.len() - self.sent_header);
            out[..n].copy_from_slice(&self.header[self.sent_header..self.sent_header + n]);
            self.sent_header += n;
            return Ok(n);
        }
        if self.pos == self.rows.len() {
            if self.next_top >= H {
                return Ok(0);
            }
            let rows = STRIP.min(H - self.next_top);
            let px = &mut self.strip[..(rows * W) as usize];
            self.frame.draw_strip(px, self.next_top, rows);
            self.rows.clear();
            for &p in px.iter() {
                let c = canvas::Rgb::from565(p);
                self.rows.extend_from_slice(&[c.2, c.1, c.0]); // BMP is BGR
            }
            self.pos = 0;
            self.next_top += rows;
        }
        let n = out.len().min(self.rows.len() - self.pos);
        out[..n].copy_from_slice(&self.rows[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::DisplayPerson;

    pub fn ctx(now: u64) -> Context {
        Context {
            now,
            offset: -4 * 3600,
            page: 0,
            pages: 3,
            online: true,
            origin: "http://192.168.1.163".into(),
            name: "gamematcher.local".into(),
            qr: None,
        }
    }

    pub fn samples(now: u64) -> Vec<DisplayCard> {
        let people = |names: &[(&str, u32, &str)]| {
            names
                .iter()
                .map(|(n, id, a)| DisplayPerson {
                    id: *id,
                    name: n.to_string(),
                    answer: a.to_string(),
                })
                .collect::<Vec<_>>()
        };
        vec![
            DisplayCard {
                kind: "upcoming".into(),
                plan: 1,
                title: "Saturday game night".into(),
                games: vec!["Catan".into(), "Wingspan".into()],
                game_kind: "board".into(),
                accent: 3,
                start: now + 200 * 60,
                people: people(&[
                    ("Dad", 1, "in"),
                    ("Mom", 3, "in"),
                    ("Sam", 2, "in"),
                    ("Alex", 4, "maybe"),
                ]),
                count_in: 3,
                count_maybe: 1,
                seats: 4,
                place: "Home".into(),
                ..Default::default()
            },
            DisplayCard {
                kind: "soon".into(),
                plan: 1,
                title: "Saturday game night".into(),
                games: vec!["Catan".into()],
                game_kind: "board".into(),
                accent: 3,
                start: now + 47 * 60,
                people: people(&[("Dad", 1, "in"), ("Mom", 3, "in"), ("Sam", 2, "in")]),
                place: "Home".into(),
                note: "bring snacks".into(),
                ..Default::default()
            },
            DisplayCard {
                kind: "needs".into(),
                plan: 7,
                title: "Wingspan".into(),
                game_kind: "card".into(),
                accent: 5,
                times: vec![now + 86_400, now + 2 * 86_400],
                count_in: 1,
                needed: 2,
                actor: "Sam".into(),
                ..Default::default()
            },
            DisplayCard {
                kind: "rsvp".into(),
                plan: 1,
                title: "Saturday game night".into(),
                games: vec!["Catan".into(), "Wingspan".into()],
                actor: "Sam".into(),
                actor_id: 2,
                answer: "in".into(),
                note: "only after 3".into(),
                at: now - 12 * 60,
                count_in: 3,
                count_maybe: 1,
                ..Default::default()
            },
            DisplayCard {
                kind: "rsvp".into(),
                plan: 2,
                title:
                    "A ridiculously long plan title that someone typed on their phone in a hurry"
                        .into(),
                games: vec![
                    "Twilight Imperium".into(),
                    "Gloomhaven".into(),
                    "Spirit Island".into(),
                    "Root".into(),
                ],
                actor: "Alexandra-Marie".into(),
                actor_id: 9,
                answer: "maybe".into(),
                note: "Only if we can start before seven, I have work early on Sunday morning"
                    .into(),
                at: now - 3 * 3600,
                count_in: 5,
                count_maybe: 2,
                ..Default::default()
            },
            DisplayCard {
                kind: "locked".into(),
                plan: 1,
                title: "Saturday game night".into(),
                games: vec!["Catan".into()],
                game_kind: "board".into(),
                accent: 3,
                start: now + 26 * 3600,
                actor: "Dad".into(),
                at: now - 30 * 60,
                ..Default::default()
            },
            DisplayCard {
                kind: "idle".into(),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn every_card_renders_every_strip() {
        let now = 1_792_260_000 - 3 * 3600;
        let mut buf = vec![0u16; (W * STRIP) as usize];
        for card in samples(now) {
            let frame = Frame::new(card, ctx(now));
            let mut strips = 0;
            frame
                .draw(&mut buf, |_, _, px| {
                    strips += 1;
                    assert!(px.iter().any(|&p| p != 0));
                    Ok::<(), ()>(())
                })
                .unwrap();
            assert_eq!(strips, 8);
        }
    }

    #[test]
    fn bmp_stream_is_complete() {
        let now = 1_792_260_000;
        let mut bmp = Vec::new();
        BmpStream::new(Frame::new(samples(now).remove(2), ctx(now)))
            .read_to_end(&mut bmp)
            .unwrap();
        assert_eq!(bmp.len() as u64, BMP_LEN);
        assert_eq!(&bmp[..2], b"BM");
    }

    /// `GM_PREVIEW=dir cargo test preview`: writes every sample card as a BMP.
    #[test]
    fn preview() {
        let Some(dir) = std::env::var_os("GM_PREVIEW") else {
            return;
        };
        let now = 1_792_260_000 - 3 * 3600;
        for (i, card) in samples(now).into_iter().enumerate() {
            let name = format!("{i}-{}.bmp", card.kind);
            let mut ctx = ctx(now);
            ctx.page = i % 3;
            let mut bmp = Vec::new();
            BmpStream::new(Frame::new(card, ctx))
                .read_to_end(&mut bmp)
                .unwrap();
            std::fs::write(std::path::Path::new(&dir).join(name), bmp).unwrap();
        }
    }
}
