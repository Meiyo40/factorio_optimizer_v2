package blocks

import config.ConfigSource
import config.RunConfiguration
import config.Tuning
import config.strictJson
import data.GameConstants
import data.RootData
import emit.toBlueprint
import emit.toBlueprintString
import emit.toJson
import geom.Dir
import geom.Kind
import geom.Layout
import geom.PlacedEntity
import geom.Tile
import geom.outputTile
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.addJsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject
import place.PlacedMachine
import place.Placement
import plan.Net
import plan.Port
import plan.ProductionPlan
import plan.Role
import plan.Terminal
import power.buildHarness
import power.placePoles
import power.poleCandidates
import route.RouteProblem
import route.route
import search.Search
import search.TrialResult
import search.score
import search.validationSpec
import validate.OutputSpec
import validate.SourceSpec
import validate.ValidationError
import validate.validate
import viz.RenderOptions
import viz.renderLayout
import java.io.File
import kotlin.math.ceil
import kotlin.math.sqrt

/** A stitched factory: the combined layout, what validation found, and where its inputs and outputs are. */
class Factory(
    val layout: Layout, val errors: List<ValidationError>, val score: Double,
    val sources: List<SourceSpec>, val outputs: List<OutputSpec>, val notes: List<String>,
) {
    val success: Boolean get() = errors.isEmpty()
}

/**
 * Runs a job as several blocks (see [splitPlan]): each block is searched on its own (its share of the time
 * budget, by machine count), then the blocks are placed in their rows with corridors between them, the belts
 * between blocks are routed through the corridors, rows and columns that only straight belts cross are taken
 * out, poles cover the whole factory, and the result is validated against the full plan. If the corridor belts
 * do not fit, the corridors widen and it tries again. With [config.BlocksConfig.repack] every block is then
 * searched again at a common height per row, and with [config.BlocksConfig.merge] neighbouring blocks are merged
 * and searched again from their placements until the budget runs out; either is kept only if the factory gets
 * smaller. Every better factory is written to [out] (blueprint, render, ports.json; factory.txt logs it all),
 * each block to `block_<n>`.
 */
