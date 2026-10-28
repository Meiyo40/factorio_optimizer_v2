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
| 4 validator, power | `validate`, `power` | |
| 5 router | `route` | |
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
