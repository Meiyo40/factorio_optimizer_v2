import config.ConfigSource
import config.ConfigTarget
import config.RouteTuning
import config.RunConfiguration
import config.Tuning
import config.strictJson
import geom.Dir
import geom.Kind
import geom.PlacedEntity
import geom.Rect
import geom.Tile
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject
import place.Annealer
import place.PlaceModel
import place.PlacedMachine
import place.Placement
import place.PlacementFixture
import place.staticObstacles
import plan.ProductionPlan
import plan.buildPlan
import route.AccessOption
import route.MinHeap
import route.PathFinder
import route.RouteProblem
import route.RouteResult
import route.ringPorts
import data.GameConstants
import validate.OutputSpec
import validate.SourceSpec
import search.layoutOf
import search.validationSpec
import validate.MachineSpec
import validate.validate
import java.io.File
import java.util.SplittableRandom
import kotlin.test.Test

/**
 * Reference outputs for the Rust port of the router, and its benchmark. Not part of the Kotlin project: the Rust
 * tooling copies this file into the container's work tree.
 *
 *   ROUTE_OUT=<dir>   writes cases/<name>.json (inputs, the router's setup, the whole RouteResult, the validator's
 *                     verdict), the plans and configs they use, java_random.json and min_heap.json
 *   BENCH_OUT=<dir>   times route() on fixed placements (BENCH_WARM, BENCH_REPS) and writes bench_cases.json and
 *                     bench_kotlin.json
 *
 * Both are skipped when their variable is not set. Every route runs with endgameTimeFactor = 1e6, so the endgame is
 * bounded by its round count and not by the clock, and the results are reproducible.
 */
class GoldenRouteTest {
    private val json = Json { }
    private val ex = "docs/examples"
    private val jobs = "src/test/resources/jobs"

    private fun tile(t: Tile) = buildJsonObject { put("x", t.x); put("y", t.y) }
    private fun nullable(s: String?): JsonElement = s?.let { JsonPrimitive(it) } ?: JsonNull
    private fun ints(a: IntArray) = buildJsonArray { a.forEach { add(JsonPrimitive(it)) } }

    private class Case(
        val name: String, val planKey: String, val config: RunConfiguration, val plan: ProductionPlan, val placement: Placement,
        val tuning: RouteTuning, val bias: Map<Tile, Double> = emptyMap(), val warm: FloatArray? = null,
        val endSeed: Long = 0, val noTunnel: Set<Tile> = emptySet(),
    )

    private fun measuredRoute(config: RunConfiguration) = Tuning.defaults().withOverrides(config.tuning).route
    private fun deterministic(t: RouteTuning) = t.copy(endgameTimeFactor = 1e6)

    /** The job as the search loads it: the job file's tuning overrides on top of tuning.json. */
    private fun loadJob(job: String): Triple<RunConfiguration, Tuning, ProductionPlan> {
        val config = RunConfiguration.load(job)
        val tuning = Tuning.defaults().withOverrides(config.tuning)
        return Triple(config, tuning, buildPlan(TestData.data, config, tuning))
    }

    /** Machines as the placer leaves them after one anneal from a seeded random (or column) start. */
    private fun placed(config: RunConfiguration, tuning: Tuning, plan: ProductionPlan, w: Int, h: Int, seed: Long, columns: Boolean = false): Placement {
        val pt = tuning.place
        val model = PlaceModel(plan, config, pt, w, h, staticObstacles(config, plan, w, h))
        val annealer = Annealer(model, SplittableRandom(seed), pt)
        val moves = minOf(pt.maxMoves, pt.movesPerMachine * plan.machines.size.coerceAtLeast(5))
        if (columns) {
            check(annealer.initialColumns(plan)) { "no column start" }
            annealer.anneal(maxMoves = moves, startRange = pt.columnStartRange, refine = true, t0Scale = pt.columnT0Scale)
        } else {
            check(annealer.initialRandom() || annealer.initialRows(plan)) { "no initial placement" }
            annealer.anneal(maxMoves = moves)
        }
        return model.placement()
    }

    private fun jobCase(name: String, job: String, w: Int, h: Int, seed: Long, columns: Boolean = false,
                        editConfig: (RunConfiguration) -> RunConfiguration = { it }, edit: (RouteTuning) -> RouteTuning = { it }): Case {
        val (c0, tuning, plan0) = loadJob(job)
        val config = editConfig(c0)
        val plan = if (config === c0) plan0 else buildPlan(TestData.data, config, tuning)
        return Case(name, File(job).nameWithoutExtension + (if (config === c0) "" else ".$name"), config, plan,
            placed(config, tuning, plan, w, h, seed, columns), edit(measuredRoute(config)))
    }

