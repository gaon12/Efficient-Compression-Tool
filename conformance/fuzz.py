#!/usr/bin/env python3
"""Mutation fuzzer built on the conformance harness.

Takes the valid inputs of a few conformance cases, applies random
mutations (bit flips, byte overwrites, truncation, duplicated or deleted
ranges) and runs every given binary on each mutant with the usual
conformance checks: no crash, hang, sanitizer report or panic, no change
of decoded content, no stray files. The mutants are deterministic for a
given --seed, so findings can be reproduced.

    python3 fuzz.py --impl cpp=./ect --impl go=./ect-go --count 200 --out fuzz.json
"""

from __future__ import annotations

import argparse
import concurrent.futures
import gzip
import io
import json
import os
import random
import sys
import zipfile
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import conformance as c  # noqa: E402

# Valid inputs that cover each decoder; ZIP and gzip go through -zip/-gzip.
BASE_CASES = [
    "png-rgb8", "png-pal8-trns-adam7", "png-gray16", "png-metadata",
    "jpeg-baseline-420", "jpeg-progressive", "jpeg-exif-icc-comment",
    "webp-lossy-alpha", "webp-lossless-rgba",
    "gz-name-comment-extra-hcrc", "gz-multimember",
    "zip-deflated", "zip-stored", "zip-comments",
]


def mutate(data: bytes, rng: random.Random) -> bytes:
    b = bytearray(data)
    for _ in range(rng.randint(1, 3)):
        op = rng.randrange(5)
        if not b:
            break
        if op == 0:  # flip a few bits
            for _ in range(rng.randint(1, 8)):
                i = rng.randrange(len(b))
                b[i] ^= 1 << rng.randrange(8)
        elif op == 1:  # overwrite bytes with "interesting" values
            for _ in range(rng.randint(1, 4)):
                i = rng.randrange(len(b))
                b[i] = rng.choice((0x00, 0xFF, 0x7F, 0x80, 0x01, rng.randrange(256)))
        elif op == 2:  # truncate
            del b[rng.randrange(len(b)):]
        elif op == 3:  # duplicate a range
            i = rng.randrange(len(b))
            j = min(len(b), i + rng.randint(1, 64))
            b[i:i] = b[i:j]
        else:  # delete a range
            i = rng.randrange(len(b))
            del b[i : i + rng.randint(1, 64)]
    return bytes(b)


def decodes(kind: str, data: bytes) -> bool:
    """True if a reference decoder accepts the whole input. Mutants that do
    not decode are checked as corrupt input (they must stay untouched);
    the others must keep their decoded content."""
    try:
        if kind == "png":
            c.decode_png(data)
        elif kind in ("jpeg", "webp"):
            with c.Image.open(io.BytesIO(data)) as im:
                im.load()
        elif kind == "gz":
            gzip.decompress(data)
        elif kind == "zip":
            with zipfile.ZipFile(io.BytesIO(data)) as z:
                if z.testzip() is not None:
                    return False
                for info in z.infolist():
                    z.read(info)
        return True
    except Exception:  # noqa: BLE001
        return False


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--impl", action="append", required=True, help="name=path")
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--count", type=int, default=100, help="mutants per base case")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--level", type=int, default=1)
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--timeout", type=float, default=60)
    args = ap.parse_args()

    by_name = {case.name: case for case in c.all_cases()}
    rng = random.Random(args.seed)
    mutants = []
    for name in BASE_CASES:
        base = by_name[name]
        (fname, data), = base.files.items()
        for k in range(args.count):
            m = mutate(data, rng)
            kind = base.kind if decodes(base.kind, m) else "corrupt"
            mutants.append(c.Case(f"{name}~{args.seed}.{k}", kind, {fname: m}, list(base.args)))
    impls = [tuple(s.split("=", 1)) for s in args.impl]
    print(f"{len(mutants)} mutants x {len(impls)} impls", file=sys.stderr)

    findings = []
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as ex:
        futs = [ex.submit(c.run_case, n, b, m, args.level, args.timeout) for n, b in impls for m in mutants]
        for i, f in enumerate(concurrent.futures.as_completed(futs), 1):
            findings += [x for x in f.result() if x.severity == "error"]
            if i % 200 == 0:
                print(f"  {i}/{len(futs)}, {len(findings)} errors", file=sys.stderr, flush=True)
    findings.sort(key=lambda x: (x.impl, x.case))
    args.out.write_text(json.dumps([x.__dict__ for x in findings], indent=1, ensure_ascii=False))
    for x in findings:
        print(f"[{x.impl}] {x.case}: {x.message[:300]}")
    print(f"{len(findings)} errors", file=sys.stderr)


if __name__ == "__main__":
    main()
