"""Time to first text token via an Anthropic-compatible endpoint (streaming).

Usage: ANTHROPIC_BASE_URL=... ANTHROPIC_AUTH_TOKEN=... python -I llm_ttft.py <model> [runs] [thinking: disabled|adaptive]
"""
import json
import os
import statistics
import sys
import time
import urllib.request

model, runs = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 5
thinking = sys.argv[3] if len(sys.argv) > 3 else "disabled"
base, key = os.environ["ANTHROPIC_BASE_URL"].rstrip("/"), os.environ["ANTHROPIC_AUTH_TOKEN"]
body = json.dumps({
    "model": model, "max_tokens": 400, "stream": True, "thinking": {"type": thinking},
    "system": "You are Ghost, a voice assistant for GitHub issue triage. Answer in one or two short spoken sentences.",
    "messages": [{"role": "user", "content": "What needs my sign-off before anyone can begin development?"}],
}).encode()
ttfts, totals = [], []
for _ in range(runs):
    req = urllib.request.Request(f"{base}/v1/messages", data=body, method="POST", headers={
        "content-type": "application/json", "x-api-key": key, "authorization": f"Bearer {key}",
        "anthropic-version": "2023-06-01"})
    t0 = time.perf_counter(); first = None
    with urllib.request.urlopen(req, timeout=60) as r:
        for line in r:
            if first is None and line.startswith(b"data:") and b'"text_delta"' in line:
                first = time.perf_counter() - t0
    ttfts.append(first); totals.append(time.perf_counter() - t0)
print(json.dumps({"model": model, "thinking": thinking, "runs": runs, "ttft_p50": round(statistics.median(ttfts), 3),
                  "ttft_min": round(min(ttfts), 3), "ttft_max": round(max(ttfts), 3),
                  "total_p50": round(statistics.median(totals), 3)}))