    /** A hand placement of src/test/resources/placements, planned and routed as RouterTest does (built-in tuning). */
    private fun fixtureCase(name: String, fixture: String,
                            editConfig: (RunConfiguration) -> RunConfiguration = { it }, edit: (RouteTuning) -> RouteTuning = { it }): Case {
        val fx = PlacementFixture.load("placements/$fixture.json")
        val config = editConfig(RunConfiguration.load(fx.job))
        val plan = buildPlan(TestData.data, config)
        return Case(name, "$fixture.fixture", config, plan, fx.toPlacement(plan), edit(RouteTuning()))
    }

    // ---- the router's setup, read from the private fields ----
    @Suppress("UNCHECKED_CAST")
    private fun <T> peek(o: Any, name: String): T = o.javaClass.getDeclaredField(name).also { it.isAccessible = true }.get(o) as T

    /** [terminal, belt x, belt y, inserter x, inserter y, inserter dir, long, trick, side]: thousands per case. */
    private fun optionJson(o: AccessOption) = buildJsonArray {
        add(JsonPrimitive(o.terminal)); add(JsonPrimitive(o.beltTile.x)); add(JsonPrimitive(o.beltTile.y))
        add(JsonPrimitive(o.insTile.x)); add(JsonPrimitive(o.insTile.y)); add(JsonPrimitive(o.insDir.name))
        add(JsonPrimitive(o.long)); add(JsonPrimitive(o.trick)); add(JsonPrimitive(o.side.name))
    }

    private fun setupJson(pf: PathFinder) = buildJsonObject {
        putJsonArray("nets") {
            peek<List<Any>>(pf, "nets").forEach { n ->
                add(buildJsonObject {
                    put("idx", peek<Int>(n, "idx")); put("plan_id", peek<Int>(n, "planId")); put("item", peek<String>(n, "item"))
                    put("terms", ints(peek(n, "terms"))); put("port_tiles", ints(peek(n, "portTiles"))); put("port_out", ints(peek(n, "portOut")))
                    put("hpwl", peek<Int>(n, "hpwl")); put("source_rate", peek<Double>(n, "sourceRate"))
                    put("out_tiles", ints(peek(n, "outTiles"))); put("out_out", ints(peek(n, "outOut"))); put("out_rate", peek<Double>(n, "outRate"))
                    put("item2", nullable(peek<String?>(n, "item2"))); put("source_rate2", peek<Double>(n, "sourceRate2"))
                    put("src_port", peek<Int>(n, "srcPort")); put("out_port", peek<Int>(n, "outPort"))
                })
            }
        }
        putJsonArray("vts") {
            peek<List<Any>>(pf, "vts").forEach { v ->
                add(buildJsonObject {
                    put("id", peek<Int>(v, "id")); put("terminal", peek<plan.Terminal>(v, "base").id)
                    put("rate", peek<Double>(v, "rate")); put("k", peek<Int>(v, "k")); put("net", peek<Int>(v, "net")); put("slot", peek<Int>(v, "slot"))
                })
            }
        }
        putJsonArray("options") { peek<List<AccessOption>>(pf, "options").forEach { add(optionJson(it)) } }
        putJsonArray("sink_options") {
            peek<Map<Int, List<AccessOption>>>(pf, "sinkOptions").forEach { (term, opts) ->
                add(buildJsonObject { put("terminal", term); putJsonArray("options") { opts.forEach { add(optionJson(it)) } } })
            }
        }
    }

    // ---- the result ----
    private fun entityJson(e: PlacedEntity) = buildJsonArray {
        add(JsonPrimitive(e.name)); add(JsonPrimitive(e.kind.name)); add(JsonPrimitive(e.x)); add(JsonPrimitive(e.y)); add(JsonPrimitive(e.dir.name))
        add(nullable(e.recipe)); add(JsonPrimitive(e.mirror)); add(JsonPrimitive(e.w)); add(JsonPrimitive(e.h))
        add(nullable(e.item)); add(nullable(e.fluid)); add(JsonPrimitive(e.drain)); add(e.net?.let { JsonPrimitive(it) } ?: JsonNull)
    }

