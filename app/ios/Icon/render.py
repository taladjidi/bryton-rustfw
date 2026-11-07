#!/usr/bin/env python3
"""Render icon.svg into the AppIcon asset: light, dark and tinted 1024 px PNGs (opaque, no rounded corners).

Needs rsvg-convert and ImageMagick (magick). The art is Ferris the crab from https://rustacean.net
(ferris-rustacean.svg, public domain, CC0 1.0: https://creativecommons.org/publicdomain/zero/1.0/),
riding a bike drawn here. Edit icon.svg, then run this script.
"""
import pathlib
import re
import subprocess
import sys

here = pathlib.Path(__file__).resolve().parent
out = here.parent / "App/Assets.xcassets/AppIcon.appiconset"
svg = (here / "icon.svg").read_text()


def render(text, name, extra=()):
    tmp = out / (name + ".svg")
    tmp.write_text(text)
    png = out / name
    subprocess.run(["rsvg-convert", "-w", "1024", "-h", "1024", str(tmp), "-o", str(png)], check=True)
    tmp.unlink()
    # flatten: opaque, no alpha channel
    subprocess.run(["magick", str(png), *extra, "-background", "black", "-alpha", "remove", "-alpha", "off", str(png)],
                   check=True)


dark = re.sub(r'(id="bg1"[^>]*stop-color=")[^"]*', r"\g<1>#0f3b45", svg)
dark = re.sub(r'(id="bg2"[^>]*stop-color=")[^"]*', r"\g<1>#04161c", dark)
render(svg, "icon.png")
render(dark, "icon-dark.png")
render(dark, "icon-tinted.png", ["-colorspace", "Gray"])
if not all((out / n).stat().st_size > 1000 for n in ("icon.png", "icon-dark.png", "icon-tinted.png")):
    sys.exit("render failed")
