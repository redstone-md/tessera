#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
# Copyright (C) 2026 Tessera contributors.
"""Draws the pictures the registry shows of its themes and extensions.

A theme is drawn as a desktop in its colors, shapes and sizes: its wallpaper
(registry/wallpapers/<id>.jpg) under a top bar, a dock and an open popup.
The picture is a mock-up made from the theme file, not a screenshot of
Tessera. An extension's picture is its icon (extensions/<id>/icon.png)
with its corners cut round.

    python3 registry/preview.py [--font PATH]

Needs Pillow. Run it from anywhere; it writes registry/previews/ and
registry/<id>/icon.png, which update.sh then lists with their checksums.
"""

import argparse
import colorsys
import glob
import os
import tomllib

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REGISTRY = os.path.join(ROOT, "registry")
WIDTH, HEIGHT = 1280, 800
ICON_SIZE = 256

ROLES = (
    "surface_container",
    "surface_container_highest",
    "on_surface",
    "on_surface_variant",
    "outline",
    "primary",
    "secondary_container",
)

FONTS = (
    "/usr/share/fonts/opentype/inter/Inter-Regular.otf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "C:/Windows/Fonts/segoeui.ttf",
)


def rgb(text):
    text = text.lstrip("#")
    return tuple(int(text[i : i + 2], 16) for i in (0, 2, 4))


def mix(a, b, share):
    return tuple(round(x + (y - x) * share) for x, y in zip(a, b))


def derived(seed, light):
    """Roles near those Tessera derives from a seed, for a theme that sets none."""
    h, _, s = colorsys.rgb_to_hls(*(c / 255 for c in seed))
    tone = lambda l, sat=0.25: tuple(round(c * 255) for c in colorsys.hls_to_rgb(h, l, min(s, sat)))
    if light:
        return dict(zip(ROLES, (tone(0.95), tone(0.89), tone(0.12), tone(0.35), tone(0.6), seed, tone(0.85, 0.6))))
    return dict(zip(ROLES, (tone(0.10), tone(0.16), tone(0.92), tone(0.72), tone(0.45), seed, tone(0.28, 0.4))))


class Theme:
    """What a theme file says, with Tessera's defaults for what it leaves out."""

    def __init__(self, path):
        with open(path, "rb") as file:
            data = tomllib.load(file)
        self.id = os.path.splitext(os.path.basename(path))[0]
        light = data.get("scheme", "dark") == "light"
        seed = rgb(data["seed"]) if data.get("seed", "#").startswith("#") else (0xAD, 0xD2, 0x8E)
        self.colors = derived(seed, light)
        self.colors.update({role: rgb(value) for role, value in data.get("colors", {}).items()})
        shape, spacing = data.get("shape", {}), data.get("spacing", {})
        self.radius = (shape.get("small", 8), shape.get("medium", 16), shape.get("large", 24))
        self.gap = (spacing.get("tiny", 4), spacing.get("small", 8), spacing.get("medium", 16))
        self.pill = data.get("bar", {}).get("pill_height", 32)
        self.flat = data.get("bar", {}).get("flat", False)
        dock = data.get("dock", {})
        self.dock = (dock.get("container_height", 60), dock.get("item_size", 46), dock.get("icon_size", 32))
        self.label = data.get("typography", {}).get("label_size", 14)


class Painter:
    """Draws on an image at twice the size and scales it down, for smooth edges."""

    SCALE = 2

    def __init__(self, base, font_path):
        self.image = base.resize((WIDTH * self.SCALE, HEIGHT * self.SCALE), Image.LANCZOS).convert("RGBA")
        self.font_path = font_path

    def s(self, value):
        return round(value * self.SCALE)

    def box(self, x, y, w, h, radius, fill, blur_under=False):
        x, y, w, h, r = map(self.s, (x, y, w, h, radius))
        if blur_under:
            region = self.image.crop((x, y, x + w, y + h)).filter(ImageFilter.GaussianBlur(self.s(18)))
            mask = Image.new("L", region.size, 0)
            ImageDraw.Draw(mask).rounded_rectangle((0, 0, w - 1, h - 1), r, fill=255)
            self.image.paste(region, (x, y), mask)
        layer = Image.new("RGBA", self.image.size, (0, 0, 0, 0))
        ImageDraw.Draw(layer).rounded_rectangle((x, y, x + w, y + h), r, fill=fill)
        self.image.alpha_composite(layer)

    def text(self, x, y, text, size, fill, anchor="lm"):
        font = ImageFont.truetype(self.font_path, self.s(size))
        ImageDraw.Draw(self.image).text((self.s(x), self.s(y)), text, font=font, fill=fill, anchor=anchor)

    def line(self, points, fill, width):
        ImageDraw.Draw(self.image).line([(self.s(a), self.s(b)) for a, b in points], fill=fill, width=self.s(width), joint="curve")

    def arc(self, box, start, end, fill, width):
        ImageDraw.Draw(self.image).arc([self.s(v) for v in box], start, end, fill=fill, width=self.s(width))

    def done(self):
        return self.image.resize((WIDTH, HEIGHT), Image.LANCZOS).convert("RGB")


