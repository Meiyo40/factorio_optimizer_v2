import blocks.BlockPlan
import blocks.Split
import blocks.blockPlan
import blocks.compactLayout
import blocks.mergeBlocks
import blocks.runFactory
import blocks.splitPlan
import config.BlocksConfig
import config.BoxConfig
import config.ConfigSource
import config.ConfigTarget
import config.FluidConfig
import config.PowerConfig
import config.ReservedArea
import config.ResearchConfig
import config.RunConfiguration
import config.SearchConfig
import config.Tuning
import config.strictJson
import data.GameConstants
import emit.decodeBlueprint
import emit.toLayout
import geom.Dir
import geom.Kind
import geom.Layout
import geom.PlacedEntity
import geom.Tile
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject
import place.PlacedMachine
import place.Placement
import plan.ProductionPlan
import plan.buildPlan
import search.Search
import search.TrialResult
import search.runTrial
import validate.OutputSpec
import validate.SourceSpec
import java.io.File
import java.util.SplittableRandom
import kotlin.test.Test

/**
 * Reference outputs for the Rust port of the trial loop, the box-size search, the blocks and the factory. Not part
 * of the Kotlin project: the Rust tooling copies this file into the container's work tree. Each dump is skipped
 * unless its variable is set to a directory:
 *
 *   TRIAL_OUT=<dir>    runTrial cases: cases/<name>.json (inputs and the whole TrialResult), plans/, configs/, tunings/
 *   SPLIT_OUT=<dir>    splitPlan, blockPlan and mergeBlocks of several jobs, compactLayout of layouts, and samples of
 *                      java.util.HashMap iteration and of "%.nf" formatting
 *   SEARCH_OUT=<dir>   Search runs: <name>/ with the inputs, expected.json and the files the search wrote
 *   FACTORY_OUT=<dir>  runFactory runs: <name>/ likewise, with every block's blueprint
 *   TRIAL_SCAN=<dir>   not a golden: runs trials of variants to find sizes and seeds that reach a branch (the power
 *                      retries were found this way); read its "trial ..." lines
 *
 * Every route runs with endgameTimeFactor = 1e6, so the endgame is bounded by its round count and not by the clock;
 * searches use one thread, a budget that is never reached and a small extra_rounds, so a run is reproducible.
 */
class GoldenTrialTest {
    private val json = Json { }
    private val ex = "docs/examples"
    private val jobs = "src/test/resources/jobs"

    private fun tile(t: Tile) = buildJsonObject { put("x", t.x); put("y", t.y) }
    private fun nullable(s: String?): JsonElement = s?.let { JsonPrimitive(it) } ?: JsonNull

    private fun entityJson(e: PlacedEntity) = buildJsonArray {
        add(JsonPrimitive(e.name)); add(JsonPrimitive(e.kind.name)); add(JsonPrimitive(e.x)); add(JsonPrimitive(e.y)); add(JsonPrimitive(e.dir.name))
        add(nullable(e.recipe)); add(JsonPrimitive(e.mirror)); add(JsonPrimitive(e.w)); add(JsonPrimitive(e.h))
        add(nullable(e.item)); add(nullable(e.fluid)); add(JsonPrimitive(e.drain)); add(e.net?.let { JsonPrimitive(it) } ?: JsonNull)
    }

    private fun layoutJson(l: Layout) = buildJsonObject {
        put("w", l.w); put("h", l.h); put("label", l.label)
        putJsonArray("entities") { l.entities.forEach { add(entityJson(it)) } }
        putJsonArray("wires") { l.wires.forEach { (a, b) -> add(buildJsonArray { add(JsonPrimitive(a)); add(JsonPrimitive(b)) }) } }
        putJsonArray("tiles") { l.tiles.forEach { t -> add(buildJsonArray { add(JsonPrimitive(t.name)); add(JsonPrimitive(t.x)); add(JsonPrimitive(t.y)) }) } }
    }

    private fun placementJson(p: Placement) = buildJsonObject {
        put("w", p.w); put("h", p.h)
        putJsonArray("machines") { p.machines.forEach { m -> add(buildJsonObject { put("id", m.id); put("x", m.x); put("y", m.y); put("rot", m.rot.ordinal); put("mirror", m.mirror) }) } }
    }

    private fun sourceJson(s: SourceSpec) = buildJsonObject { put("tile", tile(s.tile)); put("item", s.item); put("rate", s.rate); put("fluid", s.fluid); put("lane", s.lane); put("port", s.port) }
    private fun outputJson(o: OutputSpec) = buildJsonObject { put("tile", tile(o.tile)); put("item", o.item); put("rate", o.rate); put("fluid", o.fluid); put("port", o.port) }
    private fun errorJson(e: validate.ValidationError) = buildJsonObject { put("type", e.type.name); put("x", e.tile.x); put("y", e.tile.y); put("message", e.message) }
    private fun strings(l: List<String>) = buildJsonArray { l.forEach { add(JsonPrimitive(it)) } }
    private fun ints(l: List<Int>) = buildJsonArray { l.forEach { add(JsonPrimitive(it)) } }

    private fun write(file: File, o: JsonElement) {
        file.parentFile.mkdirs()
        file.writeText(json.encodeToString(JsonElement.serializer(), o))
    }

    /** A log line without its clock: the stamp and any "1.2s" duration vary between runs. */
    private fun norm(line: String) = line.replace(Regex("^\\[ *[0-9.]+s\\] "), "").replace(Regex("[0-9]+\\.[0-9]s"), "Ns")

    private fun det(t: Tuning) = t.copy(route = t.route.copy(endgameTimeFactor = 1e6))