    private fun machineSpecJson(m: MachineSpec) = buildJsonObject {
        put("product", m.product)
        putJsonArray("ingredients") { m.ingredients.sorted().forEach { add(JsonPrimitive(it)) } }
        put("output_rate", m.outputRate); put("sink_rate", m.sinkRate)
        putJsonObject("demand") { m.demand.forEach { (k, v) -> put(k, v) } }
        put("needs_fluid", nullable(m.needsFluid))
        putJsonObject("fluid_in") { m.fluidIn.forEach { (k, v) -> put(k, v) } }
        putJsonObject("fluid_out") { m.fluidOut.forEach { (k, v) -> put(k, v) } }
        putJsonObject("products") { m.products.forEach { (k, v) -> put(k, v) } }
        putJsonObject("sinks") { m.sinks.forEach { (k, v) -> put(k, v) } }
    }

    private fun bits(f: FloatArray) = buildJsonObject {
        put("len", f.size)
        putJsonArray("nonzero") { f.forEachIndexed { i, v -> if (v != 0f) add(buildJsonArray { add(JsonPrimitive(i)); add(JsonPrimitive(v.toRawBits())) }) } }
    }

    private fun sourceJson(s: SourceSpec) = buildJsonObject { put("tile", tile(s.tile)); put("item", s.item); put("rate", s.rate); put("fluid", s.fluid); put("lane", s.lane); put("port", s.port) }
    private fun outputJson(o: OutputSpec) = buildJsonObject { put("tile", tile(o.tile)); put("item", o.item); put("rate", o.rate); put("fluid", o.fluid); put("port", o.port) }

    private fun resultJson(r: RouteResult, pf: PathFinder) = buildJsonObject {
        put("success", r.success); put("iterations", r.iterations); put("overuse", r.overuse); put("chains", r.chains)
        put("expansions", pf.expansions)
        putJsonArray("failed_nets") { r.failedNets.forEach { add(JsonPrimitive(it)) } }
        putJsonArray("trace") { pf.trace.forEach { add(JsonPrimitive(it)) } }
        putJsonArray("log") { r.log.forEach { add(JsonPrimitive(it)) } }
        putJsonArray("entities") { r.entities.forEach { add(entityJson(it)) } }
        putJsonArray("sources") { r.sources.forEach { add(sourceJson(it)) } }
        putJsonArray("outputs") { r.outputs.forEach { add(outputJson(it)) } }
        putJsonArray("conflicts") { r.conflicts.forEach { add(tile(it)) } }
        putJsonArray("conflict_nets") { r.conflictNets.forEach { (t, ids) -> add(buildJsonObject { put("tile", tile(t)); putJsonArray("nets") { ids.forEach { add(JsonPrimitive(it)) } } }) } }
        putJsonArray("di") {
            r.di.forEach { d ->
                add(buildJsonObject {
                    put("producer", d.producer); put("consumer", d.consumer); put("item", d.item); put("flow", d.flow)
                    putJsonArray("tiles") { d.tiles.forEach { add(tile(it)) } }
                    put("long", d.long); put("ins_dir", d.insDir.name)
                })
            }
        }
        putJsonObject("stats") { put("belts", r.stats.belts); put("undergrounds", r.stats.undergrounds); put("inserters", r.stats.inserters) }
        putJsonArray("chain_stats") { r.chainStats.forEach { c -> add(buildJsonObject { put("item", c.item); put("terminals", c.terminals); put("hpwl", c.hpwl); put("tiles", c.tiles) }) } }
        // The map in the order Kotlin iterates it: the placer sums it per bin in that order.
        putJsonArray("history") { r.history.forEach { (t, v) -> add(buildJsonObject { put("x", t.x); put("y", t.y); put("v", v) }) } }
        put("raw_history", bits(r.rawHistory))
    }

    private val seenPlans = HashSet<String>()
    private val verdicts = ArrayList<String>()

