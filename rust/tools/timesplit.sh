#!/usr/bin/env bash
# Where does a Kotlin trial spend its time? Runs TimeSplitTest in the container under Java Flight Recorder and
# attributes every execution sample to the first of route, place, validate, power that appears anywhere on its
# stack (routing and placing never nest). Prints the share of each. Run after `docker.sh sync`.
#
#   rust/tools/timesplit.sh [seeds per case, default 3] [chemical box, default 40] [electromagnetic box, default 46]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
seeds="${1:-3}"; chem="${2:-40}"; em="${3:-46}"

"$here/docker.sh" run '
set -e
rm -rf /work/out/jfr && mkdir -p /work/out/jfr
export TIMESPLIT=1 TIMESPLIT_SEEDS='"$seeds"' TIMESPLIT_CHEM='"$chem"' TIMESPLIT_EM='"$em"'
export JAVA_TOOL_OPTIONS="-Xmx6g -XX:StartFlightRecording=settings=profile,filename=/work/out/jfr/rec-%p.jfr,dumponexit=true"
bash ./gradlew --no-daemon test --rerun --tests TimeSplitTest -i 2>&1 | grep -E "^ *timesplit |FAILED|BUILD|error:|e: " | cut -c1-200
# The recording of the test worker is the one with the most execution samples.
best=""; most=0
for f in /work/out/jfr/rec-*.jfr; do
  n=$(jfr summary "$f" 2>/dev/null | awk "/jdk.ExecutionSample/ {print \$2; exit}")
  n=${n:-0}
  if [ "$n" -gt "$most" ]; then most=$n; best=$f; fi
done
echo "samples: $most"
jfr print --events jdk.ExecutionSample --stack-depth 64 "$best" | awk "
  /jdk.ExecutionSample/ { if (inEv) flush(); inEv=1; r=0; p=0; v=0; w=0; next }
  inEv && /^ *route\\./    { r=1 }
  inEv && /^ *place\\./    { p=1 }
  inEv && /^ *validate\\./ { v=1 }
  inEv && /^ *power\\./    { w=1 }
  function flush() {
    cat = r ? \"route\" : p ? \"place\" : v ? \"validate\" : w ? \"power\" : \"other\"
    count[cat]++; total++; inEv=0 }
  END { if (inEv) flush(); for (c in count) printf \"%-12s %6d  %5.1f%%\\n\", c, count[c], 100*count[c]/total }
" | sort -k2 -n -r
'
