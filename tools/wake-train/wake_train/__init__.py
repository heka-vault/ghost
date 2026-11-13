"""Retraining the "hey ghost" detector on the operator's own wake takes (ADR 0065)."""

import os

# One BLAS thread, set before numpy loads: the head's small products run fastest so (7.6 ms
# a step against 11.7 ms with four, on the workstation shared with other jobs), and the
# feature workers have the other cores.
for _v in ("OPENBLAS_NUM_THREADS", "OMP_NUM_THREADS", "MKL_NUM_THREADS"):
    os.environ.setdefault(_v, "1")
