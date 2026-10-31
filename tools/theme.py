#!/usr/bin/env python3
"""Colour themes on the computer (docs/theming.md, T7): the built-in themes as example files, and a theme file drawn
through the simulator.

  theme.py examples OUT_DIR [--drive]   writes DARK.ini, OUTDOOR.ini and LIGHT.ini: the INI the firmware reads, every
                                        key set to the built-in's value, a comment line per key. Nothing is written to
                                        a mounted drive (/Volumes) unless --drive: copy the files to \\Rustfw\\Themes\\.
  theme.py preview FILE [--out PNG]     draws the file's overview, a ride page and the menu through the simulator
                                        (cargo test-host, the ignored test theme_preview in src/sim/theme_tests.rs);
                                        prints the BMPs, or one PNG of the three when PIL is installed.
  theme.py selftest                     the examples equal test-data/themes (the fixture src/ui/theme.rs parses back to
                                        the built-ins), each key once, every comment plain, the colours round-trip.

The built-in values below are the RGB565 constants of src/ui/mod.rs, src/training/ride.rs and src/ui/theme.rs,
written as the same 8-bit channels; the Rust test on the fixture is what keeps them equal to the code.
"""
import argparse
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIXTURE = os.path.join(ROOT, "test-data", "themes")
SHOTS = ("overview", "page", "menu")

# The colour keys, in the order of src/ui/theme.rs NAMES.
NAMES = [
    "background", "text", "line", "dim", "accent", "on_accent", "alarm", "on_alarm", "ok", "warn", "ferris", "cog",
    "course", "zone1", "zone2", "zone3", "zone4", "zone5", "zone6", "zone7",
]
# The metrics (built-in values, bounds) as src/ui/theme.rs ZONE_BAR, FRAME, MENU_ROW.
METRICS = [("zone_bar", 8, 4, 8), ("frame", 3, 1, 4), ("menu_row", 44, 44, 56)]