    /** Routes the case, writes cases/<name>.json, returns the result (for cases that warm-start from it). */
    private fun routeCase(out: File, c: Case): RouteResult {
        val obstacles = staticObstacles(c.config, c.plan, c.placement.w, c.placement.h)
        val tuning = deterministic(c.tuning)
        val pf = PathFinder(RouteProblem(c.plan, c.config, tuning, c.placement, obstacles, c.bias, c.warm, null, c.endSeed, c.noTunnel))
        val setup = setupJson(pf)
        val r = pf.run()
        val layout = layoutOf(c.placement, r.entities)
        val spec = validationSpec(TestData.data, c.plan, c.config, c.placement, r.sources, requirePower = false, outputs = r.outputs)
        val errors = if (r.success) validate(layout, spec) else emptyList()
        val ringed = ringPorts(r.entities, r.sources, r.outputs, c.placement.w, c.placement.h, GameConstants.beltTier(c.config.beltTier).belt)
        if (seenPlans.add(c.planKey)) File(out, "plans/${c.planKey}.plan.json").writeText(c.plan.toJson())
        File(out, "configs/${c.name}.json").writeText(strictJson.encodeToString(RunConfiguration.serializer(), c.config))
        val file = buildJsonObject {
            put("upstream", System.getenv("UPSTREAM") ?: "unknown")
            put("case", c.name); put("config", "configs/${c.name}.json"); put("plan", "plans/${c.planKey}.plan.json")
            put("w", c.placement.w); put("h", c.placement.h)
            putJsonArray("placement") { c.placement.machines.forEach { m -> add(buildJsonObject { put("id", m.id); put("x", m.x); put("y", m.y); put("rot", m.rot.ordinal); put("mirror", m.mirror) }) } }
            putJsonArray("obstacles") { obstacles.sortedWith(compareBy({ it.y }, { it.x })).forEach { add(tile(it)) } }
            put("tuning", strictJson.encodeToJsonElement(RouteTuning.serializer(), tuning))
            putJsonArray("tile_bias") { c.bias.forEach { (t, v) -> add(buildJsonObject { put("x", t.x); put("y", t.y); put("v", v) }) } }
            if (c.warm != null) put("warm_history", bits(c.warm))
            put("endgame_seed", c.endSeed)
            putJsonArray("no_tunnel") { c.noTunnel.sortedWith(compareBy({ it.y }, { it.x })).forEach { add(tile(it)) } }
            put("setup", setup)
            put("result", resultJson(r, pf))
            putJsonObject("spec") {
                put("pole_ring", spec.poleRing); put("require_power", spec.requirePower); put("fluid_mode", spec.fluidMode); put("check_rates", spec.checkRates)
                putJsonObject("inserter_rate") {
                    listOf("inserter", "long-handed-inserter", "fast-inserter", "bulk-inserter").forEach { n ->
                        putJsonArray(n) { add(JsonPrimitive(spec.inserterRate(n, false))); add(JsonPrimitive(spec.inserterRate(n, true))) }
                    }
                }
                putJsonArray("machines") { spec.machines.forEach { (t, m) -> add(buildJsonObject { put("tile", tile(t)); put("spec", machineSpecJson(m)) }) } }
                putJsonArray("outside_allowed") { spec.outsideAllowed.sortedWith(compareBy({ it.y }, { it.x })).forEach { add(tile(it)) } }
            }
            putJsonObject("ringed") {
                putJsonArray("added") { ringed.entities.drop(r.entities.size).forEach { add(entityJson(it)) } }
                putJsonArray("sources") { ringed.sources.forEach { add(sourceJson(it)) } }
                putJsonArray("outputs") { ringed.outputs.forEach { add(outputJson(it)) } }
            }
            putJsonArray("errors") { errors.forEach { e -> add(buildJsonObject { put("type", e.type.name); put("x", e.tile.x); put("y", e.tile.y); put("message", e.message) }) } }
        }
        File(out, "cases/${c.name}.json").writeText(json.encodeToString(JsonObject.serializer(), file))
        val kinds = r.entities.groupingBy { it.kind }.eachCount()
        val line = "route ${c.name}: ${c.plan.machines.size} machines ${c.placement.w}x${c.placement.h}, success=${r.success} iter=${r.iterations} overuse=${r.overuse} " +
            "failed=${r.failedNets.size} expansions=${pf.expansions} entities=${r.entities.size} errors=${errors.size} ${r.millis}ms | " +
            "splitters=${kinds[Kind.SPLITTER] ?: 0} ug=${kinds[Kind.UG_IN] ?: 0} pipeUg=${kinds[Kind.PIPE_UG] ?: 0} chests=${kinds[Kind.CHEST] ?: 0} di=${r.di.size} " +
            "shared=${r.log.count { it.startsWith("shared belt") }} endgame=${r.log.any { it.startsWith("endgame") }} back=${r.log.any { it.startsWith("back to") }}"
        println(line)
        verdicts += line
        return r
    }

    // ---- the fuzz instances of RouterFuzzTest, generated the same way ----
    private val fuzzItems = listOf(
        "iron-gear-wheel", "copper-cable", "electronic-circuit", "pipe", "iron-stick", "inserter",
        "transport-belt", "automation-science-pack", "firearm-magazine", "pipe-to-ground", "burner-inserter",
    )

