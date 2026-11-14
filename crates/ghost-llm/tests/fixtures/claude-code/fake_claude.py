"""A stand-in for Claude Code in Ghost's tests (ADR 0069): the same stream-json, line for
line in the shape of the recordings beside it, driven by a scenario, so CI needs no real
program and no account.

Ghost runs it with an environment built from scratch, so the scenario is read from the one
place that environment names: $HOME/.fake-claude/scenario.json. What it was run with (argv
and the environment's names) and what reached it (turns, interrupts, the relay's answers)
are written beside it for the tests to read.

The scenario:
  {"version": "2.1.325", "help": "<path>", "status": {...},
   "turns": [[step, ...], ...]}    one list of steps per operator turn
A step: {"text": "..."} streamed a word at a time ({"pace": seconds} between words);
{"tool": name, "input": {...}} a call through the relay, its answer awaited;
{"die": true}; {"error": "authentication_failed"}; {"retry": 401} (repeated until
interrupted); {"limit": true}.
"""

import json
import os
import sys
import threading
import time
import urllib.request

HOME = os.environ.get("HOME", "/nonexistent")
DIR = os.path.join(HOME, ".fake-claude")
SCENARIO = json.load(open(os.path.join(DIR, "scenario.json")))
LOCK = threading.Lock()


def log(name, obj):
    with LOCK:
        with open(os.path.join(DIR, name), "a") as f:
            f.write(json.dumps(obj) + "\n")


def out(obj):
    with LOCK:
        sys.stdout.write(json.dumps(obj) + "\n")
        sys.stdout.flush()


args = sys.argv[1:]
log(
    "runs.jsonl",
    {
        "argv": args,
        "env": sorted(os.environ.keys()),
        "inherited": {
            k: os.environ[k] for k in os.environ if k.startswith(("ANTHROPIC", "CLAUDE"))
        },
    },
)

if args[:1] == ["--version"]:
    print(SCENARIO.get("version", "2.1.325") + " (Claude Code)")
    sys.exit(0)
if args[:1] == ["--help"]:
    sys.stdout.write(open(SCENARIO["help"]).read())
    sys.exit(0)
if "--ghost-probe-unknown" in args:
    # The real parser names the first option it doesn't know (2.1.325); a scenario's
    # "no_prompt_file" stands for a version without the file form.
    known = not SCENARIO.get("no_prompt_file")
    bad = "--ghost-probe-unknown" if known else args[0]
    sys.stderr.write(f"error: unknown option '{bad}'\n")
    sys.exit(1)
if args[:2] == ["auth", "status"]:
    st = SCENARIO.get(
        "status", {"loggedIn": True, "authMethod": "claude.ai", "subscriptionType": "pro"}
    )
    print(json.dumps(st))
    sys.exit(0 if st.get("loggedIn") else 1)


def arg(name):
    return args[args.index(name) + 1] if name in args else None


# What the prompt file was, as the program found it: its mode and its text.
prompt_path = arg("--system-prompt-file")
if prompt_path:
    log(
        "prompt.jsonl",
        {
            "path": prompt_path,
            "mode": oct(os.stat(prompt_path).st_mode & 0o777),
            "text": open(prompt_path).read(),
        },
    )

# --- the relay, as the program's app -------------------------------------------------------
apps = json.load(open(arg("--mcp-config")))["mcpServers"]
relay = apps.get("ghost")
NEXT_ID = [0]


def rpc(method, params=None, notify=False, bearer=None, origin=None):
    body = {"jsonrpc": "2.0", "method": method}
    if params is not None:
        body["params"] = params
    if not notify:
        NEXT_ID[0] += 1
        body["id"] = NEXT_ID[0]
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    headers.update(relay.get("headers", {}))
    req = urllib.request.Request(
        relay["url"], data=json.dumps(body).encode(), headers=headers, method="POST"
    )
    with urllib.request.urlopen(req, timeout=600) as r:
        text = r.read().decode()
    return json.loads(text) if text and not notify else None


TOOLS = []
if relay:
    rpc(
        "initialize",
        {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "fake", "version": "0"},
        },
    )
    rpc("notifications/initialized", notify=True)
    TOOLS = [t["name"] for t in rpc("tools/list")["result"]["tools"]]

INTERRUPTED = threading.Event()
TURNS = list(SCENARIO.get("turns", []))
QUEUE = []
COND = threading.Condition()


def ev(e):
    out({"type": "stream_event", "event": e, "parent_tool_use_id": None})


def result(error=False, status=None):
    out(
        {
            "type": "result",
            "subtype": "success" if not error else "error_during_execution",
            "is_error": error,
            "api_error_status": status,
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 9,
                "output_tokens": 5,
                "cache_read_input_tokens": 300,
                "cache_creation_input_tokens": 0,
            },
        }
    )


