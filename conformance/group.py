#!/usr/bin/env python3
"""Groups conformance findings with the same message (numbers and
addresses masked) and lists the affected cases.

    python3 group.py findings.jsonl
"""

import collections
import json
import re
import sys

rows = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
groups = collections.defaultdict(list)
for r in rows:
    key = (r["severity"], r["impl"], re.sub(r"0x[0-9a-f]+|\d+", "N", r["message"])[:150])
    groups[key].append(f"{r['case']}-{r['level']}")
for (severity, impl, message), cases in sorted(groups.items()):
    print(len(cases), severity, impl, message.replace("\n", " | "))
    print("      ", sorted(set(cases))[:8])
