import config.RunConfiguration
import config.SearchConfig
import config.Tuning
import config.strictJson
import emit.VerifyKit
import emit.decodeBlueprint
import emit.toLayout
import geom.Layout
import geom.PlacedEntity
import geom.Tile
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.double
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import plan.buildPlan
import search.TrialResult
import search.calibrate
import search.runTrial
import validate.ErrorType
import validate.ValidationError
import viz.HeatOverlay
import viz.PortLabel
import viz.RenderOptions
import viz.portLabels
import viz.renderLayout
import java.io.File
import javax.imageio.ImageIO
import kotlin.test.Test

/**
 * Reference outputs for the Rust port of the renders, the verify kit and calibrate. Not part of the Kotlin project:
 * the Rust tooling copies this file into the container's work tree. Each dump is skipped unless its variable is set
 * to a directory:
 *
 *   VIZ_OUT=<dir>     renderLayout cases: cases/<name>.json (layout, options, size) and png/<name>.png
 *   VERIFY_OUT=<dir>  every VerifyKit case's layout (verify_kit.json)
 *   CALIB_OUT=<dir>   calibrate on two small jobs: base tuning, job configs, report, tuned tuning
 *
 * Trials and searches run with endgameTimeFactor = 1e6, one thread, a budget that is never reached and a small
 * extra_rounds, so they are reproducible.
 */
class GoldenVizTest {
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

    private fun write(file: File, o: JsonElement) {
        file.parentFile.mkdirs()
        file.writeText(json.encodeToString(JsonElement.serializer(), o))
    }

    private fun det(t: Tuning) = t.copy(route = t.route.copy(endgameTimeFactor = 1e6))

    private fun optionsJson(o: RenderOptions) = buildJsonObject {
        put("px", o.px); put("show_supply", o.showSupply); put("title", nullable(o.title))
        putJsonArray("errors") { o.errors.forEach { e -> add(buildJsonObject { put("type", e.type.name); put("x", e.tile.x); put("y", e.tile.y); put("message", e.message) }) } }
        put("heat", o.heat?.let { h ->
            buildJsonObject {
                putJsonArray("values") { h.values.entries.sortedWith(compareBy({ it.key.y }, { it.key.x })).forEach { (t, v) -> add(buildJsonObject { put("x", t.x); put("y", t.y); put("v", v) }) } }
                putJsonArray("overused") { h.overused.sortedWith(compareBy({ it.y }, { it.x })).forEach { add(tile(it)) } }
            }
        } ?: JsonNull)
        putJsonArray("ports") { o.ports.forEach { p -> add(buildJsonObject { put("x", p.tile.x); put("y", p.tile.y); put("item", p.item); put("rate", p.rate); put("input", p.input); put("lane", p.lane); put("fluid", p.fluid) }) } }
    }

    /** Draws the case with the Kotlin renderer and writes its description and the image's size. */
    private fun case(out: File, name: String, layout: Layout, opt: RenderOptions) {
        val png = File(out, "png/$name.png")
        renderLayout(layout, png, opt)
        val img = ImageIO.read(png)
        write(File(out, "cases/$name.json"), buildJsonObject {
            put("upstream", System.getenv("UPSTREAM") ?: "unknown")
            put("case", name); put("png", "png/$name.png"); put("width", img.width); put("height", img.height)
            put("layout", layoutJson(layout)); put("options", optionsJson(opt))
        })
        println("viz $name: ${layout.entities.size} entities ${layout.w}x${layout.h} px ${opt.px} -> ${img.width}x${img.height}")
    }

    private fun kit(id: String) = VerifyKit.cases.first { it.id == id }.layout

    private fun trial(job: String, size: Int, seed: Long, edit: (RunConfiguration) -> RunConfiguration = { it }): TrialResult {
        val config = edit(RunConfiguration.load(job))
        val tuning = det(Tuning.defaults().withOverrides(config.tuning))
        val plan = buildPlan(TestData.data, config, tuning)
        return runTrial(TestData.data, plan, config, tuning, size, size, seed).also { check(it.success) { "$job did not route: ${it.stage}" } }
    }

