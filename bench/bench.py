#!/usr/bin/env python3
"""Cross-implementation benchmark for ECT (C/C++, Go and Rust).

The script runs the same workloads with every given `ect` binary on the
current machine and records wall time and output size. It only needs the
Python standard library.

Workloads (all inputs are deterministic):

* gzip  - `ect -gzip -L <files>` on the 11 files of the Canterbury Corpus
          (2.8 MB of text, HTML, C source, a spreadsheet and binaries).
          Every .gz is decompressed and compared with its source.
* zip   - `ect -L -zip corpus.zip` on a ZIP of the same files stored
          without compression. `-zip` is required by the C/C++ version to
          touch ZIP files at all; all three rewrite the archive in place.
          The rewritten archive is extracted and compared.
* png   - `ect -L <files>` on four generated PNGs written with fast zlib
          settings (photo-like gradient, screenshot-like flat areas, RGBA
          icon, grayscale pattern).

Each workload runs at the requested levels in two thread modes:

* single   - one file at a time (Go/Rust: `--mt-file=1`, C/C++ default),
* parallel - `--mt-file=<CPU count>` for every implementation.

Usage:

    python3 bench.py --impl cpp=path/to/ect --impl go=path/to/ect \\
        --impl rust=path/to/ect --corpus cantrbry.zip --out results.json

The JSON output is consumed by `report.py`.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import math
import os
import platform
import shutil
import statistics
import struct
import subprocess
import sys
import tempfile
import time
import zipfile
import zlib
from collections.abc import Callable
from pathlib import Path

# SHA-256 of https://corpus.canterbury.ac.nz/resources/cantrbry.zip
CANTERBURY_SHA256 = "c44b686dfc137e74aba4db0540e5d6568cb09e270ba8f8411d2f9df24f39a1a6"


# --------------------------------------------------------------------------
# Corpus preparation
# --------------------------------------------------------------------------


def load_canterbury(zip_path: Path) -> dict[str, bytes]:
    """Returns {file name: content} after verifying the archive checksum."""
    data = zip_path.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    if digest != CANTERBURY_SHA256:
        sys.exit(f"unexpected corpus checksum {digest}, expected {CANTERBURY_SHA256}")
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        return {info.filename: z.read(info) for info in z.infolist() if not info.is_dir()}


def png_bytes(width: int, height: int, color_type: int, rows: list[bytes]) -> bytes:
    """Encodes 8-bit scanlines as a PNG using filter 0 and zlib level 1,
    i.e. a typical "fast export" that leaves room for optimization."""

    def chunk(kind: bytes, payload: bytes) -> bytes:
        crc = zlib.crc32(kind + payload) & 0xFFFFFFFF
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", crc)

    raw = b"".join(b"\x00" + row for row in rows)
    ihdr = struct.pack(">IIBBBBB", width, height, 8, color_type, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 1))
        + chunk(b"IEND", b"")
    )


class Lcg:
    """Small deterministic PRNG so the images are identical everywhere."""

    def __init__(self, seed: int) -> None:
        self.state = seed

    def next(self) -> int:
        self.state = (self.state * 1103515245 + 12345) & 0x7FFFFFFF
        return self.state >> 16


def generate_pngs() -> dict[str, bytes]:
    images: dict[str, bytes] = {}

    # Photo-like: smooth gradients with low-amplitude noise (RGB, 640x480).
    rng = Lcg(1)
    rows = []
    for y in range(480):
        row = bytearray()
        for x in range(640):
            n = rng.next() % 9 - 4
            r = int(127 + 100 * math.sin(x / 70.0) * math.cos(y / 90.0)) + n
            g = int(110 + 80 * math.sin((x + y) / 120.0)) + n
            b = int(90 + 60 * math.cos(x / 45.0 - y / 60.0)) + n
            row += bytes((max(0, min(255, r)), max(0, min(255, g)), max(0, min(255, b))))
        rows.append(bytes(row))
    images["photo.png"] = png_bytes(640, 480, 2, rows)

    # Screenshot-like: flat panels, borders and "text" stripes (RGB, 800x600).
    rows = []
    for y in range(600):
        row = bytearray()
        for x in range(800):
            if y < 40:
                c = (45, 52, 64)
            elif x < 200:
                c = (240, 240, 245) if (y // 24) % 2 else (230, 232, 238)
            elif 60 <= y % 30 < 66 or (y % 30 < 12 and (x // 7) % 5 != 0 and 220 <= x < 760):
                c = (30, 30, 30)
            else:
                c = (255, 255, 255)
            row += bytes(c)
        rows.append(bytes(row))
    images["screenshot.png"] = png_bytes(800, 600, 2, rows)

    # Icon with transparency: filled circle with soft edge (RGBA, 256x256).
    rows = []
    for y in range(256):
        row = bytearray()
        for x in range(256):
            d = math.hypot(x - 128, y - 128)
            alpha = 255 if d < 100 else max(0, int(255 - (d - 100) * 25))
            row += bytes((220, 80 + x // 4, 40 + y // 4, alpha))
        rows.append(bytes(row))
    images["icon.png"] = png_bytes(256, 256, 6, rows)

    # Grayscale pattern with noise (8-bit gray, 512x512).
    rng = Lcg(7)
    rows = []
    for y in range(512):
        rows.append(bytes(((x ^ y) & 0xF0) // 2 + rng.next() % 16 for x in range(512)))
    images["pattern.png"] = png_bytes(512, 512, 0, rows)
    return images


def stored_zip(files: dict[str, bytes]) -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_STORED) as z:
        for name in sorted(files):
            info = zipfile.ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0))
            z.writestr(info, files[name])
    return buf.getvalue()


# --------------------------------------------------------------------------
# Running
# --------------------------------------------------------------------------


def thread_args(impl: str, mode: str, cpus: int) -> list[str]:
    if mode == "parallel":
        return [f"--mt-file={cpus}"]
    # The C/C++ version processes one file at a time unless --mt-file is
    # given; Go and Rust default to all CPUs, so pin them to one worker.
    return [] if impl == "cpp" else ["--mt-file=1"]


def run_once(binary: str, args: list[str], cwd: Path) -> float:
    start = time.perf_counter()
    proc = subprocess.run(
        [binary, *args], cwd=cwd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False
    )
    elapsed = time.perf_counter() - start
    if proc.returncode != 0:
        raise RuntimeError(
            f"{binary} {' '.join(args)} failed ({proc.returncode}): {proc.stderr.decode(errors='replace')}"
        )
    return elapsed


def workload_gzip(corpus: dict[str, bytes], work: Path) -> tuple[list[str], Callable[[], int]]:
    for name, data in corpus.items():
        (work / name).write_bytes(data)
    names = sorted(corpus)

    def check() -> int:
        total = 0
        for name in names:
            gz = (work / (name + ".gz")).read_bytes()
            if gzip.decompress(gz) != corpus[name]:
                raise RuntimeError(f"gzip output for {name} does not round-trip")
            total += len(gz)
        return total

    return ["-gzip", *names], check


def workload_zip(corpus: dict[str, bytes], work: Path) -> tuple[list[str], Callable[[], int]]:
    (work / "corpus.zip").write_bytes(stored_zip(corpus))

    def check() -> int:
        path = work / "corpus.zip"
        with zipfile.ZipFile(path) as z:
            for name, data in corpus.items():
                if z.read(name) != data:
                    raise RuntimeError(f"zip entry {name} changed")
        return path.stat().st_size

    return ["-zip", "corpus.zip"], check


def workload_png(images: dict[str, bytes], work: Path) -> tuple[list[str], Callable[[], int]]:
    for name, data in images.items():
        (work / name).write_bytes(data)
    names = sorted(images)

    def check() -> int:
        total = 0
        for name in names:
            data = (work / name).read_bytes()
            if not data.startswith(b"\x89PNG\r\n\x1a\n"):
                raise RuntimeError(f"{name} is no longer a PNG")
            total += len(data)
        return total

    return names, check


def cpu_model() -> str:
    """Best-effort human readable CPU name (empty if unknown)."""
    try:
        if sys.platform.startswith("linux"):
            for line in Path("/proc/cpuinfo").read_text().splitlines():
                if line.startswith("model name"):
                    return line.split(":", 1)[1].strip()
        elif sys.platform == "darwin":
            return subprocess.run(
                ["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True, check=False
            ).stdout.strip()
        elif sys.platform == "win32":
            import winreg

            key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
            return str(winreg.QueryValueEx(key, "ProcessorNameString")[0]).strip()
    except (OSError, ValueError):
        pass
    return ""


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--impl", action="append", required=True, help="name=path of an ect binary (repeatable)"
    )
    parser.add_argument("--corpus", required=True, type=Path, help="path to cantrbry.zip")
    parser.add_argument("--levels", default="1,3,9", help="comma separated levels")
    parser.add_argument("--runs", type=int, default=3, help="runs per measurement (median)")
    parser.add_argument(
        "--max-level-runs", type=int, default=1, help="runs for level 9, which is very slow for C/C++"
    )
    parser.add_argument("--modes", default="single,parallel")
    parser.add_argument("--workloads", default="gzip,zip,png")
    parser.add_argument("--label", default="", help="free-form machine label")
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()

    impls = dict(item.split("=", 1) for item in args.impl)
    for name, path in impls.items():
        impls[name] = str(Path(path).resolve())
        if not Path(impls[name]).is_file():
            sys.exit(f"{name}: binary not found at {path}")

    corpus = load_canterbury(args.corpus)
    images = generate_pngs()
    inputs = {
        "gzip": (workload_gzip, corpus, sum(map(len, corpus.values()))),
        "zip": (workload_zip, corpus, sum(map(len, corpus.values()))),
        "png": (workload_png, images, sum(map(len, images.values()))),
    }
    cpus = os.cpu_count() or 1
    results = []

    for workload in args.workloads.split(","):
        prepare, data, input_bytes = inputs[workload]
        for level in (int(x) for x in args.levels.split(",")):
            runs = args.max_level_runs if level >= 9 else args.runs
            for mode in args.modes.split(","):
                for impl, binary in impls.items():
                    times, size = [], None
                    for _ in range(runs):
                        work = Path(tempfile.mkdtemp(prefix="ect-bench-"))
                        try:
                            cli, check = prepare(data, work)
                            cmd = [f"-{level}", "-quiet", *thread_args(impl, mode, cpus), *cli]
                            times.append(run_once(binary, cmd, work))
                            size = check()
                        finally:
                            shutil.rmtree(work, ignore_errors=True)
                    entry = {
                        "workload": workload,
                        "level": level,
                        "mode": mode,
                        "impl": impl,
                        "input_bytes": input_bytes,
                        "output_bytes": size,
                        "seconds": statistics.median(times),
                        "runs": runs,
                    }
                    results.append(entry)
                    print(
                        f"{workload:5} -{level} {mode:8} {impl:5} "
                        f"{entry['seconds']:8.3f}s  {input_bytes} -> {size}",
                        flush=True,
                    )

    meta = {
        "label": args.label,
        "system": platform.system(),
        "release": platform.release(),
        "machine": platform.machine(),
        "processor": platform.processor(),
        "cpu_model": cpu_model(),
        "cpus": cpus,
        "python": platform.python_version(),
    }
    args.out.write_text(json.dumps({"meta": meta, "results": results}, indent=2))


if __name__ == "__main__":
    main()
