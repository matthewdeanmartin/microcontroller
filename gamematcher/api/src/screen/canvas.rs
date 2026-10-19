//! Drawing into one horizontal strip of the 320 × 172 panel. Every
//! primitive takes absolute screen coordinates and draws only the rows that
//! fall inside the strip, so a card is drawn by running the same code once
//! per strip (`spec/07-display.md#how-its-drawn-ram`). Pixels are RGB565,
//! stored in native byte order: the panel driver sends the buffer as it is
//! (big-endian pixels came out pink on the board, October 17, 2026).

pub const W: i32 = 320;
pub const H: i32 = 172;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        let f = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8;
        Rgb(f(self.0, other.0), f(self.1, other.1), f(self.2, other.2))
    }

    pub fn to565(self) -> u16 {
        (u16::from(self.0) >> 3) << 11 | (u16::from(self.1) >> 2) << 5 | u16::from(self.2) >> 3
    }

    pub fn from565(v: u16) -> Rgb {
        let r = (v >> 11) as u8 & 0x1f;
        let g = (v >> 5) as u8 & 0x3f;
        let b = v as u8 & 0x1f;
        Rgb(r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2)
    }

    /// The web UI's avatar hue rule, `(seed * 137) mod 360`, so a person or
    /// a game has the same colour on the phone and on the screen.
    pub fn hue(seed: u32, saturation: f32, lightness: f32) -> Rgb {
        let h = ((seed.wrapping_mul(137)) % 360) as f32 / 360.0;
        let q = if lightness < 0.5 {
            lightness * (1.0 + saturation)
        } else {
            lightness + saturation - lightness * saturation
        };
        let p = 2.0 * lightness - q;
        let channel = |mut t: f32| {
            t = t.rem_euclid(1.0);
            let v = if t < 1.0 / 6.0 {
                p + (q - p) * 6.0 * t
            } else if t < 0.5 {
                q
            } else if t < 2.0 / 3.0 {
                p + (q - p) * (2.0 / 3.0 - t) * 6.0
            } else {
                p
            };
            (v * 255.0).round() as u8
        };
        Rgb(channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0))
    }
}

pub struct Canvas<'a> {
    px: &'a mut [u16],
    top: i32,
    rows: i32,
}

impl<'a> Canvas<'a> {
    /// `px` holds `rows` full-width rows starting at screen row `top`.
    pub fn new(px: &'a mut [u16], top: i32, rows: i32) -> Self {
        debug_assert!(px.len() >= (rows * W) as usize);
        Self { px, top, rows }
    }

    /// Screen rows `y0..y1` that are inside this strip.
    fn span(&self, y0: i32, y1: i32) -> std::ops::Range<i32> {
        y0.max(self.top)..y1.min(self.top + self.rows)
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        ((0..W).contains(&x) && (self.top..self.top + self.rows).contains(&y))
            .then(|| ((y - self.top) * W + x) as usize)
    }

    pub fn get(&self, x: i32, y: i32) -> Option<Rgb> {
        self.index(x, y).map(|i| Rgb::from565(self.px[i]))
    }

    pub fn put(&mut self, x: i32, y: i32, c: Rgb) {
        if let Some(i) = self.index(x, y) {
            self.px[i] = c.to565();
        }
    }

    /// Paints `c` over the pixel with coverage `a` (0..=1).
    pub fn blend(&mut self, x: i32, y: i32, c: Rgb, a: f32) {
        if a > 0.0 {
            self.blend8(x, y, c, (a.min(1.0) * 255.0) as u8);
        }
    }

    /// [`Canvas::blend`] with integer coverage 0..=255. The C6 has no FPU,
    /// so per-pixel work stays in integers (text is all this).
    pub fn blend8(&mut self, x: i32, y: i32, c: Rgb, a: u8) {
        if a == 0 {
            return;
        }
        let Some(i) = self.index(x, y) else { return };
        if a == 255 {
            self.px[i] = c.to565();
            return;
        }
        let under = self.px[i];
        let (a, inv) = (u32::from(a), 255 - u32::from(a));
        // Mix in RGB565 space, channel by channel.
        let r = (u32::from(under >> 11) * inv + u32::from(c.0 >> 3) * a) / 255;
        let g = (u32::from((under >> 5) & 0x3f) * inv + u32::from(c.1 >> 2) * a) / 255;
        let b = (u32::from(under & 0x1f) * inv + u32::from(c.2 >> 3) * a) / 255;
        self.px[i] = (r << 11 | g << 5 | b) as u16;
    }