    /** A committed example: its blueprint decoded (no net ids) and the port labels of its ports.json, as --render does. */
    private fun example(name: String): Pair<Layout, List<PortLabel>> {
        val dir = "$ex/$name"
        val ports = Json.parseToJsonElement(File("$dir/ports.json").readText()).jsonObject
        val box = ports.getValue("box").jsonObject
        val w = box.getValue("w").jsonPrimitive.int; val h = box.getValue("h").jsonPrimitive.int
        val layout = decodeBlueprint(File("$dir/best_output_string.txt").readText().trim()).toLayout(w, h)
        fun labels(key: String, input: Boolean) = (ports[key]?.jsonArray ?: JsonArray(emptyList())).map { it.jsonObject }.map { o ->
            val lane = when (o["lane"]?.jsonPrimitive?.content) { "left" -> 0; "right" -> 1; else -> -1 }
            PortLabel(Tile(o.getValue("x").jsonPrimitive.int, o.getValue("y").jsonPrimitive.int), o.getValue("item").jsonPrimitive.content,
                o.getValue("rate").jsonPrimitive.double, input, lane, o.getValue("fluid").jsonPrimitive.boolean)
        }
        return layout to (labels("inputs", true) + labels("outputs", false))
    }

    @Test
    fun dumpVizGoldens() {
        val out = System.getenv("VIZ_OUT")?.let { File(it) } ?: return
        listOf("cases", "png").forEach { File(out, it).mkdirs() }

        // The hand-built kit layouts: every belt, underground, splitter, inserter, chest, pipe and pole shape.
        case(out, "kit_v2_lanes", kit("v2-inserter-lanes"), RenderOptions(title = "v2 lanes"))
        case(out, "kit_v3_undergrounds", kit("v3-underground-pairing"), RenderOptions())
        case(out, "kit_v5_poles_supply", kit("v5-poles-without-wires"), RenderOptions(showSupply = true, title = "supply areas"))
        case(out, "kit_v6_infinity", kit("v6-infinity-settings"), RenderOptions())
        case(out, "kit_v8_inserters", kit("v8-inserter-rates"), RenderOptions(px = 24, showSupply = true))
        case(out, "kit_v12_pipe_to_ground", kit("v12-pipe-to-ground-span"), RenderOptions(px = 24))
        case(out, "kit_v13_splitter", kit("v13-splitter-lanes"), RenderOptions())
        case(out, "kit_v16_weaving", kit("v16-underground-weaving"), RenderOptions(px = 28))
        case(out, "kit_v11_refineries", kit("v11-refinery-connections"), RenderOptions(px = 20))
        case(out, "kit_v17_fluid_machines", kit("v17-space-age-fluid-connections"), RenderOptions(px = 16))

        // Committed example blueprints, decoded: belts coloured by chain, pipes, ports on the ring with their legend.
        example("red_green_science").let { (l, p) -> case(out, "example_red_green", l, RenderOptions(px = 16, ports = p, title = "${l.w}x${l.h}")) }
        example("chemical_science").let { (l, p) -> case(out, "example_chemical", l, RenderOptions(px = 14, ports = p, title = "${l.w}x${l.h}")) }

        // Real trials: nets colour the belts, marker tiles, ring ports, a harness in test mode.
        val reserved = trial("$jobs/t3.json", 24, 1) { it.copy(reserved = listOf(config.ReservedArea(w = 5, h = 4))) }
        case(out, "trial_t3_reserved", reserved.layout!!, RenderOptions(px = 16, ports = portLabels(reserved.sources, reserved.outputs), title = "t3 reserved"))
        val testMode = trial("$jobs/t4.json", 24, 1) { it.copy(testMode = true, fluids = config.FluidConfig("infinity-pipe")) }
        case(out, "trial_t4_test_mode", testMode.layout!!, RenderOptions(px = 16, ports = portLabels(testMode.sources, testMode.outputs)))

        // Validator errors numbered on the tiles and listed under the picture; a heat overlay with overused tiles.
        val some = reserved.layout!!
        val errors = listOf(
            ValidationError(ErrorType.OVERLAP, Tile(3, 4), "two entities on one tile"),
            ValidationError(ErrorType.UNPOWERED, Tile(10, 8), "machine is outside every pole's supply area"),
            ValidationError(ErrorType.STARVED, Tile(15, 15), "no iron-plate reaches this inserter"),
        )
        case(out, "trial_errors", some, RenderOptions(px = 16, errors = errors, title = "with errors"))
        val heat = HeatOverlay(
            (0 until 40).associate { Tile(2 + it % 10, 3 + it / 10) to (1.0 + it) },
            setOf(Tile(5, 5), Tile(6, 5), Tile(12, 12)),
        )
        case(out, "trial_heat", some, RenderOptions(px = 16, heat = heat, title = "heat"))
    }