fun runFactory(data: RootData, config: RunConfiguration, tuning: Tuning, plan: ProductionPlan, out: File): Factory? {
    val cfg = config.blocks!!
    require(plan.ports.none { p -> p.specs.any { it.edge == null } }) {
        "a factory of blocks takes its inputs and outputs by edge (with from and to), not by x and y"
    }
    out.mkdirs()
    val logFile = File(out, "factory.txt")
    logFile.writeText("")
    val t0 = System.currentTimeMillis()
    val deadline = t0 + (config.search.timeBudgetS * 1000).toLong()
    fun say(msg: String) {
        val line = "[%7.1fs] %s".format((System.currentTimeMillis() - t0) / 1000.0, msg)
        println(line); logFile.appendText(line + "\n")
    }
    fun left() = (deadline - System.currentTimeMillis()) / 1000.0
    val tier = GameConstants.beltTier(config.beltTier)
    var split = splitPlan(plan, cfg, tier.throughput)
    split.warnings.forEach { say("WARNING: $it") }
    say("${plan.machines.size} machines in ${split.blocks.size} blocks (${split.rows} row(s)): " +
        split.blocks.mapIndexed { b, ms -> "block ${b + 1}: ${ms.size}" }.joinToString(", "))
    split.links.filter { it.inner }.groupBy { it.from to it.to }.forEach { (ab, ls) ->
        say("  block ${ab.first + 1} -> block ${ab.second + 1}: " + ls.groupBy { it.item }.map { (i, l) -> "$i ${"%.2f".format(l.sumOf { it.rate })}/s (${l.size} belt(s))" }.joinToString(", "))
    }
    var bps = split.blocks.indices.map { blockPlan(plan, split, it, tuning) }
    val later = (if (cfg.repack) cfg.repackShare else 0.0) + (if (cfg.merge) cfg.mergeShare else 0.0)
    fun blockConfig(budget: Double, fixedHeight: Int? = null, startSize: Int? = null) = config.copy(
        blocks = null, testMode = false, reserved = emptyList(),
        box = config.box.copy(startSize = startSize, maxSize = null, fixedHeight = fixedHeight, fixedWidth = if (fixedHeight != null) null else config.box.fixedWidth),
        search = config.search.copy(timeBudgetS = budget),
    )

    // 1. Blocks, one after another, each with all threads and its share of the budget.
    val total = plan.machines.size.toDouble()
    var results = ArrayList<TrialResult>()
    for (bp in bps) {
        val budget = minOf(left(), config.search.timeBudgetS * cfg.blockTimeShare * (1 - later) * bp.globalIds.size / total)
        val dir = File(out, "block_${bp.index + 1}")
        bp.plan.write(File(dir, "plan.json").path)
        say("block ${bp.index + 1}: ${bp.globalIds.size} machines, ${"%.0f".format(budget)} s")
        val best = Search(data, blockConfig(budget), tuning, bp.plan, dir).run().best
        if (best == null) { say("block ${bp.index + 1}: no layout found; giving up"); return null }
        say("block ${bp.index + 1}: ${best.w} x ${best.h}")
        results += best
    }

    // 2. Stitch, widening the corridors when the belts between blocks do not fit. Keep the smallest valid factory.
    var best: Factory? = null
    fun stitchAny(sp: Split, bpl: List<BlockPlan>, res: List<TrialResult>, what: String): Factory? {
        var last: Factory? = null
        // Inputs and outputs routed to the factory's edge share the corridor along the rows: one track per belt, two
        // per pipe (pipes of different fluids may not touch). Compaction takes out what stays empty.
        val tracks = sp.links.filter { it.routed }.sumOf { if (it.fluid) 2 else 1 } * (if (config.testMode) 5 else 1)
        for (attempt in 0..3) {
            val corridor = cfg.corridor + 3 * attempt
            val f = runCatching { stitch(data, config, tuning, plan, sp, bpl, res, corridor, corridor + tracks) }
                .getOrElse { e -> say("$what, corridor $corridor: failed with $e"); null } ?: run {
                say("$what, corridor $corridor: the belts between blocks did not route"); null
            } ?: continue
            say("$what, corridor $corridor: ${f.layout.w} x ${f.layout.h}, " +
                (if (f.success) "valid" else "${f.errors.size} validation errors: ${f.errors.take(5)}") +
                f.notes.joinToString("") { "; $it" })
            last = f
            if (f.success) return f
        }
        return last
    }
    fun area(f: Factory) = f.layout.w * f.layout.h
    fun consider(f: Factory?, what: String): Boolean {
        if (f == null || !f.success) return false
        val b = best
        if (b != null && area(f) >= area(b)) { say("$what: ${f.layout.w} x ${f.layout.h} is not smaller than ${b.layout.w} x ${b.layout.h}"); return false }
        best = f
        writeFactory(f, out, config)
        say("$what: ${f.layout.w} x ${f.layout.h} (${area(f)} tiles), saved")
        return true
    }
    val first = stitchAny(split, bps, results, "factory")
    if (first == null || !first.success) { first?.let { writeFactory(it, out, config) }; return first }
    consider(first, "factory")

    // 3. Repack: every block again at a common height per row: the height of the row's tallest block (no empty
    //    space above the others), and heights that make the factory about square. Each row repacked or not, the
    //    smallest valid factory wins.
    if (cfg.repack && left() > 60) {
        val areas = results.map { it.w * it.h }
        val corridor = cfg.corridor
        val width = ceil(sqrt(areas.sum() * 1.15)).toInt()
        val rows = (0 until split.rows).map { r -> split.blocks.indices.filter { split.slots[it].row == r } }
        val tallest = rows.map { bs -> bs.maxOfOrNull { results[it].h } ?: 0 }
        val square = rows.map { bs -> maxOf(12, ceil(bs.sumOf { areas[it] } * 1.1 / maxOf(1, width - corridor * (bs.size - 1))).toInt()) }
        val plans = listOf("tallest" to tallest, "square" to square).distinctBy { it.second }
        val budget = config.search.timeBudgetS * cfg.repackShare / plans.size
        for ((name, heights) in plans) {
            if (left() < 60) break
            say("repack ($name): rows at height $heights")
            val fresh = ArrayList(results)
            for (b in split.blocks.indices) {
                if (left() < 30) break
                val hgt = heights[split.slots[b].row]
                if (hgt == results[b].h && name == "tallest") continue   // already that tall
                val start = ceil(sqrt(ceil(areas[b] * 1.2 / hgt) * hgt)).toInt()
                val r = runCatching {
                    Search(data, blockConfig(minOf(left(), budget * bps[b].globalIds.size / total), fixedHeight = hgt, startSize = start),
                        tuning, bps[b].plan, File(out, "repack_$name/block_${b + 1}")).run().best
                }.getOrElse { e -> say("repack block ${b + 1}: failed with $e"); null }
                if (r != null) { fresh[b] = r; say("repack block ${b + 1}: ${r.w} x ${r.h} (was ${results[b].w} x ${results[b].h})") }
                else say("repack block ${b + 1}: nothing at height $hgt, keeping ${results[b].w} x ${results[b].h}")
            }
            val base = ArrayList(results)
            for (mask in 1 until (1 shl split.rows)) {
                val mix = ArrayList(results.indices.map { b -> if ((mask shr split.slots[b].row) and 1 == 1) fresh[b] else base[b] })
                if (consider(stitchAny(split, bps, mix, "repacked ($name) rows ${(0 until split.rows).filter { (mask shr it) and 1 == 1 }}"), "repacked factory")) results = mix
            }
        }
    }

    // 4. Merge neighbours: two blocks of a row become one, searched from their placements side by side.
    if (cfg.merge) {
        val maxM = cfg.mergeMaxMachines ?: (2 * cfg.maxMachines)
        val tried = HashSet<Set<Int>>()
        while (left() > 120) {
            val pairs = split.blocks.indices.flatMap { a ->
                split.blocks.indices.filter { b -> split.slots[b].row == split.slots[a].row && split.slots[b].col == split.slots[a].col + 1 }.map { a to it }
            }.filter { (a, b) -> split.blocks[a].size + split.blocks[b].size <= maxM && (split.blocks[a] + split.blocks[b]).toSet() !in tried }
            if (pairs.isEmpty()) { say("merge: no more pairs to try"); break }
            val (l, r) = pairs.minBy { (a, b) -> split.blocks[a].size + split.blocks[b].size }
            tried += (split.blocks[l] + split.blocks[r]).toSet()
            val merged = mergeBlocks(split, l, r)
            val m = l - (if (r < l) 1 else 0)
            val bp = blockPlan(plan, merged, m, tuning).let { BlockPlan(m, it.plan, it.globalIds, it.linkOfPort) }
            // The two placements side by side, the right one a corridor further on, aligned as in their row.
            val hgt = maxOf(results[l].h, results[r].h)
            val bottom = split.rows == 2 && split.slots[l].row == 1
            val wide = results[l].w + cfg.corridor + results[r].w
            val local = bp.globalIds.withIndex().associate { (i, g) -> g to i }
            val warm = ArrayList<PlacedMachine>()
            for ((b, dx) in listOf(l to 0, r to results[l].w + cfg.corridor)) {
                val dy = if (bottom) hgt - results[b].h else 0
                results[b].placement!!.machines.forEach { pm -> warm += pm.copy(id = local.getValue(bps[b].globalIds[pm.id]), x = pm.x + dx, y = pm.y + dy) }
            }
            val budget = (left() / pairs.size).coerceIn(600.0, 3600.0).coerceAtMost(left() - 60)
            say("merge blocks ${l + 1} and ${r + 1} (${bp.globalIds.size} machines) from ${wide} x $hgt, ${"%.0f".format(budget)} s")
            val dir = File(out, "merge/${tried.size}_blocks_${l + 1}_${r + 1}")
            bp.plan.write(File(dir, "plan.json").path)
            val res = runCatching {
                Search(data, blockConfig(budget, fixedHeight = hgt, startSize = ceil(sqrt(wide.toDouble() * hgt)).toInt()), tuning, bp.plan, dir,
                    startFrom = Placement(wide, hgt, warm.sortedBy { it.id })).run().best
            }.getOrElse { e -> say("merge blocks ${l + 1} and ${r + 1}: failed with $e"); null }
            if (res == null) { say("merge blocks ${l + 1} and ${r + 1}: no layout"); continue }
            say("merge blocks ${l + 1} and ${r + 1}: ${res.w} x ${res.h} instead of $wide x $hgt")
            val gone = if (split.slots[l].col < split.slots[r].col) r else l
            val newResults = ArrayList(results.filterIndexed { i, _ -> i != gone }).also { it[m] = res }
            val newBps = merged.blocks.indices.map { i -> if (i == m) bp else blockPlan(plan, merged, i, tuning) }
            if (consider(stitchAny(merged, newBps, newResults, "merged ${l + 1}+${r + 1}"), "merged factory")) {
                split = merged; bps = newBps; results = newResults
            }
        }
    }
    say("done: ${best?.let { "${it.layout.w} x ${it.layout.h} (${area(it)} tiles)" } ?: "no valid factory"}")
    return best
}

