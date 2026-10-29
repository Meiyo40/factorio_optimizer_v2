#!/usr/bin/env bash
# Runs inside the container (see speed.sh): one job searched by the Kotlin build and by the Rust one, with the same
# time budget and thread count, and what each left behind.
#
#   search_in_container.sh <job file, relative to /work> <budget_s> <threads>
set -euo pipefail
job="$1"; budget="$2"; threads="$3"
name="$(basename "$job" .json)"
dir="/work/out/search-bench/$name"
rm -rf "$dir" && mkdir -p "$dir"
# The same job for both: this budget and these threads, everything else as the job file says.
sed "s/\"time_budget_s\": *[0-9.]*/\"time_budget_s\": $budget/; s/\"threads\": *[0-9]*/\"threads\": $threads/" "$job" > "$dir/job.json"
grep -q '"time_budget_s": '"$budget" "$dir/job.json" || { echo "the job has no search section to set the budget in"; exit 1; }

echo "== Kotlin, budget ${budget} s, ${threads} threads"
JAVA_TOOL_OPTIONS=-Xmx6g bash ./gradlew --no-daemon run --args="--config=$dir/job.json --out=$dir/kotlin" 2>&1 | grep -E "done:" | cut -c1-200 | tail -1
bash rust/tools/search_report.sh "$dir/kotlin"

echo "== Rust, budget ${budget} s, ${threads} threads"
(cd rust && cargo build --release --bin factorio_optimizer 2>&1 | tail -1)
/cache/target/release/factorio_optimizer --config="$dir/job.json" --out="$dir/rust" | grep -E "done:" | cut -c1-200 | tail -1
bash rust/tools/search_report.sh "$dir/rust"
