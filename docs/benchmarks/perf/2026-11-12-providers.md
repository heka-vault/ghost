# Model providers compared, 2026-11-12

`make perf-providers`: the provider check Ghost runs from Settings › Model and `ghostd llm test` (ADR 0056), with the same fixed synthetic prompts, against each provider the environment had a key for. 5 timed calls per model; first byte and first word are seconds from the call, median / worst. Keys came from the variables named, never from Ghost's vault, and are not in this file.

| provider | key from | model | role | first byte | first word | streams | tools | cache | thinking |
|---|---|---|---|---|---|---|---|---|---|
| proxy | ANTHROPIC_AUTH_TOKEN | claude-sonnet-5-5 | main | 0.52 / 0.59 | 0.52 / 0.59 | yes | yes | yes | yes |
| proxy | ANTHROPIC_AUTH_TOKEN | claude-haiku-5-5 | fast | 0.48 / 0.51 | 0.48 / 0.51 | yes | yes | yes | no |

## What it found

- proxy claude-sonnet-5-5 cache: 6071 tokens read from the cache
- proxy claude-sonnet-5-5 thinking: accepts between_tools
- proxy claude-haiku-5-5 cache: 6071 tokens read from the cache
- proxy claude-haiku-5-5 thinking: refuses between_tools; Ghost sends its requests without it (HTTP 400: "thinking.type.between_tools" is not supported for this model.)

## Not measured

- anthropic: set ANTHROPIC_API_KEY to measure it
- openrouter: set OPENROUTER_API_KEY to measure it
- openrouter-openai: set OPENROUTER_API_KEY to measure it
- openai: set OPENAI_API_KEY to measure it

## Addresses

- proxy: ANTHROPIC_BASE_URL (on the operator's network)

## Against the morning's live run

The same day's live session went through OpenRouter's Anthropic API with Haiku 5.5 in front
of Sonnet 5.5. The fast model's first byte in `latency.jsonl` was 1.1 to 2.7 s (the first
model call of each spoken turn, Ghost's full prompt and tools, mostly cached). Through the
subscription proxy above, with the check's short prompts, both models' first word came in
about half a second. The two are not the same request, so the gap is not all the provider's.
Run `make perf-providers PROVIDERS="proxy openrouter openrouter-openai anthropic"` with
`OPENROUTER_API_KEY` and `ANTHROPIC_API_KEY` set in the shell to compare like with like. The
machine was under heavy load (load average about 40) during this run; the waits are the
network's and the provider's, not the CPU's.

Haiku 5.5 refused `between_tools` through the proxy too. Its route remembers this (ADR 0056),
so the fast model's requests leave it out, and so would the main model's if Haiku were the
main model.
