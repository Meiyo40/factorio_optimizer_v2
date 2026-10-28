import config.PlaceTuning
import config.RunConfiguration
import config.Tuning
import config.strictJson
import geom.Dir
import geom.Tile
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.addJsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import place.AnnealStats
import place.Annealer
import place.PlaceModel
import place.PlacedMachine
import place.Placement
import place.staticObstacles
import plan.ProductionPlan
import plan.buildPlan
import java.io.File
import java.util.SplittableRandom
import java.util.concurrent.Callable
import java.util.concurrent.Executors
import kotlin.math.pow
import kotlin.test.Test

/**
 * Reference outputs for the Rust port of the placer, and its benchmark. Not part of the Kotlin project: the Rust
 * tooling (rust/tools/docker.sh) copies this file into the container's work tree.
 *
 *   GOLDEN_OUT=<dir>  writes one JSON per case (plan, parameters, exact result) and the plans they use
 *   BENCH_OUT=<dir>   times the annealer (BENCH_WARM, BENCH_REPS, BENCH_THREADS, BENCH_MAXMOVES) and writes bench.json
 *
 * Both are skipped when their variable is not set, like DiagTest and SweepTest.
 */
class GoldenPlaceTest {
    private val pretty = Json { prettyPrint = true }

    private class Step(
        val t0: Double? = null,
        /** Alternative to [t0]: the previous anneal's last temperature divided by this (the feedback rounds). */
        val t0LastDivisor: Double? = null,
        val maxMoves: Long,
        val startRange: Int? = null,
        val refine: Boolean = false,
        val t0Scale: Double = 1.0,
        /** (x, y, value) router history handed to the model before this anneal, with [beta2]. */
        val history: List<Triple<Int, Int, Double>>? = null,
        val beta2: Double = 0.0,
    )

    private class Case(
        val name: String, val job: String, val w: Int, val h: Int, val seed: Long, val tuning: PlaceTuning,
        val start: String, val steps: List<Step>, val from: String? = null,
    )

    private fun load(job: String): Pair<RunConfiguration, ProductionPlan> {
        val config = RunConfiguration.load(job)
        val tuning = Tuning.defaults().withOverrides(config.tuning)
        return config to buildPlan(TestData.data, config, tuning)
    }

    private fun budget(plan: ProductionPlan, t: PlaceTuning): Long =
        minOf(t.maxMoves, t.movesPerMachine * plan.machines.size.coerceAtLeast(5))

    private fun codeOf(m: PlaceModel, i: Int) = m.rot[i]

    private fun placementJson(m: PlaceModel): List<JsonElement> = (0 until m.nM).map { i ->
        buildJsonObject { put("id", i); put("x", m.px[i]); put("y", m.py[i]); put("code", codeOf(m, i)) }
    }

    private fun termsJson(m: PlaceModel) = buildJsonObject {
        put("wl", m.wl); put("cong", m.cong); put("pin", m.pin); put("di", m.di); put("fb", m.fb); put("cost", m.cost)
    }

    private fun statsJson(s: AnnealStats) = buildJsonObject {
        put("initial_cost", s.initialCost); put("final_cost", s.finalCost); put("t0", s.t0)
        put("moves", s.moves); put("temperatures", s.temperatures)
    }

