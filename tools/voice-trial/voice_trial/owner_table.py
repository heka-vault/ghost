"""The owner's table from one or more `ghost-perf run` records: a plain spoken reply (the
greeting and small-talk scenarios) and a reply after one lookup (the lookup scenarios), p50
and p95 of every voice turn's reply, by configuration.

uv run --project tools/voice-trial python -I -m voice_trial.owner_table RUN_DIR [RUN_DIR ...]
"""

import json
import sys
from collections import defaultdict

PLAIN = {"greeting", "small_talk"}
LOOKUP = {"lookup", "lookup_pause"}


def pct(xs: list[float], q: float) -> float:
    xs = sorted(xs)
    if not xs:
        return float("nan")
    if q == 50:
        n = len(xs)
        return xs[n // 2] if n % 2 else (xs[n // 2 - 1] + xs[n // 2]) / 2
    return xs[min(len(xs) - 1, round(q / 100 * (len(xs) - 1)))]


def main(dirs: list[str]) -> None:
    cells: dict[tuple[str, str], list[float]] = defaultdict(list)
    for d in dirs:
        runs = json.load(open(f"{d}/records.json"))["runs"]
        for run in runs:
            kind = "plain" if run["scenario"] in PLAIN else "lookup" if run["scenario"] in LOOKUP else None
            if kind is None:
                continue
            for rec in run["records"]:
                if rec.get("kind") == "voice" and rec.get("spoke") and rec.get("reply") is not None:
                    cells[(run["config"], kind)].append(rec["reply"])
    configs = sorted({c for c, _ in cells})
    print("| config | plain p50 / p95 | after one lookup p50 / p95 |")
    print("|---|---|---|")
    for c in configs:
        p, lk = cells[(c, "plain")], cells[(c, "lookup")]
        print(
            f"| {c} | {pct(p, 50):.2f} / {pct(p, 95):.2f} (n={len(p)}) "
            f"| {pct(lk, 50):.2f} / {pct(lk, 95):.2f} (n={len(lk)}) |"
        )


if __name__ == "__main__":
    main(sys.argv[1:])
