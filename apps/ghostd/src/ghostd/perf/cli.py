"""``ghostd perf``: the latency harness and its micro-benchmarks.

    ghostd perf run    [--scenario a,b] [--runs N] [--model m1,m2] [--endpoint current,openrouter]
                       [--thinking between_tools,adaptive] [--transport desk,webrtc]
                       [--history N] [--set turn_grace_secs=0.4] [--no-jev] [--out DIR]
    ghostd perf micro  [--only stt,tts,jev,turn] [--n N]
    ghostd perf llm    [--model ...] [--endpoint current,openrouter] [--openrouter-models a]
    ghostd perf report DIR/records.json

Comma lists in ``run`` make a matrix: every combination runs every scenario. Output goes
to ``.scratch/perf/<timestamp>/`` (report.md, records.json, latency.jsonl). Keys are read
from the environment and never printed.
"""

from __future__ import annotations

import argparse
import asyncio
import itertools
import json
import os
import sys
import time
from datetime import datetime
from pathlib import Path

from loguru import logger

REPO = Path(__file__).resolve().parents[5]
SCRATCH = REPO / ".scratch" / "perf"


def _csv(s: str | None) -> list[str]:
    return [x.strip() for x in (s or "").split(",") if x.strip()]


def main(argv: list[str]) -> int:
    sys.stdout.reconfigure(line_buffering=True)  # type: ignore[attr-defined]  # progress in logs
    ap = argparse.ArgumentParser(prog="ghostd perf", description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)

    r = sub.add_parser("run", help="scripted conversations through the real pipeline")
    r.add_argument("--scenario", default="", help="comma list (default: all)")
    r.add_argument("--runs", type=int, default=3)
    r.add_argument("--model", default="", help="comma list (default: GHOST_LLM_MODEL)")
    r.add_argument(
        "--endpoint",
        default="current",
        help="current (the model connection), a provider id, proxy, openrouter (comma list)",
    )
    r.add_argument("--thinking", default="", help="between_tools, adaptive")
    r.add_argument("--transport", default="desk", help="desk, webrtc (comma list)")
    r.add_argument("--history", type=int, default=0, help="seed N earlier turns")
    r.add_argument("--set", action="append", default=[], help="VoiceSettings field=value")
    r.add_argument("--no-jev", action="store_true", help="regex confirmation only")
    r.add_argument(
        "--ablate",
        default="none",
        help="variants to compare, comma list: none, all, or names joined by + "
        "(tts_clauses, prompt_lines, system_cache, shared_client)",
    )
    r.add_argument("--out", type=Path, default=None)
    r.add_argument("--show-text", action="store_true", help="print the dialogue (dev only)")
    r.add_argument("--log-level", default="WARNING")

    m = sub.add_parser("micro", help="component micro-benchmarks (STT, TTS, Jev, Smart Turn)")
    m.add_argument("--only", default="stt,tts,jev,turn")
    m.add_argument("--n", type=int, default=5)
    m.add_argument("--out", type=Path, default=None)

    lm = sub.add_parser("llm", help="model and endpoint TTFB with Ghost's real prompt and tools")
    lm.add_argument(
        "--model",
        default="",
        help="for a named endpoint (default: Claude Sonnet and Haiku, or the provider's own)",
    )
    lm.add_argument(
        "--endpoint", default="current", help="current, proxy, openrouter, a provider id, +sysbp"
    )
    lm.add_argument("--openrouter-models", default="", help="non-Anthropic models via OpenRouter")
    lm.add_argument("--thinking", default="between_tools")
    lm.add_argument("--n", type=int, default=4)
    lm.add_argument("--out", type=Path, default=None)

    rp = sub.add_parser("report", help="re-render a report from records.json")
    rp.add_argument("records", type=Path)

    a = ap.parse_args(argv)
    if a.cmd == "report":
        from ghostd.perf.report import render

        data = json.loads(a.records.read_text())
        print(render(data["runs"], meta=data["meta"]))
        return 0
    out = a.out or SCRATCH / datetime.now().strftime("%Y%m%d-%H%M%S")
    out.mkdir(parents=True, exist_ok=True)
    if a.cmd == "run":
        return asyncio.run(_run(a, out))
    if a.cmd == "micro":
        from ghostd.perf.micro import run_micro

        return asyncio.run(run_micro(_csv(a.only), a.n, out))
    if a.cmd == "llm":
        from ghostd.perf.llm import run_llm

        return asyncio.run(
            run_llm(
                models=_csv(a.model),
                endpoints=_csv(a.endpoint),
                openrouter_models=_csv(a.openrouter_models),
                thinking=a.thinking,
                n=a.n,
                out=out,
            )
        )
    return 2


