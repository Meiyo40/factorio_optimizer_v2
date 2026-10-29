#!/usr/bin/env bash
# Speed of the Rust port against the JVM, in the container where both run on the same CPU. Run `docker.sh sync` first,
# and leave the host alone while it runs (the Docker VM shares the CPU).
#
#   rust/tools/speed.sh trials [cases]       whole trials on golden cases: Kotlin, then Rust, same work checked
#   rust/tools/speed.sh rust [cases]         Rust only, against the JVM numbers of the last trials run (for measuring a change)
#   rust/tools/speed.sh profile <case>       callgrind profile of one trial in Rust (instruction counts per function)
#   rust/tools/speed.sh search <job> [budget_s] [threads]
#                                            a real search of a job in Kotlin and in Rust: trials done and best box over time
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
mode="${1:-}"; shift || true

case "$mode" in
  trials)
    cases="${1:-}"
    "$here/docker.sh" run '
set -e
rm -rf /work/out/trial-bench
export TRIAL_BENCH_OUT=/work/out/trial-bench JAVA_TOOL_OPTIONS=-Xmx6g'"${cases:+ BENCH_CASES=$cases}"'
bash ./gradlew --no-daemon test --rerun --tests GoldenTrialTest.benchTrials -i 2>&1 | grep -E "^ *bench |FAILED|BUILD|error:|e: |Exception|Caused" | cut -c1-220
cd rust && cargo build --release --bin trial_bench 2>&1 | tail -1
/cache/target/release/trial_bench --dir /work/rust/tests/golden/trial --kotlin /work/out/trial-bench/trial_bench_kotlin.json'"${cases:+ --cases $cases}"'
'
    ;;
  profile)
    case_name="${1:?usage: speed.sh profile <case>}"
    "$here/docker.sh" run '
set -e
cd rust
CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release --bin trial_bench 2>&1 | tail -1
mkdir -p /work/out/profile && rm -f /work/out/profile/callgrind.'"$case_name"'.out
valgrind --tool=callgrind --callgrind-out-file=/work/out/profile/callgrind.'"$case_name"'.out --dump-instr=no \
  /cache/target/release/trial_bench --dir /work/rust/tests/golden/trial --only '"$case_name"' --warm 1 --reps 1 --no-check 2>&1 | tail -3
callgrind_annotate --inclusive=no /work/out/profile/callgrind.'"$case_name"'.out 2>/dev/null | head -45
'
    ;;
  search)
    job="${1:?usage: speed.sh search <job file relative to the repo> [budget_s] [threads]}"; budget="${2:-120}"; threads="${3:-8}"
    "$here/docker.sh" run "bash rust/tools/search_in_container.sh $job $budget $threads"
    ;;
  rust)
    # Only Rust, against the JVM numbers of the last `trials` run: for measuring a change.
    cases="${1:-}"
    "$here/docker.sh" run "
set -e
cd rust && cargo build --release --bin trial_bench 2>&1 | tail -1
/cache/target/release/trial_bench --dir /work/rust/tests/golden/trial --kotlin /work/out/trial-bench/trial_bench_kotlin.json${cases:+ --cases $cases}"
    ;;
  *)
    sed -n '2,9p' "$0"
    exit 1
    ;;
esac
