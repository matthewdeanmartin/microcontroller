//! Anti-aliased text from the build-time glyph atlases (`fonts.rs`,
//! generated from Atkinson Hyperlegible, SIL OFL 1.1).
use super::canvas::{Canvas, Rgb};

pub struct Glyph {
    pub ch: char,
    pub w: u32,
    pub h: u32,
    pub left: i32,
    /// Rows from the baseline up to the bitmap's top edge.
    pub top: i32,
    pub advance: u32,
    pub offset: u32,
}

pub struct Face {
    pub ascent: i32,
    pub descent: i32,
    pub line: u32,
    /// Sorted by `ch`.
    pub glyphs: &'static [Glyph],
    /// 4-bit coverage, two pixels per byte, high nibble first.
    pub bits: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/fonts.rs"));

impl Face {
    fn glyph(&self, ch: char) -> Option<&Glyph> {
        let ch = match ch {
            '\u{2019}' | '\u{2018}' if self.find(ch).is_none() => '\'',
            c => c,
        };
        self.find(ch).or_else(|| self.find('?'))
    }

    fn find(&self, ch: char) -> Option<&Glyph> {
        self.glyphs
            .binary_search_by(|g| g.ch.cmp(&ch))
            .ok()
            .map(|i| &self.glyphs[i])
    }

    pub fn width(&self, text: &str) -> i32 {
        text.chars()
            .filter_map(|c| self.glyph(c))
            .map(|g| g.advance as i32)
            .sum()
    }

    /// `text`, cut with "…" to fit `max` px.
    pub fn fit(&self, text: &str, max: i32) -> String {
        if self.width(text) <= max {
            return text.to_string();
        }
        let ellipsis = self.width("…");
        let mut out = String::new();
        let mut width = 0;
        for c in text.chars() {
            let w = self.glyph(c).map_or(0, |g| g.advance as i32);
            if width + w + ellipsis > max {
                break;
            }
            out.push(c);
            width += w;
        }
        out.trim_end().to_string() + "…"
    }