    private class Scen(val key: String, val config: RunConfiguration, val tuning: Tuning, val plan: ProductionPlan)

    /** The job as the search loads it (the job's tuning overrides on top of tuning.json), with [edit] applied last. */
    private fun scen(key: String, config: RunConfiguration, edit: (Tuning) -> Tuning = { it }): Scen {
        val tuning = det(edit(Tuning.defaults().withOverrides(config.tuning)))
        return Scen(key, config, tuning, buildPlan(TestData.data, config, tuning))
    }

    private fun job(path: String) = RunConfiguration.load(path)

    private fun saveInputs(out: File, s: Scen) {
        File(out, "plans/${s.key}.plan.json").takeIf { !it.exists() }?.let { it.parentFile.mkdirs(); it.writeText(s.plan.toJson()) }
        File(out, "configs/${s.key}.json").takeIf { !it.exists() }?.let { it.parentFile.mkdirs(); it.writeText(strictJson.encodeToString(RunConfiguration.serializer(), s.config)) }
        File(out, "tunings/${s.key}.json").takeIf { !it.exists() }?.let { it.parentFile.mkdirs(); it.writeText(s.tuning.toJson()) }
    }

    // ---- trials ----
    private fun trialJson(r: TrialResult) = buildJsonObject {
        put("success", r.success); put("stage", r.stage); put("w", r.w); put("h", r.h); put("seed", r.seed)
        put("score", r.score); put("anneal_t0", r.annealT0)
        put("notes", strings(r.notes))
        putJsonArray("errors") { r.errors.forEach { add(errorJson(it)) } }
        putJsonArray("sources") { r.sources.forEach { add(sourceJson(it)) } }
        putJsonArray("outputs") { r.outputs.forEach { add(outputJson(it)) } }
        put("summary", r.summary?.let { s ->
            buildJsonObject {
                put("rounds", s.rounds); put("overuse", s.overuse); put("failed_nets", ints(s.failedNets))
                putJsonArray("hotspots") { s.hotspots.forEach { (t, nets) -> add(buildJsonObject { put("tile", tile(t)); put("nets", ints(nets.toList())) }) } }
            }
        } ?: JsonNull)
        put("layout", r.layout?.let { layoutJson(it) } ?: JsonNull)
        put("placement", r.placement?.let { placementJson(it) } ?: JsonNull)
        put("route", r.route?.let { rr ->
            buildJsonObject {
                put("success", rr.success); put("iterations", rr.iterations); put("overuse", rr.overuse); put("chains", rr.chains)
                put("failed_nets", ints(rr.failedNets)); put("entities", rr.entities.size)
                putJsonArray("conflicts") { rr.conflicts.forEach { add(tile(it)) } }
                put("log", strings(rr.log))
            }
        } ?: JsonNull)
    }

    private val verdicts = ArrayList<String>()

    private fun trial(out: File, name: String, s: Scen, w: Int, h: Int, seed: Long, warm: Placement? = null, warmT0: Double? = null, lattice: Boolean? = null): TrialResult {
        saveInputs(out, s)
        val r = runTrial(TestData.data, s.plan, s.config, s.tuning, w, h, seed, warm, warmT0, lattice = lattice ?: (s.config.power.mode == "lattice"))
        write(File(out, "cases/$name.json"), buildJsonObject {
            put("upstream", System.getenv("UPSTREAM") ?: "unknown")
            put("case", name); put("scenario", s.key); put("plan", "plans/${s.key}.plan.json"); put("config", "configs/${s.key}.json"); put("tuning", "tunings/${s.key}.json")
            put("w", w); put("h", h); put("seed", seed)
            put("warm", warm?.let { placementJson(it) } ?: JsonNull)
            put("warm_t0", warmT0?.let { JsonPrimitive(it) } ?: JsonNull)
            put("lattice", lattice?.let { JsonPrimitive(it) } ?: JsonNull)
            put("result", trialJson(r))
        })
        val flags = listOf("column start", "round ", "spread retry", "near miss", "power attempt", "set cover failed").filter { f -> r.notes.any { it.contains(f) } }
        val line = "trial $name: ${s.plan.machines.size} machines ${w}x$h seed $seed -> ${r.stage} score ${if (r.success) "%.1f".format(r.score) else "-"} " +
            "rounds=${r.summary?.rounds} errors=${r.errors.size} ${r.millis}ms notes=${r.notes.size} $flags"
        println(line)
        verdicts += line
        return r
    }

