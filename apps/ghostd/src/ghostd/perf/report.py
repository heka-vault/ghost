"""Markdown report from harness runs: where each turn's time went, per scenario and
stage, the model calls and the prompt cache, the tools, and the biggest contributors.
Numbers only; the harness never writes what was said."""

from __future__ import annotations

import statistics
from collections import defaultdict
from collections.abc import Iterable, Sequence
from typing import Any

from ghost_voice.latency import cache_share, percentile, stage_values

STAGE_ORDER = ("hear", "stt", "pre", "turn", "hold", "llm", "llm_ttfb", "tools", "tts", "other")


def _f(x: float | None, nd: int = 2) -> str:
    return "-" if x is None else f"{x:.{nd}f}"


def _pct(x: float | None) -> str:
    return "-" if x is None else f"{x:.0%}"


def _p(vals: Sequence[float], q: float) -> float | None:
    return percentile([v for v in vals if v is not None], q)


def _mean(vals: Iterable[float | None]) -> float | None:
    xs = [v for v in vals if v is not None]
    return statistics.fmean(xs) if xs else None


def voice_turns(runs: Sequence[dict[str, Any]]) -> list[dict[str, Any]]:
    return [r for run in runs for r in run["records"] if r.get("kind") == "voice" and r["spoke"]]


def calls_of(records: Iterable[dict[str, Any]]) -> list[dict[str, Any]]:
    return [c for r in records for c in ((r.get("llm") or {}).get("calls") or [])]


def render(
    runs: Sequence[dict[str, Any]],
    *,
    meta: dict[str, Any],
) -> str:
    by_config: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for run in runs:
        by_config[run["config"]].append(run)
    out: list[str] = [f"# Ghost latency harness, {meta.get('date', '')}", ""]
    out += [f"- {k}: {v}" for k, v in meta.items() if k != "date"]
    out.append("")
    if len(by_config) > 1:
        out += _comparison(by_config)
    for config, cruns in by_config.items():
        out += _config_section(config, cruns)
    return "\n".join(out) + "\n"


def narration(calls: Sequence[dict[str, Any]]) -> tuple[int, int]:
    """Tool-calling model calls that wrote text first, out of all tool-calling calls
    (hang_up and hold excepted: their text is spoken)."""
    tool_calls = [c for c in calls if c.get("tools") and c["tools"] not in (["hang_up"], ["hold"])]
    return sum(1 for c in tool_calls if c.get("text")), len(tool_calls)


def first_call_cache(runs: Sequence[dict[str, Any]]) -> list[tuple[int, int]]:
    """(cache read, cache write) of each session's first model call."""
    out = []
    for run in runs:
        for r in run["records"]:
            calls = (r.get("llm") or {}).get("calls") or []
            if calls and calls[0].get("in") is not None:
                out.append((calls[0]["cache_read"], calls[0]["cache_write"]))
                break
    return out


