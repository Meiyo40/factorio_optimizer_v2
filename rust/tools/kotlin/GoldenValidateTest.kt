import config.RunConfiguration
import config.Tuning
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
import kotlinx.serialization.json.addJsonObject
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
import kotlinx.serialization.json.putJsonObject
import place.PlacedMachine
import place.Placement
import plan.buildPlan
import power.buildHarness
import power.placePoles
import power.poleCandidates
import power.poleLattice
import search.validationSpec
import validate.MachineSpec
import validate.OutputSpec
import validate.SourceSpec
import validate.ValidationError
import validate.ValidationSpec
import validate.recipeInfoFrom
import validate.validate
import java.io.File
import java.util.SplittableRandom
import kotlin.test.Test

/**
 * Reference outputs for the Rust port of the validator, the power placer and the test harness. Not part of the
 * Kotlin project: the Rust tooling copies this file into the container's work tree. Skipped unless VALIDATE_OUT is
 * set (a directory). Writes validate/<example>.json, power/<example>.json, recipe_info.json and java_samples.json.
 */
class GoldenValidateTest {
    private val pretty = Json { prettyPrint = true }
    private val examples = listOf(
        "docs/examples/red_green_science", "docs/examples/red_green_science_shared", "docs/examples/chemical_science",
        "docs/examples/electromagnetic_science", "docs/examples/metallurgic_science",
        "docs/examples/gleba_agricultural_carbon_fiber", "docs/examples/six_science_factory", "docs/status/six_science_9_blocks",
    )
    /** The ones small enough for power placement in the Kotlin (it is quadratic in the box). */
    private val powerExamples = setOf("red_green_science", "chemical_science", "electromagnetic_science")
    /** Mutations are worth their size only on the smaller layouts. */
    private val mutated = setOf("red_green_science", "red_green_science_shared", "chemical_science", "electromagnetic_science", "gleba_agricultural_carbon_fiber")

    private fun tile(t: Tile) = buildJsonObject { put("x", t.x); put("y", t.y) }
    private fun nullable(s: String?): JsonElement = s?.let { JsonPrimitive(it) } ?: JsonNull

    private fun errorJson(e: ValidationError) = buildJsonObject { put("type", e.type.name); put("x", e.tile.x); put("y", e.tile.y); put("message", e.message) }

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

    private fun entityJson(e: PlacedEntity) = buildJsonObject {
        put("name", e.name); put("kind", e.kind.name); put("x", e.x); put("y", e.y); put("dir", e.dir.name)
        put("item", nullable(e.item)); put("fluid", nullable(e.fluid)); put("drain", e.drain)
    }

    private class Example(val dir: String, val name: String, val w: Int, val h: Int, val config: RunConfiguration, val layout: Layout,
                          val inputs: List<SourceSpec>, val outputs: List<OutputSpec>, val spec: ValidationSpec)

    private fun load(dir: String): Example {
        val name = dir.substringAfterLast('/')
        val config = RunConfiguration.load("$dir.json")
        val tuning = Tuning.defaults().withOverrides(config.tuning)
        val plan = buildPlan(TestData.data, config, tuning)
        val ports = Json.parseToJsonElement(File("$dir/ports.json").readText()).jsonObject
        val box = ports.getValue("box").jsonObject
        val w = box.getValue("w").jsonPrimitive.int; val h = box.getValue("h").jsonPrimitive.int
        val layout = decodeBlueprint(File("$dir/best_output_string.txt").readText().trim()).toLayout(w, h)
        fun lane(o: JsonObject) = when (o["lane"]?.jsonPrimitive?.content) { "left" -> 0; "right" -> 1; else -> -1 }
        val inputs = ports.getValue("inputs").jsonArray.map { it.jsonObject }.map { o ->
            SourceSpec(Tile(o.getValue("x").jsonPrimitive.int, o.getValue("y").jsonPrimitive.int), o.getValue("item").jsonPrimitive.content,
                o.getValue("rate").jsonPrimitive.double, o.getValue("fluid").jsonPrimitive.boolean, lane(o))
        }
        val outputs = ports.getValue("outputs").jsonArray.map { it.jsonObject }.map { o ->
            OutputSpec(Tile(o.getValue("x").jsonPrimitive.int, o.getValue("y").jsonPrimitive.int), o.getValue("item").jsonPrimitive.content,
                o.getValue("rate").jsonPrimitive.double, o.getValue("fluid").jsonPrimitive.boolean)
        }
        // A blueprint does not say which recipe a furnace makes, so machines are matched to plan machines by name
        // (and recipe where it is known), one each in plan order; the verdicts only have to agree between languages.
        val used = HashSet<Int>()
        val placed = layout.entities.filter { it.kind == Kind.MACHINE }.mapNotNull { m ->
            val fits = plan.machines.filter { it.machine == m.name && (m.recipe == null || it.recipe == m.recipe) }
            val inst = fits.firstOrNull { it.id !in used } ?: fits.firstOrNull() ?: return@mapNotNull null
            used += inst.id
            PlacedMachine(inst.id, m.x, m.y)
        }
        val spec = validationSpec(TestData.data, plan, config, Placement(w, h, placed), inputs, requirePower = config.power.mode != "none", outputs = outputs)
        return Example(dir, name, w, h, config, layout, inputs, outputs, spec)
    }