/** Places the finished blocks, routes the belts between them, adds poles (and the test harness) and validates. */
fun stitch(
    data: RootData, config: RunConfiguration, tuning: Tuning, plan: ProductionPlan, split: Split,
    bps: List<BlockPlan>, results: List<TrialResult>, corridor: Int, rowCorridor: Int = corridor,
): Factory? {
    val k = bps.size
    // Geometry: every row left to right in slot order with a corridor between blocks, row 0 against the top
    // edge, row 1 against the bottom, and [rowCorridor] along the rows (below a single row, between two).
    val rowH = IntArray(split.rows)
    split.slots.forEachIndexed { b, s -> rowH[s.row] = maxOf(rowH[s.row], results[b].h) }
    val ox = IntArray(k)
    var widest = 0
    for (r in 0 until split.rows) {
        var x = 0
        split.blocks.indices.filter { split.slots[it].row == r }.sortedBy { split.slots[it].col }.forEach { b -> ox[b] = x; x += results[b].w + corridor }
        widest = maxOf(widest, x - corridor)
    }
    // Reserved areas: columns of their own east of the blocks, against the top edge.
    var rx = widest + corridor
    val reservedRects = config.reserved.map { r -> geom.Rect(rx, 0, r.w, r.h).also { rx += r.w + corridor } }
    val W = reservedRects.lastOrNull()?.let { it.x2 + 1 } ?: widest
    val H = maxOf(rowH[0] + rowCorridor + (if (split.rows == 2) rowH[1] else 0), reservedRects.maxOfOrNull { it.h } ?: 0)
    val oy = IntArray(k) { if (split.slots[it].row == 0) 0 else H - results[it].h }
    fun inBlock(b: Int, t: Tile) = t.x >= ox[b] && t.x < ox[b] + results[b].w && t.y >= oy[b] && t.y < oy[b] + results[b].h

    val entities = ArrayList<PlacedEntity>()
    val machines = ArrayList<PlacedMachine>()
    // Entities carry the full plan's net of their item (blocks number their nets on their own), so a corridor
    // belt handing over to a block's belt counts as the same net.
    val netOfItem = plan.nets.filter { !it.fluid }.associate { it.item to it.id } + plan.nets.filter { it.fluid }.associate { it.item to it.id }
    fun global(e: PlacedEntity, local: ProductionPlan) = e.net?.let { n -> local.nets.getOrNull(n)?.item?.let { netOfItem[it] } }
    for (b in 0 until k) {
        results[b].layout!!.entities.filter { it.kind != Kind.POLE }.forEach { entities += it.copy(x = it.x + ox[b], y = it.y + oy[b], net = global(it, bps[b].plan)) }
        results[b].placement!!.machines.forEach { pm -> machines += pm.copy(id = bps[b].globalIds[pm.id], x = pm.x + ox[b], y = pm.y + oy[b]) }
    }
    val byTile = HashMap<Tile, PlacedEntity>().also { m -> entities.forEach { e -> e.rect.tiles().forEach { m[it] = e } } }

    // Link ends: an output belt of one block, an input belt of another; edge links are the factory's ports, either
    // where they leave their block or, routed through the corridors, on the factory edge the job asks for.
    val starts = HashMap<Int, Pair<Tile, Dir>>()   // link -> first tile outside the exporting block, the way its belt points
    val ends = HashMap<Int, Pair<Tile, Dir>>()     // link -> last tile outside the importing block, the way into it
    val sources = ArrayList<SourceSpec>()
    val outputs = ArrayList<OutputSpec>()
    for (b in 0 until k) {
        results[b].outputs.forEach { o ->
            val l = split.links[bps[b].linkOfPort[o.port] ?: return@forEach]
            val t = Tile(o.tile.x + ox[b], o.tile.y + oy[b])
            if (!l.throughCorridor) { outputs += o.copy(tile = t, port = -1); return@forEach }
            val d = if (o.fluid) sideOf(o.tile, results[b].w, results[b].h) else byTile[t]?.dir ?: return null
            starts[l.id] = (t + d) to d
        }
        results[b].sources.forEach { s ->
            val l = split.links[bps[b].linkOfPort[s.port] ?: return@forEach]
            val t = Tile(s.tile.x + ox[b], s.tile.y + oy[b])
            if (!l.throughCorridor) { sources += s.copy(tile = t, port = -1); return@forEach }
            // The side the port is on, as the block was searched (after a merge a neighbour may sit elsewhere).
            val out = sideOf(s.tile, results[b].w, results[b].h)
            ends[l.id] = (t + out) to out.opposite
        }
    }
    val corridorLinks = split.links.filter { it.throughCorridor }
    if (corridorLinks.any { (it.from >= 0 && it.id !in starts) || (it.to >= 0 && it.id !in ends) }) return null

    // The belts (pipes) through the corridors: one net per link, from a fixed start tile (or the factory edge the
    // job asks for) to a fixed end tile (or that edge).
    val ports = ArrayList<Port>(); val terminals = ArrayList<Terminal>(); val nets = ArrayList<Net>()
    val edgePorts = HashSet<Int>()
    fun edgeSpecs(item: String, output: Boolean): List<ConfigSource> {
        val specs = plan.ports.first { it.item == item && it.output == output }.specs
        if (!config.testMode) return specs
        // Test mode: every fifth tile of the edge only, so each port's harness (a feeder with inserters and chests
        // on both sides) has room next to its neighbours.
        return place.portCandidates(specs, W, H).filter { (t, d) -> (if (d == Dir.N || d == Dir.S) t.x else t.y) % 5 == 2 }
            .map { (t, d) -> ConfigSource(item, x = t.x, y = t.y, dir = d?.name) }
    }
    corridorLinks.forEach { l ->
        val p1 = if (l.from < 0) Port(ports.size, l.item, edgeSpecs(l.item, false), fluid = l.fluid, single = true).also { edgePorts += it.id }
            else starts.getValue(l.id).let { (s, sd) -> Port(ports.size, l.item, listOf(ConfigSource(l.item, x = s.x, y = s.y, dir = sd.opposite.name)), fluid = l.fluid, single = true) }
        ports += p1
        val p2 = if (l.to < 0) Port(ports.size, l.item, edgeSpecs(l.item, true), fluid = l.fluid, output = true, single = true).also { edgePorts += it.id }
            else ends.getValue(l.id).let { (e, ed) -> Port(ports.size, l.item, listOf(ConfigSource(l.item, x = e.x, y = e.y, dir = ed.name)), fluid = l.fluid, output = true, single = true) }
        ports += p2
        val t1 = Terminal(terminals.size, Role.SOURCE, l.item, l.rate, 1, port = p1.id, fluid = l.fluid).also { terminals += it }
        val t2 = Terminal(terminals.size, Role.OUTPUT, l.item, l.rate, 1, port = p2.id, fluid = l.fluid).also { terminals += it }
        nets += Net(nets.size, l.item, l.rate, listOf(t1.id, t2.id), 1, fluid = l.fluid)
    }
    val stitchPlan = ProductionPlan(emptyList(), ports, terminals, nets, emptyMap(), emptyMap(), emptyMap(), plan.beltTier, emptyList())
    val blockTiles = HashSet<Tile>()
    for (b in 0 until k) for (y in oy[b] until oy[b] + results[b].h) for (x in ox[b] until ox[b] + results[b].w) blockTiles += Tile(x, y)
    val reservedTiles = reservedRects.flatMap { it.tiles() }.toSet()
    blockTiles += reservedTiles
    // Tiles a block belt points into from inside (chain ends at the block edge) must not get a corridor belt.
    val startTiles = starts.values.map { it.first }.toSet()
    val pointedAt = entities.filter { it.kind.beltFamily }.mapNotNull { it.outputTile }.filter { t -> t !in blockTiles && t !in startTiles }
    val stitchConfig = config.copy(blocks = null, laneSharing = false)
    // Everything the blocks put outside their boxes (ring ports) is in the way too.
    val entityTiles = entities.flatMap { it.rect.tiles() }
    // A corridor pipe beside a block's pipe would join it: keep those tiles free, except where a pipe link ends.
    val besidePipes = if (corridorLinks.none { it.fluid }) emptyList() else {
        val linkTiles = (starts.values + ends.values).map { it.first }.toSet()
        entities.filter { it.kind == Kind.PIPE || it.kind == Kind.PIPE_UG }.flatMap { it.rect.tiles() }
            .flatMap { t -> Dir.entries.map { t + it } }.filter { it !in blockTiles && it !in linkTiles }
    }
    val routed = route(RouteProblem(stitchPlan, stitchConfig, tuning.route, Placement(W, H, emptyList()),
        obstacles = blockTiles + entityTiles + pointedAt + besidePipes, noTunnel = blockTiles))
    if (!routed.success) return null
    entities += routed.entities.map { it.copy(net = global(it, stitchPlan)) }
    sources += routed.sources.filter { it.port in edgePorts }.map { it.copy(port = -1) }
    outputs += routed.outputs.filter { it.port in edgePorts }.map { it.copy(port = -1) }

    val blockRects = (0 until k).map { b -> geom.Rect(ox[b], oy[b], results[b].w, results[b].h) }

    // Test harness on the factory's ports, then poles over everything, block borders, and validation.
    fun finish(
        ents: List<PlacedEntity>, mach: List<PlacedMachine>, srcs: List<SourceSpec>, outs: List<OutputSpec>,
        w: Int, h: Int, rects: List<geom.Rect>, reserved: List<geom.Rect>, notes: MutableList<String>,
    ): Factory {
        val harness = if (config.testMode) buildHarness(srcs, ents, w, h, config, outs) else null
        val all = ArrayList(ents).also { it += harness?.entities ?: emptyList() }
        harness?.warnings?.let { notes += it }
        var poles = emptyList<PlacedEntity>()
        var wires = emptyList<Pair<Int, Int>>()
        val keepFree = reserved.flatMap { it.tiles() }.toSet()
        if (config.power.mode != "none") {
            val extra = (harness?.tiles ?: emptySet()).flatMap { t -> (-2..2).flatMap { dy -> (-2..2).map { dx -> Tile(t.x + dx, t.y + dy) } } }
            val ps = placePoles(all, poleCandidates(w, h, config.power.allowOutsideRing, extra).filter { it !in keepFree }, config.power.pole)
            if (!ps.ok) notes += "power: ${ps.uncovered.size} entities not covered"
            poles = ps.poles; wires = ps.wires
        }
        val offset = all.size
        // Block borders as floor tiles, so each block shows in the game.
        val marks = if (config.markerTile == "none") emptyList() else (rects + reserved).flatMap { geom.borderTiles(it, config.markerTile) }
        val layout = Layout(w, h, all + poles, wires.map { (a, b) -> a + offset to b + offset }, "factorio-optimizer factory ${w}x$h", marks)
        val outside = (harness?.tiles ?: emptySet()) + poles.flatMap { it.rect.tiles() }.filter { it.x !in 0 until w || it.y !in 0 until h } +
            srcs.map { it.tile }   // ring ports (in test mode the harness feeds them instead of listing them)
        val spec = validationSpec(data, plan, config, Placement(w, h, mach), if (config.testMode) emptyList() else srcs,
            requirePower = config.power.mode != "none", outsideAllowed = outside.toSet(), outputs = outs)
        val errors = validate(layout, spec)
        return Factory(layout, errors, score(layout, config), srcs, outs, notes)
    }

    // Rows and columns that only straight belts and pipes cross go first; if that breaks anything (it should
    // not), the factory as stitched.
    if (config.blocks?.compact != false) {
        val c = runCatching { compactLayout(entities, emptyList(), W, H) }.getOrNull()
        if (c != null && c.rowsRemoved + c.colsRemoved > 0) {
            fun rect(r: geom.Rect) = geom.Rect(c.x(r.x), c.y(r.y), c.x(r.x2 + 1) - c.x(r.x), c.y(r.y2 + 1) - c.y(r.y))
            val notes = mutableListOf("compacted: ${c.rowsRemoved} rows and ${c.colsRemoved} columns removed from ${W} x $H")
            val f = finish(c.entities, machines.map { it.copy(x = c.x(it.x), y = c.y(it.y)) }, sources.map { it.copy(tile = c.tile(it.tile)) },
                outputs.map { it.copy(tile = c.tile(it.tile)) }, c.w, c.h, blockRects.map(::rect), reservedRects.map(::rect), notes)
            if (f.success) return f
            println("compaction broke the factory (${f.errors.take(3)}); keeping it as stitched")
        }
    }
    return finish(entities, machines, sources, outputs, W, H, blockRects, reservedRects, mutableListOf())
}