    private fun randomPlacement(plan: ProductionPlan, w: Int, h: Int, rnd: SplittableRandom, spacing: Int): Placement? {
        val placedMachines = mutableListOf<PlacedMachine>()
        val rects = mutableListOf<Rect>()
        for (m in plan.machines) {
            var ok = false
            repeat(200) {
                if (ok) return@repeat
                val x = rnd.nextInt(0, w - m.w + 1)
                val y = rnd.nextInt(0, h - m.h + 1)
                val r = Rect(x, y, m.w, m.h)
                val grown = Rect(x - spacing, y - spacing, m.w + 2 * spacing, m.h + 2 * spacing)
                if (rects.none { it.overlaps(grown) }) {
                    rects += r; placedMachines += PlacedMachine(m.id, x, y, Dir.entries[rnd.nextInt(4)]); ok = true
                }
            }
            if (!ok) return null
        }
        return Placement(w, h, placedMachines)
    }

    private fun beltFuzz(n: Int): List<Case> {
        val rnd = SplittableRandom(4242)
        val edges = listOf("west", "north", "east", "south", "any")
        val cases = ArrayList<Case>()
        while (cases.size < n) {
            val targets = (1..rnd.nextInt(1, 4)).map { ConfigTarget(fuzzItems[rnd.nextInt(fuzzItems.size)], 0.1 + rnd.nextDouble() * 1.2) }.distinctBy { it.name }
            val sources = listOf("iron-plate", "copper-plate").map { ConfigSource(it, edge = edges[rnd.nextInt(edges.size)]) }
            val config = RunConfiguration(targets = targets, sources = sources, laneSharing = rnd.nextBoolean())
            val plan = buildPlan(TestData.data, config)
            if (plan.machines.size !in 2..8) continue
            val size = rnd.nextInt(12, 31)
            val placement = randomPlacement(plan, size, size, rnd, spacing = rnd.nextInt(0, 3)) ?: continue
            val name = "fuzz_${cases.size + 1}"
            cases += Case(name, name, config, plan, placement, RouteTuning(maxIterations = 30))
        }
        return cases
    }

    private fun fluidFuzz(n: Int): List<Case> {
        val rnd = SplittableRandom(777)
        val fluidItems = listOf("plastic-bar", "sulfur", "sulfuric-acid", "lubricant", "solid-fuel", "explosives")
        val cases = ArrayList<Case>()
        while (cases.size < n) {
            val targets = (1..rnd.nextInt(1, 3)).map { ConfigTarget(fluidItems[rnd.nextInt(fluidItems.size)], 0.2 + rnd.nextDouble() * 2.0) }.distinctBy { it.name }
            val config = RunConfiguration(targets = targets)
            val plan = buildPlan(TestData.data, config)
            if (plan.machines.size !in 2..14) continue
            val size = rnd.nextInt(24, 41)
            val placement = randomPlacement(plan, size, size, rnd, spacing = rnd.nextInt(1, 4)) ?: continue
            val name = "fluidfuzz_${cases.size + 1}"
            cases += Case(name, name, config, plan, placement, RouteTuning(maxIterations = 30))
        }
        return cases
    }

