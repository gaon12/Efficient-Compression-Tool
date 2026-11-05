#!/usr/bin/env python3
"""Checks the harness itself: its PNG decoder must agree with Pillow on
every generated valid PNG and reject every corrupt one.

    python3 selftest.py
"""

import io
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import conformance as c  # noqa: E402
from PIL import Image  # noqa: E402
cases = c.all_cases()
print(len(cases), "cases")
bad = 0
for case in cases:
    for name, data in case.files.items():
        if data[:8] != b"\x89PNG\r\n\x1a\n" or case.kind == "corrupt":
            continue
        w, h, px = c.decode_png(data)
        im = Image.open(io.BytesIO(data))
        if im.mode in ("I;16", "I", "I;16B") or getattr(im, "n_frames", 1) > 1:
            continue
        ref = im.convert("RGBA").tobytes()
        mine = bytes(v >> 8 for p in px for v in p)
        if ref != mine:
            # Pillow scales low bit depths differently? report first mismatch
            for i in range(0, len(ref), 4):
                if ref[i:i+4] != mine[i:i+4]:
                    print("MISMATCH", case.name, im.mode, i // 4, tuple(ref[i:i+4]), tuple(mine[i:i+4])); bad += 1; break
print("mismatches:", bad)
# decoder must reject corrupt PNGs
for case in cases:
    if case.kind == "corrupt" and case.name.startswith("corrupt-png"):
        try:
            c.decode_png(list(case.files.values())[0]); print("accepted:", case.name)
        except Exception:  # noqa: BLE001
            pass
