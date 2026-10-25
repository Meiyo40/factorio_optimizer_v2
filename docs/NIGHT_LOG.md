# Night log (2026-10-23 → 24)

Working notes for the unattended night run: what was measured, what was changed, what is open. Newest last.

## State at the start of the night

- All 64 tests pass. Nothing committed (the user commits).
- Benchmarks: `DiagTest` (env `DIAG=<job>,<size>,<seeds>`, optional `DIAG_TUNING=<file>`), run on snapshot
  copies of the project by `bench2.sh` in the session scratchpad so the working copy can change meanwhile.
- Full searches: chemical science 0.5/s → 37 × 37 in 5 min; red + green + chemical science 1/s from ore
  (110 machines) → 68 × 68 in 8 min (2 threads).

## What worked today (see STATUS.md D6 and TUNING.md for numbers)

1. Placer belt-access demand (biggest effect by far).
2. Router endgame (rip up and reroute near conflicts, undo worse rounds, start from the best iteration).
3. Direction-aware chain ends; placer feedback from the best iteration.
4. Underground weaving (express under fast).
5. Near-miss retries with a one-machine nudge.
6. Validator: per-lane items; producer allocation by max flow through consumer inserter capacities; machine
   input inserters by max flow; starvation located by an upstream-first walk.
7. Router bug: branch splitter on a side-fed curve (underground entrances had no recorded heading) — fixed.

## What did not help

- `max_chain_visits` 40 (fewer, longer, branching chains): worse than 12.
- `lane_sharing` on: valid, ~10% fewer chains, no success gain at measured sizes → opt-in (also needs v2/v13
  checked in game).
- `access_demand` 2 vs 1: about equal; 1 kept.

## Open questions for the night

- Six science (from ore, ~300 machines?) in one block: where does it fail?
- Parameter sweeps with more seeds: access demand/belt tiles/range, weave penalty, endgame and near-miss knobs,
  feedback rounds; lane sharing at the smallest sizes.

## Night entries

### Sweep 1 (started ~01:00): placer and router knobs, 8 seeds each

Tool: `SweepTest` (new; `SWEEP=<spec>`), 9 threads. Cases: chemical science 0.5/s at 36 and 38, the 54-machine
job (plates as inputs) at 50 and 52, the 110-machine job from ore at 66 and 70. Variants: base, access_demand
0.5 / 1.5, access_belt_tiles 2 / 4, access_range 3, weave_penalty 0.3 / 3, feedback_rounds 6, near_miss_max
10, lane_sharing on. Results: scratchpad `sweep_s1.tsv` (label, job, size, seed, stage, ms, overuse, failed,
score). Six science probe at 130 × 130 (1 seed) running alongside.

### Six science probe (all six packs at 1/s from ore)

- The plan has 618 machines, 44 nets, 1,550 terminals, 144 belt chains.
- 130 × 130, 1 seed: failed after 38 min. Rounds ended at 3,127 / 2,954 / 2,313 / 2,578 conflicts, many of
  them weave-tier underground channels. 130 is about the machine-area estimate (s_lo), far too small.
- Next: probe 180–220 later in the night (hours per trial).

### Sweep 1 results (528 trials, 8 seeds per cell; successes)

| Variant | chem 36 | chem 38 | 110 @ 66 | 110 @ 70 | 54 @ 50 | 54 @ 52 | total |
|---|---|---|---|---|---|---|---|
| base | 0 | 3 | 1 | 7 | 0 | 8 | 19/48 |
| access 0.5 | 0 | 3 | 1 | 8 | 0 | 6 | 18 |
| access 1.5 | 0 | 3 | 1 | 7 | 2 | 7 | 20 |
| belt tiles 2 | 0 | 3 | 0 | 8 | 0 | 6 | 17 |
| belt tiles 4 | 0 | 1 | 0 | 8 | 1 | 8 | 18 |
| range 3 | 0 | 3 | 0 | 8 | 0 | 7 | 18 |
| weave penalty 0.3 | 0 | 2 | 0 | 8 | 2 | 6 | 18 |
| weave penalty 3 | 0 | 3 | 0 | 8 | 0 | 4 | 15 |
| feedback rounds 6 | 0 | 2 | 1 | 8 | 0 | 8 | 19 |
| near miss ≤ 10 | 0 | 2 | 1 | 6 | 1 | 8 | 18 |
| lane sharing | 0 | 3 | 0 | 8 | 1 | 7 | 19 |

- All within noise: the knobs are on a plateau; defaults stay.
- Failed trials at the edge sizes end with a median of 20–40 conflicts (few near misses): placement-limited.
- One trial routed but failed validation: a pipe-to-ground entrance on a machine's fluid connection tile (it
  joins only through its open side, so the machine got no fluid). **Fixed** in `Pipes.kt` (no pipe-to-ground
  end on a connection tile) and covered by a new fluid fuzz test (40 random oil-processing instances; every
  success must validate; 21 route).

