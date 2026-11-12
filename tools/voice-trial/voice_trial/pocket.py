"""Kyutai Pocket TTS on this CPU through sherpa-onnx: time to the first audio chunk and the
real-time factor, by length, threads and flow steps. Measurement only; nothing of it ships.

uv run --project tools/voice-trial python -I -m voice_trial.pocket MODEL_DIR [--threads 2,4,6] [--steps 2,5]
"""

import argparse
import os
import time
import wave

import numpy as np
import sherpa_onnx

LINES = {
    2: "Labeled it.",
    4: "Three issues are waiting.",
    8: "Three issues are waiting for your sign-off today.",
    16: "Issue one forty two asks whether to use Redis or Postgres for the session cache today.",
}


def read_wav(path: str) -> tuple[np.ndarray, int]:
    with wave.open(path) as w:
        rate, n, ch, width = w.getframerate(), w.getnframes(), w.getnchannels(), w.getsampwidth()
        raw = w.readframes(n)
    assert width == 2
    x = np.frombuffer(raw, dtype=np.int16).reshape(-1, ch)[:, 0].astype(np.float32) / 32768
    return x, rate


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("dir")
    p.add_argument("--threads", default="2,4,6")
    p.add_argument("--steps", default="2,5")
    p.add_argument("--n", type=int, default=3)
    a = p.parse_args()
    d = a.dir
    ref, rate = read_wav(
        os.path.join(d, "test_wavs", sorted(os.listdir(os.path.join(d, "test_wavs")))[0])
    )
    print(f"load {os.getloadavg()[0]:.1f}")
    for threads in [int(t) for t in a.threads.split(",")]:
        tts = sherpa_onnx.OfflineTts(
            sherpa_onnx.OfflineTtsConfig(
                model=sherpa_onnx.OfflineTtsModelConfig(
                    pocket=sherpa_onnx.OfflineTtsPocketModelConfig(
                        lm_flow=f"{d}/lm_flow.int8.onnx",
                        lm_main=f"{d}/lm_main.int8.onnx",
                        encoder=f"{d}/encoder.onnx",
                        decoder=f"{d}/decoder.int8.onnx",
                        text_conditioner=f"{d}/text_conditioner.onnx",
                        vocab_json=f"{d}/vocab.json",
                        token_scores_json=f"{d}/token_scores.json",
                    ),
                    num_threads=threads,
                    provider="cpu",
                )
            )
        )
        for steps in [int(s) for s in a.steps.split(",")]:
            cfg = sherpa_onnx.GenerationConfig()
            cfg.reference_audio = ref
            cfg.reference_sample_rate = rate
            cfg.num_steps = steps
            tts.generate("Warm up.", cfg)
            for words, text in LINES.items():
                firsts, wholes, secs, chunks = [], [], 0.0, 0
                for _ in range(a.n):
                    first = [None]
                    count = [0]
                    t = time.perf_counter()

                    def cb(samples, progress, first=first, count=count, t=t):
                        count[0] += 1
                        if first[0] is None:
                            first[0] = time.perf_counter() - t
                        return 1

                    audio = tts.generate(text, cfg, callback=cb)
                    wholes.append(time.perf_counter() - t)
                    firsts.append(first[0] or wholes[-1])
                    secs = len(audio.samples) / audio.sample_rate
                    chunks = count[0]
                print(
                    f"  threads {threads} steps {steps} {words:>2} words: first chunk {np.median(firsts) * 1000:5.0f} ms"
                    f" ({chunks} callbacks), whole {np.median(wholes) * 1000:5.0f} ms, audio {secs:4.2f} s,"
                    f" RTF {np.median(wholes) / max(secs, 0.01):.2f}, rate {audio.sample_rate}"
                )
    print(f"load {os.getloadavg()[0]:.1f}")


if __name__ == "__main__":
    main()
