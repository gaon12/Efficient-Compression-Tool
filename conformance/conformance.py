#!/usr/bin/env python3
"""Cross-implementation robustness and losslessness test for ECT.

Generates a deterministic set of valid, unusual and corrupt PNG, JPEG,
WebP, gzip and ZIP inputs, runs every given `ect` binary on each of them
(in a fresh directory, with a timeout) and checks:

- the process neither crashed (signal, sanitizer report, Go/Rust panic)
  nor hung,
- every optimized file still decodes to exactly the same content (pixels,
  decompressed bytes, ZIP entries) and did not grow,
- corrupt inputs were left byte-for-byte untouched,
- no stray temporary files were left behind.

Usage:

    python3 conformance.py --impl cpp=./ect --impl rust=./ect-rs --out findings.json
    python3 group.py findings.jsonl     # group identical findings

See README.md in this directory.

Needs Pillow (for JPEG/WebP generation and decoding). PNG files are
checked with the strict decoder below, which also covers 16-bit samples,
low bit depths, palettes, tRNS and Adam7 interlacing.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import gzip
import io
import json
import os
import random
import re
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
import zipfile
import zlib
from dataclasses import dataclass, field
from pathlib import Path

from PIL import Image

# ---------------------------------------------------------------------------
# PNG encoder / decoder
# ---------------------------------------------------------------------------

CHANNELS = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}
ADAM7 = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]


def png_chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


def pack_row(samples: list[int], bitdepth: int) -> bytes:
    if bitdepth == 8:
        return bytes(samples)
    if bitdepth == 16:
        return b"".join(struct.pack(">H", s) for s in samples)
    out = bytearray()
    per = 8 // bitdepth
    for i in range(0, len(samples), per):
        byte = 0
        group = samples[i : i + per]
        for j, s in enumerate(group):
            byte |= s << (8 - bitdepth * (j + 1))
        out.append(byte)
    return bytes(out)


def make_png(
    pixels: list[list[tuple[int, ...]]],
    colortype: int,
    bitdepth: int,
    *,
    interlace: bool = False,
    palette: list[tuple[int, int, int]] | None = None,
    trns: bytes | None = None,
    extra: list[tuple[bytes, bytes]] | None = None,
    after_idat: list[tuple[bytes, bytes]] | None = None,
    zlevel: int = 0,
    idat_split: int = 0,
    filter_type: int = 0,
) -> bytes:
    """Writes a PNG; `pixels[y][x]` is a tuple of raw samples."""
    h = len(pixels)
    w = len(pixels[0])
    ch = CHANNELS[colortype]
    bpp = max(1, ch * bitdepth // 8)

    def encode_image(rows: list[list[tuple[int, ...]]]) -> bytes:
        out = bytearray()
        prev = None
        for row in rows:
            raw = pack_row([s for px in row for s in px], bitdepth)
            if filter_type == 1:  # Sub
                f = bytes((raw[i] - (raw[i - bpp] if i >= bpp else 0)) & 255 for i in range(len(raw)))
            elif filter_type == 2 and prev is not None:  # Up
                f = bytes((raw[i] - prev[i]) & 255 for i in range(len(raw)))
            else:
                f = raw
            out += bytes((filter_type if (filter_type != 2 or prev is not None) else 0,)) + f
            prev = raw
        return bytes(out)

    if interlace:
        data = bytearray()
        for x0, y0, dx, dy in ADAM7:
            rows = [[pixels[y][x] for x in range(x0, w, dx)] for y in range(y0, h, dy)]
            rows = [r for r in rows if r]
            if rows:
                data += encode_image(rows)
        raw = bytes(data)
    else:
        raw = encode_image(pixels)
    comp = zlib.compress(raw, zlevel)
    out = b"\x89PNG\r\n\x1a\n"
    out += png_chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, bitdepth, colortype, 0, 0, int(interlace)))
    for kind, data in extra or []:
        out += png_chunk(kind, data)
    if palette is not None:
        out += png_chunk(b"PLTE", b"".join(bytes(c) for c in palette))
    if trns is not None:
        out += png_chunk(b"tRNS", trns)
    if idat_split:
        for i in range(0, len(comp), idat_split):
            out += png_chunk(b"IDAT", comp[i : i + idat_split])
    else:
        out += png_chunk(b"IDAT", comp)
    for kind, data in after_idat or []:
        out += png_chunk(kind, data)
    out += png_chunk(b"IEND", b"")
    return out


class PNGError(Exception):
    pass


def png_chunks(data: bytes) -> list[tuple[bytes, bytes]]:
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise PNGError("bad signature")
    pos = 8
    chunks = []
    while pos < len(data):
        if pos + 12 > len(data):
            raise PNGError("truncated chunk")
        (n,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + n]
        if len(body) != n or pos + 12 + n > len(data):
            raise PNGError("truncated chunk body")
        (crc,) = struct.unpack(">I", data[pos + 8 + n : pos + 12 + n])
        if zlib.crc32(kind + body) != crc:
            raise PNGError(f"CRC error in {kind!r}")
        chunks.append((kind, body))
        pos += 12 + n
        if kind == b"IEND":
            break
    if not chunks or chunks[-1][0] != b"IEND":
        raise PNGError("missing IEND")
    return chunks


def decode_png(data: bytes) -> tuple[int, int, list[tuple[int, int, int, int]]]:
    """Decodes a PNG to (w, h, RGBA16 pixels)."""
    chunks = png_chunks(data)
    ihdr = chunks[0]
    if ihdr[0] != b"IHDR":
        raise PNGError("IHDR not first")
    w, h, bd, ct, cm, fm, il = struct.unpack(">IIBBBBB", ihdr[1])
    if ct not in CHANNELS or cm or fm or il > 1 or not 0 < w < 2**31 or not 0 < h < 2**31:
        raise PNGError("bad IHDR")
    plte = b"".join(b for k, b in chunks if k == b"PLTE")
    trns = b"".join(b for k, b in chunks if k == b"tRNS")
    idat = b"".join(b for k, b in chunks if k == b"IDAT")
    raw = zlib.decompress(idat)
    ch = CHANNELS[ct]
    bpp = max(1, ch * bd // 8)
    maxv = (1 << bd) - 1

    def unfilter(buf: bytes, pos: int, pw: int, ph: int) -> tuple[list[bytes], int]:
        stride = (pw * ch * bd + 7) // 8
        rows = []
        prev = bytearray(stride)
        for _ in range(ph):
            if pos + 1 + stride > len(buf):
                raise PNGError("not enough image data")
            ft = buf[pos]
            line = bytearray(buf[pos + 1 : pos + 1 + stride])
            pos += 1 + stride
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                b = prev[i]
                c = prev[i - bpp] if i >= bpp else 0
                if ft == 0:
                    pass
                elif ft == 1:
                    line[i] = (line[i] + a) & 255
                elif ft == 2:
                    line[i] = (line[i] + b) & 255
                elif ft == 3:
                    line[i] = (line[i] + (a + b) // 2) & 255
                elif ft == 4:
                    p = a + b - c
                    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                    pr = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                    line[i] = (line[i] + pr) & 255
                else:
                    raise PNGError(f"bad filter {ft}")
            rows.append(bytes(line))
            prev = line
        return rows, pos

    def samples(row: bytes, n: int) -> list[int]:
        if bd == 8:
            return list(row[:n])
        if bd == 16:
            return [struct.unpack(">H", row[2 * i : 2 * i + 2])[0] for i in range(n)]
        out = []
        for i in range(n):
            bit = i * bd
            out.append((row[bit // 8] >> (8 - bd - bit % 8)) & maxv)
        return out

    def to_rgba(s: tuple[int, ...]) -> tuple[int, int, int, int]:
        def scale(v: int) -> int:
            return v * 65535 // maxv

        if ct == 3:
            i = s[0]
            if 3 * i + 3 > len(plte):
                raise PNGError("palette index out of range")
            r, g, b = plte[3 * i : 3 * i + 3]
            a = trns[i] if i < len(trns) else 255
            return (r * 257, g * 257, b * 257, a * 257)
        if ct == 0:
            a = 0 if len(trns) >= 2 and struct.unpack(">H", trns[:2])[0] == s[0] else maxv
            return (scale(s[0]),) * 3 + (scale(a),)
        if ct == 2:
            key = struct.unpack(">HHH", trns[:6]) if len(trns) >= 6 else None
            a = 0 if key == tuple(s) else maxv
            return (scale(s[0]), scale(s[1]), scale(s[2]), scale(a))
        if ct == 4:
            return (scale(s[0]),) * 3 + (scale(s[1]),)
        return tuple(scale(v) for v in s)  # type: ignore[return-value]

    img: list[tuple[int, int, int, int] | None] = [None] * (w * h)
    pos = 0
    passes = ADAM7 if il else [(0, 0, 1, 1)]
    for x0, y0, dx, dy in passes:
        pw = len(range(x0, w, dx))
        ph = len(range(y0, h, dy))
        if pw == 0 or ph == 0:
            continue
        rows, pos = unfilter(raw, pos, pw, ph)
        for j, row in enumerate(rows):
            smp = samples(row, pw * ch)
            for i in range(pw):
                img[(y0 + j * dy) * w + x0 + i * dx] = to_rgba(tuple(smp[i * ch : (i + 1) * ch]))
    if pos != len(raw):
        # Extra data after the image is tolerated by decoders; report it.
        pass
    return w, h, img  # type: ignore[return-value]


def same_pixels(a, b) -> str | None:
    if a[0] != b[0] or a[1] != b[1]:
        return f"size {a[0]}x{a[1]} -> {b[0]}x{b[1]}"
    for i, (p, q) in enumerate(zip(a[2], b[2])):
        if p == q or (p[3] == 0 and q[3] == 0):
            continue
        return f"pixel {i % a[0]},{i // a[0]}: {p} -> {q}"
    return None


# ---------------------------------------------------------------------------
# Test input generation
# ---------------------------------------------------------------------------

RNG = random.Random(20261105)


def gradient(w: int, h: int, ch: int, maxv: int, noise: int = 0) -> list[list[tuple[int, ...]]]:
    rows = []
    for y in range(h):
        row = []
        for x in range(w):
            px = []
            for c in range(ch):
                v = ((x * (c + 1) * 7 + y * (3 - c % 3) * 5) * maxv) // max(1, (w + h) * 8)
                if noise:
                    v += RNG.randint(-noise, noise)
                px.append(max(0, min(maxv, v)))
            row.append(tuple(px))
        rows.append(row)
    return rows


def alpha_mix(rows, maxv):
    """Makes some pixels fully transparent with random (hidden) colors."""
    out = []
    for y, row in enumerate(rows):
        r = []
        for x, px in enumerate(row):
            if (x + y) % 5 == 0:
                px = tuple(RNG.randint(0, maxv) for _ in px[:-1]) + (0,)
            r.append(px)
        out.append(r)
    return out


@dataclass
class Case:
    name: str
    kind: str  # png, jpeg, webp, gz, zip, raw (for -gzip/-zip), corrupt, fs
    files: dict[str, bytes]
    args: list[str] = field(default_factory=list)
    levels: tuple[int, ...] = (1, 3, 9)
    cpp_only: bool = False
    setup: str = ""  # readonly, symlink


def png_cases() -> list[Case]:
    cases = []
    specs = [
        ("gray1", 0, 1), ("gray2", 0, 2), ("gray4", 0, 4), ("gray8", 0, 8), ("gray16", 0, 16),
        ("rgb8", 2, 8), ("rgb16", 2, 16), ("ga8", 4, 8), ("ga16", 4, 16), ("rgba8", 6, 8), ("rgba16", 6, 16),
    ]
    for name, ct, bd in specs:
        maxv = (1 << bd) - 1
        rows = gradient(37, 23, CHANNELS[ct], maxv, noise=maxv // 40)
        if ct in (4, 6):
            rows = alpha_mix(rows, maxv)
        cases.append(Case(f"png-{name}", "png", {"a.png": make_png(rows, ct, bd)}))
        cases.append(Case(f"png-{name}-adam7", "png", {"a.png": make_png(rows, ct, bd, interlace=True)}))
    # Palettes, including low bit depths and tRNS.
    for bd in (1, 2, 4, 8):
        n = 1 << bd
        pal = [(RNG.randrange(256), RNG.randrange(256), RNG.randrange(256)) for _ in range(min(n, 200))]
        rows = [[(RNG.randrange(len(pal)),) for _ in range(29)] for _ in range(17)]
        cases.append(Case(f"png-pal{bd}", "png", {"a.png": make_png(rows, 3, bd, palette=pal)}))
        trns = bytes(RNG.randrange(256) for _ in range(len(pal) // 2))
        cases.append(
            Case(f"png-pal{bd}-trns-adam7", "png", {"a.png": make_png(rows, 3, bd, palette=pal, trns=trns, interlace=True)})
        )
    # Color key transparency.
    rows = gradient(20, 20, 3, 255)
    rows[3][3] = (10, 20, 30)
    cases.append(Case("png-rgb-colorkey", "png", {"a.png": make_png(rows, 2, 8, trns=struct.pack(">HHH", 10, 20, 30))}))
    rows = gradient(20, 20, 1, 65535)
    cases.append(Case("png-gray16-colorkey", "png", {"a.png": make_png(rows, 0, 16, trns=struct.pack(">H", rows[0][0][0]))}))
    # Edge sizes.
    cases.append(Case("png-1x1", "png", {"a.png": make_png([[(1, 2, 3, 4)]], 6, 8)}))
    cases.append(Case("png-1x300", "png", {"a.png": make_png(gradient(1, 300, 3, 255), 2, 8)}))
    cases.append(Case("png-300x1", "png", {"a.png": make_png(gradient(300, 1, 3, 255), 2, 8)}))
    cases.append(Case("png-1x1-adam7", "png", {"a.png": make_png([[(200,)]], 0, 8, interlace=True)}))
    cases.append(Case("png-3x3-adam7", "png", {"a.png": make_png(gradient(3, 3, 4, 255), 6, 8, interlace=True)}))
    # Fully transparent, single color, many colors.
    cases.append(Case("png-all-transparent", "png", {"a.png": make_png([[(9, 9, 9, 0)] * 16] * 16, 6, 8)}))
    cases.append(Case("png-solid", "png", {"a.png": make_png([[(7, 8, 9)] * 64] * 64, 2, 8)}))
    noisy = [[(RNG.randrange(256), RNG.randrange(256), RNG.randrange(256), 255) for _ in range(64)] for _ in range(64)]
    cases.append(Case("png-noise-rgba", "png", {"a.png": make_png(noisy, 6, 8, zlevel=9)}))
    # 257 distinct colors (just over the palette limit) and exactly 256.
    for n in (256, 257):
        cols = [(i % 256, (i * 7) % 256, i // 256 * 100) for i in range(n)]
        rows = [[cols[(y * 20 + x) % n] for x in range(20)] for y in range(20)]
        cases.append(Case(f"png-{n}-colors", "png", {"a.png": make_png(rows, 2, 8)}))
    # Metadata and chunk layout.
    meta = [
        (b"gAMA", struct.pack(">I", 45455)),
        (b"cHRM", struct.pack(">8I", 31270, 32900, 64000, 33000, 30000, 60000, 15000, 6000)),
        (b"pHYs", struct.pack(">IIB", 2835, 2835, 1)),
        (b"tEXt", b"Comment\x00hello"),
        (b"iTXt", b"Title\x00\x00\x00ko\x00\xec\xa0\x9c\xeb\xaa\xa9\x00\xec\x95\x88\xeb\x85\x95"),
        (b"zTXt", b"Note\x00\x00" + zlib.compress(b"compressed text")),
        (b"prVt", b"private ancillary chunk"),
    ]
    rows = gradient(30, 30, 3, 255)
    cases.append(Case("png-metadata", "png", {"a.png": make_png(rows, 2, 8, extra=meta)}))
    cases.append(Case("png-metadata-strip", "png", {"a.png": make_png(rows, 2, 8, extra=meta)}, args=["-strip"]))
    icc = b"\x00" * 128 + b"fake-profile" * 10
    cases.append(Case("png-iccp", "png", {"a.png": make_png(rows, 2, 8, extra=[(b"iCCP", b"icc\x00\x00" + zlib.compress(icc))])}))
    cases.append(Case("png-sbit-srgb", "png", {"a.png": make_png(rows, 2, 8, extra=[(b"sRGB", b"\x00"), (b"sBIT", b"\x05\x06\x05")])}))
    cases.append(Case("png-split-idat", "png", {"a.png": make_png(rows, 2, 8, idat_split=7)}))
    cases.append(Case("png-text-after-idat", "png", {"a.png": make_png(rows, 2, 8, after_idat=[(b"tEXt", b"k\x00v")])}))
    cases.append(Case("png-filtered-paeth-input", "png", {"a.png": make_png(rows, 2, 8, filter_type=2, zlevel=6)}))
    # Palette with unused entries and duplicate colors.
    pal = [(5, 5, 5)] * 4 + [(RNG.randrange(256), 0, 0) for _ in range(60)]
    rows = [[(RNG.randrange(8),) for _ in range(25)] for _ in range(25)]
    cases.append(Case("png-pal-dupes-unused", "png", {"a.png": make_png(rows, 3, 8, palette=pal, trns=b"\x00\x80")}))
    # Larger image (exercises block splitting / multithreading).
    big = gradient(400, 300, 3, 255, noise=12)
    cases.append(Case("png-large-rgb", "png", {"a.png": make_png(big, 2, 8, zlevel=1)}, levels=(1, 3)))
    # Animated PNG (must stay animated and keep every frame).
    cases.append(Case("png-apng", "png", {"a.png": make_apng()}))
    # Several files with --mt-file.
    multi = {f"m{i}.png": make_png(gradient(40 + i, 30, 3, 255, noise=5), 2, 8) for i in range(6)}
    cases.append(Case("png-multi-mt", "png", multi, args=["--mt-file=4"]))
    return cases


def make_apng() -> bytes:
    frames = [Image.new("RGBA", (24, 24), (i * 60, 255 - i * 60, 30, 255)) for i in range(4)]
    for i, f in enumerate(frames):
        for x in range(i * 4, i * 4 + 6):
            f.putpixel((x, x), (0, 0, 0, 0))
    buf = io.BytesIO()
    frames[0].save(buf, "PNG", save_all=True, append_images=frames[1:], duration=100, loop=0)
    return buf.getvalue()


def jpeg_cases() -> list[Case]:
    cases = []
    base = Image.new("RGB", (67, 45))
    for y in range(45):
        for x in range(67):
            base.putpixel((x, y), ((x * 4) % 256, (y * 5) % 256, ((x + y) * 3 + RNG.randrange(20)) % 256))

    def jpg(im: Image.Image, **kw) -> bytes:
        buf = io.BytesIO()
        im.save(buf, "JPEG", **kw)
        return buf.getvalue()

    cases.append(Case("jpeg-baseline-420", "jpeg", {"a.jpg": jpg(base, quality=85, subsampling=2)}))
    cases.append(Case("jpeg-baseline-444", "jpeg", {"a.jpg": jpg(base, quality=95, subsampling=0)}))
    cases.append(Case("jpeg-baseline-422", "jpeg", {"a.jpg": jpg(base, quality=75, subsampling=1)}))
    cases.append(Case("jpeg-progressive", "jpeg", {"a.jpg": jpg(base, quality=80, progressive=True)}))
    cases.append(Case("jpeg-optimized", "jpeg", {"a.jpg": jpg(base, quality=80, optimize=True)}))
    cases.append(Case("jpeg-gray", "jpeg", {"a.jpg": jpg(base.convert("L"), quality=90)}))
    cases.append(Case("jpeg-cmyk", "jpeg", {"a.jpg": jpg(base.convert("CMYK"), quality=90)}))
    cases.append(Case("jpeg-1x1", "jpeg", {"a.jpg": jpg(Image.new("RGB", (1, 1), (1, 2, 3)))}))
    cases.append(Case("jpeg-odd-17x9", "jpeg", {"a.jpg": jpg(base.crop((0, 0, 17, 9)))}))
    cases.append(Case("jpeg-q100", "jpeg", {"a.jpg": jpg(base, quality=100)}))
    exif = Image.Exif()
    exif[0x0112] = 6  # orientation
    exif[0x010F] = "ECT test camera"
    cases.append(Case("jpeg-exif-icc-comment", "jpeg", {"a.jpg": jpg(base, quality=85, exif=exif.tobytes(), icc_profile=b"\x00" * 200, comment=b"hello")}))
    cases.append(Case("jpeg-exif-strip", "jpeg", {"a.jpg": jpg(base, quality=85, exif=exif.tobytes(), comment=b"hello")}, args=["-strip"]))
    try:
        cases.append(Case("jpeg-restart", "jpeg", {"a.jpg": jpg(base, quality=85, restart_marker_blocks=2)}))
    except TypeError:
        pass
    cases.append(Case("jpeg-progressive-flag", "jpeg", {"a.jpg": jpg(base, quality=85)}, args=["-progressive"], cpp_only=True))
    big = base.resize((800, 600))
    cases.append(Case("jpeg-large", "jpeg", {"a.jpg": jpg(big, quality=90)}, levels=(1, 3)))
    # Trailing data after EOI (some cameras append data); must survive or be kept.
    cases.append(Case("jpeg-trailing-data", "jpeg", {"a.jpg": jpg(base, quality=85) + b"TRAILER-DATA" * 4}))
    cases.append(Case("jpeg-trailing-data-strip", "jpeg", {"a.jpg": jpg(base, quality=85) + b"TRAILER-DATA" * 4}, args=["-strip"]))
    mpf = b"\xff\xe2" + (4 + 2 + 8).to_bytes(2, "big") + b"MPF\x00" + b"MM\x00*\x00\x00\x00\x08"
    first = jpg(base, quality=85)
    cases.append(Case("jpeg-mpf", "jpeg", {"a.jpg": first[:2] + mpf + first[2:] + jpg(base.rotate(90), quality=85)}))
    return cases


def webp_cases() -> list[Case]:
    im = Image.new("RGBA", (50, 40))
    for y in range(40):
        for x in range(50):
            im.putpixel((x, y), (x * 5 % 256, y * 6 % 256, 128, 255 if (x + y) % 7 else 0))

    def webp(img, **kw) -> bytes:
        buf = io.BytesIO()
        img.save(buf, "WEBP", **kw)
        return buf.getvalue()

    frames = [im, im.rotate(90), im.rotate(180)]
    buf = io.BytesIO()
    frames[0].save(buf, "WEBP", save_all=True, append_images=frames[1:], lossless=True, duration=80)
    return [
        Case("webp-lossless-rgba", "webp", {"a.webp": webp(im, lossless=True, quality=0)}),
        Case("webp-lossless-rgb", "webp", {"a.webp": webp(im.convert("RGB"), lossless=True, quality=0)}),
        Case("webp-lossy", "webp", {"a.webp": webp(im.convert("RGB"), quality=80)}),
        Case("webp-lossy-alpha", "webp", {"a.webp": webp(im, quality=70)}),
        Case("webp-animated", "webp", {"a.webp": buf.getvalue()}),
        Case("webp-exif", "webp", {"a.webp": webp(im, lossless=True, exif=b"Exif\x00\x00MM\x00*\x00\x00\x00\x08\x00\x00")}),
    ]


TEXT = b"".join(b"line %d: the quick brown fox jumps over the lazy dog\n" % i for i in range(3000))


def gz_with_header(data: bytes, *, name: bytes = b"", comment: bytes = b"", extra: bytes = b"", hcrc: bool = False) -> bytes:
    flags = (8 if name else 0) | (16 if comment else 0) | (4 if extra else 0) | (2 if hcrc else 0)
    hdr = b"\x1f\x8b\x08" + bytes((flags,)) + struct.pack("<I", 1700000000) + b"\x00\x03"
    if extra:
        hdr += struct.pack("<H", len(extra)) + extra
    if name:
        hdr += name + b"\x00"
    if comment:
        hdr += comment + b"\x00"
    if hcrc:
        hdr += struct.pack("<H", zlib.crc32(hdr) & 0xFFFF)
    co = zlib.compressobj(1, zlib.DEFLATED, -15)
    body = co.compress(data) + co.flush()
    return hdr + body + struct.pack("<II", zlib.crc32(data), len(data) & 0xFFFFFFFF)


def gz_cases() -> list[Case]:
    rnd = bytes(RNG.randrange(256) for _ in range(20000))
    return [
        Case("gz-plain", "gz", {"a.txt.gz": gzip.compress(TEXT, 1)}),
        Case("gz-name-comment-extra-hcrc", "gz", {"a.gz": gz_with_header(TEXT, name=b"a.txt", comment=b"c", extra=b"AB\x02\x00xy", hcrc=True)}),
        Case("gz-empty-content", "gz", {"e.gz": gzip.compress(b"", 1)}),
        Case("gz-multimember", "gz", {"m.gz": gzip.compress(TEXT[:5000], 1) + gzip.compress(TEXT[5000:], 1)}),
        Case("gz-incompressible", "gz", {"r.gz": gzip.compress(rnd, 1)}),
        Case("gz-trailing-zeros", "gz", {"t.gz": gzip.compress(TEXT, 1) + b"\x00" * 16}),
        Case("gz-stored-blocks", "gz", {"s.gz": gzip.compress(TEXT, 0)}),
        Case("gz-large-repetitive", "gz", {"l.gz": gzip.compress(b"A" * 3_000_000, 1)}, levels=(1, 3)),
    ]


def zip_bytes(entries, *, compression=zipfile.ZIP_DEFLATED, comment=b"", zip64=False, unseekable=False) -> bytes:
    raw = io.BytesIO()

    class Unseekable(io.RawIOBase):
        def writable(self):
            return True

        def write(self, b):
            return raw.write(b)

    target = Unseekable() if unseekable else raw
    with zipfile.ZipFile(target, "w", compression, compresslevel=1, allowZip64=True) as z:
        for e in entries:
            name, data = e[0], e[1]
            zi = zipfile.ZipInfo(name, date_time=(2020, 5, 17, 13, 37, 42))
            zi.compress_type = e[2] if len(e) > 2 else compression
            zi.external_attr = 0o100644 << 16
            if len(e) > 3:
                zi.comment = e[3]
            if name.endswith("/"):
                zi.external_attr = (0o40755 << 16) | 0x10
                z.writestr(zi, b"")
            elif zip64:
                with z.open(zi, "w", force_zip64=True) as f:
                    f.write(data)
            else:
                z.writestr(zi, data)
        z.comment = comment
    return raw.getvalue()


def zip_cases() -> list[Case]:
    rnd = bytes(RNG.randrange(256) for _ in range(30000))
    files = [("a.txt", TEXT), ("dir/", b""), ("dir/b.bin", rnd), ("empty.txt", b""), ("이름/유니코드.txt", "안녕".encode() * 500)]
    inner = zip_bytes([("x.txt", TEXT[:3000])])
    png = make_png(gradient(30, 30, 3, 255), 2, 8)
    return [
        Case("zip-deflated", "zip", {"a.zip": zip_bytes(files)}, args=["-zip"]),
        Case("zip-stored", "zip", {"a.zip": zip_bytes(files, compression=zipfile.ZIP_STORED)}, args=["-zip"]),
        Case("zip-stored-strict", "zip", {"a.zip": zip_bytes(files, compression=zipfile.ZIP_STORED)}, args=["-zip", "--strict"]),
        Case("zip-comments", "zip", {"a.zip": zip_bytes([("a.txt", TEXT, zipfile.ZIP_DEFLATED, b"entry comment")], comment=b"archive comment")}, args=["-zip"]),
        Case("zip-data-descriptor", "zip", {"a.zip": zip_bytes(files, unseekable=True)}, args=["-zip"]),
        Case("zip-zip64", "zip", {"a.zip": zip_bytes([("a.txt", TEXT), ("b.txt", TEXT[:100])], zip64=True)}, args=["-zip"]),
        Case("zip-nested-zip-and-png", "zip", {"a.zip": zip_bytes([("inner.zip", inner), ("img.png", png)])}, args=["-zip"]),
        Case("zip-empty-archive", "zip", {"a.zip": zip_bytes([])}, args=["-zip"]),
        Case("zip-only-dirs", "zip", {"a.zip": zip_bytes([("d/", b""), ("d/e/", b"")])}, args=["-zip"]),
        Case("zip-no-flag", "zip", {"a.zip": zip_bytes(files, compression=zipfile.ZIP_STORED)}),
        Case("zip-dupe-names", "zip", {"a.zip": zip_dupes()}, args=["-zip"]),
    ]


def zip_dupes() -> bytes:
    import warnings

    warnings.simplefilter("ignore")
    return zip_bytes([("same.txt", TEXT[:2000]), ("same.txt", TEXT[2000:4000])])


def raw_cases() -> list[Case]:
    rnd = bytes(RNG.randrange(256) for _ in range(50000))
    png = make_png(gradient(30, 30, 3, 255), 2, 8)
    return [
        Case("gzip-text", "raw-gzip", {"a.txt": TEXT}, args=["-gzip"]),
        Case("gzip-empty", "raw-gzip", {"e.txt": b""}, args=["-gzip"]),
        Case("gzip-random", "raw-gzip", {"r.bin": rnd}, args=["-gzip"]),
        Case("gzip-png-file", "raw-gzip", {"a.png": png}, args=["-gzip"]),
        Case("zip-dir", "raw-zip", {"d/a.txt": TEXT, "d/sub/b.bin": rnd, "d/sub/e.txt": b""}, args=["-zip"]),
        Case("zip-file", "raw-zip", {"a.txt": TEXT}, args=["-zip"]),
    ]


def corrupt_variants(name: str, data: bytes, fname: str) -> list[Case]:
    n = len(data)
    flip = bytearray(data)
    for i in range(n // 2, min(n, n // 2 + 8)):
        flip[i] ^= 0x5A
    return [
        Case(f"corrupt-{name}-truncated", "corrupt", {fname: data[: n // 2]}),
        Case(f"corrupt-{name}-truncated-end", "corrupt", {fname: data[: n - 5]}),
        Case(f"corrupt-{name}-bitflip", "corrupt", {fname: bytes(flip)}),
        Case(f"corrupt-{name}-header-only", "corrupt", {fname: data[:40]}),
    ]


def corrupt_cases() -> list[Case]:
    cases = []
    png = make_png(gradient(40, 40, 3, 255, noise=10), 2, 8, zlevel=1)
    cases += corrupt_variants("png", png, "a.png")
    # Valid structure, bad CRC.
    bad_crc = bytearray(png)
    bad_crc[29] ^= 1  # inside IHDR CRC
    cases.append(Case("corrupt-png-ihdr-crc", "corrupt", {"a.png": bytes(bad_crc)}))
    # IDAT that decompresses to too little data.
    short = png_chunk(b"IHDR", struct.pack(">IIBBBBB", 50, 50, 8, 2, 0, 0, 0)) + png_chunk(b"IDAT", zlib.compress(b"\x00" * 10)) + png_chunk(b"IEND", b"")
    cases.append(Case("corrupt-png-short-idat", "corrupt", {"a.png": b"\x89PNG\r\n\x1a\n" + short}))
    # Huge declared dimensions (decompression bomb guard).
    bomb = png_chunk(b"IHDR", struct.pack(">IIBBBBB", 100000, 100000, 8, 6, 0, 0, 0)) + png_chunk(b"IDAT", zlib.compress(b"\x00" * 1000)) + png_chunk(b"IEND", b"")
    cases.append(Case("corrupt-png-huge-dims", "corrupt", {"a.png": b"\x89PNG\r\n\x1a\n" + bomb}))
    zero = png_chunk(b"IHDR", struct.pack(">IIBBBBB", 0, 10, 8, 2, 0, 0, 0)) + png_chunk(b"IDAT", zlib.compress(b"")) + png_chunk(b"IEND", b"")
    cases.append(Case("corrupt-png-zero-width", "corrupt", {"a.png": b"\x89PNG\r\n\x1a\n" + zero}))
    badpal = make_png([[(5,)] * 4] * 4, 3, 8, palette=[(1, 2, 3)] * 2)
    cases.append(Case("corrupt-png-index-out-of-palette", "corrupt", {"a.png": badpal}))
    buf = io.BytesIO()
    Image.new("RGB", (60, 40), (10, 200, 30)).save(buf, "JPEG", quality=80)
    cases += corrupt_variants("jpeg", buf.getvalue(), "a.jpg")
    gz = gzip.compress(TEXT, 1)
    cases += corrupt_variants("gz", gz, "a.gz")
    bad = bytearray(gz)
    bad[-8] ^= 0xFF  # CRC32 trailer
    cases.append(Case("corrupt-gz-crc", "corrupt", {"a.gz": bytes(bad)}))
    bad = bytearray(gz)
    bad[-1] ^= 0x01  # ISIZE trailer
    cases.append(Case("corrupt-gz-isize", "corrupt", {"a.gz": bytes(bad)}))
    z = zip_bytes([("a.txt", TEXT), ("b.txt", TEXT[:500])])
    for c in corrupt_variants("zip", z, "a.zip"):
        c.args = ["-zip"]
        cases.append(c)
    # ZIP whose stored CRC does not match the data.
    zb = bytearray(zip_bytes([("a.txt", TEXT), ("b.txt", TEXT[:900])], compression=zipfile.ZIP_STORED))
    zb[30 + len("a.txt") + 100] ^= 0xFF  # a byte of a.txt's stored data
    cases.append(Case("corrupt-zip-crc", "corrupt", {"a.zip": bytes(zb)}, args=["-zip"]))
    # Empty and garbage files with every extension.
    for ext in ("png", "jpg", "gz", "zip", "webp"):
        cases.append(Case(f"corrupt-empty-{ext}", "corrupt", {f"a.{ext}": b""}, args=["-zip"] if ext == "zip" else []))
        cases.append(Case(f"corrupt-garbage-{ext}", "corrupt", {f"a.{ext}": bytes(RNG.randrange(256) for _ in range(3000))}))
    # Signature only.
    cases.append(Case("corrupt-png-signature-only", "corrupt", {"a.png": b"\x89PNG\r\n\x1a\n"}))
    cases.append(Case("corrupt-jpeg-soi-only", "corrupt", {"a.jpg": b"\xff\xd8"}))
    cases.append(Case("corrupt-gz-magic-only", "corrupt", {"a.gz": b"\x1f\x8b"}))
    cases.append(Case("corrupt-zip-eocd-only", "corrupt", {"a.zip": b"PK\x05\x06" + b"\x00" * 18}, args=["-zip"]))
    return cases


def fs_cases() -> list[Case]:
    png = make_png(gradient(40, 40, 3, 255, noise=10), 2, 8, zlevel=1)
    return [
        Case("fs-readonly", "fs", {"a.png": png}, setup="readonly", levels=(3,)),
        Case("fs-symlink", "fs", {"real.png": png}, setup="symlink", levels=(3,)),
        Case("fs-spaces-unicode", "png", {"이미지 파일 (1).png": png}, levels=(3,)),
        Case("fs-dash-name", "png", {"-a.png": png}, setup="dashdash", levels=(3,)),
        Case("fs-keep-mtime", "fs", {"a.png": png}, args=["-keep"], setup="mtime", levels=(3,)),
        Case("fs-dir-no-recurse", "fs", {"d/a.png": png, "d/s/b.png": png}, setup="dir", levels=(3,)),
        Case("fs-dir-recurse", "fs", {"d/a.png": png, "d/s/b.png": png}, args=["-recurse"], setup="dir", levels=(3,)),
        Case("fs-wrong-extension", "png", {"a.jpg": png}, levels=(3,)),
        Case("fs-missing-file", "fs", {}, setup="missing", levels=(3,)),
    ]


def all_cases() -> list[Case]:
    return png_cases() + jpeg_cases() + webp_cases() + gz_cases() + zip_cases() + raw_cases() + corrupt_cases() + fs_cases()


# ---------------------------------------------------------------------------
# Checks
# ---------------------------------------------------------------------------


def pil_pixels(data: bytes):
    im = Image.open(io.BytesIO(data))
    frames = []
    n = getattr(im, "n_frames", 1)
    for i in range(n):
        im.seek(i)
        f = im.convert("RGBA")
        frames.append((f.size, f.tobytes()))
    return im.format, frames


def compare_pil(before: bytes, after: bytes, ignore_transparent: bool) -> str | None:
    try:
        fa, a = pil_pixels(before)
        fb, b = pil_pixels(after)
    except Exception as e:  # noqa: BLE001
        return f"output does not decode: {e}"
    if fa != fb:
        return f"format {fa} -> {fb}"
    if len(a) != len(b):
        return f"frame count {len(a)} -> {len(b)}"
    for i, ((sa, pa), (sb, pb)) in enumerate(zip(a, b)):
        if sa != sb:
            return f"frame {i} size {sa} -> {sb}"
        if pa != pb:
            for j in range(0, len(pa), 4):
                if pa[j : j + 4] != pb[j : j + 4] and not (ignore_transparent and pa[j + 3] == 0 and pb[j + 3] == 0):
                    k = j // 4
                    return f"frame {i} pixel {k % sa[0]},{k // sa[0]}: {tuple(pa[j:j+4])} -> {tuple(pb[j:j+4])}"
    return None


def apng_info(data: bytes):
    kinds = [k for k, _ in png_chunks(data)]
    return kinds.count(b"acTL"), kinds.count(b"fcTL"), kinds.count(b"fdAT")


def check_png(before: bytes, after: bytes) -> str | None:
    try:
        b = decode_png(before)
    except Exception:  # noqa: BLE001
        return None
    try:
        a = decode_png(after)
    except Exception as e:  # noqa: BLE001
        return f"output PNG invalid: {e}"
    diff = same_pixels(b, a)
    if diff:
        return "pixels changed: " + diff
    if apng_info(before)[0]:
        if apng_info(before) != apng_info(after):
            return f"APNG structure changed {apng_info(before)} -> {apng_info(after)}"
        return compare_pil(before, after, ignore_transparent=True)
    return None


def raw_entries(data: bytes) -> dict:
    """Maps entry name -> (CRC ok?, CRC, raw compressed bytes or content)."""
    out = {}
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        for i in z.infolist():
            off = i.header_offset
            n, m = struct.unpack("<HH", data[off + 26 : off + 30])
            raw = data[off + 30 + n + m : off + 30 + n + m + i.compress_size]
            try:
                content = z.read(i)
                out[i.filename] = (True, i.CRC, content)
            except Exception:  # noqa: BLE001
                out[i.filename] = (False, i.CRC, raw)
    return out


def compare_damaged_zip(before: bytes, after: bytes) -> str | None:
    """A damaged archive may be rewritten only if every damaged entry is
    copied verbatim and every intact entry keeps its content."""
    try:
        b, a = raw_entries(before), raw_entries(after)
    except Exception as e:  # noqa: BLE001
        return f"cannot compare: {e}"
    if list(b) != list(a):
        return f"entry names changed {list(b)} -> {list(a)}"
    for name, (ok, crc, data) in b.items():
        if a[name] != (ok, crc, data):
            return f"entry {name} changed (intact={ok})"
    return None


NESTED_MAGIC = (b"\x89P", b"\xff\xd8", b"PK", b"\x1f\x8b", b"RI")


def zip_entries(data: bytes):
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        bad = z.testzip()
        if bad:
            raise ValueError(f"bad CRC in {bad}")
        return [(i.filename, z.read(i), i.date_time, i.comment) for i in z.infolist()], z.comment


def check_file(kind: str, name: str, before: bytes, after: bytes, args: list[str]) -> list[tuple[str, str]]:
    """Returns (severity, message) findings for one input file."""
    out: list[tuple[str, str]] = []
    if kind == "corrupt":
        if before != after:
            if before[:2] == b"\xff\xd8" and not compare_pil(before, after, ignore_transparent=False):
                out.append(("warn", "undetectably corrupt JPEG was rewritten (pixel-identical)"))
            elif before[:2] == b"PK" and (zmsg := compare_damaged_zip(before, after)) is None:
                out.append(("warn", "damaged ZIP: bad entries kept verbatim, good entries optimized"))
            else:
                out.append(("error", f"corrupt input was modified ({len(before)} -> {len(after)} bytes)"))
        return out
    if len(after) > len(before):
        out.append(("error", f"file grew {len(before)} -> {len(after)} bytes"))
    ext = name.rsplit(".", 1)[-1].lower()
    msg = None
    if before[:8] == b"\x89PNG\r\n\x1a\n":
        msg = check_png(before, after)
        if not msg and "-strip" not in args and before != after:
            # Without -strip, color-space and text/physical metadata must
            # survive (private chunks with the safe-to-copy bit unset may go).
            keep = {b"cHRM", b"gAMA", b"iCCP", b"sRGB", b"cICP", b"pHYs", b"tEXt", b"zTXt", b"iTXt", b"eXIf", b"tIME"}
            lost = sorted(k.decode() for k, d in png_chunks(before) if k in keep and (k, d) not in png_chunks(after))
            if lost:
                msg = f"PNG metadata dropped without -strip: {lost}"
    elif before[:2] == b"\xff\xd8":
        msg = compare_pil(before, after, ignore_transparent=False)
        if not msg and "-strip" not in args:
            if b"Exif" in before and b"Exif" not in after:
                msg = "EXIF dropped without -strip"
            elif b"ICC_PROFILE" in before and b"ICC_PROFILE" not in after:
                msg = "ICC profile dropped without -strip"
        if not msg and before.endswith(b"TRAILER-DATA") and not after.endswith(b"TRAILER-DATA" * 4) and "-strip" not in args:
            msg = "data after JPEG EOI was dropped without -strip"
        if not msg and b"MPF\x00" in before[:200] and before != after:
            msg = "multi-picture (MPF) JPEG was modified"
    elif before[:4] == b"RIFF":
        msg = compare_pil(before, after, ignore_transparent=b"VP8L" in before)
    elif before[:2] == b"\x1f\x8b":
        try:
            if gzip.decompress(before) != gzip.decompress(after):
                msg = "gzip content changed"
        except Exception as e:  # noqa: BLE001
            msg = f"gzip output invalid: {e}"
    elif before[:2] == b"PK":
        try:
            eb, cb = zip_entries(before)
            ea, ca = zip_entries(after)
            if [n for n, *_ in eb] != [n for n, *_ in ea]:
                msg = f"ZIP entry names changed: {[n for n, *_ in eb]} -> {[n for n, *_ in ea]}"
            else:
                for (n, d0, _, _), (_, d1, _, _) in zip(eb, ea):
                    if d0 == d1:
                        continue
                    # ECT (C/C++) optimizes embedded PNG/JPEG/ZIP/gzip files
                    # too; accept that only if the nested file is equivalent.
                    nested = [m for sev, m in check_file("nested", n, d0, d1, []) if sev == "error"]
                    if not nested and d0[:2] in NESTED_MAGIC:
                        out.append(("warn", f"embedded file {n} was re-optimized (content equivalent)"))
                    else:
                        msg = f"ZIP entry {n} changed: {nested or 'different bytes'}"
                        break
                if [t for _, _, t, _ in eb] != [t for _, _, t, _ in ea]:
                    out.append(("warn", "ZIP entry timestamps changed"))
            if not msg and (cb != ca or [c for *_, c in eb] != [c for *_, c in ea]):
                out.append(("warn", "ZIP comments changed"))
        except Exception as e:  # noqa: BLE001
            msg = f"ZIP output invalid: {e}"
    elif ext:
        pass
    if msg:
        out.append(("error", msg))
    return out


@dataclass
class Finding:
    impl: str
    case: str
    level: int
    severity: str
    message: str


MEMORY_LIMIT = 2 << 30


def limit_memory() -> None:
    """Caps the address space of uninstrumented binaries (sanitizer and race
    builds reserve huge virtual ranges and are left alone)."""
    import resource

    resource.setrlimit(resource.RLIMIT_AS, (MEMORY_LIMIT, MEMORY_LIMIT))


def run_case(impl: str, binary: str, case: Case, level: int, timeout: float) -> list[Finding]:
    findings: list[Finding] = []

    def add(sev: str, msg: str) -> None:
        findings.append(Finding(impl, case.name, level, sev, msg))

    work = Path(tempfile.mkdtemp(prefix="ectconf-"))
    try:
        for rel, data in case.files.items():
            p = work / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(data)
        targets = sorted({rel.split("/")[0] for rel in case.files}) if case.files else []
        args = [f"-{level}", "-quiet", *case.args]
        if impl.startswith(("go", "rust")) and "--mt-file=4" not in case.args:
            args.append("--mt-file=1")
        if case.setup == "readonly":
            os.chmod(work / "a.png", 0o444)
        if case.setup == "symlink":
            os.symlink("real.png", work / "link.png")
            targets = ["link.png"]
        if case.setup == "mtime":
            os.utime(work / "a.png", (1_500_000_000, 1_500_000_000))
        if case.setup == "missing":
            targets = ["does-not-exist.png"]
        if case.setup == "dashdash":
            args.append("--")
        before = {}
        for p in work.rglob("*"):
            if p.is_file() and not p.is_symlink():
                before[str(p.relative_to(work))] = p.read_bytes()
        env = dict(os.environ, ASAN_OPTIONS="detect_leaks=1:abort_on_error=0:allocator_may_return_null=1:max_allocation_size_mb=4096:hard_rss_limit_mb=1500", UBSAN_OPTIONS="print_stacktrace=1")
        try:
            r = subprocess.run(
                [binary, *args, *targets],
                cwd=work,
                capture_output=True,
                timeout=timeout,
                env=env,
                check=False,
                preexec_fn=None if "-" in impl else limit_memory,
            )
        except subprocess.TimeoutExpired:
            add("error", f"timeout after {timeout:.0f}s")
            return findings
        err = r.stderr.decode(errors="replace")
        if r.returncode < 0:
            add("error", f"killed by signal {-r.returncode}: {err[-300:]}")
        for marker in ("AddressSanitizer", "LeakSanitizer", "runtime error:", "panic:", "panicked at", "WARNING: DATA RACE", "fatal error:"):
            if marker in err:
                i = err.index(marker)
                add("error", f"{marker} {err[max(0, i - 200): i + 1200]}")
                break
        after = {}
        for p in work.rglob("*"):
            if p.is_file() and not p.is_symlink():
                after[str(p.relative_to(work))] = p.read_bytes()
        new = sorted(set(after) - set(before))
        expected_new = set()
        if case.kind == "raw-gzip":
            expected_new = {k + ".gz" for k in before}
        if case.kind == "raw-zip":
            expected_new = {t + ".zip" for t in targets} | {t.rsplit(".", 1)[0] + ".zip" for t in targets}
        stray = [n for n in new if n not in expected_new]
        if stray:
            add("error", f"unexpected files left: {stray}")
        for n in sorted(set(before) - set(after)):
            if case.kind not in ("raw-gzip", "raw-zip"):
                add("error", f"input file disappeared: {n}")
        if case.kind == "raw-gzip":
            for k, data in before.items():
                gz = after.get(k + ".gz")
                if gz is None:
                    add("warn" if not data else "error", f"{k}.gz not created (rc={r.returncode})")
                    continue
                try:
                    if gzip.decompress(gz) != data:
                        add("error", f"{k}.gz content differs")
                except Exception as e:  # noqa: BLE001
                    add("error", f"{k}.gz invalid: {e}")
        elif case.kind == "raw-zip":
            for t in targets:
                z = after.get(t + ".zip") or after.get(t.rsplit(".", 1)[0] + ".zip")
                if z is None:
                    add("error", f"{t}.zip not created (rc={r.returncode})")
                    continue
                try:
                    ents, _ = zip_entries(z)
                except Exception as e:  # noqa: BLE001
                    add("error", f"{t}.zip invalid: {e}")
                    continue
                files = {n: d for n, d, _, _ in ents if not n.endswith("/")}
                want = {k if t != k.split("/")[0] or "/" in k else k: v for k, v in before.items()}
                got_sorted = sorted(files.values())
                if got_sorted != sorted(want.values()):
                    add("error", f"{t}.zip entries differ: {sorted(files)}")
        else:
            for k, data in before.items():
                if k in after:
                    for sev, msg in check_file(case.kind, k, data, after[k], case.args):
                        add(sev, f"{k}: {msg}")
        # File-system expectations.
        if case.setup == "readonly":
            mode = stat.S_IMODE(os.stat(work / "a.png").st_mode)
            if mode != 0o444:
                add("warn", f"read-only file mode changed to {oct(mode)}")
        if case.setup == "symlink":
            if not (work / "link.png").is_symlink():
                add("error", "symlink was replaced by a regular file")
            if after.get("real.png") is not None:
                for sev, msg in check_file("png", "real.png", before["real.png"], after["real.png"], []):
                    add(sev, f"real.png: {msg}")
        if case.setup == "mtime":
            if int(os.stat(work / "a.png").st_mtime) != 1_500_000_000:
                add("error", "-keep did not preserve the modification time")
        if case.setup == "dir":
            changed = [k for k in before if before[k] != after.get(k)]
            want = {"d/a.png", "d/s/b.png"} if "-recurse" in case.args else {"d/a.png"}
            if set(changed) != want:
                add("warn", f"directory handling: optimized {sorted(changed)}, expected {sorted(want)}")
        if case.setup == "missing" and r.returncode == 0:
            add("warn", "missing input file reported success (exit code 0)")
        if case.kind == "corrupt" and r.returncode != 0 and not err.strip():
            add("warn", f"nonzero exit {r.returncode} without a message")
        if case.kind not in ("corrupt",) and case.setup != "missing" and r.returncode != 0:
            add("warn", f"exit code {r.returncode}: {err.strip()[:300]}")
    finally:
        for p in work.rglob("*"):
            try:
                os.chmod(p, 0o700 if p.is_dir() else 0o600, follow_symlinks=False)
            except (OSError, NotImplementedError):
                pass
        shutil.rmtree(work, ignore_errors=True)
    return findings


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--impl", action="append", required=True, help="name=path (names starting with cpp get C/C++-only cases)")
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--timeout", type=float, default=120)
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--filter", default="", help="only run cases whose name matches this regular expression")
    ap.add_argument("--levels", default="", help="override levels, e.g. 1,3")
    args = ap.parse_args()
    impls = [tuple(s.split("=", 1)) for s in args.impl]
    cases = [c for c in all_cases() if re.search(args.filter, c.name)]
    jobs = []
    for name, binary in impls:
        for c in cases:
            if c.cpp_only and not name.startswith("cpp"):
                continue
            levels = tuple(int(x) for x in args.levels.split(",")) if args.levels else c.levels
            for lvl in levels:
                jobs.append((name, binary, c, lvl))
    print(f"{len(cases)} cases, {len(jobs)} runs", file=sys.stderr)
    findings: list[Finding] = []
    log = args.out.with_suffix(".jsonl").open("w", encoding="utf-8")
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as ex:
        futs = [ex.submit(run_case, n, b, c, lvl, args.timeout) for n, b, c, lvl in jobs]
        for i, f in enumerate(concurrent.futures.as_completed(futs), 1):
            res = f.result()
            findings += res
            for x in res:
                log.write(json.dumps(x.__dict__, ensure_ascii=False) + "\n")
            log.flush()
            if i % 100 == 0:
                print(f"  {i}/{len(jobs)}", file=sys.stderr, flush=True)
    log.close()
    findings.sort(key=lambda f: (f.severity, f.impl, f.case, f.level))
    args.out.write_text(json.dumps([f.__dict__ for f in findings], indent=1, ensure_ascii=False))
    # Compact summary: group identical messages.
    groups: dict[tuple, list] = {}
    for f in findings:
        groups.setdefault((f.severity, f.impl, f.case, f.message[:160]), []).append(f.level)
    for (sev, impl, case, msg), lv in sorted(groups.items()):
        print(f"[{sev}] {impl:10s} {case:32s} -{','.join(map(str, sorted(lv)))}: {msg}")
    print(f"{sum(f.severity == 'error' for f in findings)} errors, {sum(f.severity == 'warn' for f in findings)} warnings", file=sys.stderr)


if __name__ == "__main__":
    main()