    @Test
    fun dumpTrialGoldens() {
        val out = System.getenv("TRIAL_OUT")?.let { File(it) } ?: return
        listOf("plans", "configs", "tunings", "cases").forEach { File(out, it).mkdirs() }
        fun many(prefix: String, s: Scen, size: Int, seeds: LongRange) = seeds.map { trial(out, "${prefix}_${size}_s$it", s, size, size, it) }

        val t1 = scen("t1", job("$jobs/t1.json"))
        val t2 = scen("t2", job("$jobs/t2.json"))
        val t3 = scen("t3", job("$jobs/t3.json"))
        many("t1", t1, 10, 1L..2L)
        many("t2", t2, 14, 1L..2L)
        many("t3", t3, 24, 1L..3L)
        // Around the smallest box that works: failed feedback rounds and the near-miss retry happen here.
        listOf(15, 16, 17, 18, 20).forEach { many("t3", t3, it, 1L..2L) }

        val t4cfg = job("$jobs/t4.json")
        many("t4_infinity", scen("t4_infinity", t4cfg.copy(fluids = FluidConfig("infinity-pipe"))), 24, 1L..2L)
        many("t4_pipes", scen("t4_pipes", t4cfg), 26, 1L..2L)
        many("t4_test", scen("t4_test", t4cfg.copy(testMode = true, fluids = FluidConfig("infinity-pipe"))), 24, 1L..1L)
        many("t1_test", scen("t1_test", job("$jobs/t1.json").copy(testMode = true)), 10, 1L..1L)
        many("acid", scen("acid", job("$jobs/acid.json")), 14, 1L..2L)
        many("acid_test", scen("acid_test", job("$jobs/acid.json").copy(testMode = true)), 14, 1L..1L)
        val pair = job("$jobs/pair.json")
        many("pair_shared", scen("pair_shared", pair.copy(testMode = true, laneSharing = true)), 14, 1L..2L)
        many("pair_apart", scen("pair_apart", pair.copy(testMode = true, laneSharing = false)), 14, 1L..1L)

        val t3cfg = job("$jobs/t3.json")
        many("t3_reserved", scen("t3_reserved", t3cfg.copy(reserved = listOf(ReservedArea(w = 5, h = 4)))), 24, 1L..2L)
        many("t3_inside", scen("t3_inside", t3cfg.copy(portsOnRing = false)), 24, 1L..1L)
        many("t3_lattice", scen("t3_lattice", t3cfg.copy(power = PowerConfig(mode = "lattice"))), 24, 1L..2L)
        many("t3_nopower", scen("t3_nopower", t3cfg.copy(power = PowerConfig(mode = "none"))), 24, 1L..1L)
        many("t3_nomarker", scen("t3_nomarker", t3cfg.copy(markerTile = "none")), 24, 1L..1L)
        many("t3_smallpole", scen("t3_smallpole", t3cfg.copy(power = PowerConfig(pole = "small-electric-pole"))), 24, 1L..2L)
        // Power that fails on the first set-cover try and is fixed by the warm reroute (the sizes and seeds were found by scanning).
        val smallPole = PowerConfig(pole = "small-electric-pole", allowOutsideRing = false)
        val r9 = scen("t3_reserved_9", t3cfg.copy(reserved = listOf(ReservedArea(w = 9, h = 9, x = 8, y = 8)), power = smallPole))
        trial(out, "t3_reserved_9_20_s3", r9, 20, 20, 3); trial(out, "t3_reserved_9_21_s1", r9, 21, 21, 1)
        val r7 = scen("t3_reserved_7", t3cfg.copy(reserved = listOf(ReservedArea(w = 7, h = 7, x = 9, y = 9)), power = smallPole))
        trial(out, "t3_reserved_7_20_s1", r7, 20, 20, 1); trial(out, "t3_reserved_7_23_s6", r7, 23, 23, 6)
        val r5 = scen("t3_reserved_5_edge", t3cfg.copy(reserved = listOf(ReservedArea(w = 5, h = 24, corner = "north-east")), power = smallPole))
        trial(out, "t3_reserved_5_edge_20_s1", r5, 20, 20, 1)
        many("t3_columns", scen("t3_columns", t3cfg) { it.copy(place = it.place.copy(initial = "columns")) }, 24, 1L..2L)
        many("t3_mixed", scen("t3_mixed", t3cfg) { it.copy(place = it.place.copy(initial = "mixed")) }, 24, 1L..2L)

        // A start from a larger box's placement, with and without its temperature (the refine branch).
        val base = trial(out, "t3_base_28", t3, 28, 28, 4)
        trial(out, "t3_warm_t0_20", t3, 20, 20, 3, warm = base.placement, warmT0 = base.annealT0)
        trial(out, "t3_warm_refine_20", t3, 20, 20, 3, warm = base.placement)
        trial(out, "t3_warm_same_28", t3, 28, 28, 5, warm = base.placement, warmT0 = base.annealT0)

        // The retries, forced: spread whenever a trial fails, and the near-miss retry whatever the margin.
        val spreadEdit = { t: Tuning -> t.copy(route = t.route.copy(maxIterations = 12), place = t.place.copy(feedbackRounds = 1, spreadMax = 100000, spreadAttempts = 2, nearMissMax = 0)) }
        val nearEdit = { t: Tuning -> t.copy(route = t.route.copy(maxIterations = 12, endgameRounds = 4), place = t.place.copy(feedbackRounds = 1, nearMissMax = 100000, nearMissAttempts = 3)) }
        val spread = scen("t3_spread", t3cfg, spreadEdit)
        val near = scen("t3_nearmiss", t3cfg, nearEdit)
        listOf(13, 14, 15, 16).forEach { size -> many("t3_spread", spread, size, 1L..2L); many("t3_nearmiss", near, size, 1L..2L) }

        // Jobs of the other trial tests.
        val plastic = RunConfiguration(targets = listOf(ConfigTarget("plastic-bar", 2.0)), sources = listOf(ConfigSource("crude-oil", edge = "west"), ConfigSource("coal", edge = "north")))
        many("plastic", scen("plastic", plastic), 24, 1L..2L)
        val iron = RunConfiguration(targets = listOf(ConfigTarget("iron-gear-wheel", 20.0)), sources = listOf(ConfigSource("iron-plate", edge = "west")), research = ResearchConfig(2))
        many("iron40", scen("iron40", iron), 28, 1L..2L)
        val foundry = RunConfiguration(targets = listOf(ConfigTarget("iron-gear-wheel", 4.0)), machines = listOf("foundry"), sources = listOf(ConfigSource("molten-iron", edge = "west")))
        many("foundry", scen("foundry", foundry), 16, 1L..2L)
        val em = RunConfiguration(targets = listOf(ConfigTarget("electronic-circuit", 4.0)), machines = listOf("electromagnetic-plant"),
            sources = listOf(ConfigSource("iron-plate", edge = "west"), ConfigSource("copper-plate", edge = "west")))
        many("em", scen("em", em), 16, 1L..2L)
        val crusher = RunConfiguration(targets = listOf(ConfigTarget("iron-ore", 10.0)), machines = listOf("crusher"), rawResources = emptyList(),
            sources = listOf(ConfigSource("metallic-asteroid-chunk", edge = "west")),
            forbiddenRecipes = listOf("advanced-metallic-asteroid-crushing", "metallic-asteroid-reprocessing"))
        many("crusher", scen("crusher", crusher), 12, 1L..2L)
        val direct = RunConfiguration(targets = listOf(ConfigTarget("iron-plate", 0.5), ConfigTarget("electronic-circuit", 0.5)),
            machines = listOf("crusher", "assembling-machine-3", "electric-furnace", "chemical-plant", "foundry"), rawResources = emptyList(),
            sources = listOf(ConfigSource("metallic-asteroid-chunk", edge = "west"), ConfigSource("oxide-asteroid-chunk", edge = "north")),
            forbiddenRecipes = listOf("metallic-asteroid-reprocessing", "oxide-asteroid-reprocessing"))
        many("direct", scen("direct", direct), 24, 1L..3L)
        val platform = RunConfiguration(targets = listOf(ConfigTarget("iron-gear-wheel", 2.0)), machines = listOf("crusher", "electric-furnace", "assembling-machine-3"), rawResources = emptyList(),
            sources = listOf(ConfigSource("metallic-asteroid-chunk", edge = "west")), forbiddenRecipes = listOf("advanced-metallic-asteroid-crushing", "metallic-asteroid-reprocessing"))
        many("platform", scen("platform", platform), 22, 1L..2L)
        val pinned = RunConfiguration.parse("""{"targets": [{"name": "electronic-circuit", "amount": 6.0}], "machine": "assembling-machine-2",
            "inserters": {"allowed": ["inserter"]}, "sources": [{"name": "copper-plate", "edge": "west", "from": 5, "to": 5}, {"name": "iron-plate", "edge": "north"}]}""")
        val fluids = RunConfiguration.parse("""{"targets": [{"name": "sulfur", "amount": 2.0}, {"name": "lubricant", "amount": 10.0}],
            "sources": [{"name": "petroleum-gas", "edge": "west", "from": 0, "to": 0}, {"name": "heavy-oil", "edge": "west", "from": 1, "to": 1}, {"name": "water", "edge": "west", "from": 2, "to": 2}]}""")
        many("fluids", scen("fluids", fluids), 20, 1L..1L)

        // The examples, at sizes the search reaches.
        many("red_green", scen("red_green", job("$ex/red_green_science.json")), 20, 1L..1L)
        many("red_green_shared", scen("red_green_shared", job("$ex/red_green_science_shared.json")), 18, 1L..1L)
        many("chem", scen("chem", job("$ex/chemical_science.json")), 40, 4L..4L)
        many("em_science", scen("em_science", job("$ex/electromagnetic_science.json")), 46, 2L..2L)
        many("metallurgic", scen("metallurgic", job("$ex/metallurgic_science.json")), 52, 1L..1L)
        // Tight boxes of the examples: the retries as they happen in a real search (one is slow).
        many("chem_tight", scen("chem_tight", job("$ex/chemical_science.json")) , 36, 7L..7L)

        File(out, "verdicts.txt").writeText(verdicts.joinToString("\n"))
    }

