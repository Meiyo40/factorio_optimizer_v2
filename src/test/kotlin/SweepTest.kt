import config.RunConfiguration
import config.Tuning
import plan.ProductionPlan
import plan.buildPlan
import search.runTrial
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlin.test.Test

/**
 * Parameter sweep, skipped unless SWEEP=<spec file> is set. Each spec line (tab-separated):
 * `label  tuning-file-or-dash  job-file  size  firstSeed-lastSeed`. Trials run in parallel (SWEEP_THREADS,
 * default 8), interleaved across lines, and every result is appended to SWEEP_OUT (default build/sweep.tsv) as
 * soon as it finishes: label, job, size, seed, stage, ms, overuse, failed nets, score, and the closest routing run's
 * conflicts and unrouted chains.
 */
class SweepTest {
    private class Task(val label: String, val tuning: String, val job: String, val size: Int, val seed: Long)

    @Test
    fun sweep() {
        val spec = System.getenv("SWEEP") ?: return
        val out = File(System.getenv("SWEEP_OUT") ?: "build/sweep.tsv")
        val threads = System.getenv("SWEEP_THREADS")?.toIntOrNull() ?: 8
        val lines = File(spec).readLines().filter { it.isNotBlank() && !it.startsWith("#") }.map { it.split("\t") }
        val perLine = lines.map { p ->
            val (a, b) = p[4].split("-").map { it.trim().toLong() }
            (a..b).map { Task(p[0], p[1], p[2], p[3].trim().toInt(), it) }
        }
        // Round-robin over lines, so partial results cover every variant.
        val tasks = (0 until (perLine.maxOfOrNull { it.size } ?: 0)).flatMap { i -> perLine.mapNotNull { it.getOrNull(i) } }
        val tunings = ConcurrentHashMap<String, Tuning>()
        val plans = ConcurrentHashMap<String, Pair<RunConfiguration, ProductionPlan>>()
        fun tuningOf(t: String) = tunings.getOrPut(t) { if (t == "-") Tuning.defaults() else Tuning.load(t) }
        val lock = Any()
        val pool = Executors.newFixedThreadPool(threads)
        tasks.forEach { t ->
            pool.submit {
                val line = try {
                    val (config, plan) = plans.getOrPut(t.job + "|" + t.tuning) {
                        val c = RunConfiguration.load(t.job)
                        c to buildPlan(TestData.data, c, tuningOf(t.tuning).withOverrides(c.tuning))
                    }
                    val tuning = tuningOf(t.tuning).withOverrides(config.tuning)
                    val r = runTrial(TestData.data, plan, config, tuning, t.size, t.size, t.seed)
                    listOf(t.label, t.job.substringAfterLast('/'), t.size, t.seed, r.stage, r.millis, r.route?.overuse ?: -1,
                        r.route?.failedNets?.size ?: -1, if (r.success) "%.1f".format(r.score) else "-",
                        r.summary?.overuse ?: -1, r.summary?.failedNets?.size ?: -1).joinToString("\t")
                } catch (e: Throwable) {
                    listOf(t.label, t.job.substringAfterLast('/'), t.size, t.seed, "error", 0, -1, -1, e.toString().take(200)).joinToString("\t")
                }
                synchronized(lock) { out.appendText(line + "\n") }
            }
        }
        pool.shutdown()
        pool.awaitTermination(24, TimeUnit.HOURS)
    }
}
