"""Build the Press app icons from the source mark.

One mark, three tiles. Green is the default; ink and sheet are the alternates a
user can switch to. Everything below is derived from src/app.css: the tile
colours are --accent / --ink / --card, the glyph colours are --on-accent and the
two --ink values, and nothing here invents a hue that is not already in the
stylesheet.

The tile is a superellipse (|x|^n + |y|^n = 1, n = 5), not a rounded rectangle.
macOS corners are continuous-curvature and the difference is plainly visible at
1024. It sits on the Big Sur icon grid: an 824px body centred in a 1024px
canvas, the 100px margin left for the baked drop shadow.

Depth is five layers, in order: vertical gradient, off-canvas specular sheen,
1px bevel (light along the top edge, dark along the bottom), a hairline rim to
seat the tile against a light desktop, and a shadow cast by the glyph onto the
tile. Each is applied to the superellipse mask itself rather than to a
bounding box, so the bevel follows the corners.

Everything is rendered at 4x and downsampled once, which is cheaper than
antialiasing each layer and gives cleaner edges on the curve.

Run: python3 scripts/make-icons.py
"""

import struct
from pathlib import Path

import numpy as np
from PIL import Image, ImageFilter

import cairosvg

ROOT = Path(__file__).resolve().parent.parent
ICONS = ROOT / "src-tauri" / "icons"
VARIANTS = ICONS / "variants"
PREVIEWS = ROOT / "static" / "icons"
MARK = ROOT / "static" / "press.svg"
LOOP = ROOT / "static" / "press-loop.svg"  # the top form alone, for 16px

SS = 4  # supersample factor
CANVAS = 1024
BODY = 824  # macOS icon grid: 824 body in a 1024 canvas
GLYPH_H = 0.60  # glyph height as a fraction of the body
EXPONENT = 5.0  # superellipse exponent

# The mark is 1:2.3, so it thins out to a smudge if the small sizes are simply
# downsampled from the master. Three tiers instead of one:
#
#   > 32   the full mark, with the cast shadow and the sheen.
#   32     the full mark set taller, no cast shadow, sheen halved. Still reads:
#          the loop and three sheets survive.
#   16     the loop alone. At 16 the three sheets merge into one grey block no
#          matter how they are scaled, and a mark that is 1:2.3 leaves most of
#          the tile empty to fit. The loop is 1:1.27, so it fills the tile and
#          stays a shape you can name. Dropping detail is the only thing that
#          works at this size — every icon set does it, and the generators that
#          claim otherwise have not looked at their own 16.
SMALL_CUTOFF = 32
SMALL_GLYPH_H = 0.72
MICRO_CUTOFF = 16
MICRO_GLYPH_H = 0.62

MARK_ASPECT = 111.145 / 256.582
LOOP_ASPECT = 110.90 / 140.70


def hexrgb(h):
    h = h.lstrip("#")
    return np.array([int(h[i : i + 2], 16) for i in (0, 2, 4)], dtype=np.float64)


# Gradients are deliberately shallow. A wide top-to-bottom ramp is what makes an
# icon read as 2012, and it also fights the flat surfaces this app is built from
# — the depth should be legible without being the first thing you notice.
VARIANTS_SPEC = {
    # --accent #137e63, glyph --on-accent. The default.
    "green": {
        "stops": [(0.00, "#16906f"), (0.52, "#137e63"), (1.00, "#10715a")],
        "glyph": "#ffffff",
        "bevel_top": 0.26,
        "bevel_bottom": 0.24,
        "rim": 0.14,
        "sheen": 0.22,
        "glyph_shadow": (7, 12, 0.26),
    },
    # --ink #20201e with the dark theme's --ink #d4cdc0 on it.
    "ink": {
        "stops": [(0.00, "#292926"), (0.52, "#20201e"), (1.00, "#171715")],
        "glyph": "#d4cdc0",
        "bevel_top": 0.15,
        "bevel_bottom": 0.42,
        "rim": 0.30,
        "sheen": 0.11,
        "glyph_shadow": (7, 12, 0.34),
    },
    # --card #ffffff falling to --desk, with --ink on it. The paper cutout.
    "sheet": {
        "stops": [(0.00, "#ffffff"), (0.55, "#fcfcfa"), (1.00, "#f0efea")],
        "glyph": "#20201e",
        "bevel_top": 0.85,
        "bevel_bottom": 0.06,
        "rim": 0.11,
        "sheen": 0.40,
        "glyph_shadow": (5, 9, 0.16),
    },
}