    /** Looks for trials whose power placement fails (to reach the set-cover retry and the lattice fallback). TRIAL_SCAN=<dir>. */
    @Test
    fun scanPower() {
        val out = System.getenv("TRIAL_SCAN")?.let { File(it) } ?: return
        listOf("plans", "configs", "tunings", "cases").forEach { File(out, it).mkdirs() }
        val t3 = job("$jobs/t3.json")
        val small = PowerConfig(pole = "small-electric-pole", allowOutsideRing = false)
        val variants = listOf(
            "two_areas" to t3.copy(reserved = listOf(ReservedArea(w = 6, h = 20, corner = "north-east"), ReservedArea(w = 6, h = 10, corner = "south-west")), power = small),
            "centre_cross" to t3.copy(reserved = listOf(ReservedArea(w = 12, h = 3, x = 6, y = 10), ReservedArea(w = 3, h = 12, x = 10, y = 6)), power = small),
            "three_blocks" to t3.copy(reserved = listOf(ReservedArea(w = 5, h = 5, x = 4, y = 4), ReservedArea(w = 5, h = 5, x = 14, y = 4), ReservedArea(w = 5, h = 5, x = 9, y = 14)), power = small),
        )
        for ((name, cfg) in variants) {
            val sc = scen("scan_$name", cfg)
            for (size in listOf(22, 24, 26, 28)) for (seed in 1L..8L) trial(out, "scan_${name}_${size}_s$seed", sc, size, size, seed)
        }
    }

    // ---- blocks ----
    private fun linkJson(l: blocks.Link) = buildJsonObject {
        put("id", l.id); put("item", l.item); put("rate", l.rate); put("from", l.from); put("to", l.to); put("fluid", l.fluid); put("routed", l.routed)
    }

    private fun splitJson(sp: Split) = buildJsonObject {
        putJsonArray("blocks") { sp.blocks.forEach { add(ints(it)) } }
        putJsonArray("slots") { sp.slots.forEach { add(buildJsonArray { add(JsonPrimitive(it.row)); add(JsonPrimitive(it.col)) }) } }
        put("rows", sp.rows); put("cols", sp.cols)
        putJsonArray("links") { sp.links.forEach { add(linkJson(it)) } }
        put("warnings", strings(sp.warnings))
    }