    private fun specJson(ex: Example) = buildJsonObject {
        val s = ex.spec
        put("pole_ring", s.poleRing); put("require_power", s.requirePower); put("fluid_mode", s.fluidMode); put("check_rates", s.checkRates)
        putJsonObject("inserter_rate") {
            listOf("inserter", "long-handed-inserter", "fast-inserter", "bulk-inserter").forEach { n ->
                putJsonArray(n) { add(JsonPrimitive(s.inserterRate(n, false))); add(JsonPrimitive(s.inserterRate(n, true))) }
            }
        }
        putJsonArray("machines") { s.machines.forEach { (t, m) -> addJsonObject { put("tile", tile(t)); put("spec", machineSpecJson(m)) } } }
        putJsonArray("sources") { s.sources.forEach { o -> addJsonObject { put("tile", tile(o.tile)); put("item", o.item); put("rate", o.rate); put("fluid", o.fluid); put("lane", o.lane); put("port", o.port) } } }
        putJsonArray("outputs") { s.outputs.forEach { o -> addJsonObject { put("tile", tile(o.tile)); put("item", o.item); put("rate", o.rate); put("fluid", o.fluid); put("port", o.port) } } }
        putJsonArray("outside_allowed") { s.outsideAllowed.sortedWith(compareBy({ it.y }, { it.x })).forEach { add(tile(it)) } }
    }

    // ---- mutations: small operation lists, applied the same way on both sides ----
    private class Op(val op: String, val index: Int, val dx: Int = 0, val dy: Int = 0, val item: String? = null)

    private fun opJson(o: Op) = buildJsonObject {
        put("op", o.op); put("index", o.index); put("dx", o.dx); put("dy", o.dy); put("item", nullable(o.item))
    }

    /** remove: drops the entity and any wire to it, renumbering the rest; rotate: a quarter turn clockwise; move;
     *  set_item: the filter of an inserter or chest. */
    private fun apply(l: Layout, o: Op): Layout = when (o.op) {
        "remove" -> {
            fun re(i: Int) = if (i > o.index) i - 1 else i
            l.copy(entities = l.entities.filterIndexed { i, _ -> i != o.index },
                wires = l.wires.filter { (a, b) -> a != o.index && b != o.index }.map { (a, b) -> re(a) to re(b) })
        }
        "rotate" -> l.copy(entities = l.entities.mapIndexed { i, e -> if (i == o.index) e.copy(dir = e.dir.cw) else e })
        "move" -> l.copy(entities = l.entities.mapIndexed { i, e -> if (i == o.index) e.copy(x = e.x + o.dx, y = e.y + o.dy) else e })
        "set_item" -> l.copy(entities = l.entities.mapIndexed { i, e -> if (i == o.index) e.copy(item = o.item) else e })
        else -> error(o.op)
    }

    private fun mutations(l: Layout, seed: Long): List<List<Op>> {
        val rnd = SplittableRandom(seed)
        fun of(vararg kinds: Kind) = l.entities.indices.filter { l.entities[it].kind in kinds }
        fun pick(list: List<Int>) = list[rnd.nextInt(list.size)]
        val makers = listOf<() -> Op?>(
            { of(Kind.BELT).takeIf { it.isNotEmpty() }?.let { Op("remove", pick(it)) } },
            { of(Kind.INSERTER).takeIf { it.isNotEmpty() }?.let { Op("remove", pick(it)) } },
            { of(Kind.POLE).takeIf { it.isNotEmpty() }?.let { Op("remove", pick(it)) } },
            { of(Kind.BELT).takeIf { it.isNotEmpty() }?.let { Op("rotate", pick(it)) } },
            { of(Kind.INSERTER).takeIf { it.isNotEmpty() }?.let { Op("rotate", pick(it)) } },
            { of(Kind.MACHINE).takeIf { it.isNotEmpty() }?.let { Op("move", pick(it), dx = if (rnd.nextBoolean()) 1 else -1) } },
            { of(Kind.INSERTER).takeIf { it.isNotEmpty() }?.let { Op("set_item", pick(it), item = "iron-plate") } },
            { of(Kind.UG_IN, Kind.UG_OUT, Kind.SPLITTER).takeIf { it.isNotEmpty() }?.let { Op("remove", pick(it)) } },
            { of(Kind.PIPE, Kind.PIPE_UG, Kind.INFINITY_PIPE).takeIf { it.isNotEmpty() }?.let { Op("remove", pick(it)) } },
        )
        val singles = (0 until 27).mapNotNull { k -> makers[k % makers.size]()?.let { listOf(it) } }
        val doubles = (0 until 4).mapNotNull { k -> val a = makers[(k * 2) % makers.size](); val b = makers[(k * 2 + 5) % makers.size](); if (a != null && b != null) listOf(a, b) else null }
        return singles + doubles
    }