def run_turn(steps):
    INTERRUPTED.clear()
    out(
        {
            "type": "system",
            "subtype": "init",
            "tools": ["mcp__ghost__" + t for t in TOOLS],
            "model": arg("--model"),
        }
    )
    for step in steps:
        if "die" in step:
            os._exit(3)
        if "error" in step:
            out(
                {
                    "type": "assistant",
                    "error": step["error"],
                    "message": {
                        "model": "<synthetic>",
                        "role": "assistant",
                        "content": [
                            {
                                "type": "text",
                                "text": "Words of the program's own that Ghost never repeats",
                            }
                        ],
                    },
                }
            )
            result(error=True)
            return
        if "limit" in step:
            out({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected"}})
            continue
        if "retry" in step:
            n = 0
            while not INTERRUPTED.is_set():
                n += 1
                out(
                    {
                        "type": "system",
                        "subtype": "api_retry",
                        "attempt": n,
                        "max_retries": 10,
                        "retry_delay_ms": 500 * n,
                        "error_status": step["retry"],
                        "error": "authentication_failed" if step["retry"] == 401 else "rate_limit",
                    }
                )
                time.sleep(0.1)
            result(error=True)
            return
        ev(
            {
                "type": "message_start",
                "message": {
                    "model": "claude-haiku-5-5",
                    "id": "msg_fixture",
                    "type": "message",
                    "role": "assistant",
                    "content": [],
                    "usage": {
                        "input_tokens": 9,
                        "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 300,
                        "output_tokens": 1,
                    },
                },
            }
        )
        if "text" in step:
            ev(
                {
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": ""},
                }
            )
            words = step["text"].split(" ")
            for i, w in enumerate(words):
                if INTERRUPTED.is_set():
                    result(error=True)
                    return
                ev(
                    {
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": {"type": "text_delta", "text": (" " if i else "") + w},
                    }
                )
                time.sleep(step.get("pace", 0))
            ev({"type": "content_block_stop", "index": 0})
            ev(
                {
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn"},
                    "usage": {"output_tokens": len(words)},
                }
            )
            ev({"type": "message_stop"})
        if "tool" in step:
            calls = step["tool"] if isinstance(step["tool"], list) else [step["tool"]]
            inputs = step.get("input", {})
            for i, name in enumerate(calls):
                ev(
                    {
                        "type": "content_block_start",
                        "index": i,
                        "content_block": {
                            "type": "tool_use",
                            "id": "toolu_fixture",
                            "name": "mcp__ghost__" + name,
                            "input": {},
                        },
                    }
                )
                ev({"type": "content_block_stop", "index": i})
            ev(
                {
                    "type": "message_delta",
                    "delta": {"stop_reason": "tool_use"},
                    "usage": {"output_tokens": 40},
                }
            )
            ev({"type": "message_stop"})
            answers = [None] * len(calls)

            def call(i, name, answers=answers, inputs=inputs):
                try:
                    answers[i] = rpc("tools/call", {"name": name, "arguments": inputs})
                except Exception as e:
                    answers[i] = {"error": str(e)}
                log("results.jsonl", {"name": name, "answer": answers[i]})

            threads = [threading.Thread(target=call, args=(i, n)) for i, n in enumerate(calls)]
            for t in threads:
                t.start()
            while any(t.is_alive() for t in threads):
                if INTERRUPTED.is_set():
                    for t in threads:
                        t.join(timeout=5)
                    result(error=True)
                    return
                time.sleep(0.01)
            out(
                {
                    "type": "user",
                    "message": {
                        "role": "user",
                        "content": [
                            {
                                "type": "tool_result",
                                "tool_use_id": "toolu_fixture",
                                "content": [{"type": "text", "text": "(the relay's answer)"}],
                            }
                        ],
                    },
                }
            )
    result()


def turns():
    while True:
        with COND:
            while not QUEUE:
                COND.wait()
            item = QUEUE.pop(0)
        if item is None:
            return
        # The scenario's place is kept beside it, so a process started again goes on.
        at_path = os.path.join(DIR, "turn-index")
        at = int(open(at_path).read()) if os.path.exists(at_path) else 0
        with open(at_path, "w") as f:
            f.write(str(at + 1))
        steps = TURNS[at] if at < len(TURNS) else [{"text": "Nothing scripted."}]
        run_turn(steps)


worker = threading.Thread(target=turns, daemon=True)
worker.start()
for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("type") == "control_request":
        sub = msg["request"]["subtype"]
        log("control.jsonl", {"subtype": sub})
        if sub == "interrupt":
            INTERRUPTED.set()
        resp = (
            {
                "models": [
                    {"value": "haiku", "displayName": "Haiku"},
                    {"value": "sonnet", "displayName": "Sonnet"},
                ]
            }
            if sub == "initialize"
            else {}
        )
        out(
            {
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": msg["request_id"],
                    "response": resp,
                },
            }
        )
    elif msg.get("type") == "user":
        log("turns.jsonl", {"text": msg["message"]["content"]})
        with COND:
            QUEUE.append(msg)
            COND.notify()
with COND:
    QUEUE.append(None)
    COND.notify()
worker.join(timeout=30)