def cover(image, size):
    w, h = size
    scale = max(w / image.width, h / image.height)
    image = image.resize((round(image.width * scale), round(image.height * scale)), Image.LANCZOS)
    left, top = (image.width - w) // 2, (image.height - h) // 2
    return image.crop((left, top, left + w, top + h))


def alpha(color, value):
    return (*color, round(255 * value))


def draw_theme(theme, font):
    wallpaper = os.path.join(REGISTRY, "wallpapers", theme.id + ".jpg")
    c = theme.colors
    if os.path.exists(wallpaper):
        base = cover(Image.open(wallpaper).convert("RGB"), (WIDTH, HEIGHT))
    else:
        base = Image.new("RGB", (WIDTH, HEIGHT), mix(c["surface_container"], c["primary"], 0.25))
    p = Painter(base, font)
    small, medium, large = theme.radius
    tiny, gap, wide = theme.gap
    pill, label = theme.pill, theme.label
    field = None if theme.flat else alpha(c["surface_container"], 0.92)
    top = 10 + pill / 2

    # The top bar: the mark and workspaces, the clock, the status.
    def pill_at(x, w):
        if field:
            p.box(x, 10, w, pill, min(pill / 2, max(medium, 6)), field, blur_under=True)

    pill_at(14, 210)
    for i in range(3):
        p.box(30 + i * 7, top - 7, 5, 5, 1.5, c["primary"])
    p.box(33, top - 1, 12, 7, 2, c["primary"])
    for i in range(4):
        w = 34 if i == 1 else 18
        x = 64 + sum(34 if j == 1 else 18 for j in range(i)) + i * 8
        p.box(x, top - 7, w, 14, 7, c["primary"] if i == 1 else alpha(c["on_surface_variant"], 0.55))
    pill_at(WIDTH / 2 - 95, 190)
    p.text(WIDTH / 2, top, "Thu 26 Nov  14:32", label, c["on_surface"], "mm")
    pill_at(WIDTH - 214, 200)
    x0 = WIDTH - 196
    for r in (12, 8, 4):
        p.arc((x0 + 12 - r, top + 2 - r, x0 + 12 + r, top + 2 + r), 225, 315, c["on_surface"], 2)
    p.box(x0 + 40, top - 4, 6, 8, 1, c["on_surface"])
    p.line([(x0 + 46, top - 4), (x0 + 52, top - 9), (x0 + 52, top + 9), (x0 + 46, top + 4)], c["on_surface"], 2)
    p.box(x0 + 66, top - 1, 48, 4, 2, alpha(c["on_surface_variant"], 0.4))
    p.box(x0 + 66, top - 1, 30, 4, 2, c["primary"])
    p.text(x0 + 124, top, "62%", label * 0.9, c["on_surface_variant"], "lm")
    p.line([(x0 + 158, top - 6), (x0 + 176, top - 6), (x0 + 176, top + 6), (x0 + 158, top + 6), (x0 + 158, top - 6)], c["on_surface"], 2)
    p.box(x0 + 160, top - 4, 10, 8, 1, c["primary"])

    # The dock, with an application in front marked.
    container, item, icon = theme.dock
    apps = 7
    dock_w = apps * (item + gap) + gap + wide
    dock_x, dock_y = (WIDTH - dock_w) / 2, HEIGHT - container - 14
    p.box(dock_x, dock_y, dock_w, container, max(large, 8), alpha(c["surface_container"], 0.9), blur_under=True)
    glyphs = ("folder", "web", "code", "mail", "music", "photo", "terminal")
    for i, glyph in enumerate(glyphs):
        x = dock_x + wide / 2 + gap + i * (item + gap)
        y = dock_y + (container - icon) / 2 - 2
        left = x + (item - icon) / 2
        p.box(left, y, icon, icon, icon * 0.26, c["surface_container_highest"])
        ink = c["primary"] if i % 2 == 0 else c["on_surface_variant"]
        p.box(left + icon * 0.3, y + icon * 0.3, icon * 0.4, icon * 0.4, icon * 0.12, ink)
        if i in (0, 2, 4):
            width = 18 if i == 2 else 6
            p.box(x + item / 2 - width / 2, dock_y + container - 8, width, 4, 2, c["primary"] if i == 2 else c["on_surface_variant"])

    # A popup: a calendar beside a list of notifications.
    px_, py_, pw, ph = WIDTH - 404, 10 + pill + 12, 380, 360
    p.box(px_, py_, pw, ph, max(large, 6), alpha(c["surface_container"], 0.96), blur_under=True)
    p.text(px_ + wide + 4, py_ + 30, "November 2026", label + 3, c["on_surface"], "lm")
    cell = 40
    for row in range(5):
        for col in range(7):
            day = row * 7 + col - 5
            if not 1 <= day <= 30:
                continue
            cx, cy = px_ + wide + 4 + col * cell + cell / 2, py_ + 70 + row * 34
            if day == 26:
                p.box(cx - 15, cy - 15, 30, 30, 15, c["primary"])
                p.text(cx, cy, str(day), label, c["surface_container"], "mm")
            else:
                p.text(cx, cy, str(day), label, c["on_surface_variant"], "mm")
    ny = py_ + 250
    p.box(px_ + wide, ny, pw - 2 * wide, 88, max(medium, 4), c["surface_container_highest"])
    p.box(px_ + wide + 14, ny + 18, 28, 28, 8, c["secondary_container"])
    p.text(px_ + wide + 56, ny + 26, "Build finished", label, c["on_surface"], "lm")
    p.text(px_ + wide + 56, ny + 50, "All 214 tests passed in 38 s", label * 0.9, c["on_surface_variant"], "lm")
    p.box(px_ + pw - wide - 84, ny + 58, 70, 22, 11, c["primary"])

    # A window of the file manager, in the theme.
    wx, wy, ww, wh = 60, 110, 600, 400
    p.box(wx, wy, ww, wh, max(large, 6), alpha(c["surface_container"], 0.97), blur_under=True)
    p.box(wx + 220, wy + 92, ww - 232, wh - 104, max(large, 6), alpha(c["surface_container_highest"], 0.5))
    p.text(wx + 20, wy + 22, "Documents", label, c["on_surface"], "lm")
    p.box(wx + 120, wy + 52, ww - 140, 30, max(medium, 4), alpha(c["surface_container_highest"], 0.8))
    p.text(wx + 134, wy + 67, "This PC  \u203a  C:  \u203a  Users  \u203a  Documents", label * 0.9, c["on_surface"], "lm")
    for i, name in enumerate(("Home", "Desktop", "Documents", "Downloads", "Pictures", "Music")):
        y = wy + 108 + i * 36
        if name == "Documents":
            p.box(wx + 10, y - 15, 200, 30, max(medium, 4), c["secondary_container"])
        p.box(wx + 24, y - 6, 14, 11, 2, c["primary"])
        p.text(wx + 48, y, name, label, c["on_surface"], "lm")
    files = (("Projects", "", True), ("Invoices", "", True), ("Roadmap 2027.md", "14 KB", False),
             ("Budget.xlsx", "86 KB", False), ("Photo 01.jpg", "3.2 MB", False), ("Photo 02.jpg", "2.9 MB", False))
    for i, (name, size, folder) in enumerate(files):
        y = wy + 110 + i * 40
        if i == 2:
            p.box(wx + 228, y - 16, ww - 248, 32, max(small, 3), c["secondary_container"])
        p.box(wx + 242, y - 7, 16, 13, 2, c["primary"] if folder else c["on_surface_variant"])
        p.text(wx + 270, y, name, label, c["on_surface"], "lm")
        p.text(wx + ww - 40, y, size, label * 0.9, c["on_surface_variant"], "rm")
    return p.done()


