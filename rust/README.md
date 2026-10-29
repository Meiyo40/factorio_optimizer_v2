# factorio_optimizer (Rust port)

A port of the Kotlin generator in the parent directory, in progress. The layout mirrors the Kotlin tree
(`src/main/kotlin/<package>/<File>.kt` becomes `src/<package>/<file>.rs`), so an upstream change can be
re-applied file by file. Ported from upstream commit `022c312`. The Kotlin sources are not changed by this port.

Resources (`recipes.json`, `tuning.json`) are compiled in from `../src/main/resources`.

```sh
cargo test
```

| Stage | Kotlin package | State |
|-------|----------------|-------|
| 1 foundations | `data`, `config`, `geom`, `emit` (blueprint codec) | done |
| 2 planner | `plan` | done (LP on `microlp`) |
| 3 placer | `place` | done, same-seed parity with Kotlin (see below) |
| 4 validator, power | `validate`, `power` | done, same verdicts and messages as Kotlin (see below) |
| 5 router | `route`, `search/Trial.kt` (`validationSpec`, `layoutOf`) | done, same routes as Kotlin (see below) |
| 6 trial, search, blocks, CLI | `search`, `blocks`, `Main.kt` | trial, search, block splitter and compaction done, same results as Kotlin (see below); factory and CLI next |
| 7 renders, verify kit, calibrate | `viz`, `emit/VerifyKit.kt`, `search/Calibrate.kt` | |

