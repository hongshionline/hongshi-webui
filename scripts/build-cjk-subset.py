"""Subset a CJK face down to the characters this client can actually put on screen.

The interface is Chinese and the fonts that ship with Windows are not round, so a
rounded CJK webfont is the only way to make the Chinese match the Latin — but a full
face is 14 MB per weight, which is twenty times the rest of the binary. Subsetting is
what makes it affordable, and subsetting is only safe if the character set is derived
from the sources rather than typed by hand: a string added to a page later would
otherwise render as tofu.

Where the characters come from:

* `web/` — every page, script and stylesheet.
* `src/` — Chinese in Rust reaches the screen too: the shell's own error reasons,
  the missing-kernel notice, the HTTP backend name shown in 设置.
* **GB2312 level 1**, the 3755 most common characters, generated here rather than
  kept as a data file. Only the server sends text the client did not author — 每日资讯
  headlines — and it can contain any character at all, so the Regular face carries
  the common set and a headline does not break into a second typeface mid-sentence.

The two weights get different sets, and that is the point rather than an oversight:
the common set is 6x the size of the client's own text, and every place this interface
renders Chinese at ≥600 is a string it wrote itself. `Regular` takes the common set
because that is the weight remote text arrives in; `Bold` takes the client's own
characters, which is all it can ever be asked for.

    python scripts/build-cjk-subset.py \
        --regular target/ResourceHanRoundedCN-Regular.ttf \
        --bold target/ResourceHanRoundedCN-Bold.ttf \
        --out web/fonts

Exits non-zero on anything it cannot do, so it is safe to run from a build.
"""

import argparse
import re
import sys
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent

# CJK punctuation and fullwidth forms the UI uses, plus the ASCII the face should
# carry so a Chinese string containing a digit or a colon still renders in one face
# rather than two. ASCII is also in Nunito, which comes first in the stack — this is
# belt and braces for the strings that mix both.
PUNCTUATION = (
    "，。、；：？！“”‘’（）《》〈〉【】〔〕「」『』—…·～￥％＋－×÷＝"
    "0123456789"
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
    " .,:;!?()[]{}<>/\\|-–_=+*&%$#@'\"`~^"
)

CJK = re.compile(r"[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff\uff00-\uffef\u3000-\u303f]")
SOURCES = {"web": (".html", ".js", ".css"), "src": (".rs",)}


def common_characters() -> set[str]:
    """GB2312 level 1: the 3755 characters that make up ordinary modern Chinese.

    Generated from the encoding's own layout instead of shipping a list of thousands
    of characters in a build script. Level 1 is the row range `0xB0A1`-`0xD7F9`, which
    is ordered by pinyin and holds exactly the common set.
    """
    characters: set[str] = set()
    for high in range(0xB0, 0xD8):
        for low in range(0xA1, 0xFF):
            try:
                characters.add(bytes([high, low]).decode("gb2312"))
            except UnicodeDecodeError:
                continue  # the gaps at the end of each row
    return characters


def collect() -> tuple[set[str], list[str]]:
    """Every character the client can display, and where each source contributed."""
    chars: set[str] = set(PUNCTUATION)
    report: list[str] = []

    for folder, extensions in SOURCES.items():
        found: set[str] = set()
        files = 0
        for path in sorted((ROOT / folder).rglob("*")):
            if path.suffix not in extensions or not path.is_file():
                continue
            files += 1
            text = path.read_text(encoding="utf-8", errors="replace")
            found.update(CJK.findall(text))
        chars |= found
        report.append(f"{folder}/: {len(found)} CJK characters from {files} file(s)")

    return chars, report


def subset_one(source: Path, out: Path, chars: set[str], weight: str) -> Path:
    options = subset.Options()
    # Keep the vertical and layout tables that shape CJK correctly, and drop everything
    # a browser does not need: hinting, legacy tables, and the thousands of glyphs that
    # are not in the set.
    options.layout_features = ["*"]
    options.drop_tables += ["DSIG", "vhea", "vmtx", "VORG", "EBDT", "EBLC", "EBSC", "SVG "]
    options.notdef_outline = True
    options.recalc_bounds = True
    options.desubroutinize = False

    font = TTFont(source, lazy=False)
    subsetter = subset.Subsetter(options=options)
    subsetter.populate(text="".join(sorted(chars)))
    subsetter.subset(font)

    # The subset must not claim to be the whole family: `font-display: swap` means the
    # system face shows first, and a stack that named this font for a character it no
    # longer has would be describing a font it is not.
    family = "Hongshi Han Rounded"
    for record in font["name"].names:
        if record.nameID == 1:
            font["name"].setName(family, 1, record.platformID, record.platEncID, record.langID)
        elif record.nameID == 2:
            font["name"].setName(weight, 2, record.platformID, record.platEncID, record.langID)
        elif record.nameID in (4, 6):
            font["name"].setName(f"{family} {weight}", record.nameID, record.platformID,
                                 record.platEncID, record.langID)

    out.parent.mkdir(parents=True, exist_ok=True)
    font.flavor = "woff2"
    font.save(out)
    font.close()
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--regular", type=Path, help="the full Regular face to subset")
    parser.add_argument("--bold", type=Path, help="the full Bold face to subset")
    parser.add_argument("--out", type=Path, default=ROOT / "web/fonts")
    parser.add_argument("--write-charset", type=Path,
                        help="also write the client's own set here, one character per line")
    args = parser.parse_args()

    own, report = collect()
    for line in report:
        print(line)

    common = common_characters()
    print(f"gb2312 level 1: {len(common)} characters, {len(common - own)} of them new")
    print(f"client's own text: {len(own)} characters")
    print(f"regular will carry {len(own | common)}, bold will carry {len(own)}")

    if args.write_charset:
        args.write_charset.parent.mkdir(parents=True, exist_ok=True)
        args.write_charset.write_text("\n".join(sorted(own)), encoding="utf-8")
        print(f"charset written to {args.write_charset}")

    if not args.regular and not args.bold:
        print("no face given, so nothing was subset")
        return 0

    for source, weight, chars in (
        (args.regular, "Regular", own | common),
        (args.bold, "Bold", own),
    ):
        if not source:
            continue
        if not source.is_file():
            print(f"missing: {source}", file=sys.stderr)
            return 2
        out = args.out / f"han-rounded-{weight.lower()}.woff2"
        subset_one(source, out, chars, weight)
        print(f"{out.relative_to(ROOT)}: {out.stat().st_size / 1024:.0f} KB "
              f"({len(chars)} characters)")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