/** The box side a port tile of a [w] x [h] block is on (on the ring around it, or on its edge). */
private fun sideOf(t: Tile, w: Int, h: Int): Dir = when {
    t.x < 0 -> Dir.W; t.x >= w -> Dir.E; t.y < 0 -> Dir.N; t.y >= h -> Dir.S
    t.x == 0 -> Dir.W; t.x == w - 1 -> Dir.E; t.y == 0 -> Dir.N; else -> Dir.S
}

private fun writeFactory(f: Factory, out: File, config: RunConfiguration) {
    val l = f.layout
    File(out, "best_output_string.txt").writeText(l.toBlueprintString(config.quality))
    File(out, "best_blueprint.json").writeText(l.toBlueprint(config.quality).toJson())
    renderLayout(l, File(out, "best.png"), RenderOptions(errors = f.errors, ports = viz.portLabels(f.sources, f.outputs), title = "factory ${l.w}x${l.h} score ${"%.1f".format(f.score)}"))
    File(out, "ports.json").writeText(strictJson.encodeToString(JsonObject.serializer(), factoryManifest(f)))
}

/** Where the factory's inputs and outputs are (box-relative tiles, edge, item, rate, lane for shared belts). */
fun factoryManifest(f: Factory): JsonObject {
    val w = f.layout.w; val h = f.layout.h
    fun edge(x: Int, y: Int) = when { y <= 0 -> "north"; y >= h - 1 -> "south"; x <= 0 -> "west"; x >= w - 1 -> "east"; else -> "inside" }
    return buildJsonObject {
        putJsonObject("box") { put("w", w); put("h", h) }
        putJsonArray("inputs") {
            f.sources.forEach { s ->
                addJsonObject {
                    put("item", s.item); put("rate", s.rate); put("fluid", s.fluid); put("x", s.tile.x); put("y", s.tile.y); put("edge", edge(s.tile.x, s.tile.y))
                    if (s.lane >= 0) put("lane", if (s.lane == 0) "left" else "right")
                }
            }
        }
        putJsonArray("outputs") {
            f.outputs.forEach { o -> addJsonObject { put("item", o.item); put("rate", o.rate); put("fluid", o.fluid); put("x", o.tile.x); put("y", o.tile.y); put("edge", edge(o.tile.x, o.tile.y)) } }
        }
    }
}
