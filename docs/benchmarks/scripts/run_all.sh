#!/usr/bin/env bash
# Usage: run_all.sh <bench_root> <scripts_dir>
R=$1; S=$2; PY=$R/env/bin/python; OUT=$R/results.jsonl; : > $OUT
for e in kokoro:int8 kokoro:fp32 piper:lessac-medium piper:ryan-high; do
  $PY -I $S/tts_bench.py $e $R/models $R/out 6 2>>$R/err.log | tail -1 >> $OUT; echo "done $e"
done
for e in moonshine:tiny moonshine:base parakeet:v2 parakeet:v3 fw:tiny.en fw:base.en fw:small.en fw:distil-small.en fw:medium.en fw:distil-large-v3 fw:large-v3-turbo; do
  $PY -I $S/stt_bench.py $e $R/models $R/audio 6 2>>$R/err.log | tail -1 >> $OUT; echo "done $e"
done
echo ALLDONE
