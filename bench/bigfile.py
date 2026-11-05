#!/usr/bin/env python3
"""Times `ect -<level> -quiet -gzip FILE` on one large input.

bench.py uses the Canterbury Corpus, whose largest file is about 1 MB.
This script complements it with a single big text file (for example the
first 10 MB of enwik8) to show how the deflate core scales. Each .gz is
decompressed and compared with the input.

    python3 bigfile.py --impl a=./ect --impl b=./other --input enwik10m \
        --levels 3,6 --runs 3 --out big.json
"""

from __future__ import annotations

import argparse
import gzip
import json
import shutil
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--impl", action="append", required=True, help="name=path (repeatable)")
    ap.add_argument("--input", required=True, type=Path)
    ap.add_argument("--levels", default="3,6")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()

    data = args.input.read_bytes()
    results = []
    for level in (int(x) for x in args.levels.split(",")):
        for item in args.impl:
            name, binary = item.split("=", 1)
            binary = str(Path(binary).resolve())
            times, size = [], None
            for _ in range(args.runs):
                work = Path(tempfile.mkdtemp(prefix="ect-big-"))
                try:
                    src = work / args.input.name
                    src.write_bytes(data)
                    start = time.perf_counter()
                    proc = subprocess.run([binary, f"-{level}", "-quiet", "-gzip", src.name], cwd=work,
                                          stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False)
                    times.append(time.perf_counter() - start)
                    if proc.returncode != 0:
                        raise RuntimeError(f"{name} -{level} failed: {proc.stderr.decode(errors='replace')}")
                    gz = (work / (src.name + ".gz")).read_bytes()
                    if gzip.decompress(gz) != data:
                        raise RuntimeError(f"{name} -{level}: output does not round-trip")
                    size = len(gz)
                finally:
                    shutil.rmtree(work, ignore_errors=True)
            entry = {"workload": f"big:{args.input.name}", "level": level, "impl": name,
                     "input_bytes": len(data), "output_bytes": size,
                     "seconds": statistics.median(times), "runs": args.runs}
            results.append(entry)
            print(f"-{level} {name:16} {entry['seconds']:8.3f}s  {len(data)} -> {size}", flush=True)
    args.out.write_text(json.dumps({"results": results}, indent=2))


if __name__ == "__main__":
    main()
