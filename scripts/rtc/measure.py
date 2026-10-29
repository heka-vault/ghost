"""Measure one media stack on loopback: build ``rtc-probe`` with the stack's feature, run
a call from the aiortc phone (``phone.py``) against it, and print the numbers the README
tables are made of.

    .venv/bin/python -I scripts/rtc/measure.py --backend str0m --seconds 300 --restart-at 150

One-way latency in each direction (the bursts' send times on one side paired with their
receive times on the other, both wall clock, both on this machine), packet loss and jitter
as each side counts them, the stack's CPU as a share of one core (``/proc/<pid>/schedstat``),
the ``restart_pc`` mid-call, and the fingerprint check (the ``a=fingerprint`` the phone
read in each signed-less answer equals what ``Media::fingerprint()`` reported). The raw
reports are written beside the summary. Run from the repository root.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PHONE = Path(__file__).resolve().parent / "phone.py"


def cpu_seconds(pid: int) -> float:
    with open(f"/proc/{pid}/schedstat", encoding="ascii") as f:
        return int(f.read().split()[0]) / 1e9


def pair(sent: list[float], received: list[float]) -> list[float]:
    """Each receive paired with the latest send before it within 0.9 s."""
    out = []
    j = 0
    for r in received:
        while j + 1 < len(sent) and sent[j + 1] <= r:
            j += 1
        if j < len(sent) and sent[j] <= r and r - sent[j] < 0.9:
            out.append(r - sent[j])
    return out


def pct(values: list[float], p: float) -> float:
    if not values:
        return float("nan")
    s = sorted(values)
    k = min(len(s) - 1, int(round(p / 100 * (len(s) - 1))))
    return s[k]


def summary(values: list[float]) -> dict[str, float]:
    ms = [v * 1000 for v in values]
    return {
        "count": len(ms),
        "p50_ms": pct(ms, 50),
        "p95_ms": pct(ms, 95),
        "max_ms": max(ms) if ms else float("nan"),
        "mean_ms": sum(ms) / len(ms) if ms else float("nan"),
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--backend", required=True, choices=["str0m", "webrtc", "libwebrtc"])
    ap.add_argument("--seconds", type=float, default=300.0)
    ap.add_argument("--restart-at", type=float, default=150.0)
    ap.add_argument("--out", default=str(ROOT / "crates/ghost-rtc/measurements"))
    ap.add_argument("--no-build", action="store_true")
    args = ap.parse_args()
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y-%m-%d")
    tag = f"{stamp}-{args.backend}"

    if not args.no_build:
        subprocess.run(
            ["cargo", "build", "--release", "-p", "ghost-rtc", "--example", "rtc-probe",
             "--features", args.backend],
            cwd=ROOT, check=True,
        )
    probe_report = out_dir / f"{tag}-desk.json"
    phone_report = out_dir / f"{tag}-phone.json"
    probe = subprocess.Popen(
        [str(ROOT / "target/release/examples/rtc-probe"), "--listen", "127.0.0.1:0",
         "--report", str(probe_report)],
        cwd=ROOT, stdout=subprocess.PIPE, text=True,
    )
    assert probe.stdout is not None
    line = probe.stdout.readline()
    if "listening on" not in line:
        probe.kill()
        print(f"probe did not start: {line!r}", file=sys.stderr)
        return 1
    url = line.split("listening on ")[1].split()[0]
    print(f"measure: {args.backend} at {url}, {args.seconds:.0f} s, restart at {args.restart_at:.0f} s", file=sys.stderr)
    probe_cpu0 = cpu_seconds(probe.pid)
    t0 = time.time()
    phone = subprocess.Popen(
        [sys.executable, "-I", str(PHONE), "--url", url, "--seconds", str(args.seconds),
         "--restart-at", str(args.restart_at), "--report", str(phone_report)],
        cwd=ROOT,
    )
    # Sample the desk's CPU while the call is still up: just before the phone's end.
    probe_cpu = None
    while phone.poll() is None:
        time.sleep(0.5)
        try:
            probe_cpu = cpu_seconds(probe.pid) - probe_cpu0
        except FileNotFoundError:
            break
    wall = time.time() - t0
    phone_rc = phone.wait()
    try:
        probe.wait(timeout=20)
    except subprocess.TimeoutExpired:
        probe.kill()
    desk = json.loads(probe_report.read_text())
    ph = json.loads(phone_report.read_text())

    up = pair(ph["sent"], desk["received"])
    down = pair(desk["sent"], ph["received"])
    desk_stats = desk["stats"]
    ph_in = ph["stats"].get("inbound", {})
    ph_remote = ph["stats"].get("remote_inbound", {})
    answers_phone = ph["answers"]
    answers_desk = desk.get("answers", [])
    fingerprints_match = len(answers_phone) == len(answers_desk) and all(
        a["fingerprint"] == b["fingerprint"] and a["sha256"] for a, b in zip(answers_phone, answers_desk)
    )
    restart_ok = (
        args.restart_at > 0
        and len(answers_phone) == 2
        and answers_phone[1]["restart"]
        and len(ph["bot_ready"]) == 2
        and sum(1 for t in desk["received"] if t > answers_phone[1]["at"] + 0.5) > 0
        and sum(1 for t in ph["received"] if t > answers_phone[1]["at"] + 0.5) > 0
    )
    result = {
        "backend": args.backend,
        "date": stamp,
        "seconds": wall,
        "uplink_phone_to_desk": summary(up),
        "downlink_desk_to_phone": summary(down),
        "bursts_sent_by_phone": len(ph["sent"]),
        "bursts_heard_by_desk": len(desk["received"]),
        "bursts_sent_by_desk": len(desk["sent"]),
        "bursts_heard_by_phone": len(ph["received"]),
        "desk": {
            "packets_received": desk_stats["packets_received"],
            "packets_lost": desk_stats["packets_lost"],
            "concealed": desk_stats["concealed"],
            "frames_dropped": desk_stats["frames_dropped"],
            "jitter_ms": desk_stats["jitter_ms"],
            "packets_sent": desk_stats["packets_sent"],
            "cpu_percent_of_one_core": (probe_cpu / wall * 100) if probe_cpu is not None and wall else None,
            "cpu_percent_self_reported": desk.get("cpu_percent"),
        },
        "phone": {
            "packets_received": ph_in.get("packets_received"),
            "packets_lost": ph_in.get("packets_lost"),
            "jitter_ms": (ph_in.get("jitter") or 0) * 1000 if ph_in else None,
            "remote_inbound_packets_lost": ph_remote.get("packets_lost"),
            "remote_round_trip_ms": (ph_remote.get("round_trip_time") or 0) * 1000 if ph_remote else None,
            "bot_ready": len(ph["bot_ready"]),
            "server_messages": ph["server_messages"],
            "other_messages": ph["other_messages"],
            "errors": ph["errors"],
            "exit": phone_rc,
        },
        "answers": [
            {"restart": a["restart"], "took_ms": a["took"] * 1000, "fingerprint": a["fingerprint"],
             "setup": a["setup"], "candidates": a["candidates"], "ice_lite": a["ice_lite"]}
            for a in answers_phone
        ],
        "restart_ok": restart_ok,
        "fingerprints_match": fingerprints_match,
        "links": desk["links"],
    }
    (out_dir / f"{tag}.json").write_text(json.dumps(result, indent=2))
    u, d = result["uplink_phone_to_desk"], result["downlink_desk_to_phone"]
    print(f"\n{args.backend}: {wall:.0f} s on loopback")
    print(f"  uplink   phone -> desk: p50 {u['p50_ms']:.1f} ms, p95 {u['p95_ms']:.1f} ms, max {u['max_ms']:.1f} ms ({u['count']} bursts)")
    print(f"  downlink desk -> phone: p50 {d['p50_ms']:.1f} ms, p95 {d['p95_ms']:.1f} ms, max {d['max_ms']:.1f} ms ({d['count']} bursts)")
    print(f"  desk: {desk_stats['packets_received']} packets in, {desk_stats['packets_lost']} lost, {desk_stats['concealed']} concealed, jitter {desk_stats['jitter_ms']:.2f} ms, {desk_stats['packets_sent']} out, {desk_stats['frames_dropped']} frames dropped")
    print(f"  phone: {ph_in.get('packets_received')} packets in, {ph_in.get('packets_lost')} lost, jitter {result['phone']['jitter_ms']:.2f} ms; remote-inbound lost {ph_remote.get('packets_lost')}")
    cpu = result["desk"]["cpu_percent_of_one_core"]
    print(f"  desk CPU: {cpu:.1f}% of one core" if cpu is not None else "  desk CPU: n/a")
    print(f"  restart_pc: {'ok' if restart_ok else 'FAILED'}; fingerprints: {'match' if fingerprints_match else 'MISMATCH'}; answers {[round(a['took_ms']) for a in result['answers']]} ms; phone errors {ph['errors']}")
    return 0 if phone_rc == 0 and fingerprints_match else 1


if __name__ == "__main__":
    sys.exit(main())