def rounded_icon(path):
    image = Image.open(path).convert("RGBA").resize((ICON_SIZE * 4, ICON_SIZE * 4), Image.LANCZOS)
    mask = Image.new("L", image.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, image.width - 1, image.height - 1), image.width * 0.22, fill=255)
    image.putalpha(Image.composite(image.getchannel("A"), mask, mask))
    return image.resize((ICON_SIZE, ICON_SIZE), Image.LANCZOS)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--font", default=next((f for f in FONTS if os.path.exists(f)), None))
    font = parser.parse_args().font
    os.makedirs(os.path.join(REGISTRY, "previews"), exist_ok=True)
    for path in sorted(glob.glob(os.path.join(ROOT, "themes", "*.toml"))):
        theme = Theme(path)
        out = os.path.join(REGISTRY, "previews", theme.id + ".jpg")
        draw_theme(theme, font).save(out, quality=86, optimize=True, progressive=True)
        print("theme", theme.id)
    for path in sorted(glob.glob(os.path.join(ROOT, "extensions", "*", "icon.png"))):
        extension = os.path.basename(os.path.dirname(path))
        target = os.path.join(REGISTRY, extension)
        if os.path.isdir(target):
            rounded_icon(path).save(os.path.join(target, "icon.png"), optimize=True)
            print("extension", extension)


if __name__ == "__main__":
    main()
