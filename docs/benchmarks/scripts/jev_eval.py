#!/usr/bin/env python3
"""Jev vs rules on short operator turns: accuracy per intent, safe thresholds, latency.

Input: JSONL from jev_make_utterances.py (or hand-labeled real turns in the same
shape). Output: a markdown report. Keys are read from the environment
(TYPESAFE_API_KEY or OPENROUTER_API_KEY) or from --env-file, and never printed.

    uv run python docs/benchmarks/scripts/jev_eval.py .scratch/jev/synthetic.jsonl --dry-run
    uv run python docs/benchmarks/scripts/jev_eval.py .scratch/jev/synthetic.jsonl --report .scratch/jev/results.md
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import statistics
import sys
import time
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "packages" / "domain" / "src"))

from ghost_domain.confirmation import Verdict, classify_confirmation, normalize  # noqa: E402
from ghost_domain.confirmation import ConfirmationContext  # noqa: E402
from ghost_domain.jev import (  # noqa: E402
    INTENTS,
    JevClient,
    JevError,
    confirmation_questions,
    confirmation_state,
)

LABELS = list(INTENTS)

# --- the rules baseline: today's regex extended with the fixed intents -------------

_SLEEP = {
    "go to sleep", "sleep", "that's all", "that's all for now", "that's it", "thanks ghost",
    "thank you ghost", "we're done", "goodnight ghost", "that'll do", "all done", "we're good",
    "later ghost", "bye ghost", "done for now", "i'm done", "that's enough for now",
    "that's it thanks", "thanks that's all", "thanks that's everything",
}
_CANCEL = {
    "never mind", "nevermind", "forget it", "scratch that", "forget that", "skip it", "leave it",
    "never mind that", "drop it", "forget about it", "skip that one", "scrap that", "skip",
    "next one", "moving on", "ignore that", "drop that", "let it go", "forget i asked",
}
_REPEAT = {
    "say that again", "what", "sorry what", "repeat that", "come again", "say again",
    "what was that", "one more time", "pardon", "huh", "what did you say", "repeat",
    "didn't catch that", "i missed that", "again please", "can you repeat that",
    "read that again", "sorry", "hmm what", "once more",
}
_FILLERS = {"ghost", "okay", "ok", "oh", "um", "hey", "so"}


def rules_intent(text: str, pending: bool) -> str:
    norm = normalize(text)
    if not norm:
        return "noise"
    words = norm.split()
    stripped = " ".join(w for w in words if w not in _FILLERS) or norm
    if len(words) > 6:
        return "ask"
    for phrases, label in ((_SLEEP, "sleep"), (_CANCEL, "cancel"), (_REPEAT, "repeat")):
        if norm in phrases or stripped in phrases:
            return label
    if any(p in norm for p in ("go to sleep", "that's all", "thanks ghost", "thank you ghost")):
        return "sleep"
    if any(p in norm for p in ("never mind", "forget it", "scratch that", "skip")):
        return "cancel"
    if any(p in norm for p in ("say that again", "repeat", "come again", "say again")):
        return "repeat"
    v = classify_confirmation(text)
    if v is Verdict.AFFIRMATIVE:
        return "affirm" if pending else "ask"
    if v is Verdict.NEGATIVE:
        return "deny" if pending else "cancel"
    return "ask" if len(words) >= 3 else "noise"


# --- Jev -------------------------------------------------------------------------


def state_for(row: dict) -> dict:
    ctx = row["context"]
    return confirmation_state(
        row["text"],
        ConfirmationContext(
            pending_action=ctx.get("pending_action") or "", question=ctx.get("question") or ""
        ),
    )


async def run_jev(client: JevClient, rows: list[dict], concurrency: int) -> list[dict | None]:
    sem = asyncio.Semaphore(concurrency)
    qs = confirmation_questions()

    async def one(row: dict) -> dict | None:
        async with sem:
            for attempt in range(3):
                try:
                    data = await client.ask(state_for(row), qs)
                    intent = data["answers"]["intent"]
                    return {
                        "choice": intent["choice"],
                        "confidence": float(intent.get("confidence") or 0),
                        "directed": float(data["answers"]["directed"].get("noul") or 0),
                        "seconds": data["seconds"],
                        "tokens": int((data.get("usage") or {}).get("input_tokens") or 0),
                        "model": data.get("model"),
                    }
                except JevError as e:
                    err = str(e)
                    if "429" in err or "529" in err or "5" == err[-3:-2]:
                        await asyncio.sleep(0.5 * (attempt + 1))
                        continue
                    return {"error": err}
            return {"error": err}

    return await asyncio.gather(*(one(r) for r in rows))


# --- metrics ---------------------------------------------------------------------


def prf(truth: list[str], pred: list[str]) -> dict[str, tuple[float, float, int]]:
    out = {}
    for lb in LABELS:
        tp = sum(1 for t, p in zip(truth, pred) if t == lb and p == lb)
        fp = sum(1 for t, p in zip(truth, pred) if t != lb and p == lb)
        fn = sum(1 for t, p in zip(truth, pred) if t == lb and p != lb)
        prec = tp / (tp + fp) if tp + fp else 0.0
        rec = tp / (tp + fn) if tp + fn else 0.0
        out[lb] = (prec, rec, tp + fn)
    return out


def table(rows: list[list[str]], header: list[str]) -> str:
    lines = ["| " + " | ".join(header) + " |", "|" + "---|" * len(header)]
    lines += ["| " + " | ".join(r) + " |" for r in rows]
    return "\n".join(lines)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("data", type=Path)
    ap.add_argument("--dry-run", action="store_true", help="size the requests, call nothing")
    ap.add_argument("--report", type=Path, default=None)
    ap.add_argument("--env-file", type=Path, default=None, help="KEY=value file to read keys from")
    ap.add_argument("--concurrency", type=int, default=8)
    ap.add_argument("--timeout", type=float, default=5.0)
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--save", type=Path, default=None, help="write rows + raw Jev answers as JSONL")
    a = ap.parse_args()

    rows = [json.loads(line) for line in a.data.read_text().splitlines() if line.strip()]
    if a.limit:
        rows = rows[: a.limit]
    truth = [r["label"] for r in rows]
    pending = [bool(r["context"].get("pending_action")) for r in rows]

    out: list[str] = [f"# Jev vs rules on {len(rows)} utterances", ""]
    out.append(f"Source: `{a.data}`. Labels: {dict(Counter(truth))}.")
    blanks = sum(1 for r in rows if not r["text"])
    if blanks:
        out.append(f"Parakeet heard nothing for {blanks} clips (kept, labeled as intended).")
    out.append("")

    # Rules
    rule_pred = [rules_intent(r["text"], p) for r, p in zip(rows, pending)]
    acc = sum(t == p for t, p in zip(truth, rule_pred)) / len(rows)
    out.append(f"## Rules baseline: accuracy {acc:.1%}")
    out.append("")
    out.append(
        table(
            [[lb, f"{p:.2f}", f"{r:.2f}", str(n)] for lb, (p, r, n) in prf(truth, rule_pred).items()],
            ["intent", "precision", "recall", "n"],
        )
    )
    out.append("")

    if a.dry_run:
        q = confirmation_questions()
        est = statistics.mean(len(json.dumps({"state": state_for(r), "questions": q})) for r in rows) / 4
        out.append(f"## Dry run: ~{est:.0f} tokens per request, {len(rows)} requests, "
                   f"~${est * len(rows) * 0.042 / 1e6:.4f} at Jev list price.")
        print("\n".join(out))
        return

    env = dict(os.environ)
    if a.env_file and a.env_file.exists():
        for line in a.env_file.read_text().splitlines():
            if "=" in line and not line.startswith("#"):
                k, v = line.split("=", 1)
                env.setdefault(k.strip(), v.strip())
    client = JevClient.from_env("on", env, timeout=a.timeout)
    assert client is not None
    print(f"asking jev via {client.provider} ({len(rows)} requests)", file=sys.stderr)
    t0 = time.time()

    async def run_and_close():
        try:
            return await run_jev(client, rows, a.concurrency)
        finally:
            await client.aclose()

    results = asyncio.run(run_and_close())
    if a.save:
        a.save.parent.mkdir(parents=True, exist_ok=True)
        with a.save.open("w") as f:
            for r, j in zip(rows, results):
                f.write(json.dumps({**r, "jev": j}) + "\n")
    wall = time.time() - t0

    ok = [(r, j) for r, j in zip(rows, results) if j and "error" in j]
    errors = Counter(j["error"] for _, j in ok)
    good = [(t, j) for t, j in zip(truth, results) if j and "error" not in j]
    if not good:
        out.append(f"## Jev: every request failed: {dict(errors)}")
        print("\n".join(out))
        return

    jev_pred = [j["choice"] if j and "error" not in j else "error" for j in results]
    acc = sum(t == p for t, p in zip(truth, jev_pred)) / len(rows)
    lat = sorted(j["seconds"] for _, j in good)
    p50 = lat[len(lat) // 2]
    p95 = lat[int(len(lat) * 0.95) - 1]
    tokens = sum(j["tokens"] for _, j in good)
    model = Counter(j["model"] for _, j in good).most_common(1)[0][0]
    out.append(
        f"## Jev ({client.provider}, model {model}): accuracy {acc:.1%}, "
        f"{len(good)}/{len(rows)} answered, {len(errors and ok)} errors {dict(errors) if errors else ''}"
    )
    out.append("")
    out.append(f"Latency from this workstation: p50 {p50 * 1000:.0f} ms, p95 {p95 * 1000:.0f} ms, "
               f"wall {wall:.1f} s at concurrency {a.concurrency}. "
               f"Tokens {tokens} = ${tokens * 0.042 / 1e6:.4f}.")
    out.append("")
    out.append(
        table(
            [[lb, f"{p:.2f}", f"{r:.2f}", str(n)] for lb, (p, r, n) in prf(truth, jev_pred).items()],
            ["intent", "precision", "recall", "n"],
        )
    )
    out.append("")

    # Confusion for Jev
    conf: dict[str, Counter] = defaultdict(Counter)
    for t, p in zip(truth, jev_pred):
        conf[t][p] += 1
    out.append("### Jev confusion (rows = truth)")
    out.append("")
    out.append(
        table(
            [[t] + [str(conf[t][p]) for p in LABELS] for t in LABELS],
            ["truth \\ pred"] + LABELS,
        )
    )
    out.append("")

    # Threshold sweep: the lowest confidence at which there are zero false sleep/affirm,
    # and what recall remains there. Below threshold we fall back to rules.
    out.append("### Confidence threshold sweep (below threshold -> rules fallback)")
    out.append("")
    sweep = []
    for th in (0.0, 0.5, 0.6, 0.7, 0.8, 0.9, 0.95):
        pred = []
        for r, p, j, rp in zip(rows, pending, results, rule_pred):
            if j and "error" not in j and j["confidence"] >= th and j["directed"] >= 0.5:
                pred.append(j["choice"])
            else:
                pred.append(rp)
        acc_t = sum(t == q for t, q in zip(truth, pred)) / len(rows)
        false_sleep = sum(1 for t, q in zip(truth, pred) if q == "sleep" and t != "sleep")
        false_affirm = sum(1 for t, q in zip(truth, pred) if q == "affirm" and t != "affirm")
        m = prf(truth, pred)
        used = sum(1 for j in results if j and "error" not in j and j["confidence"] >= th and j["directed"] >= 0.5)
        sweep.append([
            f"{th:.2f}", f"{used / len(rows):.0%}", f"{acc_t:.1%}", str(false_sleep), str(false_affirm),
            f"{m['sleep'][1]:.2f}", f"{m['cancel'][1]:.2f}", f"{m['repeat'][1]:.2f}",
            f"{m['affirm'][1]:.2f}", f"{m['deny'][1]:.2f}",
        ])
    out.append(table(sweep, ["threshold", "jev used", "accuracy", "false sleep", "false affirm",
                             "R sleep", "R cancel", "R repeat", "R affirm", "R deny"]))
    out.append("")

    # The safety axis: what would have executed a write that the operator did not okay.
    for th in (0.8, 0.9):
        bad = [
            (r, j)
            for r, j in zip(rows, results)
            if j and "error" not in j and j["choice"] == "affirm" and r["label"] != "affirm"
            and j["confidence"] >= th and j["directed"] >= 0.5
        ]
        out.append(f"### False affirms Jev would act on at threshold {th}: {len(bad)}")
        out.append("")
        for r, j in bad:
            out.append(
                f"- `{r['text']}` (meant {r['label']}: {r['intended']}, {r['variant']}, "
                f"{'pending' if r['context'].get('pending_action') else 'nothing pending'}) "
                f"conf {j['confidence']:.2f} directed {j['directed']:.2f}"
            )
        out.append("")

    # Where Jev beat the rules and vice versa, with what Parakeet heard.
    wins = [(r, j) for r, j, rp in zip(rows, results, rule_pred)
            if j and "error" not in j and j["choice"] == r["label"] and rp != r["label"]]
    losses = [(r, j, rp) for r, j, rp in zip(rows, results, rule_pred)
              if j and "error" not in j and j["choice"] != r["label"] and rp == r["label"]]
    out.append(f"### Jev right, rules wrong: {len(wins)}")
    out.append("")
    for r, j in wins[:25]:
        out.append(f"- `{r['text']}` ({r['intended']}, {r['variant']}) -> {j['choice']} {j['confidence']:.2f}")
    out.append("")
    out.append(f"### Rules right, Jev wrong: {len(losses)}")
    out.append("")
    for r, j, rp in losses[:25]:
        out.append(f"- `{r['text']}` ({r['intended']}, {r['variant']}) -> jev {j['choice']} {j['confidence']:.2f}, rules {rp}")
    out.append("")

    report = "\n".join(out)
    print(report)
    if a.report:
        a.report.parent.mkdir(parents=True, exist_ok=True)
        a.report.write_text(report + "\n")
        print(f"\nwrote {a.report}", file=sys.stderr)


if __name__ == "__main__":
    main()