    @Test
    fun dumpRouteGoldens() {
        val out = System.getenv("ROUTE_OUT")?.let { File(it) } ?: return
        listOf("plans", "configs", "cases").forEach { File(out, it).mkdirs() }

        // Hand placements, as RouterTest routes them.
        routeCase(out, fixtureCase("t1", "t1"))
        routeCase(out, fixtureCase("t2_di", "t2_di"))
        routeCase(out, fixtureCase("t2_belt", "t2_belt"))
        routeCase(out, fixtureCase("t3", "t3"))
        routeCase(out, fixtureCase("cross", "cross") { it.copy(ugPenalty = 8.0, pMult = 1.5, maxIterations = 60, stagnationStop = 25) })
        routeCase(out, fixtureCase("cross_plain", "cross") { it.copy(ugPenalty = 8.0, pMult = 1.5, pFacFirst = 0.0, maxIterations = 1, endgameRounds = 0) })
        routeCase(out, fixtureCase("fork", "fork"))
        routeCase(out, fixtureCase("fork_chain", "fork", editConfig = { it.copy(tricks = it.tricks + ("B9" to false)) }))
        // The same placements with the rarer options off or on.
        routeCase(out, fixtureCase("t3_no_ug", "t3", editConfig = { it.copy(tricks = it.tricks + ("B2" to false)) }))
        routeCase(out, fixtureCase("t3_no_weave", "t3", editConfig = { it.copy(tricks = it.tricks + ("B5" to false)) }))
        routeCase(out, fixtureCase("t3_inside_ports", "t3", editConfig = { it.copy(portsOnRing = false) }))
        routeCase(out, fixtureCase("t3_keep_order", "t3") { it.copy(keepChainOrder = true, traceDetail = true) })

        // Placer-made placements of the test jobs and the examples.
        routeCase(out, jobCase("t3_random_24", "$jobs/t3.json", 24, 24, 2))
        routeCase(out, jobCase("t4_random_20", "$jobs/t4.json", 20, 20, 3))
        routeCase(out, jobCase("t4_infinity_20", "$jobs/t4.json", 20, 20, 3, editConfig = { it.copy(fluids = it.fluids.copy(mode = "infinity-pipe")) }))
        routeCase(out, jobCase("acid_random_20", "$jobs/acid.json", 20, 20, 1))
        routeCase(out, jobCase("pair_random_16", "$jobs/pair.json", 16, 16, 1))
        routeCase(out, jobCase("red_green_20", "$ex/red_green_science.json", 20, 20, 1))
        routeCase(out, jobCase("red_green_shared_18", "$ex/red_green_science_shared.json", 18, 18, 1))
        routeCase(out, jobCase("chem_columns_40", "$ex/chemical_science.json", 40, 40, 4, columns = true))
        routeCase(out, jobCase("chem_infinity_40", "$ex/chemical_science.json", 40, 40, 4, columns = true, editConfig = { it.copy(fluids = it.fluids.copy(mode = "infinity-pipe")) }))
        routeCase(out, jobCase("em_random_46", "$ex/electromagnetic_science.json", 46, 46, 2))
        routeCase(out, jobCase("metallurgic_52", "$ex/metallurgic_science.json", 52, 52, 1))
        routeCase(out, jobCase("gleba_56", "$ex/gleba_agricultural_carbon_fiber.json", 56, 56, 1))
        // Chest sinks (sinks.mode = chest): several sink terminals per item, solved in hash-map order.
        val chests = { c: RunConfiguration -> c.copy(sinks = c.sinks.copy(mode = "chest")) }
        routeCase(out, jobCase("t3_chests_24", "$jobs/t3.json", 24, 24, 2, editConfig = chests))
        routeCase(out, jobCase("red_green_chests_20", "$ex/red_green_science.json", 20, 20, 1, editConfig = chests))
        routeCase(out, jobCase("chem_chests_40", "$ex/chemical_science.json", 40, 40, 4, columns = true, editConfig = chests))
        routeCase(out, jobCase("gleba_chests_56", "$ex/gleba_agricultural_carbon_fiber.json", 56, 56, 1, editConfig = chests))
        // Tight boxes, where negotiation fails or barely succeeds and the endgame runs.
        routeCase(out, jobCase("chem_tight_36", "$ex/chemical_science.json", 36, 36, 7) { it.copy(maxIterations = 30) })
        routeCase(out, jobCase("em_tight_41", "$ex/electromagnetic_science.json", 41, 41, 2) { it.copy(maxIterations = 30) })

        // Inputs the search passes on a second attempt: warm history, tile bias, endgame seed, tunnel ban.
        val base = jobCase("t3_warm_base", "$jobs/t3.json", 24, 24, 2)
        val first = routeCase(out, base)
        val biasTiles = (3..8).associate { Tile(it, it + 2) to 0.5 }
        routeCase(out, Case("t3_warm", base.planKey, base.config, base.plan, base.placement, base.tuning, biasTiles, first.rawHistory, 7,
            noTunnel = (10..14).map { Tile(it, 12) }.toSet()))
        val chemBase = jobCase("chem_warm_base", "$ex/chemical_science.json", 36, 36, 7) { it.copy(maxIterations = 30) }
        val chemFirst = routeCase(out, chemBase)
        routeCase(out, Case("chem_warm", chemBase.planKey, chemBase.config, chemBase.plan, chemBase.placement, chemBase.tuning,
            (5..9).associate { Tile(it, 10) to 0.5 }, chemFirst.rawHistory, 3))

        // Ports that are one belt (the links of a split factory) and an item with two input ports.
        val (cs, ts, singlePlan) = loadJob("$jobs/t3.json")
        val single = singlePlan.copy(ports = singlePlan.ports.mapIndexed { i, p -> if (i < 2 || p.output) p.copy(single = true) else p })
        routeCase(out, Case("t3_single_ports", "t3.single", cs, single, placed(cs, ts, singlePlan, 24, 24, 2), measuredRoute(cs)))
        val multiConfig = RunConfiguration(
            targets = listOf(ConfigTarget("inserter", 0.8)),
            sources = listOf(ConfigSource("iron-plate", edge = "west"), ConfigSource("iron-plate", edge = "north"), ConfigSource("copper-plate", edge = "west")),
        )
        val multiPlan = buildPlan(TestData.data, multiConfig)
        routeCase(out, Case("multi_source", "multi_source", multiConfig, multiPlan, placed(multiConfig, Tuning(), multiPlan, 22, 22, 1), RouteTuning()))

        val bigConfig = RunConfiguration(
            targets = listOf(ConfigTarget("inserter", 1.2), ConfigTarget("transport-belt", 1.2)),
            sources = listOf(ConfigSource("iron-plate", edge = "west"), ConfigSource("iron-plate", edge = "north"), ConfigSource("iron-plate", edge = "south"),
                ConfigSource("copper-plate", edge = "west"), ConfigSource("copper-plate", edge = "east")),
        )
        val bigPlan = buildPlan(TestData.data, bigConfig)
        routeCase(out, Case("multi_source_big", "multi_source_big", bigConfig, bigPlan, placed(bigConfig, Tuning(), bigPlan, 28, 28, 3), RouteTuning()))

        // The fuzz instances of RouterFuzzTest (the first ones, in the same order).
        beltFuzz(30).forEach { routeCase(out, it) }
        fluidFuzz(10).forEach { routeCase(out, it) }

        File(out, "verdicts.txt").writeText(verdicts.joinToString("\n"))
        javaSamples(out)
    }