def resize(img, size):
    """Downsample in premultiplied alpha.

    Pillow resamples the four channels independently. Outside the tile the RGB
    channels still hold the tile's colour at alpha 0, and Lanczos overshoot
    lifts that alpha just above zero past the edge — which paints a halo in the
    tile's own colour, obvious against a dark desktop. Premultiplying first
    means the transparent margin carries no colour to bleed.
    """
    if img.size == (size, size):
        return img
    a = np.asarray(img).astype(np.float64)
    alpha = a[..., 3:4] / 255.0
    a[..., :3] *= alpha
    pre = Image.fromarray(a.astype(np.uint8), "RGBA").resize(
        (size, size), Image.LANCZOS
    )
    b = np.asarray(pre).astype(np.float64)
    alpha = b[..., 3:4] / 255.0
    np.divide(b[..., :3], alpha, out=b[..., :3], where=alpha > 0)
    b[..., :3] = np.clip(b[..., :3], 0, 255)
    return Image.fromarray(b.astype(np.uint8), "RGBA")


def superellipse_mask(size, n=EXPONENT):
    """Alpha mask for a superellipse filling a size x size square."""
    t = (np.arange(size) + 0.5) / size * 2.0 - 1.0
    x = np.abs(t)[None, :] ** n
    y = np.abs(t)[:, None] ** n
    return ((x + y) <= 1.0).astype(np.float64)


def vertical_gradient(size, stops):
    """size x size x 3 array interpolating the stops top to bottom."""
    pos = np.array([s[0] for s in stops])
    cols = np.stack([hexrgb(s[1]) for s in stops])
    t = (np.arange(size) + 0.5) / size
    out = np.empty((size, 3))
    for c in range(3):
        out[:, c] = np.interp(t, pos, cols[:, c])
    return np.repeat(out[:, None, :], size, axis=1)


def sheen_field(size, strength):
    """Off-canvas specular: a soft ellipse centred above the top-left corner."""
    yy, xx = np.mgrid[0:size, 0:size].astype(np.float64)
    xx = (xx / size - 0.28) / 0.60
    yy = (yy / size + 0.12) / 0.39
    d = np.sqrt(xx**2 + yy**2)
    return np.clip(1.0 - d, 0.0, 1.0) ** 1.6 * strength


def edge_band(mask, dy):
    """The band of `mask` left uncovered when the mask is shifted by dy.

    Shifting down and subtracting isolates the top edge; shifting up isolates
    the bottom. Because it operates on the mask, the band follows the corner
    curve instead of stopping where a rectangle would.
    """
    shifted = np.zeros_like(mask)
    if dy > 0:
        shifted[dy:, :] = mask[:-dy, :]
    else:
        shifted[:dy, :] = mask[-dy:, :]
    return np.clip(mask - shifted, 0.0, 1.0)


def render_mark(height, rgb, loop=False):
    """Rasterise the mark SVG at the given height, recoloured, as RGBA."""
    src, aspect = (LOOP, LOOP_ASPECT) if loop else (MARK, MARK_ASPECT)
    width = max(1, int(round(height * aspect)))
    png = cairosvg.svg2png(
        url=str(src), output_width=width, output_height=height, background_color=None
    )
    import io

    glyph = Image.open(io.BytesIO(png)).convert("RGBA")
    a = np.array(glyph)
    a[..., 0], a[..., 1], a[..., 2] = rgb
    return Image.fromarray(a)


def build(spec, tier="master"):
    small = tier != "master"
    S = BODY * SS
    mask = superellipse_mask(S)

    sheen = spec["sheen"] * (0.5 if small else 1.0)
    rgb = vertical_gradient(S, spec["stops"])
    rgb = np.clip(rgb + sheen_field(S, sheen)[..., None] * 255.0, 0, 255)

    t = max(1, int(round(1.5 * SS)))
    top = edge_band(mask, t)
    bottom = edge_band(mask, -t)
    rim = np.clip(mask - _erode(mask, max(1, int(round(1.0 * SS)))), 0.0, 1.0)

    rgb = rgb + (255.0 - rgb) * (top * spec["bevel_top"])[..., None]
    rgb = rgb * (1.0 - (bottom * spec["bevel_bottom"])[..., None])
    rgb = rgb * (1.0 - (rim * spec["rim"])[..., None])

    tile = np.dstack([np.clip(rgb, 0, 255), mask * 255.0]).astype(np.uint8)
    tile = resize(Image.fromarray(tile, "RGBA"), BODY)

    ratio = {"master": GLYPH_H, "small": SMALL_GLYPH_H, "micro": MICRO_GLYPH_H}[tier]
    glyph_h = int(round(BODY * ratio))
    glyph = render_mark(glyph_h, hexrgb(spec["glyph"]).astype(int), loop=tier == "micro")
    gx = (BODY - glyph.width) // 2
    gy = (BODY - glyph.height) // 2

    if not small:
        dy, blur, alpha = spec["glyph_shadow"]
        shadow = Image.new("RGBA", (BODY, BODY), (0, 0, 0, 0))
        sh = Image.new("RGBA", glyph.size, (0, 0, 0, 0))
        sh.putalpha(glyph.getchannel("A"))
        shadow.paste(sh, (gx, gy + dy), sh)
        shadow = shadow.filter(ImageFilter.GaussianBlur(blur))
        shadow.putalpha(shadow.getchannel("A").point(lambda v: int(v * alpha)))
        shadow = Image.composite(
            shadow, Image.new("RGBA", (BODY, BODY), (0, 0, 0, 0)), tile.getchannel("A")
        )
        tile = Image.alpha_composite(tile, shadow)

    tile.paste(glyph, (gx, gy), glyph)

    canvas = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    off = (CANVAS - BODY) // 2

    for oy, blur_r, a in ((20, 30, 0.18), (4, 8, 0.12)):
        drop = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
        body_alpha = Image.new("RGBA", (BODY, BODY), (0, 0, 0, 255))
        body_alpha.putalpha(tile.getchannel("A"))
        drop.paste(body_alpha, (off, off + oy), body_alpha)
        drop = drop.filter(ImageFilter.GaussianBlur(blur_r))
        drop.putalpha(drop.getchannel("A").point(lambda v: int(v * a)))
        canvas = Image.alpha_composite(canvas, drop)

    canvas.paste(tile, (off, off), tile)
    return canvas


