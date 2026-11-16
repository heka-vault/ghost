#!/bin/bash
# Every run of docs/benchmarks/2026-11-15-stt-engines.md, each under the shared perf lock.
# Numbers only go to $O; nothing writes a transcript.
set -u
# The interim simulation runs last, in run-lowlat.sh.
B=~/.cache/ghost-stt-bench/target/release/stt-engines
S=~/.local/share/ghost/hear/sessions/2026-11-06
O=${O:-~/.cache/ghost-stt-bench/runs}
M=~/.cache/ghost-stt-bench/models
L="flock /tmp/ghost-perf.lock"
TDT=$M/parakeet-tdt-0.6b-v3/parakeet-tdt-0.6b-v3
UNI=$M/parakeet-unified-en-0.6b/parakeet-unified-en-0.6b
NEM=$M/nemotron-speech-streaming-en-0.6b/nemotron-speech-streaming-en-0.6b
NEM35=$M/nemotron-3.5-asr-streaming-0.6b/nemotron-3.5-asr-streaming-0.6b
CAN=$M/canary-180m-flash/canary-180m-flash
MOON=$M/moonshine-streaming-medium/moonshine-streaming-medium
run() { echo "== $*"; $L $B "$@" 2>&1 | grep -vE '^\s+[A-H][0-9]+ ' | tail -3; }

run ort ort-int8 int8 $S $O
for q in F16 Q8_0; do run tc tdtv3-$q-cpu4 $TDT-$q.gguf cpu 4 $S $O; done
for q in F32 F16 Q8_0; do run tc tdtv3-$q-vulkan $TDT-$q.gguf vulkan 4 $S $O; done
run tc tdtv3-Q8_0-cpu-default $TDT-Q8_0.gguf cpu 0 $S $O
run tc unified-Q8_0-cpu4 $UNI-Q8_0.gguf cpu 4 $S $O
run tc unified-F16-cpu4 $UNI-F16.gguf cpu 4 $S $O
run tc unified-Q8_0-vulkan $UNI-Q8_0.gguf vulkan 4 $S $O
run tc nemotron-en-Q8_0-cpu4 $NEM-Q8_0.gguf cpu 4 $S $O
run tc nemotron-en-F16-cpu4 $NEM-F16.gguf cpu 4 $S $O
run tc nemotron35-Q8_0-cpu4 $NEM35-Q8_0.gguf cpu 4 $S $O
run tc canary180m-Q8_0-cpu4 $CAN-Q8_0.gguf cpu 4 $S $O
run tc canary180m-Q8_0-vulkan $CAN-Q8_0.gguf vulkan 4 $S $O
run tc moonshine-med-Q8_0-cpu4 $MOON-Q8_0.gguf cpu 4 $S $O

SO=$O/stream; mkdir -p $SO
run clock $TDT-F32.gguf $S $O/clock.json
run stream unified-Q8_0-cpu4 $UNI-Q8_0.gguf cpu 4 $S $SO
run stream unified-F16-cpu4 $UNI-F16.gguf cpu 4 $S $SO
run stream nemotron-en-Q8_0-cpu4 $NEM-Q8_0.gguf cpu 4 $S $SO
run stream nemotron-en-F16-cpu4 $NEM-F16.gguf cpu 4 $S $SO
run stream nemotron35-Q8_0-cpu4 $NEM35-Q8_0.gguf cpu 4 $S $SO
run stream moonshine-med-Q8_0-cpu4 $MOON-Q8_0.gguf cpu 4 $S $SO
run stream unified-Q8_0-vulkan $UNI-Q8_0.gguf vulkan 4 $S $SO
run stream nemotron-en-Q8_0-vulkan $NEM-Q8_0.gguf vulkan 4 $S $SO

run pipe 300 $O/isolation
run spawn 3 $TDT-F32.gguf $S $O/isolation
echo done
