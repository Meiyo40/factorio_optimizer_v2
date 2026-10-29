#!/usr/bin/env bash
# What a search left in its output folder, for comparing two runs of the same job: every improvement (when it came,
# the box, the score) and how many trials were done in the first 30 s, 60 s and in all. Used by speed.sh.
#
#   rust/tools/search_report.sh <output folder>
set -euo pipefail
dir="${1:?usage: search_report.sh <output folder>}"
echo "   improvements:"
tail -n +2 "$dir/scores.tsv" | awk -F'\t' '{ printf "     %6.1fs  %sx%s  score %s\n", $1, $2, $3, $4 }'
tail -n +2 "$dir/attempts.tsv" | awk -F'\t' '
  { all++; if ($1 <= 30) a30++; if ($1 <= 60) a60++; last = $1 }
  END { printf "   trials: %d in all (last finished at %.0f s), %d in the first 30 s, %d in the first 60 s\n", all, last, a30, a60 }'
