# /// script
# requires-python = ">=3.12"
# dependencies = ["pillow>=11", "qrcode>=8"]
# ///
"""Mockups of the board's LCD cards (spec/07-display.md), at the panel's
real size (320 x 172, landscape) and color depth (RGB565), saved 3x so they
are easy to look at. Design exploration only: the firmware draws these
itself.

    uv run tools/display_mockups.py spec/display
"""
import colorsys
import math
import sys
from pathlib import Path

import qrcode
from PIL import Image, ImageDraw, ImageFont

W, H = 320, 172
FONT = "C:/Windows/Fonts/bahnschrift.ttf"  # mockup stand-in; firmware: an OFL font

# Theme: dark (the backlight glows less on dark pixels, and it reads across a room).
BG = (16, 18, 24)
PANEL = (28, 31, 40)
TEXT = (238, 236, 230)
MUTED = (148, 152, 166)
GOOD = (111, 191, 143)
WARN = (242, 180, 76)
HOT = (240, 98, 84)


def font(size: int, weight: str = "Regular") -> ImageFont.FreeTypeFont:
    f = ImageFont.truetype(FONT, size)
    try:
        f.set_variation_by_name(weight)
    except Exception:
        pass
    return f


def hue_color(seed: int, s: float = 0.55, l: float = 0.5) -> tuple[int, int, int]:
    """Same hue rule as the web UI's avatars: (id * 137) mod 360."""
    r, g, b = colorsys.hls_to_rgb(((seed * 137) % 360) / 360, l, s)
    return int(r * 255), int(g * 255), int(b * 255)


def mix(a, b, t):
    return tuple(int(x + (y - x) * t) for x, y in zip(a, b))


def rgb565(img: Image.Image) -> Image.Image:
    """What the panel can show: 5-6-5 bits per channel."""
    px = img.load()
    for y in range(img.height):
        for x in range(img.width):
            r, g, b = px[x, y][:3]
            px[x, y] = (r & 0xF8 | r >> 5, g & 0xFC | g >> 6, b & 0xF8 | b >> 5)
    return img


def gradient(d: ImageDraw.ImageDraw, box, top, bottom):
    x0, y0, x1, y1 = box
    for y in range(y0, y1):
        d.line([(x0, y), (x1, y)], fill=mix(top, bottom, (y - y0) / max(1, y1 - y0 - 1)))


def status_bar(d: ImageDraw.ImageDraw, page: int, pages: int, clock: str = "1:12 PM"):
    d.text((8, H - 15), clock, font=font(11), fill=MUTED)
    # Wi-Fi bars.
    for i in range(4):
        h = 3 + i * 2
        d.rectangle([W - 22 + i * 4, H - 6 - h, W - 20 + i * 4, H - 6], fill=MUTED if i < 3 else (70, 74, 86))
    # Which card of the rotation.
    x = W // 2 - pages * 5
    for i in range(pages):
        d.ellipse([x + i * 10, H - 11, x + i * 10 + 5, H - 6], fill=TEXT if i == page else (70, 74, 86))


def avatar(d: ImageDraw.ImageDraw, cx: int, cy: int, r: int, name: str, seed: int, ring=None):
    if ring:
        d.ellipse([cx - r - 2, cy - r - 2, cx + r + 2, cy + r + 2], fill=ring)
    d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=hue_color(seed))
    f = font(int(r * 1.1), "Bold")
    d.text((cx, cy + 1), name[0].upper(), font=f, fill=(255, 255, 255), anchor="mm")