    // ---- real java.util.Random and Collections.shuffle, and MinHeap ties ----
    private fun javaSamples(out: File) {
        val seeds = listOf(0L, 1L, 42L, 31L * 1000 + 17, 31L * 4000 + 9 + 1_000_003L * 7, -5L, Long.MAX_VALUE, 123456789012345L)
        val bounds = listOf(1, 2, 3, 5, 10, 16, 17, 100, 1000, 65536, (1 shl 20) + 1, Int.MAX_VALUE)
        File(out, "java_random.json").writeText(json.encodeToString(JsonObject.serializer(), buildJsonObject {
            putJsonArray("ints") {
                seeds.forEach { s ->
                    val r = java.util.Random(s)
                    add(buildJsonObject {
                        put("seed", s)
                        putJsonArray("next_int") { repeat(6) { add(JsonPrimitive(r.nextInt())) } }
                        putJsonArray("bounded") { bounds.forEach { b -> add(buildJsonObject { put("bound", b); putJsonArray("values") { repeat(8) { add(JsonPrimitive(r.nextInt(b))) } } }) } }
                    })
                }
            }
            putJsonArray("shuffles") {
                for (s in seeds) for (n in listOf(0, 1, 2, 3, 5, 6, 10, 33, 100)) {
                    val rnd = java.util.Random(s)
                    val list = (0 until n).toMutableList()
                    list.shuffle(rnd)
                    // A second shuffle with the same generator checks the state it leaves behind.
                    val again = (0 until n).toMutableList().also { it.shuffle(rnd) }
                    add(buildJsonObject {
                        put("seed", s); put("n", n)
                        putJsonArray("first") { list.forEach { add(JsonPrimitive(it)) } }
                        putJsonArray("second") { again.forEach { add(JsonPrimitive(it)) } }
                    })
                }
            }
        }))
        // Random pushes and pops with few distinct keys, so that the heap's tie rules decide the order.
        val rnd = SplittableRandom(99)
        File(out, "min_heap.json").writeText(json.encodeToString(JsonObject.serializer(), buildJsonObject {
            putJsonArray("runs") {
                for (distinct in listOf(2, 3, 5, 50, 100000)) repeat(4) {
                    val heap = MinHeap(4)
                    val ops = ArrayList<JsonElement>()
                    val popped = ArrayList<Int>()
                    var size = 0
                    var next = 0
                    repeat(400) {
                        if (size > 0 && rnd.nextInt(3) == 0) {
                            ops += JsonPrimitive("pop"); popped += heap.pop(); size--
                        } else {
                            val key = rnd.nextInt(distinct) * 0.5f
                            ops.add(buildJsonArray { add(JsonPrimitive(key)); add(JsonPrimitive(next)) })
                            heap.push(key, next++); size++
                        }
                    }
                    while (size > 0) { ops += JsonPrimitive("pop"); popped += heap.pop(); size-- }
                    add(buildJsonObject {
                        put("distinct_keys", distinct)
                        put("ops", JsonArray(ops))
                        putJsonArray("popped") { popped.forEach { add(JsonPrimitive(it)) } }
                    })
                }
            }
        }))
    }