    private fun blockPlanJson(bp: BlockPlan, planFile: String) = buildJsonObject {
        put("index", bp.index); put("global_ids", ints(bp.globalIds)); put("plan", planFile)
        putJsonArray("link_of_port") { bp.linkOfPort.toSortedMap().forEach { (p, l) -> add(buildJsonArray { add(JsonPrimitive(p)); add(JsonPrimitive(l)) }) } }
    }

    private fun entities(vararg e: PlacedEntity) = e.toList()

    @Test
    fun dumpSplitGoldens() {
        val out = System.getenv("SPLIT_OUT")?.let { File(it) } ?: return
        listOf("plans", "configs", "cases", "compact").forEach { File(out, it).mkdirs() }
        val t3 = job("$jobs/t3.json")
        val t4 = job("$jobs/t4.json").copy(outputs = listOf(ConfigSource("processing-unit", edge = "east")))
        val acid = job("$jobs/acid.json").copy(outputs = listOf(ConfigSource("sulfuric-acid", edge = "east")))
        val anyEdge = t3.copy(sources = t3.sources.map { ConfigSource(it.name, edge = "any") })
        val six = job("$ex/six_science_factory.json")
        class Variant(val name: String, val config: RunConfiguration, val blocks: BlocksConfig)
        val variants = listOf(
            Variant("t3_5", t3, BlocksConfig(maxMachines = 5)),
            Variant("t3_3", t3, BlocksConfig(maxMachines = 3)),
            Variant("t3_8", t3, BlocksConfig(maxMachines = 8)),
            Variant("t3_5_rows1", t3, BlocksConfig(maxMachines = 5, rows = 1)),
            Variant("t3_3_rows2", t3, BlocksConfig(maxMachines = 3, rows = 2)),
            Variant("t3_5_split_at", t3, BlocksConfig(maxMachines = 5, splitAt = listOf("iron-gear-wheel", "electronic-circuit"))),
            Variant("t3_5_split_at_heavy", t3, BlocksConfig(maxMachines = 5, splitAt = listOf("iron-gear-wheel"), splitAtWeight = 1.0)),
            Variant("t3_5_thin_belts", t3, BlocksConfig(maxMachines = 5, linkBeltShare = 0.05)),
            Variant("t3_5_any_edge", anyEdge, BlocksConfig(maxMachines = 5)),
            Variant("t4_6", t4, BlocksConfig(maxMachines = 6)),
            Variant("acid_2", acid, BlocksConfig(maxMachines = 2)),
            Variant("red_green_4", job("$ex/red_green_science.json"), BlocksConfig(maxMachines = 4)),
            Variant("chem_6", job("$ex/chemical_science.json"), BlocksConfig(maxMachines = 6)),
            Variant("chem_12", job("$ex/chemical_science.json"), BlocksConfig(maxMachines = 12)),
            Variant("em_10", job("$ex/electromagnetic_science.json"), BlocksConfig(maxMachines = 10)),
            Variant("metallurgic_8", job("$ex/metallurgic_science.json"), BlocksConfig(maxMachines = 8)),
            Variant("gleba_15", job("$ex/gleba_agricultural_carbon_fiber.json"), BlocksConfig(maxMachines = 15)),
            Variant("six_own", six, six.blocks ?: BlocksConfig(maxMachines = 30)),
            Variant("six_2", six, BlocksConfig(maxMachines = 2)),
            Variant("six_5", six, BlocksConfig(maxMachines = 5, rows = 1)),
        )
        for (v in variants) {
            val tuning = det(Tuning.defaults().withOverrides(v.config.tuning))
            val plan = buildPlan(TestData.data, v.config, tuning)
            File(out, "plans/${v.name}.plan.json").writeText(plan.toJson())
            File(out, "configs/${v.name}.json").writeText(strictJson.encodeToString(RunConfiguration.serializer(), v.config))
            val throughput = GameConstants.beltTier(v.config.beltTier).throughput
            val split = splitPlan(plan, v.blocks, throughput)
            val blocksJson = strictJson.encodeToJsonElement(BlocksConfig.serializer(), v.blocks)
            val plansDir = File(out, "plans")
            val blockFiles = split.blocks.indices.take(6).map { b ->
                val bp = blockPlan(plan, split, b, tuning)
                File(plansDir, "${v.name}.block$b.plan.json").writeText(bp.plan.toJson())
                blockPlanJson(bp, "plans/${v.name}.block$b.plan.json")
            }
            // Every pair of neighbours in a row, merged (the factory does it for one pair after another).
            val merges = ArrayList<JsonElement>()
            for (a in split.blocks.indices) for (b in split.blocks.indices) {
                if (split.slots[a].row == split.slots[b].row && split.slots[b].col == split.slots[a].col + 1 && merges.size < (if (v.name.startsWith("six")) 2 else 4)) {
                    val merged = mergeBlocks(split, a, b)
                    val m = a - (if (b < a) 1 else 0)
                    val bp = blockPlan(plan, merged, m, tuning)
                    File(plansDir, "${v.name}.merge${a}_$b.plan.json").writeText(bp.plan.toJson())
                    merges += buildJsonObject {
                        put("a", a); put("b", b); put("split", splitJson(merged)); put("m", m)
                        put("block", blockPlanJson(bp, "plans/${v.name}.merge${a}_$b.plan.json"))
                    }
                }
            }
            write(File(out, "cases/${v.name}.json"), buildJsonObject {
                put("upstream", System.getenv("UPSTREAM") ?: "unknown")
                put("case", v.name); put("plan", "plans/${v.name}.plan.json"); put("config", "configs/${v.name}.json")
                put("blocks_config", blocksJson); put("belt_throughput", throughput)
                put("split", splitJson(split))
                put("block_plans", JsonArray(blockFiles))
                put("merges", JsonArray(merges))
            })
            println("split ${v.name}: ${plan.machines.size} machines -> ${split.blocks.size} blocks ${split.blocks.map { it.size }}, ${split.links.size} links, rows=${split.rows}, warnings=${split.warnings.size}, merges=${merges.size}")
        }

        // compactLayout: the committed example layouts (poles taken out, as the factory does) and small fixtures.
        fun belt(x: Int, y: Int, d: Dir) = PlacedEntity("fast-transport-belt", Kind.BELT, x, y, d)
        class Fixture(val name: String, val ents: List<PlacedEntity>, val w: Int, val h: Int)
        val fixtures = ArrayList(listOf(
            Fixture("belt_column", (0..5).map { belt(1, it, Dir.S) }, 3, 6),
            Fixture("gap_pointed_into", entities(belt(0, 0, Dir.S), belt(0, 2, Dir.E), belt(1, 2, Dir.E)), 2, 3),
            Fixture("inserter_across", entities(PlacedEntity("steel-chest", Kind.CHEST, 0, 0), PlacedEntity("long-handed-inserter", Kind.INSERTER, 0, 2, Dir.N), PlacedEntity("steel-chest", Kind.CHEST, 0, 4)), 1, 5),
            Fixture("pipe_column", (0..7).map { PlacedEntity("pipe", Kind.PIPE, 2, it) }, 5, 8),
            Fixture("pipe_beside_pipe", (0..5).map { PlacedEntity("pipe", Kind.PIPE, 1, it) } + (0..5).map { PlacedEntity("pipe", Kind.PIPE, 3, it) }, 5, 6),
            Fixture("belts_and_pipe", (0..9).map { belt(0, it, Dir.S) } + (0..9).map { PlacedEntity("pipe", Kind.PIPE, 4, it) } + (0..4).map { belt(it + 6, 4, Dir.E) }, 12, 10),
            Fixture("belt_turn", (0..4).map { belt(1, it, Dir.S) } + (1..6).map { belt(it, 5, Dir.E) } + (6..9).map { belt(6, it, Dir.S) }, 8, 10),
        ))
        for (name in listOf("red_green_science", "red_green_science_shared", "chemical_science", "electromagnetic_science", "metallurgic_science", "gleba_agricultural_carbon_fiber")) {
            val dir = "$ex/$name"
            val box = Json.parseToJsonElement(File("$dir/ports.json").readText()).jsonObject.getValue("box").jsonObject
            val w = box.getValue("w").jsonPrimitive.int; val h = box.getValue("h").jsonPrimitive.int
            val layout = decodeBlueprint(File("$dir/best_output_string.txt").readText().trim()).toLayout(w, h)
            fixtures += Fixture("example_$name", layout.entities.filter { it.kind != Kind.POLE }, w, h)
        }
        for (f in fixtures) {
            val c = compactLayout(f.ents, emptyList(), f.w, f.h)
            write(File(out, "compact/${f.name}.json"), buildJsonObject {
                put("case", f.name); put("w", f.w); put("h", f.h)
                putJsonArray("entities") { f.ents.forEach { add(entityJson(it)) } }
                putJsonObject("result") {
                    put("w", c.w); put("h", c.h); put("rows_removed", c.rowsRemoved); put("cols_removed", c.colsRemoved)
                    putJsonArray("entities") { c.entities.forEach { add(entityJson(it)) } }
                    put("xs", ints((-2..f.w + 2).map { c.x(it) })); put("ys", ints((-2..f.h + 2).map { c.y(it) }))
                }
            })
            println("compact ${f.name}: ${f.ents.size} entities ${f.w}x${f.h} -> ${c.w}x${c.h} (${c.rowsRemoved} rows, ${c.colsRemoved} columns)")
        }
        javaSamples(out)
    }

