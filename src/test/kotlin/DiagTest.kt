import config.RunConfiguration
import config.Tuning
import config.strictJson
import plan.ProductionPlan
import plan.buildPlan
import search.runTrial
import viz.RenderOptions
import viz.renderLayout
import java.io.File
import kotlin.test.Test

/**
 * Diagnostic, skipped unless DIAG=<job path>,<size>,<seeds> is set (DIAG_TUNING=<file> overrides tuning.json,
 * DIAG_FIRST=<seed> picks the first seed, DIAG_AT=<x>,<y>,<radius> lists the entities around a tile):
 * runs single trials and prints why routing fails. Use `./gradlew test --tests DiagTest --rerun -i`.
 */
class DiagTest {
    @Test
    fun diag() {
        val spec = System.getenv("DIAG") ?: return
        val (path, size, seeds) = spec.split(",")
        val config = RunConfiguration.load(path)
        val tuning = (System.getenv("DIAG_TUNING")?.takeIf { it.isNotBlank() }?.let { Tuning.load(it) } ?: Tuning.defaults()).withOverrides(config.tuning)
        // DIAG_PLAN=<plan.json>: a saved plan (e.g. one block of a factory) instead of planning the job.
        val plan = System.getenv("DIAG_PLAN")?.let { strictJson.decodeFromString(ProductionPlan.serializer(), File(it).readText()) }
            ?: buildPlan(TestData.data, config, tuning)
        println("DIAG machines ${plan.machines.size} nets ${plan.nets.size} terminals ${plan.terminals.size}")
        plan.machines.groupBy { it.recipe }.forEach { (r, ms) -> println("DIAG   $r x${ms.size}") }
        // DIAG_ITEM=<item>: the plan's terminals and nets of one item.
        System.getenv("DIAG_ITEM")?.let { item ->
            plan.terminals.filter { it.item == item }.forEach { println("DIAG   terminal $it ${it.machine?.let { m -> plan.machines[m].recipe } ?: ""}") }
            plan.nets.filter { it.item == item }.forEach { println("DIAG   net $it") }
        }
        val first = System.getenv("DIAG_FIRST")?.toLongOrNull() ?: 1L
        for (seed in first until first + seeds.toLong()) {
            val s = size.toInt()
            val r = runTrial(TestData.data, plan, config, tuning, s, s, seed)
            println("DIAG seed $seed: ${r.stage} ${r.millis}ms score ${r.score}")
            r.notes.forEach { println("DIAG   note $it") }
            r.errors.take(8).forEach { println("DIAG   error $it") }
            System.getenv("DIAG_NET")?.toIntOrNull()?.let { net -> r.layout?.entities?.filter { it.net == net }?.forEach { en ->
                println("DIAG   net$net: ${en.name} ${en.kind} (${en.x},${en.y}) ${en.dir}") } }
            // DIAG_AT=<x>,<y>,<radius>: every entity near a tile.
            System.getenv("DIAG_AT")?.split(",")?.map { it.trim().toInt() }?.let { (ax, ay, rad) -> r.layout?.entities?.filter { en ->
                en.rect.tiles().any { t -> kotlin.math.abs(t.x - ax) <= rad && kotlin.math.abs(t.y - ay) <= rad } }?.forEach { en ->
                println("DIAG   at: ${en.name} ${en.kind} (${en.x},${en.y}) ${en.dir} ${en.recipe ?: ""} ${en.item ?: ""} net ${en.net}") } }
            if (System.getenv("DIAG_DUMP") != null) r.layout?.let { l ->
                r.errors.filter { it.type == validate.ErrorType.UNREACHABLE || it.type.name.startsWith("FLUID") }.take(2).forEach { e ->
                    l.entities.filter { en -> en.rect.tiles().any { t -> kotlin.math.abs(t.x - e.tile.x) <= 3 && kotlin.math.abs(t.y - e.tile.y) <= 3 } }
                        .sortedWith(compareBy({ it.y }, { it.x }))
                        .forEach { en -> println("DIAG   near ${e.tile}: ${en.name} ${en.kind} (${en.x},${en.y}) ${en.dir} net ${en.net} ${en.item ?: ""}") }
                }
            }
            r.route?.let { rt ->
                println("DIAG   failed nets ${rt.failedNets.map { plan.nets[it].item }} overuse ${rt.overuse} iters ${rt.iterations} chains ${rt.chains}")
                rt.log.takeLast(8).forEach { println("DIAG   log $it") }
                rt.log.filter { it.startsWith("shared belt") }.forEach { println("DIAG   $it") }
                // With trace_detail: the iteration closest to a solution, and what its conflicts were.
                val iters = rt.log.filter { it.startsWith("iter ") }
                fun total(l: String) = Regex("tile (\\d+).* dead (\\d+) chan (\\d+) unrouted (\\d+)").find(l)
                    ?.groupValues?.drop(1)?.map { it.toInt() }?.let { (t, d, c, u) -> t + d + c + 10 * u } ?: Int.MAX_VALUE
                iters.minByOrNull(::total)?.let { println("DIAG   best $it") }
                iters.lastOrNull()?.let { println("DIAG   last $it") }
                rt.log.filter { it.startsWith("fluid") || it.startsWith("net ") }.map { it.replace(Regex("\\d+ backtracks.*"), "") }
                    .groupingBy { it }.eachCount().forEach { (k, v) -> println("DIAG   count $v x $k") }
            }
            val px = System.getenv("DIAG_PX")?.toIntOrNull() ?: 16
            r.layout?.let { renderLayout(it, File("build/renders/diag/${s}_$seed.png"), RenderOptions(errors = r.errors, px = px, title = "${r.stage} seed $seed")) }
            if (r.layout == null) r.route?.let { rt ->
                val over = rt.log.mapNotNull { Regex("overused \\((-?\\d+),(-?\\d+)\\): (.*)").find(it) }
                    .map { m -> validate.ValidationError(validate.ErrorType.OVERLAP, geom.Tile(m.groupValues[1].toInt(), m.groupValues[2].toInt()), m.groupValues[3].take(60)) }
                renderLayout(geom.Layout(s, s, rt.entities), File("build/renders/diag/${s}_${seed}_fail.png"), RenderOptions(errors = over, px = 24, title = "route seed $seed: ${rt.failedNets.map { plan.nets[it].item }}"))
            }
        }
    }
}
