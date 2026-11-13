"""The new detector: the shipped classifier kept whole, and a small head beside it on the same
frozen features, trained here.

    score = sigmoid(a * logit(shipped(x)) + head(x) + c)
    head(x) = w2 . relu(W1 . flatten(x) + b1)

``x`` is the ``[16, 96]`` stack of embeddings the daemon already computes. Keeping the
shipped model's score as an input means the head learns a correction for the operator's
voice and capture path on top of what the first training learned from thousands of voices,
which a few dozen takes could not relearn. The head is about 50,000 numbers and trains in
seconds on a CPU. ``export`` writes one ONNX graph with the daemon's input and output
(``embeddings`` -> ``score``), so ``ghost_infer::wake::WakeScorer`` loads it unchanged.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

import numpy as np

EPS = 1e-4
D_IN = 16 * 96


def logit(p: np.ndarray) -> np.ndarray:
    p = np.clip(p, EPS, 1 - EPS)
    return np.log(p / (1 - p))


@dataclass
class Head:
    W1: np.ndarray
    b1: np.ndarray
    w2: np.ndarray
    a: float
    c: float
    mean: np.ndarray  # input normalisation, folded into the graph
    std: np.ndarray

    def logits(self, x: np.ndarray, old: np.ndarray) -> np.ndarray:
        z = (x.reshape(len(x), -1) - self.mean) / self.std
        h = np.maximum(z @ self.W1 + self.b1, 0)
        return self.a * logit(old) + h @ self.w2 + self.c

    def __call__(self, x: np.ndarray, old: np.ndarray) -> np.ndarray:
        return 1 / (1 + np.exp(-self.logits(x, old)))


def train(
    x: np.ndarray,
    old: np.ndarray,
    y: np.ndarray,
    weight: np.ndarray,
    *,
    hidden: int = 32,
    epochs: int = 30,
    batch: int = 256,
    lr: float = 2e-3,
    l2: float = 1e-3,
    seed: int = 0,
    log=None,
) -> Head:
    """Weighted binary cross-entropy with Adam. ``old`` is the shipped model's score for
    each window; ``weight`` balances positives and negatives."""
    rng = np.random.default_rng(seed)
    flat = x.reshape(len(x), -1).astype(np.float32)
    mean = flat.mean(0)
    std = flat.std(0) + 1e-3
    z = (flat - mean) / std
    lo = logit(old).astype(np.float32)
    p = {
        "W1": (rng.standard_normal((D_IN, hidden)) * np.sqrt(2 / D_IN)).astype(np.float32),
        "b1": np.zeros(hidden, np.float32),
        "w2": (rng.standard_normal(hidden) * 0.01).astype(np.float32),
        "a": np.array(1.0, np.float32),
        "c": np.array(0.0, np.float32),
    }
    m = {k: np.zeros_like(v) for k, v in p.items()}
    v2 = {k: np.zeros_like(v) for k, v in p.items()}
    b1_, b2_ = 0.9, 0.999
    step = 0
    n = len(z)
    for epoch in range(epochs):
        if log and epoch % 10 == 0:
            log(f"    epoch {epoch}/{epochs}")
        order = rng.permutation(n)
        for i in range(0, n, batch):
            idx = order[i : i + batch]
            zb, lb, yb, wb = z[idx], lo[idx], y[idx], weight[idx]
            pre = zb @ p["W1"] + p["b1"]
            h = np.maximum(pre, 0)
            out = p["a"] * lb + h @ p["w2"] + p["c"]
            prob = 1 / (1 + np.exp(-out))
            d = (prob - yb) * wb / wb.sum()
            g = {
                "w2": h.T @ d + l2 * p["w2"],
                "a": np.array(np.sum(d * lb), np.float32),
                "c": np.array(np.sum(d), np.float32),
            }
            dh = np.outer(d, p["w2"]) * (pre > 0)
            g["W1"] = zb.T @ dh + l2 * p["W1"]
            g["b1"] = dh.sum(0)
            step += 1
            for k in p:
                m[k] = b1_ * m[k] + (1 - b1_) * g[k]
                v2[k] = b2_ * v2[k] + (1 - b2_) * g[k] ** 2
                mh = m[k] / (1 - b1_**step)
                vh = v2[k] / (1 - b2_**step)
                p[k] = (p[k] - lr * mh / (np.sqrt(vh) + 1e-8)).astype(np.float32)
    return Head(p["W1"], p["b1"], p["w2"], float(p["a"]), float(p["c"]), mean, std)


def export(head: Head, shipped: Path, out: Path) -> None:
    """One graph: the shipped classifier's nodes, renamed, and the head beside them."""
    import onnx
    from onnx import TensorProto, helper, numpy_helper

    old = onnx.load(str(shipped))
    g = old.graph
    inp = g.input[0].name
    old_out = g.output[0].name
    pre = "shipped_"

    def rn(name: str) -> str:
        return name if name in ("", inp) else pre + name

    nodes = []
    for nd in g.node:
        c = onnx.NodeProto()
        c.CopyFrom(nd)
        c.name = pre + nd.name
        del c.input[:]
        c.input.extend(rn(i) for i in nd.input)
        del c.output[:]
        c.output.extend(rn(o) for o in nd.output)
        nodes.append(c)
    inits = []
    for t in g.initializer:
        c = onnx.TensorProto()
        c.CopyFrom(t)
        c.name = pre + t.name
        inits.append(c)

    def const(name: str, a: np.ndarray) -> None:
        inits.append(numpy_helper.from_array(np.asarray(a, np.float32), name))

    const("h_mean", head.mean.reshape(1, -1))
    const("h_std", head.std.reshape(1, -1))
    const("h_W1", head.W1)
    const("h_b1", head.b1)
    const("h_w2", head.w2.reshape(-1, 1))
    const("h_a", np.array([head.a]))
    const("h_c", np.array([head.c]))
    const("h_eps", np.array([EPS]))
    const("h_1meps", np.array([1 - EPS]))
    const("h_one", np.array([1.0]))
    inits.append(numpy_helper.from_array(np.array([0, -1], np.int64), "h_shape"))
    H = helper.make_node
    nodes += [
        H("Reshape", [inp, "h_shape"], ["h_flat"]),
        H("Sub", ["h_flat", "h_mean"], ["h_c0"]),
        H("Div", ["h_c0", "h_std"], ["h_z"]),
        H("MatMul", ["h_z", "h_W1"], ["h_pre0"]),
        H("Add", ["h_pre0", "h_b1"], ["h_pre"]),
        H("Relu", ["h_pre"], ["h_h"]),
        H("MatMul", ["h_h", "h_w2"], ["h_head"]),
        # logit of the shipped score, clipped as in training
        H("Max", [rn(old_out), "h_eps"], ["h_p0"]),
        H("Min", ["h_p0", "h_1meps"], ["h_p"]),
        H("Sub", ["h_one", "h_p"], ["h_q"]),
        H("Div", ["h_p", "h_q"], ["h_odds"]),
        H("Log", ["h_odds"], ["h_lo"]),
        H("Mul", ["h_lo", "h_a"], ["h_alo"]),
        H("Add", ["h_alo", "h_head"], ["h_s0"]),
        H("Add", ["h_s0", "h_c"], ["h_s"]),
        H("Sigmoid", ["h_s"], ["score"]),
    ]
    graph = helper.make_graph(
        nodes,
        "hey_ghost_operator",
        [g.input[0]],
        [helper.make_tensor_value_info("score", TensorProto.FLOAT, ["batch", 1])],
        inits,
    )
    model = helper.make_model(graph, opset_imports=list(old.opset_import))
    model.ir_version = old.ir_version
    onnx.checker.check_model(model)
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_suffix(".tmp")
    onnx.save(model, str(tmp))
    tmp.replace(out)