    /// Whether screen rows `y0..y1` touch this strip.
    pub fn touches(&self, y0: i32, y1: i32) -> bool {
        y1 > self.top && y0 < self.top + self.rows
    }

    pub fn fill(&mut self, c: Rgb) {
        let v = c.to565();
        for p in self.px.iter_mut().take((self.rows * W) as usize) {
            *p = v;
        }
    }

    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgb) {
        let v = c.to565();
        for row in self.span(y, y + h) {
            for col in x.max(0)..(x + w).min(W) {
                let i = ((row - self.top) * W + col) as usize;
                self.px[i] = v;
            }
        }
    }

    /// A vertical gradient from `from` (top) to `to` (bottom).
    pub fn vgradient(&mut self, x: i32, y: i32, w: i32, h: i32, from: Rgb, to: Rgb) {
        for row in self.span(y, y + h) {
            let c = from.mix(to, (row - y) as f32 / (h - 1).max(1) as f32);
            self.rect(x, row, w, 1, c);
        }
    }

    /// Filled rounded rectangle, anti-aliased corners.
    pub fn round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, c: Rgb) {
        let r = r.min(w / 2).min(h / 2);
        for row in self.span(y, y + h) {
            for col in x.max(0)..(x + w).min(W) {
                // Distance outside the rounded corner, if in a corner zone.
                let cx = if col < x + r {
                    x + r
                } else if col >= x + w - r {
                    x + w - r - 1
                } else {
                    col
                };
                let cy = if row < y + r {
                    y + r
                } else if row >= y + h - r {
                    y + h - r - 1
                } else {
                    row
                };
                if cx == col || cy == row {
                    self.put(col, row, c);
                    continue;
                }
                let d = (((col - cx) as f32).powi(2) + ((row - cy) as f32).powi(2)).sqrt();
                self.blend(col, row, c, r as f32 + 0.5 - d);
            }
        }
    }

    /// Filled circle, anti-aliased edge.
    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, c: Rgb) {
        for row in self.span((cy - r - 1.0) as i32, (cy + r + 2.0) as i32) {
            for col in (cx - r - 1.0) as i32..=(cx + r + 1.0) as i32 {
                let d = ((col as f32 + 0.5 - cx).powi(2) + (row as f32 + 0.5 - cy).powi(2)).sqrt();
                self.blend(col, row, c, r + 0.5 - d);
            }
        }
    }

    /// An arc `thickness` wide just inside radius `r`, from `start` degrees
    /// (0 = up, clockwise) through `sweep` degrees.
    #[allow(clippy::too_many_arguments)] // geometry + colours, like any draw call
    pub fn arc(
        &mut self,
        cx: f32,
        cy: f32,
        r: f32,
        thickness: f32,
        start: f32,
        sweep: f32,
        c: Rgb,
    ) {
        let inner = r - thickness;
        for row in self.span((cy - r - 1.0) as i32, (cy + r + 2.0) as i32) {
            for col in (cx - r - 1.0) as i32..=(cx + r + 1.0) as i32 {
                let dx = col as f32 + 0.5 - cx;
                let dy = row as f32 + 0.5 - cy;
                let d = (dx * dx + dy * dy).sqrt();
                let cover = (r + 0.5 - d).min(d - inner + 0.5).clamp(0.0, 1.0);
                if cover <= 0.0 {
                    continue;
                }
                if sweep < 360.0 {
                    let angle = (dx.atan2(-dy).to_degrees() - start).rem_euclid(360.0);
                    if angle > sweep {
                        continue;
                    }
                }
                self.blend(col, row, c, cover);
            }
        }
    }

    /// A thin line (1 px, anti-aliased along its length).
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: Rgb, alpha: f32) {
        if !self.touches(y0.min(y1) as i32 - 1, y0.max(y1) as i32 + 2) {
            return;
        }
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0) as i32;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let x = x0 + (x1 - x0) * t;
            let y = y0 + (y1 - y0) * t;
            let (fx, fy) = (x.floor(), y.floor());
            let (ax, ay) = (x - fx, y - fy);
            self.blend(fx as i32, fy as i32, c, alpha * (1.0 - ax) * (1.0 - ay));
            self.blend(fx as i32 + 1, fy as i32, c, alpha * ax * (1.0 - ay));
            self.blend(fx as i32, fy as i32 + 1, c, alpha * (1.0 - ax) * ay);
            self.blend(fx as i32 + 1, fy as i32 + 1, c, alpha * ax * ay);
        }
    }

    /// Filled convex polygon (point-in-polygon per pixel; small shapes only).
    pub fn polygon(&mut self, points: &[(f32, f32)], c: Rgb) {
        let (min_y, max_y) = points
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        if !self.touches(min_y as i32, max_y as i32 + 2) {
            return;
        }
        let (min_x, max_x) = points
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
        for row in self.span(min_y as i32, max_y as i32 + 2) {
            for col in min_x as i32..=max_x as i32 + 1 {
                // 2×2 supersampling for soft edges.
                let mut hits = 0;
                for (ox, oy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    if inside(points, col as f32 + ox, row as f32 + oy) {
                        hits += 1;
                    }
                }
                self.blend(col, row, c, hits as f32 / 4.0);
            }
        }
    }

    /// Draws `data` (1 bit per module) as a QR code `module` px per module.
    #[allow(clippy::too_many_arguments)] // geometry + colours, like any draw call
    pub fn qr(
        &mut self,
        x: i32,
        y: i32,
        size: i32,
        module: i32,
        dark: impl Fn(i32, i32) -> bool,
        fg: Rgb,
        bg: Rgb,
    ) {
        let quiet = 1;
        let full = (size + 2 * quiet) * module;
        self.rect(x, y, full, full, bg);
        for row in self.span(y, y + full) {
            let my = (row - y) / module - quiet;
            for col in x.max(0)..(x + full).min(W) {
                let mx = (col - x) / module - quiet;
                if (0..size).contains(&mx) && (0..size).contains(&my) && dark(mx, my) {
                    self.put(col, row, fg);
                }
            }
        }
    }
}