### Sweep 2 (started ~01:35): placer weights, 8 seeds each

Cases: chem 38, 54 @ 50 and 52, 110 @ 66. Variants: base, anneal effort ×4 (`moves_per_machine` 80k, new knob,
default 20k), beta 40 / 10, feedback weight beta2 1 / 3, RUDY bin 3 / 6, gamma 24, delta 3 / 12.

### Sweep 2 results (352 trials; successes of 8)

| Variant | chem 38 | 110 @ 66 | 54 @ 50 | 54 @ 52 | total | avg s |
|---|---|---|---|---|---|---|
| base | 2 | 1 | 0 | 8 | 11/32 | 59 |
| moves ×4 | 3 | 1 | 1 | 6 | 11 | 104 |
| beta 40 | 1 | 1 | 0 | 5 | 7 | 66 |
| beta 10 | 4 | 0 | 0 | 7 | 11 | 61 |
| beta2 1 | 5 | 0 | 2 | 7 | 14 | 59 |
| beta2 3 | 2 | 0 | 2 | 7 | 11 | 58 |
| RUDY bin 3 | 3 | 0 | 1 | 8 | 12 | 60 |
| RUDY bin 6 | 1 | 0 | 0 | 5 | 6 | 72 |
| gamma 24 | 4 | 0 | 0 | 8 | 12 | 58 |
| delta 12 | 4 | 0 | 0 | 8 | 12 | 58 |
| delta 3 | 0 | 0 | 0 | 6 | 6 | 68 |

Placer weights are on a plateau as well (bin 6, delta 3 and beta 40 are worse; nothing clearly better).
More anneal effort does not help. Next gains need structural changes. Meanwhile (02:15) a 45-minute full search
on the 110-machine job with 9 threads, starting at 72 (`solves/3_long_night`).

### Six science at 200 × 200 (1 seed)

Failed after 76 min; rounds ended at 257 / 168 / 223 / 303 conflicts (vs 2,300+ at 130). Negotiation reaches
~170 and then diverges; the endgame never ran (it starts only at ≤ 150 conflicts). Six science in one block is
out of reach tonight at > 1 h per trial; the endgame threshold should scale with block size.

### 110-machine full search, 45 min, 9 threads

Best 65 × 65 (from 68 × 68 in 8 min with 2 threads); 66 succeeds about 1 in 3 seeds, 64 none in 18.
`solves/3_long_night`.

### Changes at ~03:00

- **Spread retries** (new, `spread_max` 60, `spread_attempts` 3, `spread_radius` 3): a trial whose best round
  ended with 5–60 conflicts moves the machines within 3 tiles of each conflict cluster one tile outward
  (outermost first) and routes again, building on any attempt that got closer; near-miss nudges follow if it
  gets within 4.
- **Endgame threshold scales with block size**: it starts at ≤ max(150, 2 × chains) conflicts
  (`endgame_overuse_per_chain`), so six science (≈150 chains) gets an endgame too.
- Sweep 3 (router knobs: p_mult, h_fac, iterations, ug penalty, pin reservation, beta2 1) runs on the code
  before these changes; sweep 4 (spread vs no spread vs 6 feedback rounds, radius 4, max 120; chem 36/38,
  54 @ 50, 110 @ 64/66, 10 seeds) follows on the new code.

### Sweep 3 results (router knobs; 352 trials, successes of 8)

| Variant | chem 38 | 110 @ 66 | 54 @ 50 | 54 @ 52 | total |
|---|---|---|---|---|---|
| base | 2 | 2 | 0 | 8 | 12/32 |
| p_mult 1.2 (90 iterations) | 6 | 2 | 2 | 8 | **18** |
| p_mult 2 | 2 | 0 | 0 | 6 | 8 |
| h_fac 0.25 / 1 | 2 / 3 | 0 / 0 | 0 / 1 | 7 / 6 | 9 / 10 |
| 100 iterations | 1 | 1 | 0 | 8 | 10 |
| ug_penalty 1 / 4 | 3 / 3 | 0 / 0 | 0 / 0 | 6 / 7 | 9 / 10 |
| pin_reservation 0 / 3 | 1 / 0 | 0 / 1 | 0 / 0 | 7 / 6 | 8 / 7 |
| beta2 1 | 3 | 0 | 2 | 7 | 12 |

Slower present-cost growth (p_mult 1.2) is the first clear gain from tuning, matching the traces where
negotiation peaks early and then diverges. Sweep 5 (queued after sweep 4) checks p_mult 1.1 / 1.2 / 1.3 / 1.5
with fresh seeds 11–22 on the spread code.