    // ---- java.util.HashMap iteration after removals, and Formatter ----
    private fun javaSamples(out: File) {
        val rnd = SplittableRandom(5)
        val runs = ArrayList<JsonElement>()
        fun <K> run(kind: String, range: Int, key: (Int) -> K, enc: (K) -> JsonElement) {
            repeat(6) {
                val map = HashMap<K, Int>()
                val ops = ArrayList<JsonElement>()
                val snapshots = ArrayList<JsonElement>()
                repeat(160) { i ->
                    if (map.isNotEmpty() && rnd.nextInt(10) < (if (i < 60) 2 else 6)) {
                        val k = map.keys.toList()[rnd.nextInt(map.size)]
                        map.remove(k); ops.add(buildJsonArray { add(JsonPrimitive("remove")); add(enc(k)) })
                    } else {
                        val k = key(rnd.nextInt(range))
                        // merge links a new node in at the head of its bucket and checks the table size first; put does neither.
                        if (rnd.nextBoolean()) { map[k] = i; ops.add(buildJsonArray { add(JsonPrimitive("put")); add(enc(k)) }) }
                        else { map.merge(k, 1, Int::plus); ops.add(buildJsonArray { add(JsonPrimitive("merge")); add(enc(k)) }) }
                    }
                    if (i % 8 == 7) snapshots.add(buildJsonObject { put("after", i + 1); put("order", JsonArray(map.keys.map(enc))) })
                }
                runs.add(buildJsonObject { put("kind", kind); put("ops", JsonArray(ops)); put("snapshots", JsonArray(snapshots)) })
            }
        }
        run("int", 300, { it }, { JsonPrimitive(it) })
        run("long", 60 * 60, { v -> (v / 60).toLong() * 1_000_000 + v % 60 }, { JsonPrimitive(it) })
        run("pair", 45 * 45, { v -> (v / 45 - 1) to v % 45 }, { p -> buildJsonArray { add(JsonPrimitive(p.first)); add(JsonPrimitive(p.second)) } })
        val values = listOf(0.0, 0.05, 0.15, 0.25, 0.35, 0.45, 0.285, 2.675, 1.005, 12.5, 13.5, 0.5, 1.5, 2.5, 99.95, 99.949, 1234.5678, 1e-7, 123456.785, 0.1 + 0.2, 7.0, -0.001, -0.25, -12.5, 1e10, 4.35, 8.345, 0.045)
        write(File(out, "java_samples.json"), buildJsonObject {
            put("hash_runs", JsonArray(runs))
            putJsonArray("fmt") {
                for (v in values) for (d in 0..3) add(buildJsonObject { put("value", v); put("digits", d); put("text", "%.${d}f".format(v)) })
            }
        })
    }

