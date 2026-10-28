import config.RunConfiguration
import config.Tuning
import search.runTrial
import kotlin.test.Test

/**
 * Runs real trials (placement, routing with feedback, power, validation) so a profiler can say where a trial spends
 * its time. Not part of the Kotlin project: the Rust tooling copies it into the container's work tree. Skipped
 * unless TIMESPLIT is set; rust/tools/timesplit.sh attaches Java Flight Recorder and aggregates the samples.
 */
class TimeSplitTest {
    @Test
    fun timeSplit() {
        if (System.getenv("TIMESPLIT") == null) return
        val seeds = (System.getenv("TIMESPLIT_SEEDS")?.toInt() ?: 3).let { 1L..it.toLong() }
        // Box sizes: comfortable by default; the best known are 36 and 41, where trials fail and routing works hardest.
        val chem = System.getenv("TIMESPLIT_CHEM")?.toInt() ?: 40
        val em = System.getenv("TIMESPLIT_EM")?.toInt() ?: 46
        val cases = listOf("docs/examples/chemical_science.json" to chem, "docs/examples/electromagnetic_science.json" to em)
        for ((job, size) in cases) {
            val config = RunConfiguration.load(job)
            val tuning = Tuning.defaults().withOverrides(config.tuning)
            val plan = plan.buildPlan(TestData.data, config, tuning)
            for (seed in seeds) {
                val r = runTrial(TestData.data, plan, config, tuning, size, size, seed)
                println("timesplit $job ${size}x$size seed $seed: stage ${r.stage}, ${r.millis} ms")
            }
        }
    }
}
