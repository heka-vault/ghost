"""Print a GGUF's streaming and attention-context keys (the latency menus in the report)."""

import struct
import sys


def rd(f, fmt):
    return struct.unpack("<" + fmt, f.read(struct.calcsize(fmt)))[0]


def rs(f):
    n = rd(f, "Q")
    return f.read(n).decode("utf-8", "replace")


SC = {0: "B", 1: "b", 2: "H", 3: "h", 4: "I", 5: "i", 6: "f", 7: "?", 10: "Q", 11: "q", 12: "d"}


def val(f, t):
    if t in SC:
        return rd(f, SC[t])
    if t == 8:
        return rs(f)
    if t == 9:
        et = rd(f, "I")
        n = rd(f, "Q")
        return [val(f, et) for _ in range(n)]
    raise ValueError(t)


with open(sys.argv[1], "rb") as f:
    assert f.read(4) == b"GGUF"
    rd(f, "I")
    rd(f, "Q")
    n = rd(f, "Q")
    for _ in range(n):
        k = rs(f)
        t = rd(f, "I")
        v = val(f, t)
        if "att_" in k or "stream" in k or "subsampling" in k or "chunk" in k:
            print(k, v if not isinstance(v, list) or len(v) < 40 else v[:40])
