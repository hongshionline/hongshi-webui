"""Measure the shipped palette so a derived theme can be checked against it.

Reads `web/main.css` and prints every token's hue / saturation / lightness, its contrast
against `--plate` and `--ground`, and the surface ramp's luminance ratios. Those are the
numbers `derivePalette` in `web/app.js` reproduces for a colour the user picks, so this is
how a change on either side gets checked: the ratios here and the tokens there have to
agree, and a theme derived from the shipped `--ground` should come back within a unit or
two per channel of the shipped tokens.

Useful to run before touching `THEME_*` in `web/app.js`, and after touching the palette.

Writes nothing.

    python scripts/palette-measure.py
"""
import colorsys
import os
import re

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CSS = open(os.path.join(ROOT, "web", "main.css"), encoding="utf-8").read()
root = CSS[CSS.index(":root {"):CSS.index("\n}", CSS.index(":root {"))]

TOKENS = {}
for name, value in re.findall(r"--([a-z0-9-]+):\s*([^;]+);", root):
    value = value.strip()
    if re.fullmatch(r"#[0-9a-fA-F]{6}", value):
        TOKENS[name] = value


def rgb(hexstr):
    hexstr = hexstr.lstrip("#")
    return tuple(int(hexstr[i:i + 2], 16) for i in (0, 2, 4))


def lin(c):
    c = c / 255
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def lum(hexstr):
    r, g, b = (lin(v) for v in rgb(hexstr))
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(a, b):
    la, lb = lum(a), lum(b)
    return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)


def hsl(hexstr):
    r, g, b = (v / 255 for v in rgb(hexstr))
    h, l, s = colorsys.rgb_to_hls(r, g, b)
    return round(h * 360, 1), round(s * 100, 1), round(l * 100, 1)


print("token      hex        H     S%    L%     contrast vs plate   vs ground")
print("-" * 78)
ground = TOKENS["ground"]
plate = TOKENS["plate"]
for name in ["ground", "wash", "plate", "rule", "fg", "fg-soft", "fg-dim", "fg-faint", "edge",
             "signal", "signal-hot", "signal-deep", "signal-dim", "signal-ink",
             "screen", "screen-surface", "screen-line", "term-fg", "term-live",
             "heat-1", "heat-2", "heat-3", "heat-4", "calendar", "dead", "ok", "warn", "high"]:
    if name not in TOKENS:
        continue
    h, s, l = hsl(TOKENS[name])
    print("%-10s %-9s %5.1f %5.1f %5.1f      %8.2f        %8.2f"
          % (name, TOKENS[name], h, s, l, contrast(TOKENS[name], plate), contrast(TOKENS[name], ground)))

print()
print("surface ramp, relative to ground's luminance (%.5f):" % lum(ground))
for name in ["wash", "plate", "rule", "screen", "screen-surface", "screen-line"]:
    print("  %-15s %.2fx" % (name, lum(TOKENS[name]) / lum(ground)))

print()
print("scrim token is rgba(46, 7, 12, 0.62) -> hsl", hsl("#2e070c"),
      " luminance %.4f = %.2fx ground" % (lum("#2e070c"), lum("#2e070c") / lum(ground)))
print("on-signal #ffffff over signal: %.2f:1" % contrast("#ffffff", TOKENS["signal"]))
