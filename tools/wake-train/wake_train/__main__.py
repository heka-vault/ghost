"""Build a new "hey ghost" detector from the operator's own wake takes, on this machine
(ADR 0064). ``make wake-train``; ``--help`` for the options.

1. Reads the labelled streams (``sets.txt``) and the kept takes the operator labelled.
2. Holds out whole recordings of the operator's takes for testing (``--folds``: each take
   is scored by a model that never saw it; the candidate is the model without fold 0).
3. Makes synthetic voices with Kokoro in voices the replay rig does not use, and augments
   the positives (level, noise, room, speed, compression).
4. Trains a head beside the shipped classifier on the frozen features (``model``), on CPU.
5. Chooses the threshold from the candidate's own curve: the lowest at which it wakes no
   more often on the training negatives than the shipped model does at its threshold, and
   a floor at which Parakeet would be asked no more often.
6. Scores before and after on every set, writes ``hey_ghost.onnx``, ``card.json`` (its hash,
   threshold, floor and every number) and ``report.md`` into the output folder.

Nothing is sent anywhere and nothing is installed: ``ghostd wake use <name>`` does that,
after the operator has read the numbers. Everything the run makes stays in ``--work`` and
``--out``, which hold features of the operator's voice: both are made 0700.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time
from concurrent.futures import ProcessPoolExecutor
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path

import numpy as np

from wake_train import augment, data, evaluate, model, synth
from wake_train.features import STEP, FrontEnd, windows

RATE = 16000
HOME = Path.home()
CACHE = HOME / ".cache" / "ghost"
DATA = Path(os.environ.get("GHOST_DATA_DIR", HOME / ".local" / "share" / "ghost"))
SHIPPED = CACHE / "wake" / "hey_ghost.onnx"
SHIPPED_THRESHOLD = 0.6
SHIPPED_FLOOR = 0.3
POS_HALF = 2  # steps either side of a take's peak that are positives


def log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def private_dir(p: Path) -> Path:
    p.mkdir(parents=True, exist_ok=True)
    os.chmod(p, 0o700)
    return p


# --- features, cached -----------------------------------------------------------------------

_FE: FrontEnd | None = None


def _fe() -> FrontEnd:
    global _FE
    if _FE is None:
        _FE = FrontEnd()
    return _FE


def _key(parts: list[str]) -> str:
    h = hashlib.sha256()
    for p in parts:
        h.update(p.encode())
        h.update(b"\0")
    return h.hexdigest()[:24]


def read_stream(s: data.Stream) -> np.ndarray:
    files = data.matches(s.path)
    if not files:
        raise FileNotFoundError(s.path)
    parts = []
    for f in files:
        parts.append(np.asarray(data.read_wav(f, s.channel), np.int16))
        if len(files) > 1:
            parts.append(np.zeros(RATE, np.int16))
    return np.concatenate(parts)


def _emb_job(args: tuple[str, str, int, str]) -> str:
    kind, path, channel, out = args
    if Path(out).exists():
        return out
    if kind == "stream":
        pcm = read_stream(data.parse_stream(f"{path}#{channel}:neg"))
    else:
        pcm = np.asarray(data.read_wav(path), np.int16)
    emb = _fe().stream_embeddings(pcm)
    tmp = out + ".tmp.npy"
    np.save(tmp, emb.astype(np.float16))
    os.replace(tmp, out)
    return out


def stream_files_key(s: data.Stream) -> list[str]:
    keys = []
    for f in data.matches(s.path):
        st = os.stat(f)
        keys += [f, str(st.st_size), str(int(st.st_mtime))]
    return keys + [str(s.channel)]


def embeddings_for(streams: list[data.Stream], work: Path, jobs: int) -> dict[str, np.ndarray]:
    d = private_dir(work / "emb")
    todo, out = [], {}
    for s in streams:
        f = str(d / f"{_key(stream_files_key(s))}.npy")
        out[s.name + s.path] = f
        todo.append(("stream", s.path, s.channel, f))
    run_jobs(todo, jobs, "streams")
    return {k: np.load(v).astype(np.float32) for k, v in out.items()}


def run_jobs(todo: list, jobs: int, what: str) -> None:
    pending = [t for t in todo if not Path(t[3]).exists()]
    if not pending:
        return
    log(f"features: {len(pending)} {what} to read on {jobs} cores")
    t0 = time.time()
    with ProcessPoolExecutor(jobs) as ex:
        for i, _ in enumerate(ex.map(_emb_job, pending), 1):
            if i % 25 == 0 or i == len(pending):
                log(f"  {i}/{len(pending)} ({time.time() - t0:.0f} s)")


def clip_embeddings(pcms: list[np.ndarray], work: Path, tag: str, jobs: int) -> list[np.ndarray]:
    """Embeddings of in-memory clips (augmented audio), cached by content."""
    d = private_dir(work / "emb-clips")
    src = private_dir(work / "clips-tmp")
    todo, files = [], []
    for pcm in pcms:
        k = hashlib.sha256(pcm.tobytes()).hexdigest()[:24]
        f = str(d / f"{k}.npy")
        files.append(f)
        if not Path(f).exists():
            w = src / f"{k}.wav"
            data.write_wav(w, pcm.tolist())
            todo.append(("clip", str(w), 0, f))
    run_jobs(todo, jobs, tag)
    for p in src.glob("*.wav"):
        p.unlink()
    return [np.load(f).astype(np.float32) for f in files]


# --- the operator's takes -------------------------------------------------------------------


@dataclass
class Take:
    group: str
    stream: str  # stream key
    peak: int  # step index of the take's peak
    pcm_path: str
    channel: int


def peaks(sc_by_stream: dict[str, np.ndarray], s: data.Stream, partner: str | None) -> list[int]:
    """For each labelled "ghost", the step the shipped model peaks at within 1.2 s, read on
    the raw array when there is one (it hears the operator best), else on this stream."""
    ref = sc_by_stream.get(partner) if partner else None
    sc = ref if ref is not None else sc_by_stream[s.name + s.path]
    out = []
    for t in s.times:
        i0 = max(0, int((t - evaluate.NEAR) * RATE / STEP))
        i1 = min(len(sc), int((t + evaluate.NEAR) * RATE / STEP) + 1)
        seg = sc[i0:i1]
        out.append(i0 + int(np.argmax(seg)) if seg.max() >= 0.2 else int(t * RATE / STEP))
    return out


def frame_labels(n: int, peaks_: list[int], s: data.Stream) -> tuple[np.ndarray, np.ndarray]:
    """(positive steps, negative steps) of a pos stream: a take's peak ± 2 steps is
    positive, 1.5 s either side of it is left out, and so is 2.8 s around another phrasing."""
    pos, skip = set(), np.zeros(n, bool)
    for p in peaks_:
        pos.update(range(max(0, p - POS_HALF), min(n, p + POS_HALF + 1)))
        a, b = p - int(1.5 * RATE / STEP), p + int(1.5 * RATE / STEP)
        skip[max(0, a) : min(n, b + 1)] = True
    for o in s.others:
        i = int(o * RATE / STEP)
        r = int(2.8 * RATE / STEP)
        skip[max(0, i - r) : min(n, i + r + 1)] = True
    neg = np.flatnonzero(~skip)
    return np.array(sorted(pos), int), neg


def partner_of(s: data.Stream, streams: list[data.Stream]) -> str | None:
    if "-aec" not in Path(s.path).name:
        return None
    raw = s.path.replace("-aec", "-raw")
    for o in streams:
        if o.path == raw:
            return o.name + o.path
    return None


# --- the run --------------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="make wake-train", description=__doc__.split("\n")[0])
    ap.add_argument("--sets", type=Path, default=Path(__file__).parent.parent / "sets.txt")
    ap.add_argument("--takes", type=Path, default=DATA / "wake" / "takes")
    ap.add_argument("--work", type=Path, default=DATA / "wake" / "train-work")
    ap.add_argument("--out", type=Path, default=DATA / "wake" / "detectors")
    ap.add_argument("--name", default=datetime.now().strftime("operator-%Y%m%d-%H%M"))
    ap.add_argument(
        "--unlabelled-accepted-are-me",
        action="store_true",
        help="treat a kept wake Ghost accepted that has no label as the operator calling Ghost",
    )
    ap.add_argument("--folds", type=int, default=3)
    ap.add_argument("--neg-holdout", type=float, default=0.34)
    ap.add_argument("--aug", type=int, default=24, help="augmented copies per operator take")
    ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 2))
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--no-synth", action="store_true")
    ap.add_argument("--dry-run", action="store_true", help="say what would be read, and stop")
    a = ap.parse_args(argv)
    os.nice(10)
    t_start = time.time()

    sets = data.read_sets(a.sets.read_text(), data.default_env())
    all_streams = [s for st in sets for s in st.streams if s.kind in ("pos", "neg", "program")]
    operator = [
        (st.title, s)
        for st in sets
        if not st.title.lower().startswith("kokoro")
        for s in st.streams
    ]
    missing = [s.path for s in all_streams if not data.matches(s.path)]
    kept = data.read_kept(a.takes)
    kept_used = [(t, data.label_of(t, a.unlabelled_accepted_are_me)) for t in kept]
    kept_used = [(t, lab) for t, lab in kept_used if lab]
    log(
        f"streams: {len(all_streams)} in {len(sets)} sets ({len(missing)} missing); kept takes: "
        f"{len(kept)}, {sum(1 for _, lab in kept_used if lab == 'pos')} me, "
        f"{sum(1 for _, lab in kept_used if lab == 'neg')} not a wake, the rest unused"
    )
    for m in missing:
        log(f"  missing, skipped: {m}")
    all_streams = [s for s in all_streams if data.matches(s.path)]
    if a.dry_run:
        log("training runs on this machine's CPU; nothing leaves it")
        return 0

    work = private_dir(a.work)
    jobs = a.jobs
    emb = embeddings_for(all_streams, work, jobs)
    shipped = model_scorer(SHIPPED)
    log(f"stage: the shipped model's scores ({time.time() - t_start:.0f} s)")
    sc_shipped = {k: shipped(windows(v)) for k, v in emb.items()}

    # the operator's takes, grouped by recording
    pos_streams = [(t, s) for t, s in operator if s.kind == "pos"]
    takes: list[Take] = []
    stream_pos: dict[str, tuple[np.ndarray, np.ndarray]] = {}
    for _title, s in pos_streams:
        key = s.name + s.path
        pk = peaks(sc_shipped, s, partner_of(s, all_streams))
        stream_pos[key] = frame_labels(len(emb[key]), pk, s)
        takes += [Take(s.group, key, p, s.path, s.channel) for p in pk]
    weight = {}
    for t in takes:
        weight[t.group] = weight.get(t.group, 0) + 1
    kept_groups = {t.group: 1 for t, lab in kept_used if lab == "pos"}
    weight.update(kept_groups)
    fold_sets = data.folds(weight.keys(), a.folds, weight)
    neg_groups = sorted({s.group for t, s in operator if s.kind == "neg"})
    neg_train, neg_held = data.split(neg_groups, a.neg_holdout)
    log(f"folds of the operator's takes: {[sorted(f) for f in fold_sets]}")
    log(f"operator negatives held out: {sorted(neg_held)}")

    # synthetic voices and music
    log(f"stage: synthetic voices ({time.time() - t_start:.0f} s)")
    syn = {"pos": [], "neg": []} if a.no_synth else synth.make(work, CACHE, jobs=jobs)
    log(f"stage: music ({time.time() - t_start:.0f} s)")
    music = load_music(work)
    rng = np.random.default_rng(a.seed)

    def synth_set():
        pos_x, neg_x = [], []
        pos_pcm = [synth.load(p) for p in syn["pos"]]
        neg_pcm = [synth.load(p) for p in syn["neg"]]
        aug_pos = [augment.apply(p, augment.draw(rng, bool(music)), rng, music) for p in pos_pcm]
        aug_neg = [augment.apply(p, augment.draw(rng, bool(music)), rng, music) for p in neg_pcm]
        e_pos = clip_embeddings(pos_pcm + aug_pos, work, "synthetic positives", jobs)
        e_neg = clip_embeddings(neg_pcm + aug_neg, work, "synthetic negatives", jobs)
        for e in e_pos:
            w = windows(e)
            sc = shipped(w)
            if sc.max() < 0.2:
                continue
            p = int(np.argmax(sc))
            pos_x.append(w[max(0, p - 1) : p + 2])
        for e in e_neg:
            neg_x.append(windows(e))
        return pos_x, neg_x

    log(f"stage: augmenting the synthetic clips ({time.time() - t_start:.0f} s)")
    syn_pos, syn_neg = synth_set()
    music_negs = music_embeddings(music, work, jobs)
    log(
        f"synthetic: {sum(len(x) for x in syn_pos)} positive and "
        f"{sum(len(x) for x in syn_neg)} negative windows; music: "
        f"{sum(len(x) for x in music_negs)} windows"
    )

    # augmented copies of each operator take, by group
    def take_aug(t: Take) -> list[np.ndarray]:
        s = data.parse_stream(f"{t.pcm_path}#{t.channel}:neg")
        pcm = read_stream(s)
        end = t.peak * STEP  # samples into the stream (the stream is padded by 2 s)
        a0, a1 = max(0, end - int(3.0 * RATE)), min(len(pcm), end + int(0.5 * RATE))
        seg = pcm[a0:a1]
        peak_at = end - a0
        r = np.random.default_rng(int(data.bucket(f"{t.stream}@{t.peak}") * 2**32))
        out_pcm, peaks_at = [], []
        for _ in range(a.aug):
            g = augment.draw(r, bool(music))
            out_pcm.append(augment.apply(seg, g, r, music))
            peaks_at.append(peak_at / g.speed)
        es = clip_embeddings(out_pcm, work, "augmented takes", jobs)
        xs = []
        for e, pa in zip(es, peaks_at, strict=True):
            w = windows(e)
            # step i of a padded clip ends i * STEP samples into it
            p = int(round(pa / STEP))
            xs.append(w[max(0, p - POS_HALF) : min(len(w), p + POS_HALF + 1)])
        return xs

    aug_cache: dict[tuple[str, int], list[np.ndarray]] = {}

    def augmented(t: Take) -> list[np.ndarray]:
        k = (t.stream, t.peak)
        if k not in aug_cache:
            aug_cache[k] = take_aug(t)
        return aug_cache[k]

    kept_x = kept_windows(kept_used, work, jobs, a.aug, music, shipped)

    def build(train_groups: set[str]):
        X, Old, Y, W = [], [], [], []

        def add(x: np.ndarray, y: float, w: float) -> None:
            if len(x) == 0:
                return
            X.append(x)
            Old.append(shipped(x))
            Y.append(np.full(len(x), y, np.float32))
            W.append(np.full(len(x), w, np.float32))

        n_pos_op = 0
        for _title, s in pos_streams:
            if s.group not in train_groups:
                continue
            key = s.name + s.path
            pos_i, neg_i = stream_pos[key]
            w_all = windows(emb[key])
            add(w_all[pos_i], 1, 1)
            add(w_all[neg_i], 0, 1)
            n_pos_op += len(pos_i)
        for t in takes:
            if t.group in train_groups:
                for x in augmented(t):
                    add(x, 1, 1)
        for _title, s in operator:
            if s.kind == "neg" and s.group in neg_train:
                add(windows(emb[s.name + s.path]), 0, 1)
        for g, (lab, xs) in kept_x.items():
            if lab == "neg" or g in train_groups:
                for x in xs:
                    add(x, 1 if lab == "pos" else 0, 1)
        for x in syn_pos:
            add(x, 1, 0.5)
        for x in syn_neg:
            add(x, 0, 1)
        for x in music_negs:
            add(x, 0, 1)
        x = np.concatenate(X)
        o, y, w = np.concatenate(Old), np.concatenate(Y), np.concatenate(W)
        npos, nneg = y.sum(), len(y) - y.sum()
        w = np.where(y == 1, w * (nneg / max(npos, 1)) * 0.5, w)
        return x, o, y, w

    def train_negatives(train_groups: set[str]) -> list[np.ndarray]:
        """Streams of the training negatives, for the threshold curve: each as a score
        stream (``scores`` takes a detector)."""
        out = []
        for _title, s in operator:
            key = s.name + s.path
            if s.kind == "neg" and s.group in neg_train:
                out.append(key)
            elif s.kind == "pos" and s.group in train_groups:
                out.append(key)
        return out

    def neg_scores(det, keys: list[str]) -> list[np.ndarray]:
        out = []
        for k in keys:
            sc = det(windows(emb[k]))
            if k in stream_pos:
                pos_i, neg_i = stream_pos[k]
                keep = np.zeros(len(sc), bool)
                keep[neg_i] = True
                sc = np.where(keep, sc, 0.0)
            out.append(sc)
        for xs in (syn_neg, music_negs):
            for x in xs:
                out.append(det(x))
        return out

    def score_takes(det, thr, floor, groups: set[str]) -> dict[str, evaluate.Count]:
        out: dict[str, evaluate.Count] = {}
        for title, s in pos_streams:
            if s.group not in groups:
                continue
            key = s.name + s.path
            sc = det(windows(emb[key]))
            c = evaluate.count(sc, thr, floor, list(s.times), list(s.others))
            out.setdefault(title, evaluate.Count()).add(c)
        return out

    def fit(train_groups: set[str], seed: int):
        x, o, y, w = build(train_groups)
        log(f"  training on {int(y.sum())} positive and {int(len(y) - y.sum())} negative windows")
        t0 = time.time()
        head = model.train(x, o, y, w, seed=seed, log=log)
        log(f"  trained in {time.time() - t0:.0f} s")

        def det(win: np.ndarray) -> np.ndarray:
            return head(win, shipped(win))

        keys = train_negatives(train_groups)
        old_negs = neg_scores(shipped, keys)
        new_negs = neg_scores(det, keys)

        def heard(thr: float, floor: float) -> tuple[int, int]:
            c = evaluate.Count()
            for v in score_takes(det, thr, floor, train_groups).values():
                c.add(v)
            return c.woke, c.checked

        point = evaluate.operating_point(
            old_negs, new_negs, heard, (SHIPPED_THRESHOLD, SHIPPED_FLOOR)
        )
        if point is None:
            f0, a0 = evaluate.rule_counts(old_negs, SHIPPED_THRESHOLD, SHIPPED_FLOOR)
            log(
                "  no threshold keeps false wakes and Parakeet's checks at the shipped "
                f"detector's ({f0} and {a0} on the training negatives)"
            )
            return head, det, None, None, {"shipped_false_wakes": f0, "shipped_asked": a0}
        thr, floor, curve = point
        log(
            f"  threshold {thr:.2f}, floor {floor:.2f}: {curve['false_wakes']} false wakes and "
            f"{curve['asked']} checks on the training negatives (shipped "
            f"{curve['shipped_false_wakes']} and {curve['shipped_asked']})"
        )
        return head, det, thr, floor, curve

    # cross-check: every take scored by a model that never saw its recording
    log(f"cross-check over {len(fold_sets)} folds")
    cv_ok = True
    cv_before: dict[str, evaluate.Count] = {}
    cv_after: dict[str, evaluate.Count] = {}
    all_groups = set(weight)
    for i, f in enumerate(fold_sets):
        log(f" fold {i}: holding out {sorted(f)}")
        _h, det, thr, floor, _c = fit(all_groups - f, a.seed + i)
        if thr is None:
            cv_ok = False
            continue
        for k, v in score_takes(det, thr, floor, f).items():
            cv_after.setdefault(k, evaluate.Count()).add(v)
        for k, v in score_takes(shipped, SHIPPED_THRESHOLD, SHIPPED_FLOOR, f).items():
            cv_before.setdefault(k, evaluate.Count()).add(v)

    # the candidate: without fold 0
    held = fold_sets[0]
    log(f"candidate: holding out {sorted(held)}")
    head, det, thr, floor, curve = fit(all_groups - held, a.seed)
    if thr is None or not cv_ok:
        out = private_dir(a.out / a.name)
        why = (
            "No threshold of the new detector keeps its false wakes and Parakeet's checks at "
            "or under the shipped detector's on the training negatives "
            + ("(the candidate)" if thr is None else "(a fold of the cross-check)")
            + ". Nothing was written that could be installed: keep the shipped detector, "
            "keep and label more wake takes, and run this again."
        )
        (out / "report.md").write_text(f"# Wake detector {a.name}: not made\n\n{why}\n")
        print(why)
        return 3

    rep = evaluate.Report()
    for title, s in [(st.title, s) for st in sets for s in st.streams]:
        key = s.name + s.path
        if key not in emb:
            continue
        w = windows(emb[key])
        sc0, sc1 = sc_shipped[key], det(w)
        if s.kind == "pos":
            seen = "no (held out)" if s.group in held else "yes"
            b = evaluate.count(sc0, SHIPPED_THRESHOLD, SHIPPED_FLOOR, list(s.times), list(s.others))
            c = evaluate.count(sc1, thr, floor, list(s.times), list(s.others))
        elif s.kind == "program":
            pcm = read_stream(s)
            pos, other, _ = evaluate.program_spans(Path(s.program), pcm, room="/rec/" in s.path)
            seen = "no (never trained on)"
            b = recount_other(b, sc0, SHIPPED_THRESHOLD, SHIPPED_FLOOR, pos, other)
            c = recount_other(c, sc1, thr, floor, pos, other)
        else:
            seen = "no (held out)" if s.group in neg_held else "yes"
            b = evaluate.count(sc0, SHIPPED_THRESHOLD, SHIPPED_FLOOR, [])
            c = evaluate.count(sc1, thr, floor, [])
        rep.add(title, s.name, seen, b, c)

    out = private_dir(a.out / a.name)
    onnx_path = out / "hey_ghost.onnx"
    model.export(head, SHIPPED, onnx_path)
    # the exported graph must score as the head does
    check = model_scorer(onnx_path)
    probe = windows(emb[next(iter(emb))])[:64]
    drift = float(np.max(np.abs(check(probe) - det(probe))))
    if drift > 1e-3:
        log(f"the exported graph differs from the trained head by {drift}: not written")
        onnx_path.unlink()
        return 1
    sha = hashlib.sha256(onnx_path.read_bytes()).hexdigest()
    card = {
        "name": a.name,
        "made": datetime.now(UTC).isoformat(timespec="seconds"),
        "file": "hey_ghost.onnx",
        "sha256": sha,
        "threshold": thr,
        "floor": floor,
        "shipped": {
            "sha256": hashlib.sha256(SHIPPED.read_bytes()).hexdigest(),
            "threshold": SHIPPED_THRESHOLD,
            "floor": SHIPPED_FLOOR,
        },
        "held_out": sorted(held),
        "negatives_held_out": sorted(neg_held),
        "kept_takes_used": len(kept_used),
        "augmented_copies": a.aug,
        "curve": curve,
        "cross_check": {
            k: {"before": cv_before[k].row(), "after": cv_after[k].row()} for k in cv_after
        },
        "sets": rep.rows,
        "seconds": round(time.time() - t_start),
    }
    (out / "card.json").write_text(json.dumps(card, indent=2) + "\n")
    os.chmod(out / "card.json", 0o600)
    os.chmod(onnx_path, 0o600)
    md = report_md(card)
    (out / "report.md").write_text(md)
    print(md)
    log(f"written to {out} in {time.time() - t_start:.0f} s; nothing installed")
    return 0


def recount_other(c, sc, thr, floor, pos, other):
    """A program's count, with hits inside another wake phrasing's clip taken out of the
    false ones."""
    c2 = evaluate.count(sc, thr, floor, [], [], pos)
    fw = fc = 0
    for t, s in evaluate.hits(sc, thr, floor):
        in_pos = any(a - 0.2 <= t <= b + evaluate.NEAR for a, b in pos)
        in_other = any(a - 0.2 <= t <= b + evaluate.NEAR for a, b in other)
        if in_pos or in_other:
            continue
        if s >= thr:
            fw += 1
        else:
            fc += 1
    c2.false_wakes, c2.false_checked = fw, fc
    return c2


def model_scorer(path: Path):
    from wake_train.features import Classifier

    return Classifier(path)


def load_music(work: Path) -> list[np.ndarray]:
    """Up to 60 music clips of 20 s from the FMA archive the first training used, read once
    into the work folder."""
    default = HOME / "projects/active/ghost/.scratch/wake-train/pod/fma.tar"
    tar = Path(os.environ.get("WAKE_MUSIC_TAR", default))
    d = work / "music"
    if not d.exists() and tar.exists():
        import tarfile

        private_dir(d)
        with tarfile.open(tar) as tf:
            members = sorted(m for m in tf.getnames() if m.endswith(".wav"))[:60]
            for m in members:
                f = tf.extractfile(m)
                if f is None:
                    continue
                (d / Path(m).name).write_bytes(f.read())
    out = []
    for f in sorted(d.glob("*.wav")) if d.exists() else []:
        try:
            pcm = to16k(f)
        except Exception:
            continue
        out.append(pcm[: 20 * RATE])
    return out


def to16k(path: Path) -> np.ndarray:
    import wave

    import soxr

    with wave.open(str(path)) as w:
        sr, n, sw = w.getframerate(), w.getnchannels(), w.getsampwidth()
        raw = w.readframes(w.getnframes())
    if sw != 2:
        raise ValueError("16-bit only")
    a = np.frombuffer(raw, np.int16).reshape(-1, n).mean(1).astype(np.float32)
    if sr != RATE:
        a = soxr.resample(a, sr, RATE)
    return np.clip(a, -32768, 32767).astype(np.int16)


def music_embeddings(music: list[np.ndarray], work: Path, jobs: int) -> list[np.ndarray]:
    # half the tracks train; the music in the report is the operator's and the rig's own
    half = music[::2]
    return [windows(e) for e in clip_embeddings(half, work, "music", jobs)]


def kept_windows(kept_used, work: Path, jobs: int, n_aug: int, music, shipped) -> dict:
    """``{group: (label, [windows])}`` for the kept takes. A take labelled the operator's
    has its positive steps (the scored frame to 0.32 s after it, where "ghost" ends) and
    augmented copies, from the stream the detector heard and, when kept, the array's; one
    labelled "me, after a miss" also gives the shipped model's best moment in the 12 s
    before as a positive (the attempt it missed). One labelled not a wake is negative
    throughout."""
    out = {}
    for t, lab in kept_used:
        streams = [np.asarray(data.read_wav(t.wav), np.int16)]
        if t.array is not None:
            streams.append(np.asarray(data.read_wav(t.array, 0), np.int16))
        es = clip_embeddings(streams, work, "kept takes", jobs)
        if lab == "neg":
            out[t.group] = ("neg", [windows(e) for e in es])
            continue
        at = int(round(t.hit_at * RATE / STEP))
        xs = []
        for e in es:
            w = windows(e)
            xs.append(w[max(0, at - 1) : at + 5])
            if t.label == "me_missed_before" and at > 40:
                early = shipped(w[: at - 25])  # ends 2 s before this hit
                p = int(np.argmax(early))
                if early[p] >= 0.05:
                    xs.append(w[max(0, p - 1) : p + 3])
        r = np.random.default_rng(int(data.bucket(t.id) * 2**32))
        augs, pks = [], []
        for _ in range(n_aug):
            g = augment.draw(r, bool(music))
            augs.append(augment.apply(streams[0], g, r, music))
            pks.append(t.hit_at * RATE / STEP / g.speed)
        es = clip_embeddings(augs, work, "augmented kept takes", jobs)
        for e2, p in zip(es, pks, strict=True):
            w2 = windows(e2)
            p = int(round(p))
            xs.append(w2[max(0, p - 1) : p + 5])
        out[t.group] = ("pos", xs)
    return out


def report_md(card: dict) -> str:
    def cell(r: dict, kind: str) -> str:
        if r["takes"]:
            return f"{r['woke']}+{r['checked']}/{r['takes']}, fw {r['false_wakes']}"
        return f"fw {r['false_wakes']}, checked {r['false_checked']}"

    lines = [
        f"# Wake detector {card['name']}",
        "",
        f"Threshold {card['threshold']:.2f}, floor {card['floor']:.2f} (shipped: "
        f"{card['shipped']['threshold']}, {card['shipped']['floor']}). sha256 "
        f"`{card['sha256'][:16]}…`. Held out: {', '.join(card['held_out'])}; negatives held "
        f"out: {', '.join(card['negatives_held_out'])}.",
        "",
        "Takes: woke at once + checked by Parakeet / takes. fw: false wakes (a hit at the "
        "threshold away from any take); checked: Parakeet asked about a sound away from any "
        "take.",
        "",
        "## Cross-check (each take scored by a model that never saw its recording)",
        "",
        "| set | before | after |",
        "|---|---|---|",
    ]
    for k, v in card["cross_check"].items():
        lines.append(f"| {k} | {cell(v['before'], 'pos')} | {cell(v['after'], 'pos')} |")
    lines += ["", "## Every stream, the candidate", "", "| set | stream | seen | before | after |"]
    lines.append("|---|---|---|---|---|")
    for r in card["sets"]:
        lines.append(
            f"| {r['set']} | {r['stream']} | {r['seen_in_training']} | "
            f"{cell(r['before'], '')} | {cell(r['after'], '')} |"
        )
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    raise SystemExit(main())
