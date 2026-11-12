"""The tensors that cross from Kokoro's encoder into its decoder, with their producers.

uv run --project tools/voice-trial python -I tools/voice-trial/voice_trial/boundary.py MODEL
"""

import sys

import onnx


def main(path: str) -> None:
    m = onnx.load(path, load_external_data=False)
    g = m.graph
    producer = {}
    for n in g.node:
        for o in n.output:
            producer[o] = n
    inits = {i.name for i in g.initializer}
    inputs = {i.name for i in g.input}

    def in_decoder(n) -> bool:
        return n.name.startswith("/decoder/")

    crossing = {}
    for n in g.node:
        if not in_decoder(n):
            continue
        for t in n.input:
            if not t or t in inits:
                continue
            p = producer.get(t)
            if t in inputs:
                crossing.setdefault(t, ("<graph input>", []))[1].append(n.name)
            elif p is not None and not in_decoder(p):
                crossing.setdefault(t, (f"{p.op_type} {p.name}", []))[1].append(n.name)
    for t, (p, users) in crossing.items():
        print(t, "<-", p, "->", users[:4], len(users))
    # Nodes outside both encoder and decoder
    for n in g.node:
        if not n.name.startswith("/encoder/") and not in_decoder(n):
            print("other:", n.op_type, n.name, list(n.input), list(n.output))


if __name__ == "__main__":
    main(sys.argv[1])
