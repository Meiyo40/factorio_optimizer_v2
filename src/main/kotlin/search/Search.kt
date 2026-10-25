package search

import config.RunConfiguration
import config.Tuning
import data.RootData
import emit.toBlueprint
import emit.toBlueprintString
import emit.toJson
import geom.Kind
import plan.ProductionPlan
import viz.RenderOptions
import viz.generateScoreGraph
import viz.renderLayout
import viz.timestampToScore
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.add
import kotlinx.serialization.json.addJsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject
import java.io.File
import java.util.concurrent.Callable
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlin.math.ceil
import kotlin.math.roundToInt
import kotlin.math.sqrt

data class Improvement(val elapsedMs: Long, val w: Int, val h: Int, val score: Double, val belts: Int, val undergrounds: Int, val inserters: Int, val seed: Long)

class SearchResult(val best: TrialResult?, val improvements: List<Improvement>, val trials: Int, val log: List<String>)

/**
 * Stage 5: binary search on the box size, K seeds per size in parallel, warm starts from the best
 * placement so far, and anytime outputs on every improvement. A longer run never gives a worse result.
 */
class Search(
    private val data: RootData,
    private val config: RunConfiguration,
    private val tuning: Tuning,
    private val plan: ProductionPlan,
    private val out: File,
    private val clock: () -> Long = System::currentTimeMillis,
    /** A placement to start from until the search has a success of its own (it is refined, not re-annealed). */
    private val startFrom: place.Placement? = null,
    /** Called for every finished trial (calibration collects routing statistics here). */
    private val onTrial: (TrialResult) -> Unit = {},
) {
    private val start = clock()
    private val deadline = start + (config.search.timeBudgetS * 1000).toLong()
    private val log = ArrayList<String>()
    private val improvements = ArrayList<Improvement>()
    @Volatile private var best: TrialResult? = null
    private var trials = 0
    private val scores = ArrayList<timestampToScore>()
    /** Seeds used so far per size, so every retry at a size gets fresh ones. */
    private val seedsUsed = HashMap<Int, Int>()

    /** Every box tried so far, in order: tries, successes, best score and the closest failure. */
    private class SizeStats(val w: Int, val h: Int) {
        var tries = 0; var ok = 0; var bestScore = Double.MAX_VALUE
        var closestWeight = Int.MAX_VALUE; var closest = ""
    }
    private val history = LinkedHashMap<Pair<Int, Int>, SizeStats>()

    private fun item(net: Int) = plan.nets.getOrNull(net)?.item ?: "net $net"

    /** "copper-plate x6 near (0,5)": conflict tiles grouped by the items that meet there, largest groups first. */
    private fun hotspots(s: AttemptSummary): String =
        s.hotspots.entries.groupBy { (_, nets) -> nets.map(::item).distinct().sorted().joinToString("/").ifEmpty { "?" } }
            .entries.sortedByDescending { it.value.size }.take(3)
            .joinToString(", ") { (items, tiles) -> "$items x${tiles.size} near (${tiles.first().key.x},${tiles.first().key.y})" }

    private fun weight(s: AttemptSummary) = s.overuse + tuning.route.unroutedPenalty * s.failedNets.size

    /** One trial in a few words: how it ended, and for a failure how close it came and where. */
    private fun describe(r: TrialResult): String {
        val secs = "%.1fs".format(r.millis / 1000.0)
        val s = r.summary
        val runs = s?.let { ", ${it.rounds} routing run${if (it.rounds == 1) "" else "s"}" } ?: ""
        return when (r.stage) {
            "ok" -> "ok in $secs$runs, score ${"%.1f".format(r.score)}"
            "route" -> "no route in $secs$runs" + (s?.let {
                "; closest: ${it.overuse} conflicts" +
                    (if (it.failedNets.isNotEmpty()) ", unrouted ${it.failedNets.map(::item).distinct()}" else "") +
                    (if (it.hotspots.isNotEmpty()) " (${hotspots(it)})" else "")
            } ?: "")
            "placement" -> "the machines do not fit"
            else -> "routed in $secs but ${r.stage} failed: ${r.errors.take(2).joinToString("; ") { "${it.type} at (${it.tile.x},${it.tile.y}) ${it.message}" }}"
        }
    }

    /** Records a finished trial: a progress line, attempts.tsv, and the box's history. */
    private fun report(r: TrialResult, attempt: Int) {
        if (config.search.verbose) say("  ${r.w}x${r.h} try $attempt: ${describe(r)}")
        val s = r.summary
        synchronized(lock) {
            val f = File(out, "attempts.tsv")
            if (!f.exists()) f.writeText("elapsed_s\tw\th\tseed\tstage\tseconds\trouting_runs\tconflicts\tunrouted\tscore\twhere\n")
            f.appendText(listOf("%.1f".format((clock() - start) / 1000.0), r.w, r.h, r.seed, r.stage, "%.1f".format(r.millis / 1000.0),
                s?.rounds ?: 0, s?.overuse ?: "", s?.failedNets?.map(::item)?.distinct()?.joinToString(",") ?: "",
                if (r.success) "%.1f".format(r.score) else "",
                if (r.success) "" else if (r.stage == "route") s?.let(::hotspots) ?: "" else r.errors.take(2).joinToString("; ") { "${it.type} (${it.tile.x},${it.tile.y})" },
            ).joinToString("\t") + "\n")
            val h = history.getOrPut(r.w to r.h) { SizeStats(r.w, r.h) }
            h.tries++
            if (r.success) { h.ok++; h.bestScore = minOf(h.bestScore, r.score) }
            else {
                val wgt = if (r.stage == "route" && s != null) weight(s) else Int.MAX_VALUE - 1
                if (wgt < h.closestWeight || h.closest.isEmpty()) { h.closestWeight = wgt; h.closest = describe(r).substringAfter("; closest: ", describe(r)) }
            }
        }
    }

    /** The boxes tried so far, smallest first: tries, successes, and the best result or closest miss. */
    private fun sayHistory() {
        val rows = synchronized(lock) { history.values.sortedBy { it.w * it.h } }
        say("history (box: tries, successes, best or closest):")
        rows.forEach { h ->
            say("  ${h.w}x${h.h}: ${h.tries} tries, ${h.ok} ok, " +
                if (h.ok > 0) "best score ${"%.1f".format(h.bestScore)}" else "closest ${h.closest}")
        }
    }

    /** The plan in a few lines: machines per recipe, and per item its rate, inserters, belts and port. */
    private fun sayPlan(w: Int, h: Int) {
        say("plan: ${plan.machines.size} machines: " + plan.machines.groupingBy { "${it.recipe} (${it.machine})" }.eachCount().entries
            .sortedByDescending { it.value }.joinToString(", ") { "${it.value}x ${it.key}" })
        say("  %-26s %8s %9s %5s  %s".format("item", "rate/s", "inserters", "belts", "port"))
        plan.nets.sortedByDescending { n -> plan.netTerminals(n).filter { it.machine != null }.sumOf { it.k } }.forEach { n ->
            val ts = plan.netTerminals(n)
            val visits = ts.filter { it.machine != null }.sumOf { it.k }
            val ports = ts.mapNotNull { t -> t.port?.let { plan.ports[it] } }
            val port = ports.joinToString("; ") { p ->
                val tiles = place.portCandidates(p.specs, w, h).size
                val where = p.specs.joinToString(",") { s ->
                    s.edge?.let { e -> e + if (s.from != null || s.to != null) " ${s.from ?: 0}..${s.to ?: "end"}" else "" } ?: "(${s.x},${s.y})"
                }
                (if (p.output) "out " else "in ") + "$where, $tiles tile${if (tiles == 1) "" else "s"}" +
                    if (!n.fluid && visits > tuning.route.maxChainVisits * tiles) " -> ${tiles} belt${if (tiles == 1) "" else "s"} with splitter branches (more than ${tuning.route.maxChainVisits} inserters per belt)" else ""
            }
            say("  %-26s %8.2f %9s %5s  %s".format(n.item + if (n.fluid) " (fluid)" else "", n.flow, if (n.fluid) "-" else visits.toString(),
                if (n.fluid) "-" else n.beltCount.toString(), port))
        }
    }
    private val lock = Any()

    /** Box for size parameter s: aspect ratio, or a fixed width (height) with the other side carrying the area. */
    fun box(s: Int): Pair<Int, Int> {
        config.box.fixedWidth?.let { w -> return w to maxOf(3, ceil(s.toDouble() * s / w).toInt()) }
        config.box.fixedHeight?.let { h -> return maxOf(3, ceil(s.toDouble() * s / h).toInt()) to h }
        val r = sqrt(config.box.aspect)
        return maxOf(3, (s * r).roundToInt()) to maxOf(3, (s / r).roundToInt())
    }

    private fun say(msg: String) {
        val line = "[%7.1fs] %s".format((clock() - start) / 1000.0, msg)
        println(line)
        synchronized(lock) { log += line; File(out, "log.txt").appendText(line + "\n") }
    }

    private fun better(t: TrialResult, than: TrialResult?): Boolean =
        than == null || t.area < than.area || (t.area == than.area && t.score < than.score - 1e-9)

    /** Records a successful trial if it improves on the best (area first, then score) and writes outputs. */
    private fun offer(t: TrialResult) {
        if (!t.success) return
        synchronized(lock) {
            if (!better(t, best)) return
            best = t
            val l = t.layout!!
            val imp = Improvement(clock() - start, t.w, t.h, t.score, l.count(Kind.BELT), l.count(Kind.UG_IN) + l.count(Kind.UG_OUT), l.count(Kind.INSERTER), t.seed)
            improvements += imp
            writeBest(t, improvements.size)
            say("improvement #${improvements.size}: ${t.w}x${t.h} score ${"%.1f".format(t.score)} (belts ${imp.belts}, undergrounds ${imp.undergrounds}, inserters ${imp.inserters}, seed ${t.seed})")
        }
    }

    private fun writeBest(t: TrialResult, n: Int) {
        val l = t.layout!!.copy(label = "factorio-optimizer ${t.w}x${t.h}")
        val bp = l.toBlueprintString(config.quality)
        out.mkdirs()
        File(out, "best_output_string.txt").writeText(bp)
        File(out, "best_blueprint.json").writeText(l.toBlueprint(config.quality).toJson())
        File(out, "ports.json").writeText(prettyJson.encodeToString(JsonObject.serializer(), portManifest(t)))
        File(out, "allSolves").mkdirs()
        File(out, "allSolves/%03d - %dx%d - %.1f.txt".format(n, t.w, t.h, t.score)).writeText(bp)
        renderLayout(l, File(out, "best.png"), RenderOptions(ports = viz.portLabels(t.sources, t.outputs), title = "${t.w}x${t.h} score ${"%.1f".format(t.score)} seed ${t.seed}"))
        renderLayout(l, File(out, "renders/improvement_%03d.png".format(n)), RenderOptions(title = "#$n ${t.w}x${t.h} score ${"%.1f".format(t.score)}"))
        val imp = improvements.last()
        File(out, "scores.tsv").appendText(
            (if (n == 1) "elapsed_s\tw\th\tscore\tbelts\tundergrounds\tinserters\tseed\n" else "") +
                "%.1f\t%d\t%d\t%.2f\t%d\t%d\t%d\t%d\n".format(imp.elapsedMs / 1000.0, imp.w, imp.h, imp.score, imp.belts, imp.undergrounds, imp.inserters, imp.seed))
        scores += timestampToScore(clock(), t.score)
        generateScoreGraph(scores, out.path + "/", yLabel = "Score (area + weighted entities)")
    }

    /** Ctrl-C: whatever is best so far is already on disk; rewrite it and note the interruption. */
    fun flush(reason: String) {
        synchronized(lock) {
            best?.let { writeBest(it, improvements.size.coerceAtLeast(1)) }
            File(out, "log.txt").appendText("stopped: $reason; best ${best?.let { "${it.w}x${it.h} score ${"%.1f".format(it.score)}" } ?: "none"}\n")
        }
    }

    private fun timeLeft() = clock() < deadline

    /** Runs K fresh seeds at size s in parallel; true if any succeeded. */
    private fun trySize(s: Int, seeds: Int): Boolean {
        val (w, h) = box(s)
        val offset = seedsUsed.getOrDefault(s, 0)
        seedsUsed[s] = offset + seeds
        val warm = best?.placement ?: startFrom
        val warmT0 = best?.annealT0
        val pool = Executors.newFixedThreadPool(config.search.threads.coerceAtLeast(1))
        try {
            val jobs = (0 until seeds).map { k ->
                val seed = config.search.seed * 1_000_003L + s * 1009L + (k + offset)
                Callable {
                    if (!timeLeft()) null
                    else runTrial(data, plan, config, tuning, w, h, seed, warm, warmT0, deadline,
                        debugDir = if (config.debugRenders && k == 0) File(out, "debug/route_${w}x${h}_$seed") else null)
                            .also { report(it, offset + k + 1) }
                }
            }
            val results = pool.invokeAll(jobs).mapNotNull { it.get() }
            trials += results.size
            results.forEach { offer(it); onTrial(it) }
            val ok = results.count { it.success }
            say("size $s (${w}x$h): $ok/${results.size} seeds succeeded" + results.filter { !it.success }.groupingBy { it.stage }.eachCount().let { if (it.isEmpty()) "" else ", failures $it" })
            sayHistory()
            if (config.debugRenders) results.filter { !it.success && it.layout != null }.forEach { r ->
                renderLayout(r.layout!!, File(out, "debug/fail_${w}x${h}_${r.seed}.png"), RenderOptions(errors = r.errors, title = "${r.stage} ${r.errors.take(3)}"))
            }
            return ok > 0
        } finally {
            pool.shutdown()
            pool.awaitTermination(1, TimeUnit.MINUTES)
        }
    }

    fun run(): SearchResult {
        out.mkdirs()
        val area = plan.totalMachineArea() + config.reserved.sumOf { it.w * it.h }
        val cap = config.box.maxSize
        val estimate = maxOf(3, ceil(sqrt(area * tuning.search.areaFactor)).toInt())
        var hi = minOf(cap ?: Int.MAX_VALUE, config.box.startSize ?: cap ?: (2 * estimate))
        val sLo = minOf(estimate, hi)
        val k = config.search.seedsPerSize
        say("machines ${plan.machines.size}, machine area $area, s_lo $sLo, start $hi${cap?.let { ", max $it" } ?: ""}, budget ${config.search.timeBudgetS}s, $k seeds x ${config.search.threads} threads")
        box(hi).let { (w, h) -> sayPlan(w, h) }

        // 1. Upper bound: grow gently until a size succeeds, with several seed rounds per size first. A failed
        //    size usually means routing dead ends other seeds avoid; doubling the box gives sparse layouts.
        var feasible = -1
        val grow = tuning.search.growRounds.coerceAtLeast(1)
        while (timeLeft() && feasible < 0) {
            for (round in 0 until grow) {
                if (!timeLeft()) break
                if (trySize(hi, k)) { feasible = hi; break }
            }
            if (feasible >= 0) break
            if (cap != null && hi >= cap) continue   // at the largest allowed size: keep trying new seeds
            if (cap == null && hi >= 8 * sLo) { say("no success up to size $hi"); break }
            hi = minOf(cap ?: Int.MAX_VALUE, maxOf(hi + 1, ceil(hi * tuning.search.growFactor).toInt()))
        }
        if (feasible < 0) return finish("no feasible size found")

        // 2. Binary search between s_lo and the smallest known success.
        var lo = sLo
        hi = feasible
        while (lo < hi && timeLeft()) {
            val mid = (lo + hi) / 2
            if (trySize(mid, k)) hi = mid else lo = mid + 1
        }
        say("search interval closed at size $hi")

        // 3. Extra seeds at the smallest feasible size and the one below, until the budget runs out.
        var round = 1
        while (timeLeft() && round <= tuning.search.extraRounds) {
            if (hi - 1 >= sLo && trySize(hi - 1, k)) hi -= 1
            else if (timeLeft()) trySize(hi, k)
            round++
        }
        return finish(if (timeLeft()) "extra seed rounds done" else "time budget spent")
    }

    private fun finish(reason: String): SearchResult {
        say("done: $reason after $trials trials; best ${best?.let { "${it.w}x${it.h} score ${"%.1f".format(it.score)}" } ?: "none"}")
        return SearchResult(best, improvements.toList(), trials, log.toList())
    }
}