    @Test
    fun dumpGoldens() {
        val out = System.getenv("GOLDEN_OUT")?.let { File(it) } ?: return
        File(out, "plans").mkdirs()
        val measured = Tuning.defaults().place
        val builtin = PlaceTuning()
        val colTun = builtin.copy(beltCost = "path", initial = "columns", columnShare = 0.2)
        val chemCols = measured.copy(initial = "columns")
        val feedbackHistory = listOf(Triple(5, 5, 3.0), Triple(10, 12, 1.5), Triple(6, 5, 0.5), Triple(11, 12, 2.0), Triple(3, 20, 1.0))
        val jobs = "src/test/resources/jobs"
        val ex = "docs/examples"
        fun full(plan: ProductionPlan, t: PlaceTuning) = listOf(Step(maxMoves = budget(plan, t)))
        val cases = ArrayList<Case>()
        fun case(name: String, job: String, w: Int, h: Int, seed: Long, t: PlaceTuning, start: String, steps: (ProductionPlan) -> List<Step>, from: String? = null) {
            cases += Case(name, job, w, h, seed, t, start, steps(load(job).second), from)
        }
        case("t2_random_14_s1", "$jobs/t2.json", 14, 14, 1, builtin, "random", { full(it, builtin) })
        case("t3_random_24_s2", "$jobs/t3.json", 24, 24, 2, measured, "random", { full(it, measured) })
        case("t4_random_20_s3", "$jobs/t4.json", 20, 20, 3, builtin, "random", { full(it, builtin) })
        case("chem_random_36_s7", "$ex/chemical_science.json", 36, 36, 7, measured, "random", { full(it, measured) })
        case("em_random_41_s2", "$ex/electromagnetic_science.json", 41, 41, 2, measured, "random", { full(it, measured) })
        case("t3_columns_path_24_s5", "$jobs/t3.json", 24, 24, 5, colTun, "columns",
            { listOf(Step(maxMoves = budget(it, colTun), startRange = colTun.columnStartRange, refine = true, t0Scale = colTun.columnT0Scale)) })
        case("chem_columns_40_s4", "$ex/chemical_science.json", 40, 40, 4, chemCols, "columns",
            { listOf(Step(maxMoves = budget(it, chemCols), startRange = chemCols.columnStartRange, refine = true, t0Scale = chemCols.columnT0Scale)) })
        case("t3_rows_24", "$jobs/t3.json", 24, 24, 9, measured, "rows", { emptyList() })
        case("t3_feedback_24_s6", "$jobs/t3.json", 24, 24, 6, measured, "random",
            { full(it, measured) + Step(t0LastDivisor = measured.feedbackT0Divisor, maxMoves = budget(it, measured) / 2, startRange = 3, history = feedbackHistory, beta2 = measured.beta2) })
        case("t3_warm_24_s8", "$jobs/t3.json", 24, 24, 8, measured, "from",
            { listOf(Step(maxMoves = budget(it, measured), startRange = 3, refine = true, t0Scale = measured.columnT0Scale)) }, from = "t3_random_24_s2")

        val results = HashMap<String, List<JsonElement>>()
        for (c in cases) {
            val (config, plan) = load(c.job)
            File(out, "plans/${File(c.job).nameWithoutExtension}.plan.json").writeText(plan.toJson())
            val model = PlaceModel(plan, config, c.tuning, c.w, c.h, staticObstacles(config, plan, c.w, c.h))
            val annealer = Annealer(model, SplittableRandom(c.seed), c.tuning)
            val fromPlacement = c.from?.let { name ->
                val prev = results.getValue(name)
                prev.map { (it as JsonObject).let { o -> Triple(o["id"]!!.toString().toInt(), o["x"]!!.toString().toInt() to o["y"]!!.toString().toInt(), o["code"]!!.toString().toInt()) } }
            }
            val started = when (c.start) {
                "random" -> annealer.initialRandom()
                "columns" -> annealer.initialColumns(plan)
                "rows" -> annealer.initialRows(plan)
                "from" -> annealer.initialFrom(Placement(c.w, c.h, fromPlacement!!.map { (id, p, code) -> PlacedMachine(id, p.first, p.second, Dir.entries[code % 4], code >= 4) }))
                else -> error("start ${c.start}")
            }
            check(started) { "${c.name}: no initial placement" }
            val initial = buildJsonObject { putJsonArray("placement") { placementJson(model).forEach { add(it) } }; put("terms", termsJson(model)) }
            val stats = c.steps.map { s ->
                s.history?.let { model.setHistory(s.history.associate { (x, y, v) -> Tile(x, y) to v }, s.beta2) }
                val t0 = s.t0 ?: s.t0LastDivisor?.let { annealer.lastT0 / it }
                statsJson(annealer.anneal(t0 = t0, maxMoves = s.maxMoves, startRange = s.startRange, refine = s.refine, t0Scale = s.t0Scale))
            }
            results[c.name] = placementJson(model)
            val json = buildJsonObject {
                put("upstream", System.getenv("UPSTREAM") ?: "unknown")
                put("case", c.name); put("job", c.job); put("plan", "plans/${File(c.job).nameWithoutExtension}.plan.json")
                put("w", c.w); put("h", c.h); put("seed", c.seed); put("start", c.start)
                if (c.from != null) {
                    put("from", c.from)
                    putJsonArray("from_placement") { fromPlacement!!.forEach { (id, p, code) -> addJsonObject { put("id", id); put("x", p.first); put("y", p.second); put("code", code) } } }
                }
                put("place_tuning", strictJson.encodeToJsonElement(PlaceTuning.serializer(), c.tuning))
                putJsonArray("steps") {
                    c.steps.forEach { s ->
                        addJsonObject {
                            put("t0", s.t0?.let { JsonPrimitive(it) } ?: JsonPrimitive(null as String?))
                            put("t0_last_divisor", s.t0LastDivisor?.let { JsonPrimitive(it) } ?: JsonPrimitive(null as String?))
                            put("max_moves", s.maxMoves)
                            put("start_range", s.startRange?.let { JsonPrimitive(it) } ?: JsonPrimitive(null as String?))
                            put("refine", s.refine); put("t0_scale", s.t0Scale); put("beta2", s.beta2)
                            if (s.history != null) putJsonArray("history") { s.history.forEach { (x, y, v) -> addJsonObject { put("x", x); put("y", y); put("v", v) } } }
                        }
                    }
                }
                put("initial", initial)
                put("result", buildJsonObject {
                    putJsonArray("placement") { placementJson(model).forEach { add(it) } }
                    put("terms", termsJson(model))
                    putJsonArray("stats") { stats.forEach { add(it) } }
                    put("last_t0", annealer.lastT0)
                })
            }
            File(out, "${c.name}.json").writeText(pretty.encodeToString(JsonObject.serializer(), json))
            println("golden ${c.name}: ${plan.machines.size} machines, cost ${"%.4f".format(model.cost)}, moves ${stats.joinToString { (it["moves"] as JsonPrimitive).content }}")
        }
        // The annealer's moves per temperature use pow(4/3); at perfect cubes Java and Rust may round differently.
        File(out, "per_temp_x10.json").writeText((1..700).joinToString(prefix = "[", postfix = "]") { n -> (10.0 * n.toDouble().pow(4.0 / 3.0)).toInt().toString() })
    }