### Sweep 4 results (spread retries; 250 trials, successes of 10)

| Variant | chem 36 | chem 38 | 110 @ 64 | 110 @ 66 | 54 @ 50 | total | avg s |
|---|---|---|---|---|---|---|---|
| no spread | 0 | 2 | 0 | 1 | 0 | 3/50 | 80 |
| spread (60, r 3, 3 tries) | 0 | 2 | 0 | 2 | 0 | 4 | 115 |
| 6 feedback rounds, no spread | 0 | 5 | 0 | 2 | 2 | 9 | 120 |
| spread r 4, 4 tries | 0 | 2 | 0 | 1 | 0 | 3 | 129 |
| spread up to 120 | 0 | 2 | 0 | 1 | 0 | 3 | 120 |

Spreading does not help and costs ~45% more time per trial: now off by default (`spread_max` 0). Six feedback
rounds did a bit better here (9/50) but not in sweep 1 (19 vs 19): inconclusive.

### Sweep 5 results (present-cost growth p_mult; 240 trials, seeds 11–22, successes of 12)

| p_mult (max iterations) | chem 36 | chem 38 | 110 @ 64 | 110 @ 66 | 54 @ 50 | total | avg s |
|---|---|---|---|---|---|---|---|
| 1.5 (60) | 0 | 4 | 0 | 1 | 2 | 7/60 | 120 |
| 1.3 (75) | 0 | 5 | 0 | 1 | 3 | 9 | 119 |
| 1.2 (90) | 0 | 8 | 0 | 1 | 4 | 13 | 107 |
| 1.1 (120, stagnation 40) | 0 | 7 | 0 | 5 | 7 | **19** | 145 |

Monotonic: slower growth routes more. **New defaults** (tuning.json and Tuning.kt): p_mult 1.1, max_iterations
120, stagnation_stop 40. The negotiation test pins the old schedule (with 1.1 the router finds a detour instead
of the underground the test looks for). 66 tests pass.

### Sweep 6 (started ~05:25): around the new default

p_mult 1.05 (180 iterations) and, at 1.1: 6 feedback rounds, h_fac 0.25, p_fac_first 1.0, beta2 1.0.
Chem 38, 54 @ 50, 110 @ 65 / 66, seeds 23–32.

### Sweep 6 results (240 trials, seeds 23–32, successes of 10)

| Variant | chem 38 | 110 @ 65 | 110 @ 66 | 54 @ 50 | total | avg s |
|---|---|---|---|---|---|---|
| p_mult 1.1 (default) | 6 | 0 | 2 | 6 | 14/40 | 119 |
| p_mult 1.05 (180 iterations) | 8 | 1 | 4 | 5 | 18 | 181 |
| + 6 feedback rounds | 7 | 0 | 3 | 8 | 18 | 185 |
| + h_fac 0.25 | 8 | 1 | 2 | 4 | 15 | 116 |
| + p_fac_first 1.0 | 4 | 0 | 2 | 4 | 10 | 118 |
| + beta2 1.0 | 8 | 0 | 3 | 6 | 17 | 117 |

p_mult 1.05 and 6 feedback rounds route a little more per trial but take ~55% longer, so per minute of
search they are not better. beta2 1.0 is better at no cost and was ahead in sweeps 2 and 6 and level in
sweep 3 (43 vs 37 successes over 104 trials each): **new default beta2 1.0**. 66 tests pass.

### Final searches (06:30–08:30)

Chemical science 0.5/s and the 110-machine job, 2 h each, 5 threads each, with the tuned defaults:
`solves/1_long_big_throughput_night`, `solves/3_long_night2`.

### Final search results (2 h each, 5 threads, tuned defaults)

- Chemical science 0.5/s from ore and crude oil: **37 × 37** (reached in 3 min; 36 never routed in 675
  trials). `solves/1_long_big_throughput_night`.
- Red + green + chemical science 1/s from ore (110 machines): **64 × 64** (at 20 min; 65 × 65 the night before,
  68 × 68 in the evening). `solves/3_long_night2`.

## Summary of the night

- Changed defaults: `p_mult` 1.1 / 120 iterations / stagnation 40, `beta2` 1.0, `spread_max` 0 (new feature,
  off), endgame threshold per chain (new), `moves_per_machine` (new knob, unchanged default).
- Fixed: pipe-to-ground on a fluid connection tile (validation failure found by the sweeps).
- New tests and tools: fluid fuzz test, `SweepTest`, `DiagTest` seed option.
- ~1,700 benchmark trials: everything else is on a plateau; the remaining limit is placement structure.
  Six science from ore (618 machines) does not fit one block yet; split it by hand.