Not ported on purpose: `RecipeIndex` and `computeRates` in `Rates.kt` (nothing in Kotlin's `src/main` uses them),
and the item and fluid links that `interlinkProductsWithItems` fills in (look up `RootData::items` by name).

## The placer matches Kotlin exactly

`tests/place_golden.rs` replays ten runs that the Kotlin placer made (random, column, row and warm starts, a router
feedback round, fluid-heavy and Space Age plans) from the plan Kotlin built and the same seed. The Rust placer
reproduces every machine's position and orientation, the move and temperature counts, and the cost terms to 1e-9.
That also checks the `SplittableRandom` port, since any wrong draw changes the whole run.

Two details that exactness depends on, both easy to break:

- A move with a cost change of exactly 0 is accepted without drawing a random number, a change of 1e-16 draws one.
  So the order of floating point sums matters. The direct-insertion pairs of a move are summed in the iteration order
  of a `java.util.HashSet<Integer>` (`java_hash_order` in `place_model.rs`). Pin penalties are whole numbers, so
  their order does not matter.
- `10 * n^(4/3)` (moves per temperature) is checked against Java's `Math.pow` for every machine count up to 700,
  because at perfect cubes one ulp decides between 160 and 159.

The goldens are in `tests/golden/place/`, made by `tools/kotlin/GoldenPlaceTest.kt` from upstream `022c312`.

## The validator matches Kotlin exactly

`tests/validate_golden.rs` validates each of the eight committed blueprints as committed, and five of them also with
28 to 31 seeded mutations each (a dropped, turned or moved entity, a refiltered inserter, up to two at once). Rust's
error list must equal Kotlin's: same errors, same order, same message text. The blueprints as committed give 0 to 1433
errors (the 1433 is a blueprint from before the curve-lane fix), so agreeing on failing layouts is covered as well.
`tests/power_golden.rs` does the same for pole placement (five placements on each of three blueprints, three of those
fifteen failing) and for the test harness, and checks the placer against the validator. `tests/power.rs` covers bridge
poles between machines out of wire reach, which no golden reaches. The goldens are in `tests/golden/validate/` and
`tests/golden/power/`, made by `tools/kotlin/GoldenValidateTest.kt`.

What exactness depended on, and what the differential tests found:

- Kotlin's `HashMap()`, `HashSet()` and `hashSetOf()` iterate in `java.util.HashMap` order; `mutableMapOf`, `associate*`,
  `groupBy` and `toSet` in insertion order. The validator numbers its flow graph and sums floats in that order, so
  `util/java_hash.rs` (`JMap`, `JSet`) reproduces the Java order, and `tests/java_golden.rs` checks it against
  orders Java itself produced for int, string, pair and tile keys. One thing is not modelled: a ninth key in one bucket
  makes Java resize or treeify the table. Debug builds assert that no bucket gets that full.
- Java's `%.2f` rounds half up on the shortest decimal digits of the double (0.285 prints 0.29), Rust's `{:.2}` rounds
  the exact binary value. `util/text.rs` `fmt2` follows Java, checked against 150+ samples Java formatted.
- Rust's `Iterator::sum::<f64>()` starts from `-0.0`, Kotlin's `sum` and `sumOf` from `+0.0`, so an empty sum printed
  `-0.00` instead of `0.00`. Only the differential test saw it. Use `util::sum_f64` for float sums.
- `maxByOrNull` and `minByOrNull` return the first extremum, Rust's `max_by` the last, so those are plain loops.

Regenerate the goldens (Docker Desktop running, repo root):

```sh
rust/tools/docker.sh sync
rust/tools/docker.sh run 'VALIDATE_OUT=/work/out/validate-golden bash ./gradlew --no-daemon test --rerun --tests GoldenValidateTest.dumpValidatorAndPowerGoldens'
rust/tools/docker.sh out rust/bench-results
cp -r rust/bench-results/validate-golden/. rust/tests/golden/
```

## The router matches Kotlin exactly

`tests/route_golden.rs` replays 77 routes Kotlin made, from the plan Kotlin built, the placement and the router's
inputs. They cover the six hand placements of `RouterTest` and variants with an option changed (undergrounds off,
weaving off, ports inside the box, a kept chain order, splitters off); placer-made placements of the test jobs and
the examples, in pipe and infinity-pipe fluid mode and with chest sinks; tight boxes where negotiation fails and the
endgame runs; second attempts with a warm history, tile bias, endgame seed and tunnel ban; single-belt ports and an
item with several input ports; and 30 belt and 10 fluid instances of `RouterFuzzTest`'s generator. 63 of the 77
succeed, the rest fail the way Kotlin's do. For each case Rust must reproduce:

- the router's setup (chains, their terminals, every access option), checked first so a setup mistake is not
  mistaken for a routing one,
- the whole result: every entity in order, the iteration count, the score after each iteration, the number of A*
  states expanded, the conflicts and the plan nets at them, the congestion history in the order Kotlin's hash map
  iterates it, and the log,
- the validation spec built from it, the validator's verdict on the routed layout, and the ring ports.

Changing one comparison in the heap's tie rule (`<=` to `<` on push) fails every shard at iteration 0, so the test
does see a single wrong tie-break. `tests/router.rs` adds `RouterTest`'s outcome tests and `RouterFuzzTest`'s 200
belt and 40 fluid random instances, where every success must pass the validator.

Details that exactness depended on, besides those listed for the validator:

- The endgame stops starting rounds after a time (`endgame_time_factor` times what negotiation took) and shuffles with
  a `java.util.Random`. Rust keeps both; `util/java_random.rs` is checked against what the JVM produced, and the goldens
  run with the time factor at 1e6 so that only the round count ends the endgame.
- `solveSinks` walks a `HashMap<Int, ...>` and each sink changes the prices the next sees, so the order is Java's
  (`JMap`). The conflict tiles come from a `HashSet<Int>` and the trial clusters them in list order, so they use
  `JSet` as well.
- Kotlin's `Double.compareTo` puts NaN above everything and -0.0 below 0.0. A chain without consumers has a NaN
  centroid, and Rust's `total_cmp` would order the NaN `0.0 / 0.0` makes on x86 below everything, so sorts use
  `util::compare_f64`.
- The heap is not `BinaryHeap`: it has no sequence numbers, and its sift rules decide which of equal keys comes out.

Regenerate the goldens (Docker Desktop running, repo root):

```sh
rust/tools/docker.sh sync
rust/tools/docker.sh run 'UPSTREAM=022c312 ROUTE_OUT=/work/out/route-golden JAVA_TOOL_OPTIONS=-Xmx6g bash ./gradlew --no-daemon test --rerun --tests GoldenRouteTest.dumpRouteGoldens'
rust/tools/docker.sh out rust/bench-results
cp -r rust/bench-results/route-golden/. rust/tests/golden/route/
```

## The trial, the search and the block splitter match Kotlin exactly

`tests/trial_golden.rs` replays 89 trials Kotlin ran (job, box, seed, `runTrial`'s options) and compares the whole
`TrialResult`: where it stopped, the notes of every feedback round and retry, the score and the annealing
temperature, the layout entity by entity with poles and wires, the placement, the ports, the validator's errors
and the summary of how routing went. The cases cover the comfortable and the tight sizes of the test jobs (failed
feedback rounds, the near-miss retry), the spread retry (forced with `spread_max`), column and mixed starts, warm
starts with and without a temperature, test mode, lane sharing, reserved areas, ports inside the box, the lattice
and no power, the power reroute that keeps tiles free near unpowered entities, fluid and Space Age jobs, and the
examples. `the_cases_reach_every_branch` fails if a branch loses its last case. One branch has none: when the set
cover fails twice the trial falls back to the pole lattice; no job tried (tight boxes, small poles, reserved areas
in the way) got there, so that call is only covered by the lattice mode's own cases.

`tests/search_golden.rs` replays five searches (one thread, a budget that is never reached, `extra_rounds` of 2): the
improvements, the number of trials, the best result, every log line (clocks aside), the rows of `attempts.tsv` and
`scores.tsv`, `ports.json` byte for byte, and the best blueprint, which is compared by what it decodes to (two zlibs
compress to different bytes). `tests/search.rs` adds `SearchTest`'s outcome tests and checks that four threads find what
one does. `tests/split_golden.rs` replays 20 splits of jobs into blocks (the split, every block's plan, the split
after merging each pair of neighbours) and 13 compactions, and `tests/trial.rs` holds `TrialTest`'s and `SpaceAgeTest`'s
outcome tests.

What exactness depended on, besides what is listed above:

- `java.util.HashMap.merge` links a new node in at the head of its bucket and checks whether to grow before it looks
  for the key, where `put` appends and checks after. `splitPlan` fills two maps with `merge` and walks them, and an
  item's links come out in a different order (and are numbered differently) if the model treats `merge` as `put`:
  `JMap` now tracks both. It also supports `remove`, which leaves the table as large as it was.
- The harness's tile set is a `HashSet` that the power placer walks to build its candidate list; it is a `JSet`, in
  Java's order, where a Rust `HashSet` would give a different order in every process.
- Seeds, scores and times print through `fmt_fixed`, Java's `%.nf`, which rounds half up on the shortest decimals.

Not ported until the viz stage: the PNG renders (`best.png`, `renders/`, the score graph, the `debug_renders`
option) and the Ctrl-C hook. Every file the search writes is written to a temporary name and renamed, so a kill leaves
the last complete result instead.

Regenerate the goldens (Docker Desktop running, repo root); `SPLIT_OUT`, `SEARCH_OUT` and `FACTORY_OUT` work the same
way with `dumpSplitGoldens`, `dumpSearchGoldens` and `dumpFactoryGoldens`:

```sh
rust/tools/docker.sh sync
rust/tools/docker.sh run 'UPSTREAM=022c312 TRIAL_OUT=/work/out/trial JAVA_TOOL_OPTIONS=-Xmx6g bash ./gradlew --no-daemon test --rerun --tests GoldenTrialTest.dumpTrialGoldens'
rust/tools/docker.sh out rust/bench-results
cp -r rust/bench-results/trial/. rust/tests/golden/trial/
```

## Router speed against the JVM

Same container, plans, placements and tuning as the placer's benchmark above; one route per run (building the router
and running it), 3 warm-up runs, median of 5, JVM with `-Xmx6g`. "Same work" means the same iterations, A* states
expanded and entities, which the benchmark checks. The tight cases do not route (they stop after 60 iterations); they
are what the search spends its time on.

| Case | Machines | Iterations | A* states | JVM | Rust | Ratio |
|------|---------:|-----------:|----------:|----:|-----:|------:|
| red + green science, 20x20 | 10 | 16 | 13k | 8.1 ms | 3.9 ms | 2.07x |
| chemical science, 40x40 | 43 | 18 | 91k | 80 ms | 40 ms | 2.01x |
| chemical science, tight 36x36 | 43 | 60 | 11.0M | 4.7 s | 3.7 s | 1.29x |
| electromagnetic science, 46x46 | 36 | 66 | 5.1M | 2.6 s | 2.0 s | 1.30x |
| electromagnetic science, tight 41x41 | 36 | 60 | 5.6M | 6.7 s | 5.2 s | 1.27x |
| metallurgic science, 52x52 | 38 | 25 | 132k | 266 ms | 212 ms | 1.26x |

At the tight sizes the gain is what the profile below predicts. The JFR samples showed `HashMap.getNode` as 22% of the
leaf frames, and it comes from the pipe router: `relax` asks a boxed `HashSet<Int>` whether a state is banned on every
relaxation, and the search loop looks up the B2 trick by name on every expansion. Rust does the first with a stamp array
and the second once, which is worth 1 / (1 - 0.22) = 1.28x; the rest of the loop (A* over flat arrays) runs at about the
speed of the JIT's code.

```sh
rust/tools/docker.sh sync
rust/tools/docker.sh run 'BENCH_OUT=/work/out/route-bench JAVA_TOOL_OPTIONS=-Xmx6g bash ./gradlew --no-daemon test --rerun --tests GoldenRouteTest.bench; cd rust && cargo build --release --bin route_bench && /cache/target/release/route_bench --dir /work/out/route-bench'
```

## Where a trial's time goes

`tools/kotlin/TimeSplitTest.kt` runs real Kotlin trials under Java Flight Recorder (`tools/timesplit.sh`) and
attributes each sample to routing, placing, validating or powering:

| Box | Routing | Placing |
|-----|--------:|--------:|
| chemical 40, electromagnetic 46 (comfortable) | 29% | 69% |
| chemical 36, electromagnetic 41 (the tight frontier, trials of 14 to 27 s) | 89% | 11% |

The search spends its time at the frontier, so the router decides how much of the placer's speed-up survives. Its top
leaf frames there are `PathFinder.searchOnce` (23%), `java.util.HashMap.getNode` (22%, boxed-key lookups inside the A*
loop), `MinHeap.push` and `pop` (10% each) and `relax` (9%). Stage 5 measured what the Rust router gains: see above.

## Speed against the JVM

Both run in one Linux container (Java 25 and Rust 1.99, 20 logical CPUs of an i7-12700KF, 15 GB) on the same plans,
boxes, seeds and move budgets, so they do exactly the same work (the benchmark checks that). One anneal from a fresh
random placement, only the anneal timed, 3 warm-up runs, median of 7, JVM with the project's `-Xmx6g`. "x8" is eight
anneals at once. Moves per second:

| Case | Machines | JVM 1 thread | Rust 1 thread | Ratio | JVM x8 | Rust x8 | Ratio |
|------|---------:|-------------:|--------------:|------:|-------:|--------:|------:|
| red + green science | 10 | 230k | 447k | 1.94x | 1.26M | 2.73M | 2.16x |
| electromagnetic science | 36 | 146k | 252k | 1.72x | 849k | 1.65M | 1.95x |
| chemical science | 43 | 139k | 260k | 1.87x | 795k | 1.68M | 2.11x |
| six science in one box | 177 | 65k | 74k | 1.13x | 424k | 523k | 1.23x |

Run-to-run noise is about 5-10% for the JVM and 1-3% for Rust. The JVM column already includes the `spread` change
below, applied to the container's copy of the Kotlin only.

A profile (callgrind, 177 machines) showed `PlaceModel.spread`, the RUDY congestion update over a net's bounding box,
at about 70% of all instructions. Only the first and last bin of a row are partly covered; the bins between get the
same amount, so they can be added in one pass over a slice. The same rewrite speeds up the Kotlin as well: 41k to 65k
moves per second at 177 machines, with identical results. The advantage of Rust shrinks as boxes get bigger (1.9x at
10 machines, 1.1x at 177), so blocks near `max_machines` gain less than the small examples.

Reproduce (Docker Desktop running; from Git Bash or PowerShell, in the repo root):

```sh
rust/tools/docker.sh build
rust/tools/docker.sh sync
rust/tools/docker.sh run 'GOLDEN_OUT=/work/out/place bash ./gradlew --no-daemon test --rerun --tests GoldenPlaceTest.dumpGoldens'
rust/tools/docker.sh run 'export BENCH_OUT=/work/out/bench; JAVA_TOOL_OPTIONS=-Xmx6g bash ./gradlew --no-daemon test --rerun --tests GoldenPlaceTest.bench; cd rust && cargo build --release --bin place_bench && /cache/target/release/place_bench --dir /work/out/bench'
rust/tools/docker.sh out rust/bench-results
```

Licensed GPL-3.0 like the original.