async def _run(a: argparse.Namespace, out: Path) -> int:
    from ghost_voice import VoiceSettings

    from ghostd.perf.harness import Bench, RunConfig, endpoint_host, parse_ablations
    from ghostd.perf.report import render
    from ghostd.perf.scenarios import DEFAULT, SCENARIOS

    logger.remove()
    # diagnose=False: a traceback never prints its frames' variables, where a key may sit
    # out of the redactor's sight (ADR 0036).
    logger.add(
        sys.stderr,
        level=a.log_level.upper(),
        format="{time:HH:mm:ss.SSS} {level} {message}",
        diagnose=False,
    )
    names = _csv(a.scenario) or list(DEFAULT)
    unknown = [n for n in names if n not in SCENARIOS]
    if unknown:
        print(f"unknown scenario(s) {unknown}; known: {', '.join(SCENARIOS)}", file=sys.stderr)
        return 2
    overrides = dict(kv.split("=", 1) for kv in a.set)
    default_model = VoiceSettings().llm_model  # type: ignore[call-arg]
    configs = [
        RunConfig(
            model=model,
            endpoint=endpoint,
            thinking=thinking or None,
            transport=transport,
            history=a.history,
            jev=not a.no_jev,
            overrides=overrides,
            ablate=ablate,
        )
        for model, endpoint, thinking, transport, ablate in itertools.product(
            _csv(a.model) or [default_model],
            _csv(a.endpoint) or ["current"],
            _csv(a.thinking) or [""],
            _csv(a.transport) or ["desk"],
            parse_ablations(a.ablate),
        )
    ]
    total = len(configs) * len(names) * a.runs
    print(f"perf: {total} runs ({len(configs)} configs x {len(names)} scenarios x {a.runs})")
    print(f"perf: output in {out}")
    t = time.monotonic()
    bench = Bench(out, SCRATCH, show_text=a.show_text)
    print(f"perf: models loaded in {bench.load_secs:.1f}s")
    runs = []
    meta = {
        "date": datetime.now().strftime("%Y-%m-%d %H:%M"),
        "host": f"the workstation ({os.cpu_count()} CPUs)",  # no hostname: reports are committed
        "endpoints": ", ".join(sorted({endpoint_host(c) for c in configs})),
        "scenarios": ", ".join(names),
        "runs per scenario": a.runs,
        "settings overrides": overrides or "none",
        "history seeded": a.history,
        "confirmation": "regex only" if a.no_jev else "Jev when a key is set, else regex",
    }
    # Interleaved: every config runs a scenario before any runs it again, so a slow minute
    # at the endpoint lands on all of them alike.
    for i in range(1, a.runs + 1):
        for name in names:
            for cfg in configs:
                res = await bench.run(SCENARIOS[name], cfg, i)
                runs.append(res.to_json())
                replies = [r["reply"] for r in res.records if r["kind"] == "voice" and r["reply"]]
                flag = f"  !! {'; '.join(res.problems)}" if res.problems else ""
                print(
                    f"  [{cfg.label}] {name} #{i}: replies "
                    + (", ".join(f"{x:.2f}s" for x in replies) or "-")
                    + f" (wall {res.wall:.0f}s){flag}"
                )
                (out / "records.json").write_text(json.dumps({"meta": meta, "runs": runs}))
    meta["wall"] = f"{time.monotonic() - t:.0f}s"
    (out / "records.json").write_text(json.dumps({"meta": meta, "runs": runs}, indent=1))
    report = render(runs, meta=meta)
    (out / "report.md").write_text(report)
    print(f"\nperf: report {out / 'report.md'}")
    return 0