def rgb(r, g, b):
    """RGB565 of 8-bit channels, each rounded (src/ui/theme.rs `rgb`)."""
    return ((r * 31 + 127) // 255) << 11 | ((g * 63 + 127) // 255) << 5 | ((b * 31 + 127) // 255)


# DARK: today's constants (src/ui/mod.rs BLACK, WHITE, DARK, GREY, ACCENT, RED, GREEN, YELLOW, RUST, COG_COLOR; the
# text on the accent is black and the text on the alarm white; map::COURSE; ride::ZONE_COLORS).
DARK = [
    0x0000, 0xFFFF, 0x39E7, 0x8410, 0xFD20, 0x0000, 0xF800, 0xFFFF, 0x07E0, 0xFFE0, 0xF260, 0xC618, 0xC81F,
    0x4C90, 0x4C86, 0xDDC9, 0xD408, 0xB98B, 0x6237, 0x5A8D,
]
OUTDOOR = list(DARK)
OUTDOOR[2:4] = [rgb(112, 112, 112), rgb(192, 192, 192)]
OUTDOOR[13:20] = [rgb(94, 196, 178), rgb(94, 196, 70), rgb(255, 250, 98), rgb(255, 174, 84), rgb(248, 64, 118),
                  rgb(136, 90, 248), rgb(120, 110, 144)]
LIGHT = [
    0xFFFF, 0x0000, rgb(140, 140, 140), rgb(96, 96, 96), rgb(176, 64, 0), 0xFFFF, rgb(208, 0, 0), 0xFFFF,
    rgb(0, 112, 0), rgb(138, 100, 0), rgb(192, 72, 0), rgb(112, 112, 112), rgb(112, 0, 192), rgb(47, 98, 89),
    rgb(47, 98, 35), rgb(148, 125, 49), rgb(143, 87, 42), rgb(124, 32, 59), rgb(68, 45, 124), rgb(60, 55, 72),
]
BUILTIN = [("DARK", "dark", DARK), ("OUTDOOR", "outdoor", OUTDOOR), ("LIGHT", "light", LIGHT)]

# What each key does and its range (docs/features.md's table says the same).
DOC = {
    "version": "format version. A higher number is refused (THEME TOO NEW); 1 today",
    "name": "a label, not shown: THEME lists the file's own name",
    "base": "dark, outdoor or light: the built-in the keys left out are taken from",
    "overview": "the overview's layout: classic (TODAY card over four fields) or fields (a 2 x 2 table). Range classic or fields",
    "zone_bar": "height of a field's zone bar, px. Range 4 to 8",
    "frame": "frame round a cell in the page editor, px. Range 1 to 4",
    "menu_row": "menu row pitch, px; taller rows show fewer rows. Range 44 to 56",
    "background": "every screen's background. #RRGGBB",
    "text": "labels and values. #RRGGBB, at least 4.5:1 on background",
    "line": "menu row lines, empty bars. #RRGGBB, at least 3:1 on background",
    "dim": "notes and axes. #RRGGBB, at least 4.5:1 on background",
    "accent": "the selected row, banners, frames. #RRGGBB",
    "on_accent": "text on the accent. #RRGGBB, at least 4.5:1 on accent",
    "alarm": "alarm banners, BACK, bad readings. #RRGGBB, at least 4.5:1 on background; the only red, no other key may equal it",
    "on_alarm": "text on the alarm. #RRGGBB, at least 4.5:1 on alarm",
    "ok": "the strip's good dots. #RRGGBB, at least 4.5:1 on background",
    "warn": "the trainer's states. #RRGGBB, at least 4.5:1 on background",
    "ferris": "Ferris. #RRGGBB, at least 3:1 on background",
    "cog": "the overview's menu cog. #RRGGBB, at least 3:1 on background",
    "course": "the course on the map. #RRGGBB, at least 3:1 on background",
    "zone1": "power and heart rate zone 1 (grade, gauge, W' balance too). Zones 1 to 7 must differ",
    "zone2": "power and heart rate zone 2. #RRGGBB, at least 3:1 on background",
    "zone3": "power and heart rate zone 3. #RRGGBB, at least 3:1 on background",
    "zone4": "power and heart rate zone 4. #RRGGBB, at least 3:1 on background",
    "zone5": "power and heart rate zone 5. #RRGGBB, at least 3:1 on background",
    "zone6": "power and heart rate zone 6. #RRGGBB, at least 3:1 on background",
    "zone7": "power and heart rate zone 7. #RRGGBB, at least 3:1 on background",
}


def expand(c):
    """The 8-bit channels of an RGB565 colour, rounded (the inverse of `rgb`, to the nearest level)."""
    r, g, b = c >> 11, (c >> 5) & 63, c & 31
    return ((r * 255 * 2 + 31) // 62, (g * 255 * 2 + 63) // 126, (b * 255 * 2 + 31) // 62)


def hex_of(c):
    return "#%02X%02X%02X" % expand(c)


def to565(text):
    """A `#RRGGBB` value as RGB565, the firmware's way."""
    v = text[1:]
    if len(text) != 7 or text[0] != "#" or any(ch not in "0123456789abcdefABCDEF" for ch in v):
        raise ValueError(text)
    n = int(v, 16)
    return rgb(n >> 16, (n >> 8) & 255, n & 255)


def render(name, base, ink):
    out = [
        "; Rustfw colour theme %s: the built-in %s written out, every key set (docs/theming.md)." % (name, name),
        "; Copy it to \\Rustfw\\Themes\\ and edit the copy: a key left out keeps the base's value.",
        "; The file is refused (the reason shows in a notice) when a contrast or the red rule fails.",
        "[theme]",
    ]
    for key, value in [("version", "1"), ("name", name), ("base", base), ("overview", "classic")]:
        out += ["; " + DOC[key], "%s = %s" % (key, value)]
    out.append("[metrics]")
    for key, v, lo, hi in METRICS:
        out += ["; " + DOC[key], "%s = %d" % (key, v)]
    out.append("[colors]")
    for key, c in zip(NAMES, ink):
        out += ["; " + DOC[key], "%s = %s" % (key, hex_of(c))]
    return "\n".join(out) + "\n"


def examples():
    return {name + ".ini": render(name, base, ink) for name, base, ink in BUILTIN}


def write(out_dir, files):
    os.makedirs(out_dir, exist_ok=True)
    for fname, text in files.items():
        with open(os.path.join(out_dir, fname), "w", encoding="ascii", newline="\n") as f:
            f.write(text)
        print(os.path.join(out_dir, fname))


def parse_ini(text):
    """The sections and keys of a file, as the firmware reads them: `;` lines and text after `;` are comments."""
    sec, keys = None, {}
    for line in text.split("\n"):
        line = line.strip()
        if not line or line.startswith(";"):
            continue
        if line.startswith("["):
            sec = line.strip("[]").strip().lower()
            continue
        key, _, value = line.partition("=")
        keys.setdefault(sec, []).append((key.strip(), value.split(";")[0].strip()))
    return keys


def selftest():
    files = examples()
    for fname, text in files.items():
        assert len(text.encode("ascii")) < 4096, fname + ": over the 4 KB a theme file may be"
        assert all(ch.isascii() for ch in text) and "\u2013" not in text and "\u2014" not in text, fname
        lines = text.split("\n")
        for i, line in enumerate(lines):
            if line.startswith(";"):
                assert "=" not in line, "%s: a comment with '=' would read as a key: %r" % (fname, line)
            elif "=" in line:
                assert lines[i - 1].startswith(";"), "%s: the key %r has no comment above it" % (fname, line)
        ini = parse_ini(text)
        assert sorted(ini) == ["colors", "metrics", "theme"], fname
        listed = [k for sec in ini.values() for k, _ in sec]
        want = ["version", "name", "base", "overview"] + [k for k, *_ in METRICS] + NAMES
        assert sorted(listed) == sorted(want), "%s: keys %s" % (fname, sorted(set(want) ^ set(listed)))
    for name, base, ink in BUILTIN:
        for key, value in parse_ini(files[name + ".ini"])["colors"]:
            c = to565(value)
            assert c == ink[NAMES.index(key)], "%s %s: %s reads back as %#06x" % (name, key, value, c)
        assert all(to565(hex_of(c)) == c for c in ink), name + ": a colour does not round-trip"
    fixture_dir = FIXTURE
    for fname, text in files.items():
        path = os.path.join(fixture_dir, fname)
        if not os.path.isfile(path):
            raise SystemExit("missing %s: python3 -I tools/theme.py examples test-data/themes" % path)
        with open(path, encoding="ascii") as f:
            if f.read() != text:
                raise SystemExit("%s differs from tools/theme.py's output: regenerate test-data/themes" % path)
    print("theme.py selftest: %d examples, every key once, the fixture equal to a fresh build" % len(files))


def preview(path, out=None):
    path = os.path.abspath(path)
    if not os.path.isfile(path):
        raise SystemExit("no such theme file: %s" % path)
    env = dict(os.environ, THEME_FILE=path)
    r = subprocess.run(["cargo", "test-host", "--", "--ignored", "theme_preview"], cwd=ROOT, env=env)
    if r.returncode:
        raise SystemExit(r.returncode)
    shots = [os.path.join(ROOT, "target", "sim", "theme_preview_%s.bmp" % n) for n in SHOTS]
    if out is None:
        print("\n".join(shots))
        return
    try:
        from PIL import Image
    except ImportError:
        print("PIL is not installed: the BMPs instead\n" + "\n".join(shots))
        return
    images = [Image.open(s).convert("RGB") for s in shots]
    canvas = Image.new("RGB", (sum(i.width for i in images), max(i.height for i in images)))
    x = 0
    for im in images:
        canvas.paste(im, (x, 0))
        x += im.width
    canvas.save(out)
    print(os.path.abspath(out))


def main(argv):
    p = argparse.ArgumentParser(prog="theme.py", description="colour themes: example files, preview, selftest")
    sub = p.add_subparsers(dest="cmd", required=True)
    e = sub.add_parser("examples", help="write the built-in themes as example files")
    e.add_argument("out_dir")
    e.add_argument("--drive", action="store_true", help="allow a mounted drive (/Volumes)")
    v = sub.add_parser("preview", help="draw a theme file through the simulator")
    v.add_argument("file")
    v.add_argument("--out", metavar="PNG", help="one PNG of the three shots (needs PIL)")
    sub.add_parser("selftest", help="check the examples and the fixture")
    a = p.parse_args(argv)
    if a.cmd == "examples":
        # realpath first: a symlink into /Volumes is still a mounted drive
        out = os.path.realpath(a.out_dir)
        if (out == "/Volumes" or out.startswith("/Volumes/")) and not a.drive:
            p.error("refusing a mounted drive: write a folder and copy it, or pass --drive")
        write(a.out_dir, examples())
    elif a.cmd == "preview":
        preview(a.file, a.out)
    else:
        selftest()


if __name__ == "__main__":
    main(sys.argv[1:])