    // ---- searches ----
    private fun searchTuning(config: RunConfiguration) = Tuning.defaults().withOverrides(config.tuning).let { det(it).copy(search = it.search.copy(extraRounds = 2)) }
    private fun searchConfig(c: RunConfiguration) = c.copy(search = SearchConfig(timeBudgetS = 100000.0, seedsPerSize = 2, threads = 1, seed = 7))

    private fun copyTexts(from: File, to: File, names: Set<String>) {
        from.walkTopDown().filter { it.isFile && it.name in names }.forEach { f ->
            val dest = File(to, f.relativeTo(from).path)
            dest.parentFile.mkdirs(); f.copyTo(dest, overwrite = true)
        }
    }

    @Test
    fun dumpSearchGoldens() {
        val out = System.getenv("SEARCH_OUT")?.let { File(it) } ?: return
        fun case(name: String, base: RunConfiguration) {
            val config = searchConfig(base)
            val tuning = searchTuning(config)
            val plan = buildPlan(TestData.data, config, tuning)
            val dir = File(out, name).also { it.deleteRecursively(); it.mkdirs() }
            File(dir, "plan.json").writeText(plan.toJson())
            File(dir, "config.json").writeText(strictJson.encodeToString(RunConfiguration.serializer(), config))
            File(dir, "tuning.json").writeText(tuning.toJson())
            val run = File(dir, "run")
            val search = Search(TestData.data, config, tuning, plan, run)
            val res = search.run()
            fun rows(f: File, drop: Set<Int>) = f.readLines().drop(1).map { l -> l.split("\t").filterIndexed { i, _ -> i !in drop } }
            val attempts = rows(File(run, "attempts.tsv"), setOf(0, 5))
            val scores = rows(File(run, "scores.tsv"), setOf(0))
            write(File(dir, "expected.json"), buildJsonObject {
                put("upstream", System.getenv("UPSTREAM") ?: "unknown")
                put("trials", res.trials)
                putJsonArray("improvements") { res.improvements.forEach { i -> add(buildJsonObject { put("w", i.w); put("h", i.h); put("score", i.score); put("belts", i.belts); put("undergrounds", i.undergrounds); put("inserters", i.inserters); put("seed", i.seed) }) } }
                put("best", res.best?.let { b -> buildJsonObject { put("w", b.w); put("h", b.h); put("seed", b.seed); put("score", b.score); put("anneal_t0", b.annealT0) } } ?: JsonNull)
                put("log", strings(res.log.map(::norm)))
                putJsonArray("attempts") { attempts.forEach { add(strings(it)) } }
                putJsonArray("scores") { scores.forEach { add(strings(it)) } }
                putJsonArray("boxes") { (3..60).forEach { s -> val (w, h) = search.box(s); add(buildJsonArray { add(JsonPrimitive(s)); add(JsonPrimitive(w)); add(JsonPrimitive(h)) }) } }
            })
            copyTexts(run, File(dir, "files"), setOf("best_output_string.txt", "best_blueprint.json", "ports.json"))
            println("search $name: ${plan.machines.size} machines, ${res.trials} trials, improvements ${res.improvements.map { "${it.w}x${it.h} %.1f".format(it.score) }}")
        }
        val t1 = job("$jobs/t1.json")
        case("t1", t1)
        case("t2_max12", job("$jobs/t2.json").copy(box = BoxConfig(maxSize = 12)))
        case("t3", job("$jobs/t3.json"))
        case("t1_fixed_width", t1.copy(box = BoxConfig(fixedWidth = 12)))
        case("t3_aspect", job("$jobs/t3.json").copy(box = BoxConfig(aspect = 1.5)))
    }