def icon(d: ImageDraw.ImageDraw, kind: str, x: int, y: int, s: int, color):
    """Kind icons from primitives: what the firmware's sprite sheet would hold."""
    if kind == "board":  # a die with five pips
        d.rounded_rectangle([x, y, x + s, y + s], radius=s // 5, fill=color)
        p = s // 9
        for cx, cy in [(0.27, 0.27), (0.73, 0.27), (0.5, 0.5), (0.27, 0.73), (0.73, 0.73)]:
            px, py = x + int(cx * s), y + int(cy * s)
            d.ellipse([px - p, py - p, px + p, py + p], fill=BG)
    elif kind == "card":  # two fanned cards
        d.rounded_rectangle([x + s * 0.15, y, x + s * 0.75, y + s * 0.85], radius=s // 8, fill=mix(color, BG, 0.45))
        d.rounded_rectangle([x + s * 0.3, y + s * 0.15, x + s * 0.9, y + s], radius=s // 8, fill=color)
    elif kind == "video":  # a gamepad
        d.rounded_rectangle([x, y + s * 0.25, x + s, y + s * 0.8], radius=s // 4, fill=color)
        c = y + s * 0.52
        d.rectangle([x + s * 0.17, c - s * 0.04, x + s * 0.37, c + s * 0.04], fill=BG)
        d.rectangle([x + s * 0.23, c - s * 0.1, x + s * 0.31, c + s * 0.1], fill=BG)
        for bx in (0.65, 0.8):
            d.ellipse([x + s * bx - s * 0.05, c - s * 0.05, x + s * bx + s * 0.05, c + s * 0.05], fill=BG)
    else:  # rpg: a d20-ish hexagon
        pts = [(x + s / 2 + s / 2 * math.cos(math.radians(a)), y + s / 2 + s / 2 * math.sin(math.radians(a))) for a in range(-90, 270, 60)]
        d.polygon(pts, fill=color)
        d.text((x + s / 2, y + s / 2 + 1), "20", font=font(int(s * 0.38), "Bold"), fill=BG, anchor="mm")


def pattern(d: ImageDraw.ImageDraw, box, color, kind: str):
    """A faint kind-themed texture behind the header (pips / hex tiles)."""
    x0, y0, x1, y1 = box
    faint = mix(color, BG, 0.82)
    if kind == "board":
        r = 9
        for row, y in enumerate(range(y0, y1 + r, int(r * 1.5))):
            for x in range(x0 - r + (row % 2) * r, x1 + r, r * 2):
                pts = [(x + r * math.cos(math.radians(a)), y + r * math.sin(math.radians(a))) for a in range(30, 390, 60)]
                d.polygon(pts, outline=faint)
    else:
        for y in range(y0 + 4, y1, 10):
            for x in range(x0 + 4 + (y // 10 % 2) * 5, x1, 10):
                d.ellipse([x, y, x + 1, y + 1], fill=faint)


def card(name: str, draw, page: int, pages: int, out: Path):
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)
    draw(img, d)
    status_bar(d, page, pages)
    rgb565(img).resize((W * 3, H * 3), Image.NEAREST).save(out / f"{name}.png")


def upcoming(img, d):
    accent = hue_color(3, 0.6, 0.55)  # Catan's game color
    gradient(d, (0, 0, W, 58), mix(accent, BG, 0.55), BG)
    pattern(d, (0, 0, W, 58), accent, "board")
    icon(d, "board", 12, 12, 34, accent)
    d.text((56, 10), "UP NEXT", font=font(11, "Bold"), fill=accent)
    d.text((56, 22), "Saturday game night", font=font(21, "SemiBold"), fill=TEXT)
    # When: a chip with the relative time.
    d.rounded_rectangle([12, 66, 118, 86], radius=10, fill=PANEL)
    d.text((65, 76), "Sat 2:00 PM", font=font(13, "SemiBold"), fill=TEXT, anchor="mm")
    d.text((126, 76), "in 3 h 20 min", font=font(13), fill=MUTED, anchor="lm")
    d.text((12, 96), "Catan  ·  Wingspan", font=font(15, "SemiBold"), fill=TEXT)
    for i, (n, seed) in enumerate([("Dad", 1), ("Mom", 3), ("Sam", 2)]):
        avatar(d, 24 + i * 26, 135, 11, n, seed, ring=GOOD)
    avatar(d, 24 + 3 * 26, 135, 11, "Alex", 4, ring=WARN)
    d.text((136, 129), "3 in, 1 maybe", font=font(13), fill=MUTED)
    d.text((136, 143), "seats: 4", font=font(11), fill=MUTED)


def starting_soon(img, d):
    accent = HOT
    gradient(d, (0, 0, W, H - 18), mix(accent, BG, 0.7), BG)
    d.text((W / 2, 13), "GAME STARTING SOON!", font=font(13, "Bold"), fill=WARN, anchor="mm")
    # Countdown ring around the number (2 h window, 47 min left): a dim
    # full track, the remaining time bright.
    ring = [W / 2 - 42, 26, W / 2 + 42, 110]
    d.ellipse(ring, outline=mix(WARN, BG, 0.75), width=4)
    d.arc(ring, -90, -90 + 360 * 47 / 120, fill=WARN, width=4)
    d.text((W / 2, 62), "47", font=font(44, "Bold"), fill=TEXT, anchor="mm")
    d.text((W / 2, 88), "min", font=font(12), fill=MUTED, anchor="mm")
    icon(d, "board", 16, 118, 22, hue_color(3, 0.6, 0.55))
    d.text((46, 120), "Catan", font=font(17, "SemiBold"), fill=TEXT)
    d.text((46, 139), "Home · Dad, Mom, Sam · bring snacks", font=font(11), fill=MUTED)


def needs_players(img, d):
    accent = hue_color(5, 0.55, 0.55)
    d.rounded_rectangle([8, 8, W - 8, H - 24], radius=12, fill=PANEL)
    icon(d, "card", 18, 18, 30, accent)
    d.text((58, 16), "NEW PLAN · NEEDS PLAYERS", font=font(11, "Bold"), fill=accent)
    d.text((58, 29), "Wingspan", font=font(22, "SemiBold"), fill=TEXT)
    d.text((18, 62), "Sun 1:00 PM  or  Mon 7:00 PM", font=font(14), fill=TEXT)
    # Player meter: 1 of 3 needed.
    for i in range(5):
        filled = i < 1
        need = i < 3
        x = 18 + i * 22
        d.rounded_rectangle([x, 88, x + 16, 104], radius=4, fill=GOOD if filled else (PANEL if not need else (70, 74, 86)), outline=None if need else (70, 74, 86))
    d.text((134, 96), "2 more to play", font=font(14, "SemiBold"), fill=WARN, anchor="lm")
    d.text((18, 118), "posted by Sam · answer on", font=font(11), fill=MUTED)
    d.text((18, 131), "gamematcher.local", font=font(13, "SemiBold"), fill=TEXT)
    qr = qrcode.QRCode(border=1, box_size=2)
    qr.add_data("http://gamematcher.local/plans/7")
    qimg = qr.make_image(fill_color=(16, 18, 24), back_color=(238, 236, 230)).convert("RGB")
    img.paste(qimg, (W - 18 - qimg.width, 66))


def rsvp(img, d):
    accent = GOOD
    avatar(d, 52, 64, 34, "Sam", 2, ring=accent)
    d.text((100, 30), "Sam is in!", font=font(24, "SemiBold"), fill=TEXT)
    d.text((100, 60), "for Saturday game night", font=font(13), fill=MUTED)
    d.text((100, 78), "would play: Catan, Wingspan", font=font(12), fill=TEXT)
    d.text((100, 96), "“only after 3”", font=font(12), fill=MUTED)
    d.rounded_rectangle([16, 116, W - 16, 136], radius=10, fill=PANEL)
    d.text((W / 2, 126), "12 min ago · 3 in, 1 maybe", font=font(12), fill=MUTED, anchor="mm")


def idle(img, d):
    d.text((W / 2, 46), "1:12", font=font(56, "Bold"), fill=TEXT, anchor="mm")
    d.text((W / 2, 82), "Saturday, October 17", font=font(13), fill=MUTED, anchor="mm")
    d.text((W / 2, 108), "Nothing planned yet. Up for a game?", font=font(13, "SemiBold"), fill=TEXT, anchor="mm")
    d.text((W / 2, 126), "gamematcher.local", font=font(12), fill=GOOD, anchor="mm")


def main() -> None:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "spec/display")
    out.mkdir(parents=True, exist_ok=True)
    cards = [("1-upcoming", upcoming), ("2-starting-soon", starting_soon), ("3-needs-players", needs_players), ("4-rsvp", rsvp), ("5-idle", idle)]
    for i, (name, draw) in enumerate(cards):
        card(name, draw, i, 4, out)
    print("wrote", ", ".join(n for n, _ in cards), "to", out)


if __name__ == "__main__":
    main()