    private fun applyAll(l: Layout, ops: List<Op>): Layout = ops.fold(l) { acc, o -> apply(acc, o) }

    @Test
    fun dumpValidatorAndPowerGoldens() {
        val out = System.getenv("VALIDATE_OUT")?.let { File(it) } ?: return
        File(out, "validate").mkdirs(); File(out, "power").mkdirs()
        val upstream = System.getenv("UPSTREAM") ?: "unknown"
        for (dir in examples) {
            val ex = load(dir)
            val cases = ArrayList<JsonElement>()
            var skipped = 0
            fun case(name: String, ops: List<Op>) {
                val errors = runCatching { validate(applyAll(ex.layout, ops), ex.spec) }.getOrElse { skipped++; return }
                cases += buildJsonObject {
                    put("name", name)
                    putJsonArray("ops") { ops.forEach { add(opJson(it)) } }
                    putJsonArray("errors") { errors.forEach { add(errorJson(it)) } }
                }
            }
            case("as committed", emptyList())
            if (ex.name in mutated) mutations(ex.layout, 1000L + ex.name.length).forEachIndexed { i, ops -> case("mutation $i: ${ops.joinToString { "${it.op} #${it.index}" }}", ops) }
            File(out, "validate/${ex.name}.json").writeText(pretty.encodeToString(JsonObject.serializer(), buildJsonObject {
                put("upstream", upstream); put("example", ex.dir); put("job", "${ex.dir}.json"); put("w", ex.w); put("h", ex.h)
                put("spec", specJson(ex))
                putJsonArray("cases") { cases.forEach { add(it) } }
            }))
            println("validate ${ex.name}: ${ex.layout.entities.size} entities, ${cases.size} cases ($skipped skipped), as committed: ${(cases[0] as JsonObject).getValue("errors").jsonArray.size} errors")

            if (ex.name !in powerExamples) continue
            val bare = ex.layout.copy(entities = ex.layout.entities.filter { it.kind != Kind.POLE }, wires = emptyList())
            val powerCases = ArrayList<JsonElement>()
            fun power(name: String, pole: String, ring: Boolean, lattice: Boolean) {
                val ps = if (lattice) placePoles(bare.entities, emptyList(), pole, fixedPoles = poleLattice(ex.w, ex.h, pole))
                    else placePoles(bare.entities, poleCandidates(ex.w, ex.h, ring), pole)
                powerCases += buildJsonObject {
                    put("name", name); put("pole", pole); put("ring", ring); put("lattice", lattice)
                    put("ok", ps.ok); put("disconnected", ps.disconnected)
                    putJsonArray("poles") { ps.poles.forEach { add(tile(it.tile)) } }
                    putJsonArray("wires") { ps.wires.forEach { (a, b) -> add(buildJsonArray { add(JsonPrimitive(a)); add(JsonPrimitive(b)) }) } }
                    putJsonArray("uncovered") { ps.uncovered.forEach { add(tile(it.tile)) } }
                }
            }
            power("medium poles in the ring", "medium-electric-pole", true, false)
            power("medium poles inside the box", "medium-electric-pole", false, false)
            power("small poles in the ring", "small-electric-pole", true, false)
            power("substations in the ring", "substation", true, false)
            power("medium pole lattice", "medium-electric-pole", true, true)
            val harness = buildHarness(ex.inputs, bare.entities, ex.w, ex.h, ex.config, ex.outputs)
            File(out, "power/${ex.name}.json").writeText(pretty.encodeToString(JsonObject.serializer(), buildJsonObject {
                put("upstream", upstream); put("example", ex.dir); put("job", "${ex.dir}.json"); put("w", ex.w); put("h", ex.h)
                putJsonArray("sources") { ex.inputs.forEach { o -> addJsonObject { put("tile", tile(o.tile)); put("item", o.item); put("rate", o.rate); put("fluid", o.fluid); put("lane", o.lane); put("port", o.port) } } }
                putJsonArray("outputs") { ex.outputs.forEach { o -> addJsonObject { put("tile", tile(o.tile)); put("item", o.item); put("rate", o.rate); put("fluid", o.fluid); put("port", o.port) } } }
                putJsonArray("power") { powerCases.forEach { add(it) } }
                putJsonObject("harness") {
                    putJsonArray("entities") { harness.entities.forEach { add(entityJson(it)) } }
                    putJsonArray("tiles") { harness.tiles.sortedWith(compareBy({ it.y }, { it.x })).forEach { add(tile(it)) } }
                    putJsonArray("warnings") { harness.warnings.forEach { add(JsonPrimitive(it)) } }
                }
            }))
            println("power ${ex.name}: ${powerCases.size} placements, harness ${harness.entities.size} entities, ${harness.warnings.size} warnings")
        }
        // The planner-independent recipe facts the validator falls back on, for every recipe.
        val info = recipeInfoFrom(TestData.data)
        File(out, "recipe_info.json").writeText(pretty.encodeToString(JsonObject.serializer(), buildJsonObject {
            TestData.data.recipes.keys.forEach { r -> info(r)?.let { put(r, machineSpecJson(it)) } }
        }))
        // Real java.util iteration orders and Formatter output, to check the Rust emulations against.
        val rnd = SplittableRandom(7)
        val items = TestData.data.items.keys.toList()
        File(out, "java_samples.json").writeText(pretty.encodeToString(JsonObject.serializer(), buildJsonObject {
            putJsonArray("int_orders") {
                listOf(5 to 100, 13 to 100, 25 to 1000, 60 to 12000, 200 to 12000, 500 to 100000, 40 to 64).forEach { (n, range) ->
                    val keys = LinkedHashSet<Int>()
                    while (keys.size < n) keys += if (range == 64) 64 * rnd.nextInt(1000) else rnd.nextInt(range)
                    val set = HashSet<Int>().also { s -> keys.forEach { s += it } }
                    val map = HashMap<Int, Int>().also { m -> keys.forEach { m[it] = 1 } }
                    addJsonObject {
                        putJsonArray("keys") { keys.forEach { add(JsonPrimitive(it)) } }
                        putJsonArray("set_order") { set.forEach { add(JsonPrimitive(it)) } }
                        putJsonArray("map_order") { map.keys.forEach { add(JsonPrimitive(it)) } }
                    }
                }
            }
            putJsonArray("string_orders") {
                listOf(7, 20, 60, items.size).forEach { n ->
                    val keys = items.take(n)
                    val map = HashMap<String, Int>().also { m -> keys.forEach { m[it] = 1 } }
                    addJsonObject {
                        putJsonArray("keys") { keys.forEach { add(JsonPrimitive(it)) } }
                        putJsonArray("order") { map.keys.forEach { add(JsonPrimitive(it)) } }
                    }
                }
            }
            putJsonArray("string_hashes") { items.take(80).forEach { s -> addJsonObject { put("s", s); put("hash", s.hashCode()) } } }
            putJsonArray("pair_orders") {
                listOf(10, 40, 150).forEach { n ->
                    val keys = LinkedHashSet<Pair<Int, String>>()
                    while (keys.size < n) keys += rnd.nextInt(3000) to items[rnd.nextInt(items.size)]
                    val map = HashMap<Pair<Int, String>, Int>().also { m -> keys.forEach { m[it] = 1 } }
                    addJsonObject {
                        putJsonArray("keys") { keys.forEach { (a, b) -> add(buildJsonArray { add(JsonPrimitive(a)); add(JsonPrimitive(b)) }) } }
                        putJsonArray("order") { map.keys.forEach { (a, b) -> add(buildJsonArray { add(JsonPrimitive(a)); add(JsonPrimitive(b)) }) } }
                    }
                }
            }
            putJsonArray("tile_orders") {
                listOf(12, 80).forEach { n ->
                    val keys = LinkedHashSet<Tile>()
                    while (keys.size < n) keys += Tile(rnd.nextInt(60) - 3, rnd.nextInt(60) - 3)
                    val set = HashSet<Tile>().also { s -> keys.forEach { s += it } }
                    addJsonObject {
                        putJsonArray("keys") { keys.forEach { add(tile(it)) } }
                        putJsonArray("order") { set.forEach { add(tile(it)) } }
                    }
                }
            }
            putJsonArray("fmt2") {
                val tricky = listOf(0.285, 1.005, 2.675, 0.125, 0.375, 0.625, 0.875, 0.995, 9.999, 1e-7, 0.0, -0.0, -0.001, -0.125, 123456.785, 0.5, 7.2, 1.115, 4.485, 0.045, 33.0)
                val random = (0 until 150).map { if (it % 3 == 0) rnd.nextInt(100000) / 1000.0 else if (it % 3 == 1) rnd.nextInt(8000) / 8.0 else rnd.nextDouble() * 50 }
                (tricky + random).forEach { d -> addJsonObject { put("value", d); put("text", "%.2f".format(d)) } }
            }
        }))
        println("recipe info and java samples written")
    }
}
