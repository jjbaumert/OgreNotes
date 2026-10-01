#!/usr/bin/env python3
"""Regenerate Mermaid fitting advances from the Arial-compatible Liberation Sans.

Requires Pillow and fontconfig's fc-query. Fonts remain on the host; only
measured advance bounds are written into the Rust renderer.
"""
import argparse
import math
import subprocess
from pathlib import Path

from PIL import ImageFont


def coverage(font_path):
    charset = subprocess.check_output(
        ["fc-query", "--format", "%{charset}", str(font_path)], text=True
    )
    result = set()
    for value in charset.split():
        endpoints = value.split("-")
        result.update(range(int(endpoints[0], 16), int(endpoints[-1], 16) + 1))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--font-dir", type=Path,
        default=Path("/usr/share/fonts/truetype/liberation"),
    )
    parser.add_argument(
        "--out", type=Path,
        default=Path(__file__).resolve().parents[1] / "crates/mermaid/src/font_advances.rs",
    )
    args = parser.parse_args()
    paths = [args.font_dir / f"LiberationSans-{weight}.ttf"
             for weight in ("Regular", "Bold", "Italic")]
    fonts = [ImageFont.truetype(str(path), 4096) for path in paths]
    common = set.intersection(*(coverage(path) for path in paths))
    values = []
    for code in sorted(common):
        if code < 128:
            continue
        widths = [font.getlength(chr(code)) / 4096 for font in fonts]
        # The renderer applies a 1.04 multiplier to bold text.
        units = math.ceil(max(widths[0], widths[1] / 1.04, widths[2]) * 1024)
        if units >= 2048:
            raise ValueError(f"Advance exceeds packed storage: U+{code:04X}: {units}")
        values.append((code << 11) | units)
    output = (
        "// Advance bounds measured from Liberation Sans Regular, Bold and Italic.\n"
        "// Fonts are not bundled. Regenerate with scripts/measure-mermaid-fonts.py.\n"
        "// Packed Unicode scalar (21 bits) and relative advance in 1/1024 em (11 bits).\n"
        "const FONT_ADVANCES: &[u32] = &[\n"
    )
    for offset in range(0, len(values), 8):
        output += "    " + ", ".join(f"0x{x:08x}" for x in values[offset:offset + 8]) + ",\n"
    args.out.write_text(output + "];\n")
    print(f"Measured advances for {len(values)} supported non-ASCII scalars")


if __name__ == "__main__":
    main()