    @Test
    fun dumpVerifyKitGoldens() {
        val out = System.getenv("VERIFY_OUT")?.let { File(it) } ?: return
        write(File(out, "verify_kit.json"), buildJsonObject {
            put("upstream", System.getenv("UPSTREAM") ?: "unknown")
            putJsonArray("cases") {
                VerifyKit.cases.forEach { c -> add(buildJsonObject { put("id", c.id); put("title", c.title); put("check", c.check); put("layout", layoutJson(c.layout)) }) }
            }
            put("t1_hand", layoutJson(VerifyKit.t1Hand))
        })
        // The strings the kit writes, for comparing with docs/verify.
        VerifyKit.write(File(out, "written"))
        println("verify kit: ${VerifyKit.cases.size} cases")
    }

    @Test
    fun dumpCalibrateGoldens() {
        val out = System.getenv("CALIB_OUT")?.let { File(it) } ?: return
        out.mkdirs()
        val base = det(Tuning()).let { it.copy(search = it.search.copy(extraRounds = 2)) }
        fun quick(c: RunConfiguration) = c.copy(search = SearchConfig(timeBudgetS = 100000.0, seedsPerSize = 2, threads = 1, seed = 7))
        // Chains of 2 to 5 terminals and more: the red and green science job, acid with pipes, lanes shared on a belt, 40 iron a second.
        val iron = RunConfiguration(targets = listOf(config.ConfigTarget("iron-gear-wheel", 20.0)), sources = listOf(config.ConfigSource("iron-plate", edge = "west")), research = config.ResearchConfig(2))
        val list = listOf(
            "t3" to quick(RunConfiguration.load("$jobs/t3.json")), "red_green_science" to quick(RunConfiguration.load("$ex/red_green_science.json")),
            "t4" to quick(RunConfiguration.load("$jobs/t4.json")), "pair" to quick(RunConfiguration.load("$jobs/pair.json").copy(laneSharing = true)),
            "iron40" to quick(iron.copy(box = config.BoxConfig(startSize = 28))),
        )
        File(out, "base_tuning.json").writeText(base.toJson())
        list.forEach { (name, c) -> File(out, "configs/$name.json").also { it.parentFile.mkdirs() }.writeText(strictJson.encodeToString(RunConfiguration.serializer(), c)) }
        val res = calibrate(TestData.data, list, base, 100000.0, File(out, "run"))
        write(File(out, "expected.json"), buildJsonObject {
            put("upstream", System.getenv("UPSTREAM") ?: "unknown")
            put("budget", 100000.0); put("samples", res.samples)
            putJsonArray("jobs") { list.forEach { add(JsonPrimitive(it.first)) } }
            putJsonArray("report") { res.report.forEach { add(JsonPrimitive(it)) } }
            put("tuned", JsonPrimitive(strictJson.encodeToString(Tuning.serializer(), res.tuning)))
            put("ratios", JsonObject(res.areaRatios.mapValues { JsonPrimitive(it.value) }))
        })
        res.report.forEach { println("calibrate $it") }
    }
}