    // ---- factories ----
    @Test
    fun dumpFactoryGoldens() {
        val out = System.getenv("FACTORY_OUT")?.let { File(it) } ?: return
        fun case(name: String, base: RunConfiguration) {
            val config = searchConfig(base)
            val tuning = searchTuning(config)
            val plan = buildPlan(TestData.data, config, tuning)
            val dir = File(out, name).also { it.deleteRecursively(); it.mkdirs() }
            File(dir, "plan.json").writeText(plan.toJson())
            File(dir, "config.json").writeText(strictJson.encodeToString(RunConfiguration.serializer(), config))
            File(dir, "tuning.json").writeText(tuning.toJson())
            val run = File(dir, "run")
            val f = runFactory(TestData.data, config, tuning, plan, run)
            write(File(dir, "expected.json"), buildJsonObject {
                put("upstream", System.getenv("UPSTREAM") ?: "unknown")
                put("factory", f?.let { fa ->
                    buildJsonObject {
                        put("success", fa.success); put("score", fa.score); put("notes", strings(fa.notes))
                        put("layout", layoutJson(fa.layout))
                        putJsonArray("errors") { fa.errors.forEach { add(errorJson(it)) } }
                        putJsonArray("sources") { fa.sources.forEach { add(sourceJson(it)) } }
                        putJsonArray("outputs") { fa.outputs.forEach { add(outputJson(it)) } }
                    }
                } ?: JsonNull)
                put("log", strings(File(run, "factory.txt").readLines().map(::norm)))
            })
            copyTexts(run, File(dir, "files"), setOf("best_output_string.txt", "ports.json"))
            println("factory $name: ${plan.machines.size} machines -> ${f?.let { "${it.layout.w}x${it.layout.h} success=${it.success} score=${"%.1f".format(it.score)} notes=${it.notes}" } ?: "none"}")
        }
        val t3 = job("$jobs/t3.json")
        case("t3_two_blocks", t3.copy(blocks = BlocksConfig(maxMachines = 5)))
        case("t3_two_blocks_test", t3.copy(blocks = BlocksConfig(maxMachines = 5), testMode = true))
        case("t3_shared", t3.copy(blocks = BlocksConfig(maxMachines = 5), laneSharing = true))
        case("t4_edges", job("$jobs/t4.json").copy(outputs = listOf(ConfigSource("processing-unit", edge = "east")), blocks = BlocksConfig(maxMachines = 6)))
        case("acid_east", job("$jobs/acid.json").copy(outputs = listOf(ConfigSource("sulfuric-acid", edge = "east")), blocks = BlocksConfig(maxMachines = 2)))
        case("t3_reserved", t3.copy(blocks = BlocksConfig(maxMachines = 5), reserved = listOf(ReservedArea(w = 5, h = 4))))
        case("t3_repack_merge", t3.copy(blocks = BlocksConfig(maxMachines = 3, repack = true, merge = true)))
    }
    // ---- timing: runTrial on the golden trial cases ----
    private val benchCases = listOf(
        "t3_24_s1", "red_green_20_s1", "chem_40_s4", "em_science_46_s2", "metallurgic_52_s1", "t4_pipes_26_s1", "iron40_28_s1",
        "t3_16_s2", "t3_spread_14_s1", "chem_tight_36_s7",
    )

    /**
     * Times runTrial on cases of the committed trial goldens (rust/tests/golden/trial), reading their plan, config and
     * tuning from the same files the Rust benchmark reads, so both sides do the same work (the goldens prove they
     * get the same results). TRIAL_BENCH_OUT=<dir> writes trial_bench_kotlin.json there; BENCH_CASES picks cases
     * (comma separated), BENCH_WARM and BENCH_REPS set the protocol (3 and 5; a case over 10 s runs 1 and 3 times).
     */
    @Test
    fun benchTrials() {
        val out = System.getenv("TRIAL_BENCH_OUT")?.let { File(it) } ?: return
        out.mkdirs()
        val dir = File("rust/tests/golden/trial")
        val names = System.getenv("BENCH_CASES")?.split(",") ?: benchCases
        val report = ArrayList<JsonElement>()
        for (name in names) {
            val g = Json.parseToJsonElement(File(dir, "cases/$name.json").readText()).jsonObject
            fun path(key: String) = File(dir, g.getValue(key).jsonPrimitive.content)
            val config = RunConfiguration.load(path("config").path)
            val tuning = Tuning.load(path("tuning").path)
            val plan = strictJson.decodeFromString(ProductionPlan.serializer(), path("plan").readText())
            val w = g.getValue("w").jsonPrimitive.int; val h = g.getValue("h").jsonPrimitive.int
            val seed = g.getValue("seed").jsonPrimitive.content.toLong()
            val want = g.getValue("result").jsonObject
            fun once(): Pair<Long, TrialResult> {
                val t = System.nanoTime()
                val r = runTrial(TestData.data, plan, config, tuning, w, h, seed)
                return (System.nanoTime() - t) to r
            }
            val first = once()
            check(first.second.stage == want.getValue("stage").jsonPrimitive.content && first.second.notes.size == want.getValue("notes").jsonArray.size) {
                "$name: Kotlin no longer gives what the golden says (${first.second.stage})"
            }
            val heavy = first.first > 10_000_000_000L
            val warm = (System.getenv("BENCH_WARM")?.toInt() ?: if (heavy) 1 else 3) - 1
            val reps = System.getenv("BENCH_REPS")?.toInt() ?: if (heavy) 3 else 5
            repeat(warm.coerceAtLeast(0)) { once() }
            val runs = (0 until reps).map { once() }
            val times = runs.map { it.first / 1e6 }.sorted()
            val r = runs[0].second
            report += buildJsonObject {
                put("case", name); put("machines", plan.machines.size); put("w", w); put("h", h); put("seed", seed)
                put("stage", r.stage); put("score", r.score); put("notes", r.notes.size)
                put("median_ms", times[times.size / 2]); put("min_ms", times[0]); put("reps", reps)
            }
            println("bench $name: ${plan.machines.size} machines ${w}x$h -> ${r.stage}, median ${"%.1f".format(times[times.size / 2])} ms over $reps runs")
        }
        write(File(out, "trial_bench_kotlin.json"), buildJsonObject {
            put("runtime", "jvm ${System.getProperty("java.version")}"); put("cases", JsonArray(report))
        })
    }
}