def _comparison(by_config: dict[str, list[dict[str, Any]]]) -> list[str]:
    out = [
        "## Configurations compared (voice turns)",
        "",
        "| config | turns | reply p50 | reply p95 | llm p50 | llm ttfb p50 | ttfb p95 "
        "| tts synth p50 | tts p95 | calls/turn | narrated | cache | 1st call read "
        "| wake to voice p50 | problems |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for config, cruns in by_config.items():
        turns = voice_turns(cruns)
        st = [stage_values(r) for r in turns]
        calls = calls_of(turns)
        ttfbs = [c["ttfb"] for c in calls if c.get("ttfb") is not None]
        problems = sum(1 for r in cruns if r["problems"])
        said, tool_calls = narration(calls)
        first = first_call_cache(cruns)
        reads = sum(1 for read, _ in first if read > 0)
        out.append(
            f"| {config} | {len(turns)} | {_f(_p([s['reply'] for s in st], 50))} "
            f"| {_f(_p([s['reply'] for s in st], 95))} | {_f(_p([s['llm'] for s in st], 50))} "
            f"| {_f(_p(ttfbs, 50))} | {_f(_p(ttfbs, 95))} "
            f"| {_f(_p([s['tts_synth'] for s in st], 50))} "
            f"| {_f(_p([s['tts_synth'] for s in st], 95))} "
            f"| {_f(len(calls) / len(turns) if turns else None, 1)} "
            f"| {said}/{tool_calls} | {_pct(cache_share(calls))} | {reads}/{len(first)} "
            f"| {_f(_p([r['wake_to_voice'] for r in cruns], 50))} "
            f"| {problems}/{len(cruns)} |"
        )
    out += ["", *_scenario_matrix(by_config)]
    return out + [""]


def _scenario_matrix(by_config: dict[str, list[dict[str, Any]]]) -> list[str]:
    """Per scenario, each config's reply p50/p95, model time p50 and TTS synth p50."""
    configs = list(by_config)
    out = [
        "### Per scenario: reply p50 / p95, model p50, TTS synth p50 (seconds)",
        "",
        "| scenario | " + " | ".join(configs) + " |",
        "|---|" + "---|" * len(configs),
    ]
    scenarios: list[str] = []
    for cruns in by_config.values():
        for r in cruns:
            if r["scenario"] not in scenarios:
                scenarios.append(r["scenario"])
    for name in scenarios:
        cells = []
        for config in configs:
            turns = voice_turns([r for r in by_config[config] if r["scenario"] == name])
            st = [stage_values(t) for t in turns]
            if not st:
                cells.append("-")
                continue
            cells.append(
                f"{_f(_p([s['reply'] for s in st], 50))} / {_f(_p([s['reply'] for s in st], 95))}"
                f", llm {_f(_p([s['llm'] for s in st], 50))}"
                f", tts {_f(_p([s['tts_synth'] for s in st], 50))}"
            )
        out.append(f"| {name} | " + " | ".join(cells) + " |")
    return out


def _config_section(config: str, runs: list[dict[str, Any]]) -> list[str]:
    out = [f"## {config}", ""]
    turns = voice_turns(runs)
    out += _contributors(turns)
    out += _per_scenario(runs)
    out += _stages(turns)
    out += _llm(runs)
    out += _cache_sequence(runs)
    out += _tools(turns)
    out += _confirm_and_stt(runs)
    out += _opening(runs)
    problems = [(r["scenario"], r["run"], p) for r in runs for p in r["problems"]]
    if problems:
        out += ["### Problems (model drift or failures)", ""]
        out += [f"- {s} run {i}: {p}" for s, i, p in problems]
        out.append("")
    return out


def _contributors(turns: list[dict[str, Any]]) -> list[str]:
    if not turns:
        return ["No voice turns.", ""]
    st = [stage_values(r) for r in turns]
    reply = _mean(s["reply"] for s in st) or 0.0
    parts = {
        "turn hold (grace)": _mean(s["hold"] for s in st),
        "LLM (all calls on the path)": _mean(s["llm"] for s in st),
        "Smart Turn wait": _mean(s["turn"] for s in st),
        "TTS to first audio + output": _mean(s["tts"] for s in st),
        "VAD stop silence": _mean((r.get("hear") or {}).get("vad") for r in turns),
        "STT": _mean(s["stt"] for s in st),
        "tools beyond the model": _mean(s["tools"] for s in st),
        "pre-LLM processors (interceptors, Jev)": _mean(s["pre"] for s in st),
        "pipeline hops (other)": _mean(s["other"] for s in st),
    }
    ranked = sorted(((v or 0.0, k) for k, v in parts.items()), reverse=True)
    out = [
        f"### Where a voice turn's time goes ({len(turns)} turns, mean reply {reply:.2f}s)",
        "",
        "| contributor | mean s | share |",
        "|---|---|---|",
    ]
    for v, k in ranked:
        out.append(f"| {k} | {v:.2f} | {v / reply:.0%} |" if reply else f"| {k} | {v:.2f} | - |")
    return out + [""]


def _per_scenario(runs: list[dict[str, Any]]) -> list[str]:
    out = [
        "### End to end per scenario (operator stops talking -> Ghost's voice starts)",
        "",
        "reply: from the VAD's end of speech to BotStartedSpeaking, as the live log reports it. "
        "audible: from the line's last audio frame to Ghost's voice loud at the scripted "
        "speaker or the WebRTC peer (device or network buffering included; the thinking "
        "sound can count when it is loud).",
        "",
        "| scenario | runs ok | turns | reply p50 | reply p95 | audible p50 | hear p50 | "
        "llm p50 | llm ttfb p50 | tools p50 | tts p50 | other p50 |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    by: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for run in runs:
        by[run["scenario"]].append(run)
    for name, sruns in by.items():
        turns = voice_turns(sruns)
        st = [stage_values(r) for r in turns]
        ok = sum(1 for r in sruns if not r["problems"])
        audible = [a for r in sruns for a in (r.get("audible") or []) if a is not None]

        def col(k: str, q: float = 50, st: list[dict[str, Any]] = st) -> str:
            return _f(_p([s[k] for s in st], q))

        out.append(
            f"| {name} | {ok}/{len(sruns)} | {len(turns)} | {col('reply')} | {col('reply', 95)} "
            f"| {_f(_p(audible, 50))} | {col('hear')} | {col('llm')} | {col('llm_ttfb')} "
            f"| {col('tools')} | {col('tts')} | {col('other')} |"
        )
    return out + [""]


def _stages(turns: list[dict[str, Any]]) -> list[str]:
    out = [
        "### Stages across voice turns",
        "",
        "| stage | n | p50 | p95 | max |",
        "|---|---|---|---|---|",
    ]
    st = [stage_values(r) for r in turns]
    for k in ("reply", *STAGE_ORDER, "tts_synth"):
        vals = [s[k] for s in st if s.get(k) is not None]
        if not vals:
            continue
        out.append(
            f"| {k} | {len(vals)} | {_f(_p(vals, 50))} | {_f(_p(vals, 95))} | {_f(max(vals))} |"
        )
    segs = [(r.get("hear") or {}).get("segments") for r in turns]
    kept = sum((r.get("hear") or {}).get("kept_open") or 0 for r in turns)
    smart = [e for r in turns for e in ((r.get("hear") or {}).get("smart_turn") or [])]
    incomplete = sum(1 for e in smart if not e.get("complete"))
    out.append("")
    out.append(
        f"Turns split by a VAD pause: {sum(1 for s in segs if (s or 0) > 1)}/{len(turns)}; "
        f"kept open by the hold: {kept}; Smart Turn said incomplete {incomplete}/{len(smart)} "
        f"times (each costs up to its 3 s silence timeout)."
    )
    return out + [""]


def _llm(runs: list[dict[str, Any]]) -> list[str]:
    out = [
        "### Model calls",
        "",
        "| scenario | calls | calls/turn | ttfb p50 | ttfb p95 | dur p50 | dur p95 | "
        "first tool p50 | in (uncached) | cache read | cache write | out | cache hit |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    by: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for run in runs:
        by[run["scenario"]].append(run)
    for name, sruns in by.items():
        records = [r for run in sruns for r in run["records"]]
        calls = calls_of(records)
        if not calls:
            continue
        turns = [r for r in records if (r.get("llm") or {}).get("calls")]

        def p(key: str, q: float, calls: list[dict[str, Any]] = calls) -> str:
            return _f(_p([c[key] for c in calls if c.get(key) is not None], q))

        def m(key: str, calls: list[dict[str, Any]] = calls) -> str:
            v = _mean(c.get(key) for c in calls)
            return "-" if v is None else f"{v:.0f}"

        out.append(
            f"| {name} | {len(calls)} | {len(calls) / len(turns):.1f} | {p('ttfb', 50)} "
            f"| {p('ttfb', 95)} | {p('dur', 50)} | {p('dur', 95)} | {p('first_tool', 50)} "
            f"| {m('in')} | {m('cache_read')} | {m('cache_write')} | {m('out')} "
            f"| {_pct(cache_share(calls))} |"
        )
    return out + [""]


def _cache_sequence(runs: list[dict[str, Any]]) -> list[str]:
    """Token use call by call in the first run of each multi-call scenario: whether each
    call reads what the previous one wrote."""
    out = ["### Prompt cache, call by call (first run of each scenario)", ""]
    shown = set()
    for run in runs:
        if run["scenario"] in shown:
            continue
        rows = []
        for r in run["records"]:
            for i, c in enumerate((r.get("llm") or {}).get("calls") or []):
                if c.get("in") is None:
                    continue
                rows.append(
                    f"{r['kind']} {r['turn']}.{i + 1}: in {c['in']}, read {c['cache_read']}, "
                    f"write {c['cache_write']}, ttfb {_f(c.get('ttfb'))}"
                )
        if len(rows) < 2:
            continue
        shown.add(run["scenario"])
        out.append(f"- **{run['scenario']}**: " + "; ".join(rows))
    if len(out) == 2:
        return []
    return out + [""]


def _tools(turns: list[dict[str, Any]]) -> list[str]:
    calls = [t for r in turns for t in ((r.get("tools") or {}).get("calls") or [])]
    if not calls:
        return []
    out = [
        "### Tools (function call start -> result; read tools call the GitHub client)",
        "",
        "| tool | n | p50 | p95 | steps p50 |",
        "|---|---|---|---|---|",
    ]
    by: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for t in calls:
        by[t.get("op") or t["name"]].append(t)
    for name, ts in sorted(by.items()):
        steps: dict[str, list[float]] = defaultdict(list)
        for t in ts:
            for k, v in (t.get("steps") or {}).items():
                steps[k].append(v)
        step_txt = ", ".join(f"{k} {_f(_p(v, 50), 3)}" for k, v in steps.items()) or "-"
        secs = [t["secs"] for t in ts if t.get("secs") is not None]
        out.append(
            f"| {name} | {len(ts)} | {_f(_p(secs, 50), 3)} | {_f(_p(secs, 95), 3)} | {step_txt} |"
        )
    return out + [""]


def _confirm_and_stt(runs: list[dict[str, Any]]) -> list[str]:
    out = []
    confirms = [r["confirm"] for run in runs for r in run["records"] if r.get("confirm")]
    if confirms:
        by: dict[str, list[float]] = defaultdict(list)
        for c in confirms:
            by[c["classifier"]].append(c["classify"])
        parts = [f"{k} x{len(v)} classify p50 {_f(_p(v, 50), 3)}s" for k, v in by.items()]
        total = [c["total"] for c in confirms]
        out += [
            "### Confirmation",
            "",
            f"{len(confirms)} spoken answers: "
            + "; ".join(parts)
            + f"; whole step incl. the write p50 {_f(_p(total, 50), 3)}s.",
            "",
        ]
    heard = [h for run in runs for h in run.get("heard") or [] if h is not None]
    if heard:
        exact = sum(1 for h in heard if h >= 0.999)
        out += [
            "### STT accuracy on the scripted lines",
            "",
            f"{exact}/{len(heard)} lines word-perfect; mean word match {_mean(heard):.0%}.",
            "",
        ]
    return out


def _opening(runs: list[dict[str, Any]]) -> list[str]:
    openings = [r for run in runs for r in run["records"] if r.get("kind") == "opening"]
    if not openings:
        return []
    builds = [r.get("build") for r in openings if r.get("build") is not None]
    replies = [r.get("reply") for r in openings if r.get("reply") is not None]
    wake = [run["wake_to_voice"] for run in runs if run.get("wake_to_voice") is not None]
    return [
        "### Opening (session build -> greeting voice)",
        "",
        f"build p50 {_f(_p(builds, 50))}s, pipeline start -> greeting voice p50 "
        f"{_f(_p(replies, 50))}s (p95 {_f(_p(replies, 95))}), together p50 {_f(_p(wake, 50))}s "
        "(the desk also plays the chime and ducks music before this).",
        "",
    ]
