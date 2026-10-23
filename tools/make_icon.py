"""Draws Meshvault's app icon: the Phosphor cube (the title bar's logo) on an accent square.
usage: make_icon.py <Phosphor.ttf> <out dir> [preview.png]"""
import sys
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

font_path, out = sys.argv[1], Path(sys.argv[2])
CUBE = ""
TOP, BOTTOM = (0x8A, 0x98, 0xFF), (0x4F, 0x5F, 0xE8)   # dark and light theme accents

def draw(size):
    s = 1024  # draw large, then scale down
    im = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    grad = Image.new("RGBA", (s, s))
    gd = ImageDraw.Draw(grad)
    for y in range(s):
        t = y / (s - 1)
        gd.line([(0, y), (s, y)], fill=tuple(round(a + (b - a) * t) for a, b in zip(TOP, BOTTOM)) + (255,))
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle([40, 40, s - 41, s - 41], radius=220, fill=255)
    im.paste(grad, (0, 0), mask)
    # thicker strokes at small sizes so the cube still reads at 16 pixels
    stroke = {16: 26, 24: 20, 32: 16, 48: 12}.get(size, 8)
    font = ImageFont.truetype(font_path, 700)
    d = ImageDraw.Draw(im)
    d.text((s / 2, s / 2), CUBE, font=font, anchor="mm", fill="white", stroke_width=stroke, stroke_fill="white")
    return im.resize((size, size), Image.LANCZOS)

sizes = [16, 24, 32, 48, 64, 128, 256]
images = [draw(n) for n in sizes]
out.mkdir(parents=True, exist_ok=True)
images[-1].save(out / "meshvault.ico", sizes=[(n, n) for n in sizes], append_images=images[:-1])
images[-1].save(out / "meshvault.png", optimize=True)
if len(sys.argv) > 3:
    sheet = Image.new("RGBA", (sum(sizes) + 10 * len(sizes), 256), (30, 30, 36, 255))
    x = 0
    for im in images:
        sheet.paste(im, (x, 256 - im.height), im); x += im.width + 10
    sheet.save(sys.argv[3])