private val prettyJson = Json { prettyPrint = true }

/**
 * Where the block's inputs and outputs are, for connecting it: box-relative tiles (the blueprint's own
 * coordinates; a tile's centre is x + 0.5), the box edge, the item and its rate, and for two items sharing an
 * input belt, the lane each arrives on.
 */
fun portManifest(t: TrialResult): JsonObject {
    val r = t.route
    fun edge(x: Int, y: Int) = when {
        x <= 0 -> "west"; x >= t.w - 1 -> "east"; y <= 0 -> "north"; y >= t.h - 1 -> "south"; else -> "inside"
    }
    return buildJsonObject {
        putJsonObject("box") { put("w", t.w); put("h", t.h) }
        putJsonArray("inputs") {
            (t.sources.ifEmpty { r?.sources ?: emptyList() }).forEach { s ->
                addJsonObject {
                    put("item", s.item); put("rate", s.rate); put("fluid", s.fluid); put("x", s.tile.x); put("y", s.tile.y); put("edge", edge(s.tile.x, s.tile.y))
                    // A shared belt: which lane this item arrives on, looking along the belt into the block.
                    if (s.lane >= 0) put("lane", if (s.lane == 0) "left" else "right")
                }
            }
        }
        putJsonArray("outputs") {
            (t.outputs.ifEmpty { r?.outputs ?: emptyList() }).forEach { o -> addJsonObject { put("item", o.item); put("rate", o.rate); put("fluid", o.fluid); put("x", o.tile.x); put("y", o.tile.y); put("edge", edge(o.tile.x, o.tile.y)) } }
        }
    }
}

/** Entry for Main: runs the search with a Ctrl-C hook that flushes the best result. */
fun runSearch(data: RootData, config: RunConfiguration, tuning: Tuning, plan: ProductionPlan, out: String): SearchResult {
    val search = Search(data, config, tuning, plan, File(out))
    val hook = Thread { search.flush("interrupted") }
    Runtime.getRuntime().addShutdownHook(hook)
    val result = search.run()
    runCatching { Runtime.getRuntime().removeShutdownHook(hook) }
    search.flush("finished")
    return result
}