fn inside(points: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut odd = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (xi, yi) = points[i];
        let (xj, yj) = points[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            odd = !odd;
        }
        j = i;
    }
    odd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb565_round_trip_and_byte_order() {
        let c = Rgb(255, 128, 0);
        assert_eq!(Rgb::from565(c.to565()), Rgb(255, 130, 0));
        let mut px = vec![0u16; (W * 2) as usize];
        let mut canvas = Canvas::new(&mut px, 10, 2);
        canvas.put(0, 10, Rgb(255, 0, 0));
        canvas.put(5, 9, Rgb(255, 255, 255)); // above the strip: ignored
        assert_eq!(px[0], 0xF800, "native order for the panel");
        assert!(px.iter().skip(1).all(|&p| p == 0));
    }

    #[test]
    fn strips_draw_only_their_rows() {
        let mut a = vec![0u16; (W * 24) as usize];
        let mut canvas = Canvas::new(&mut a, 24, 24);
        canvas.rect(0, 0, W, H, Rgb(10, 20, 30));
        assert!(a.iter().all(|&p| p == Rgb(10, 20, 30).to565()));
        let mut b = vec![0u16; (W * 24) as usize];
        let mut canvas = Canvas::new(&mut b, 48, 24);
        canvas.rect(0, 0, W, 30, Rgb(255, 255, 255));
        assert!(b.iter().all(|&p| p == 0), "rect ends above this strip");
    }

    #[test]
    fn hue_matches_the_web_ui() {
        // hsl(137 55% 45%) for id 1, as the browser computes it.
        assert_eq!(Rgb::hue(1, 0.55, 0.45), Rgb(52, 178, 87));
    }
}
