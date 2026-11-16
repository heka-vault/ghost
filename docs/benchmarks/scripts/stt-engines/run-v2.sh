#!/bin/bash
# The follow-up of 2026-11-15 (the report's "Second pass"): everything read again with the
# scorer fix (032607a6). First, three timing passes of the three batch candidates in
# rotated order, each reading both the whole take and the harness's turn span; then every
# other row of the first pass, the clock, the streams and the interim simulation. Each run
# under the shared perf lock. Numbers only go to $O.
set -u
B=~/.cache/ghost-stt-bench/target/release/stt-engines
S=~/.local/share/ghost/hear/sessions/2026-11-06
O=${O:-~/.cache/ghost-stt-bench/runs-v2}
M=~/.cache/ghost-stt-bench/models
L="flock /tmp/ghost-perf.lock"
TDT=$M/parakeet-tdt-0.6b-v3/parakeet-tdt-0.6b-v3
UNI=$M/parakeet-unified-en-0.6b/parakeet-unified-en-0.6b
NEM=$M/nemotron-speech-streaming-en-0.6b/nemotron-speech-streaming-en-0.6b
NEM35=$M/nemotron-3.5-asr-streaming-0.6b/nemotron-3.5-asr-streaming-0.6b
CAN=$M/canary-180m-flash/canary-180m-flash
MOON=$M/moonshine-streaming-medium/moonshine-streaming-medium
run() { echo "== $*"; $L $B "$@" 2>&1 | grep -vE '^\s+[A-H][0-9]+ ' | tail -3; }

# Spans: `stt-engines spans $S $O/spans.json`, with the operator's settings.env loaded.
export STT_ENGINES_SPANS=$O/spans.json
T=$O/timing
for p in 1 2 3; do
  case $p in
    1) order="ort tdt uni" ;;
    2) order="tdt uni ort" ;;
    3) order="uni ort tdt" ;;
  esac
  for c in $order; do
    case $c in
      ort) run ort ort-fp32-p$p fp32 $S $T ;;
      tdt) run tc tdtv3-Q8_0-cpu4-p$p $TDT-Q8_0.gguf cpu 4 $S $T ;;
      uni) run tc unified-Q8_0-cpu4-p$p $UNI-Q8_0.gguf cpu 4 $S $T ;;
    esac
  done
done
unset STT_ENGINES_SPANS

run ort ort-int8 int8 $S $O
for q in F32 F16; do run tc tdtv3-$q-cpu4 $TDT-$q.gguf cpu 4 $S $O; done
for q in F32 F16 Q8_0; do run tc tdtv3-$q-vulkan $TDT-$q.gguf vulkan 4 $S $O; done
run tc tdtv3-Q8_0-cpu-default $TDT-Q8_0.gguf cpu 0 $S $O
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
run stream unified-Q8_0-cpu4-c560r560 $UNI-Q8_0.gguf cpu 4 $S $SO lcr=5600,560,560
run stream unified-Q8_0-cpu4-c160r160 $UNI-Q8_0.gguf cpu 4 $S $SO lcr=5600,160,160
run stream unified-Q8_0-cpu4-c80r0 $UNI-Q8_0.gguf cpu 4 $S $SO lcr=5600,80,0
run stream unified-F16-cpu4 $UNI-F16.gguf cpu 4 $S $SO
run stream unified-Q8_0-vulkan $UNI-Q8_0.gguf vulkan 4 $S $SO
run stream nemotron-en-Q8_0-cpu4 $NEM-Q8_0.gguf cpu 4 $S $SO
run stream nemotron-en-Q8_0-cpu4-rc6 $NEM-Q8_0.gguf cpu 4 $S $SO rc=6
run stream nemotron-en-Q8_0-cpu4-rc1 $NEM-Q8_0.gguf cpu 4 $S $SO rc=1
run stream nemotron-en-F16-cpu4 $NEM-F16.gguf cpu 4 $S $SO
run stream nemotron-en-Q8_0-vulkan $NEM-Q8_0.gguf vulkan 4 $S $SO
run stream nemotron35-Q8_0-cpu4 $NEM35-Q8_0.gguf cpu 4 $S $SO
run stream nemotron35-Q8_0-cpu4-rc6 $NEM35-Q8_0.gguf cpu 4 $S $SO rc=6
run stream moonshine-med-Q8_0-cpu4 $MOON-Q8_0.gguf cpu 4 $S $SO
run interim ghost-interim-fp32-0.3 fp32 0.3 $S $O/interim
echo v2 done