    private class BenchCase(val name: String, val job: String, val w: Int, val h: Int, val seed: Long)

    @Test
    fun bench() {
        val out = System.getenv("BENCH_OUT")?.let { File(it) } ?: return
        File(out, "plans").mkdirs()
        val warm = System.getenv("BENCH_WARM")?.toInt() ?: 3
        val reps = System.getenv("BENCH_REPS")?.toInt() ?: 7
        val threads = System.getenv("BENCH_THREADS")?.toInt() ?: 8
        val cap = System.getenv("BENCH_MAXMOVES")?.toLong() ?: 400_000L
        val tun = Tuning.defaults().place
        val ex = "docs/examples"
        val cases = listOf(
            BenchCase("m10_red_green", "$ex/red_green_science.json", 24, 24, 1),
            BenchCase("m36_electromagnetic", "$ex/electromagnetic_science.json", 41, 41, 1),
            BenchCase("m43_chemical", "$ex/chemical_science.json", 40, 40, 1),
            BenchCase("m177_six_science", "$ex/six_science_factory.json", 120, 120, 1),
        )
        val report = ArrayList<JsonElement>()
        // The Rust benchmark runs exactly these cases (same plans, boxes, seeds and move budgets).
        File(out, "bench_cases.json").writeText(pretty.encodeToString(JsonObject.serializer(), buildJsonObject {
            putJsonArray("cases") {
                cases.forEach { c ->
                    addJsonObject {
                        val plan = load(c.job).second
                        put("case", c.name); put("job", c.job); put("w", c.w); put("h", c.h); put("seed", c.seed)
                        put("max_moves", minOf(cap, budget(plan, tun))); put("plan", "plans/${c.name}.plan.json")
                    }
                }
            }
        }))
        for (c in cases) {
            val (config, plan) = load(c.job)
            File(out, "plans/${c.name}.plan.json").writeText(plan.toJson())
            val obstacles = staticObstacles(config, plan, c.w, c.h)
            val moves = minOf(cap, budget(plan, tun))
            // One anneal from a fresh random placement; only the anneal is timed. Returns (nanos, stats, cost).
            fun once(seed: Long): Triple<Long, AnnealStats, Double> {
                val model = PlaceModel(plan, config, tun, c.w, c.h, obstacles)
                val annealer = Annealer(model, SplittableRandom(seed), tun)
                check(annealer.initialRandom() || annealer.initialRows(plan)) { "${c.name}: no initial placement" }
                val t = System.nanoTime()
                val stats = annealer.anneal(maxMoves = moves)
                return Triple(System.nanoTime() - t, stats, model.cost)
            }
            repeat(warm) { once(c.seed) }
            val runs = (0 until reps).map { once(c.seed) }
            val stats = runs[0].second
            val single = runs.map { stats.moves * 1e9 / it.first }.sorted()
            // All cores: `threads` anneals at once, total moves over the wall time of the batch.
            val pool = Executors.newFixedThreadPool(threads)
            val parallel = (0 until reps).map {
                val t = System.nanoTime()
                val done = pool.invokeAll((0 until threads).map { i -> Callable { once(c.seed + i).second.moves } }).sumOf { it.get() }
                done * 1e9 / (System.nanoTime() - t)
            }.sorted()
            pool.shutdown()
            report += buildJsonObject {
                put("case", c.name); put("machines", plan.machines.size); put("w", c.w); put("h", c.h); put("seed", c.seed)
                put("max_moves", moves); put("moves", stats.moves); put("temperatures", stats.temperatures)
                put("final_cost", runs[0].third)
                put("single_moves_per_s", single[single.size / 2])
                put("parallel_threads", threads); put("parallel_moves_per_s", parallel[parallel.size / 2])
            }
            println("bench ${c.name}: ${plan.machines.size} machines, ${stats.moves} moves, single ${"%.0f".format(single[single.size / 2])}/s, x$threads ${"%.0f".format(parallel[parallel.size / 2])}/s")
        }
        File(out, "bench_kotlin.json").writeText(pretty.encodeToString(JsonObject.serializer(), buildJsonObject {
            put("runtime", "jvm ${System.getProperty("java.version")}"); put("warm", warm); put("reps", reps)
            putJsonArray("cases") { report.forEach { add(it) } }
        }))
    }
}