    /// Splits `text` into at most `lines` lines of `max` px (word wrap,
    /// the last one cut with "…").
    pub fn wrap(&self, text: &str, max: i32, lines: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut current = String::new();
        for word in text.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if self.width(&candidate) <= max || current.is_empty() {
                current = candidate;
            } else {
                out.push(std::mem::take(&mut current));
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
        if out.len() > lines {
            let rest = out[lines - 1..].join(" ");
            out.truncate(lines - 1);
            out.push(rest);
        }
        out.into_iter().map(|l| self.fit(&l, max)).collect()
    }
}

/// Text laid out to fit a box: the face chosen and its lines.
pub struct Fitted {
    pub face: &'static Face,
    pub lines: Vec<String>,
}

/// Fits `text` into `max_w` × `max_h` px without throwing words away if at
/// all possible: the largest of `faces` (largest first) whose word-wrapped
/// lines fit whole wins. Only if even the smallest can't hold it all is the
/// last line cut with "…". Says as much as fits, never less.
pub fn squeeze(text: &str, max_w: i32, max_h: i32, faces: &[&'static Face]) -> Fitted {
    let words = text.split_whitespace().count();
    for face in faces {
        let lines = (max_h / face.line.max(1) as i32).max(1) as usize;
        let wrapped = face.wrap(text, max_w, lines);
        let kept: usize = wrapped.iter().map(|l| l.split_whitespace().count()).sum();
        if kept == words && !wrapped.last().is_some_and(|l| l.ends_with('…')) {
            return Fitted {
                face,
                lines: wrapped,
            };
        }
    }
    let face = faces.last().copied().unwrap_or(&SMALL);
    let lines = (max_h / face.line.max(1) as i32).max(1) as usize;
    Fitted {
        face,
        lines: face.wrap(text, max_w, lines),
    }
}

/// For short secondary lines (times, counts): one line at any of `faces`
/// beats wrapping; only if none fits on one line does it wrap (up to
/// `max_lines`), and only after that does anything get cut.
pub fn squeeze_line(text: &str, max_w: i32, max_lines: usize, faces: &[&'static Face]) -> Fitted {
    for face in faces {
        if face.width(text) <= max_w {
            return Fitted {
                face,
                lines: vec![text.to_string()],
            };
        }
    }
    let face = faces.last().copied().unwrap_or(&SMALL);
    squeeze(text, max_w, face.line as i32 * max_lines as i32, &[face])
}

/// As many `items` as fit in `max_w`, then "+N": "Dad, Mom, Sam +2".
pub fn list_fit(face: &Face, items: &[&str], max_w: i32) -> String {
    for keep in (1..=items.len()).rev() {
        let mut s = items[..keep].join(", ");
        if keep < items.len() {
            s.push_str(&format!(" +{}", items.len() - keep));
        }
        if face.width(&s) <= max_w {
            return s;
        }
    }
    match items.len() {
        0 => String::new(),
        n => face.fit(&format!("{} +{}", items[0], n - 1), max_w),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Canvas<'_> {
    /// Draws fitted lines from `top` (the top of the first line's box).
    /// Returns the height used.
    pub fn block(&mut self, fitted: &Fitted, x: i32, top: i32, align: Align, c: Rgb) -> i32 {
        let step = fitted.face.line as i32;
        for (i, line) in fitted.lines.iter().enumerate() {
            self.text(
                fitted.face,
                x,
                top + fitted.face.ascent + i as i32 * step,
                align,
                c,
                line,
            );
        }
        step * fitted.lines.len() as i32
    }

    /// Draws `text` with its baseline at `y`. Returns the width.
    pub fn text(&mut self, face: &Face, x: i32, y: i32, align: Align, c: Rgb, text: &str) -> i32 {
        let width = face.width(text);
        let mut pen = match align {
            Align::Left => x,
            Align::Center => x - width / 2,
            Align::Right => x - width,
        };
        // The whole line's rows: nothing to do on other strips.
        if !self.touches(y - face.ascent - 2, y - face.descent + 2) {
            return width;
        }
        for ch in text.chars() {
            let Some(g) = face.glyph(ch) else { continue };
            let x0 = pen + g.left;
            let y0 = y - g.top;
            for gy in 0..g.h as i32 {
                if !self.touches(y0 + gy, y0 + gy + 1) {
                    continue;
                }
                for gx in 0..g.w as i32 {
                    let i = (gy * g.w as i32 + gx) as usize;
                    let byte = face.bits[g.offset as usize + i / 2];
                    let nibble = if i.is_multiple_of(2) {
                        byte >> 4
                    } else {
                        byte & 0x0f
                    };
                    self.blend8(x0 + gx, y0 + gy, c, nibble * 17);
                }
            }
            pen += g.advance as i32;
        }
        width
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlases_have_what_the_cards_need() {
        for face in [&SMALL, &BODY, &BOLD, &MID, &TITLE] {
            for ch in "Aaé0·–“”…".chars() {
                assert!(face.find(ch).is_some(), "{ch:?}");
            }
            assert!(
                face.glyphs.windows(2).all(|w| w[0].ch < w[1].ch),
                "sorted for binary search"
            );
        }
        assert!(HUGE.find('7').is_some() && HUGE.find(':').is_some());
        let total: usize = [&SMALL, &BODY, &BOLD, &MID, &TITLE, &HUGE]
            .iter()
            .map(|f| f.bits.len())
            .sum();
        assert!(total < 60_000, "atlases are {total} bytes of flash");
    }

    #[test]
    fn squeeze_shrinks_before_it_cuts() {
        let short = squeeze("Catan", 200, 30, &[&TITLE, &MID, &BOLD]);
        assert!(std::ptr::eq(short.face, &TITLE) && short.lines == ["Catan"]);
        // Two lines at a smaller size rather than losing words.
        let long = "Saturday afternoon game night with the cousins";
        let f = squeeze(long, 200, 44, &[&TITLE, &MID, &BOLD, &SMALL]);
        assert_eq!(f.lines.join(" "), long, "every word kept");
        assert!(f.lines.len() <= 2);
        // Only when nothing fits is it cut, at the smallest size.
        let novel = "word ".repeat(80);
        let f = squeeze(&novel, 200, 30, &[&TITLE, &SMALL]);
        assert!(std::ptr::eq(f.face, &SMALL) && f.lines.last().unwrap().ends_with('…'));
    }

    #[test]
    fn lists_say_how_many_more() {
        let names = ["Dad", "Mom", "Sam", "Alex", "Jordan"];
        assert_eq!(list_fit(&BODY, &names, 1000), "Dad, Mom, Sam, Alex, Jordan");
        let some = list_fit(&BODY, &names, 110);
        assert!(some.starts_with("Dad, ") && some.contains(" +"), "{some}");
        assert!(BODY.width(&some) <= 110);
    }

    #[test]
    fn fit_and_wrap() {
        let long = "A very long plan title that will not fit on the screen at all";
        let cut = TITLE.fit(long, 200);
        assert!(cut.ends_with('…') && TITLE.width(&cut) <= 200, "{cut}");
        let lines = BODY.wrap(long, 150, 2);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| BODY.width(l) <= 150));
        assert!(lines[1].ends_with('…'));
    }
}
