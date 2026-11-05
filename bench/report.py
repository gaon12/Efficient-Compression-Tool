#!/usr/bin/env python3
"""Renders `bench.py` result files as Markdown.

Usage:

    python3 report.py results-linux.json results-windows.json > RESULTS.md

For every result file (one per machine/OS) the report contains one table
per workload with wall time and output size for each implementation, level
and thread mode. A final section compares the operating systems for the
default level (-3) in single-worker mode.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

WORKLOAD_TITLES = {
    "gzip": "gzip: compress the 11 Canterbury Corpus files to .gz",
    "zip": "zip: re-optimize a stored (uncompressed) ZIP of the corpus (`-zip`)",
    "png": "png: re-optimize 4 fast-zlib PNGs",
}


def load(paths: list[str]) -> list[dict]:
    return [json.loads(Path(p).read_text()) for p in paths]


def impl_order(results: list[dict]) -> list[str]:
    order: list[str] = []
    for r in results:
        if r["impl"] not in order:
            order.append(r["impl"])
    return order


def fmt_time(seconds: float) -> str:
    return f"{seconds:.2f} s" if seconds >= 0.1 else f"{seconds * 1000:.0f} ms"


def fmt_size(output: int, original: int) -> str:
    return f"{output:,} ({100.0 * output / original:.1f}%)"


def machine_heading(meta: dict) -> str:
    label = meta.get("label") or meta["system"]
    cpu = meta.get("cpu_model") or meta.get("processor") or meta["machine"]
    return (
        f"### {label}\n\n"
        f"- OS: {meta['system']} {meta['release']} ({meta['machine']})\n"
        f"- CPU: {cpu}, {meta['cpus']} logical CPUs\n"
    )


def workload_table(results: list[dict], workload: str, impls: list[str]) -> str:
    rows = [r for r in results if r["workload"] == workload]
    if not rows:
        return ""
    original = rows[0]["input_bytes"]
    keys = []
    for r in rows:
        key = (r["level"], r["mode"])
        if key not in keys:
            keys.append(key)
    index = {(r["level"], r["mode"], r["impl"]): r for r in rows}

    title = WORKLOAD_TITLES.get(workload, workload)
    out = [f"**{title}** (input {original:,} bytes; size = output bytes, % of input)\n"]
    header = ["Level", "Workers"] + [f"{i} time" for i in impls] + [f"{i} size" for i in impls]
    out.append("| " + " | ".join(header) + " |")
    out.append("|" + "---|" * len(header))
    for level, mode in keys:
        cells = [f"-{level}", mode]
        entries = [index.get((level, mode, i)) for i in impls]
        times = [e["seconds"] for e in entries if e]
        best = min(times) if times else None
        for e in entries:
            if not e:
                cells.append("-")
                continue
            text = fmt_time(e["seconds"])
            cells.append(f"**{text}**" if e["seconds"] == best else text)
        sizes = [e["output_bytes"] for e in entries if e]
        smallest = min(sizes) if sizes else None
        for e in entries:
            if not e:
                cells.append("-")
                continue
            text = fmt_size(e["output_bytes"], original)
            cells.append(f"**{text}**" if e["output_bytes"] == smallest else text)
        out.append("| " + " | ".join(cells) + " |")
    return "\n".join(out) + "\n"


def os_comparison(files: list[dict], level: int = 3, mode: str = "single") -> str:
    impls: list[str] = []
    for f in files:
        for i in impl_order(f["results"]):
            if i not in impls:
                impls.append(i)
    header = ["Machine", "Workload"] + impls
    out = [
        f"### Cross-OS summary (-{level}, {mode} worker)\n",
        "| " + " | ".join(header) + " |",
        "|" + "---|" * len(header),
    ]
    for f in files:
        name = f["meta"].get("label") or f["meta"]["system"]
        for workload in WORKLOAD_TITLES:
            index = {
                r["impl"]: r
                for r in f["results"]
                if r["workload"] == workload and r["level"] == level and r["mode"] == mode
            }
            if not index:
                continue
            cells = [name, workload]
            for i in impls:
                r = index.get(i)
                cells.append(fmt_time(r["seconds"]) if r else "-")
            out.append("| " + " | ".join(cells) + " |")
    return "\n".join(out) + "\n"


def main() -> None:
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    files = load(sys.argv[1:])
    parts = []
    for f in files:
        impls = impl_order(f["results"])
        parts.append(machine_heading(f["meta"]))
        for workload in WORKLOAD_TITLES:
            table = workload_table(f["results"], workload, impls)
            if table:
                parts.append(table)
    if len(files) > 1:
        parts.append(os_comparison(files))
    print("\n".join(parts))


if __name__ == "__main__":
    main()
