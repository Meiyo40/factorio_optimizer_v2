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
| 5 router | `route` | next: the router is most of a trial's time (see below) |
| 6 trial, search, blocks, CLI | `search`, `blocks`, `Main.kt` | |
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

## Where a trial's time goes

`tools/kotlin/TimeSplitTest.kt` runs real Kotlin trials under Java Flight Recorder (`tools/timesplit.sh`) and
attributes each sample to routing, placing, validating or powering:

| Box | Routing | Placing |
|-----|--------:|--------:|
| chemical 40, electromagnetic 46 (comfortable) | 29% | 69% |
| chemical 36, electromagnetic 41 (the tight frontier, trials of 14 to 27 s) | 89% | 11% |

The search spends its time at the frontier, so the router decides how much of the placer's speed-up survives. Its top
leaf frames there are `PathFinder.searchOnce` (23%), `java.util.HashMap.getNode` (22%, boxed-key lookups inside the A*
loop), `MinHeap.push` and `pop` (10% each) and `relax` (9%). A Rust router can use flat arrays for what Kotlin keeps in
hash maps, which is the main place to win in stage 5.

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
