"""Print Kokoro's ONNX graph in outline: inputs, outputs, op counts by name prefix.

uv run --project tools/voice-trial python -I tools/voice-trial/voice_trial/inspect_kokoro.py MODEL
"""

import collections
import sys

import onnx


def main(path: str) -> None:
    m = onnx.load(path, load_external_data=False)
    g = m.graph
    print("opset", [(o.domain, o.version) for o in m.opset_import])
    print("inputs", [(i.name, i.type.tensor_type.shape) for i in g.input])
    print("outputs", [o.name for o in g.output])
    prefixes = collections.Counter()
    ops = collections.Counter()
    for n in g.node:
        name = n.name or (n.output[0] if n.output else "")
        parts = name.strip("/").split("/")
        prefixes["/".join(parts[:2])] += 1
        ops[n.op_type] += 1
    for k, v in sorted(prefixes.items(), key=lambda kv: -kv[1])[:60]:
        print(f"{v:6d}  {k}")
    print(ops.most_common(40))
    if len(sys.argv) > 2:
        needle = sys.argv[2]
        for n in g.node:
            if needle in n.name:
                print(n.op_type, n.name, list(n.input), list(n.output))


if __name__ == "__main__":
    main(sys.argv[1])