def _erode(mask, k):
    m = Image.fromarray((mask * 255).astype(np.uint8))
    m = m.filter(ImageFilter.MinFilter(2 * k + 1))
    return np.array(m).astype(np.float64) / 255.0


ICNS_TYPES = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic07", 128),
    (b"ic13", 256),
    (b"ic08", 256),
    (b"ic14", 512),
    (b"ic09", 512),
    (b"ic10", 1024),
]


def write_icns(tiers, path):
    """Assemble an .icns by hand: header, then one PNG chunk per size."""
    import io

    chunks = b""
    for tag, size in ICNS_TYPES:
        buf = io.BytesIO()
        at(tiers, size).save(buf, "PNG")
        data = buf.getvalue()
        chunks += tag + struct.pack(">I", len(data) + 8) + data
    path.write_bytes(b"icns" + struct.pack(">I", len(chunks) + 8) + chunks)


def at(tiers, size):
    """The icon at `size`, taken from whichever build suits that scale."""
    if size <= MICRO_CUTOFF:
        return resize(tiers["micro"], size)
    if size <= SMALL_CUTOFF:
        return resize(tiers["small"], size)
    return resize(tiers["master"], size)


def main():
    VARIANTS.mkdir(parents=True, exist_ok=True)
    PREVIEWS.mkdir(parents=True, exist_ok=True)
    built = {}

    for name, spec in VARIANTS_SPEC.items():
        tiers = {t: build(spec, t) for t in ("master", "small", "micro")}
        built[name] = tiers
        at(tiers, 1024).save(VARIANTS / f"press-{name}-1024.png")
        write_icns(tiers, VARIANTS / f"press-{name}.icns")
        for s in (256, 512):
            at(tiers, s).save(VARIANTS / f"press-{name}-{s}.png")
        # The same tile again where the interface can load it. Settings shows
        # the three to choose between, and a webview cannot read an .icns.
        at(tiers, 256).save(PREVIEWS / f"press-{name}.png")
        print(f"variant {name}")

    default = built["green"]

    for fname, size in (
        ("32x32.png", 32),
        ("64x64.png", 64),
        ("128x128.png", 128),
        ("128x128@2x.png", 256),
        ("icon.png", 512),
        ("Square30x30Logo.png", 30),
        ("Square44x44Logo.png", 44),
        ("Square71x71Logo.png", 71),
        ("Square89x89Logo.png", 89),
        ("Square107x107Logo.png", 107),
        ("Square142x142Logo.png", 142),
        ("Square150x150Logo.png", 150),
        ("Square284x284Logo.png", 284),
        ("Square310x310Logo.png", 310),
        ("StoreLogo.png", 50),
    ):
        at(default, size).save(ICONS / fname)

    write_icns(default, ICONS / "icon.icns")

    # Pillow builds an .ico by downsampling one image, which would lose the
    # small-size build, so the frames are handed over explicitly.
    ico_sizes = [16, 32, 48, 64, 128, 256]
    frames = [at(default, s) for s in ico_sizes]
    frames[-1].save(
        ICONS / "icon.ico",
        format="ICO",
        sizes=[(s, s) for s in ico_sizes],
        append_images=frames[:-1],
    )
    print("default set written from green")


if __name__ == "__main__":
    main()