    // ---- timing: route() on fixed placements ----
    private class BenchCase(val name: String, val job: String, val w: Int, val h: Int, val seed: Long, val maxIterations: Int, val columns: Boolean = false)

    @Test
    fun bench() {
        val out = System.getenv("BENCH_OUT")?.let { File(it) } ?: return
        listOf("plans", "configs").forEach { File(out, it).mkdirs() }
        val warm = System.getenv("BENCH_WARM")?.toInt() ?: 3
        val reps = System.getenv("BENCH_REPS")?.toInt() ?: 5
        val cases = listOf(
            BenchCase("red_green_20", "$ex/red_green_science.json", 20, 20, 1, 120),
            BenchCase("chem_comfortable_40", "$ex/chemical_science.json", 40, 40, 4, 120, columns = true),
            BenchCase("chem_tight_36", "$ex/chemical_science.json", 36, 36, 7, 60),
            BenchCase("em_46", "$ex/electromagnetic_science.json", 46, 46, 2, 120),
            BenchCase("em_tight_41", "$ex/electromagnetic_science.json", 41, 41, 2, 60),
            BenchCase("metallurgic_52", "$ex/metallurgic_science.json", 52, 52, 1, 120),
        )
        val report = ArrayList<JsonElement>()
        val described = ArrayList<JsonElement>()
        for (c in cases) {
            val (config, tuning, plan) = loadJob(c.job)
            val placement = placed(config, tuning, plan, c.w, c.h, c.seed, c.columns)
            val rt = deterministic(measuredRoute(config)).copy(maxIterations = c.maxIterations)
            val obstacles = staticObstacles(config, plan, c.w, c.h)
            File(out, "plans/${c.name}.plan.json").writeText(plan.toJson())
            File(out, "configs/${c.name}.json").writeText(strictJson.encodeToString(RunConfiguration.serializer(), config))
            fun once(): Triple<Long, RouteResult, PathFinder> {
                val pf = PathFinder(RouteProblem(plan, config, rt, placement, obstacles))
                val t = System.nanoTime()
                val r = pf.run()
                return Triple(System.nanoTime() - t, r, pf)
            }
            repeat(warm) { once() }
            val runs = (0 until reps).map { once() }
            val times = runs.map { it.first / 1e6 }.sorted()
            val (_, r, pf) = runs[0]
            described += buildJsonObject {
                put("case", c.name); put("config", "configs/${c.name}.json"); put("plan", "plans/${c.name}.plan.json"); put("w", c.w); put("h", c.h)
                putJsonArray("placement") { placement.machines.forEach { m -> add(buildJsonObject { put("id", m.id); put("x", m.x); put("y", m.y); put("rot", m.rot.ordinal); put("mirror", m.mirror) }) } }
                put("tuning", strictJson.encodeToJsonElement(RouteTuning.serializer(), rt))
            }
            report += buildJsonObject {
                put("case", c.name); put("machines", plan.machines.size); put("w", c.w); put("h", c.h)
                put("success", r.success); put("iterations", r.iterations); put("overuse", r.overuse); put("expansions", pf.expansions)
                put("entities", r.entities.size); put("median_ms", times[times.size / 2]); put("min_ms", times[0])
            }
            println("bench ${c.name}: ${plan.machines.size} machines, success=${r.success} iter=${r.iterations} expansions=${pf.expansions} median ${"%.1f".format(times[times.size / 2])} ms")
        }
        File(out, "bench_cases.json").writeText(json.encodeToString(JsonObject.serializer(), buildJsonObject { put("cases", JsonArray(described)) }))
        File(out, "bench_kotlin.json").writeText(json.encodeToString(JsonObject.serializer(), buildJsonObject {
            put("runtime", "jvm ${System.getProperty("java.version")}"); put("warm", warm); put("reps", reps); put("cases", JsonArray(report))
        }))
    }
}
