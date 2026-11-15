"""Two runs of `make perf-room` side by side (docs/benchmarks/room-harness.md).

    python3 docs/benchmarks/scripts/room_compare.py BEFORE/results.jsonl AFTER/results.jsonl

One row per room and ratio, both paths and every script summed: what each run counted,
before -> after. Reads numbers only (`results.jsonl`, or `results.json`'s "results").
"""

from __future__ import annotations

import json
import sys
from collections import defaultdict
from pathlib import Path

ORDER = ["quiet", "noise", "talker", "babble", "name", "music"]


def load(path: str) -> list[dict]:
    text = Path(path).read_text()
    if path.endswith(".jsonl"):
        return [json.loads(line) for line in text.splitlines() if line.strip()]
    return json.loads(text)["results"]


def rows(results: list[dict]) -> dict[tuple, dict]:
    out: dict[tuple, dict] = defaultdict(lambda: defaultdict(float))
    for r in results:
        key = (ORDER.index(r["condition"]), -(r["snr"] or 99), r["condition"], r["snr"])
        row = out[key]
        for k in (
            "false_turns",
            "false_acts",
            "false_below",
            "false_similar",
            "empty_opens",
            "false_cuts",
            "lost",
            "lines",
            "word_edits",
            "words",
            "cuts_made",
            "cuts_wanted",
        ):
            row[k] += r[k]
        if r["script"] != "barge":
            row["sessions"] += 1
            row["never"] += r["end_secs"] is None
    return out


def cell(a: float, b: float) -> str:
    return f"{a:.0f} → {b:.0f}"


def main(before: str, after: str) -> None:
    a, b = rows(load(before)), rows(load(after))
    print(
        "| room | SNR | false turns | of them act | well below | as loud | opened for nothing "
        "| false cuts | lines lost | WER | stops and interruptions cut | never ended |"
    )
    print("|---|---|---|---|---|---|---|---|---|---|---|---|")
    total_a: dict[str, float] = defaultdict(float)
    total_b: dict[str, float] = defaultdict(float)
    for key in sorted(set(a) | set(b)):
        x, y = a.get(key, defaultdict(float)), b.get(key, defaultdict(float))
        for k, v in x.items():
            total_a[k] += v
        for k, v in y.items():
            total_b[k] += v
        snr = "-" if key[3] is None else f"{key[3]:.0f} dB"
        print(line(key[2], snr, x, y))
    print(line("**all**", "", total_a, total_b))


def line(name: str, snr: str, x: dict, y: dict) -> str:
    def wer(r: dict) -> float:
        return 100 * r["word_edits"] / max(r["words"], 1)

    return (
        f"| {name} | {snr} | {cell(x['false_turns'], y['false_turns'])} "
        f"| {cell(x['false_acts'], y['false_acts'])} "
        f"| {cell(x['false_below'], y['false_below'])} "
        f"| {cell(x['false_similar'], y['false_similar'])} "
        f"| {cell(x['empty_opens'], y['empty_opens'])} "
        f"| {cell(x['false_cuts'], y['false_cuts'])} "
        f"| {x['lost']:.0f}/{x['lines']:.0f} → {y['lost']:.0f}/{y['lines']:.0f} "
        f"| {wer(x):.0f}% → {wer(y):.0f}% "
        f"| {x['cuts_made']:.0f}/{x['cuts_wanted']:.0f} → "
        f"{y['cuts_made']:.0f}/{y['cuts_wanted']:.0f} "
        f"| {x['never']:.0f}/{x['sessions']:.0f} → {y['never']:.0f}/{y['sessions']:.0f} |"
    )


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
