import config.BoxConfig
import config.SearchConfig
import config.Tuning
import emit.decodeBlueprint
import emit.toLayout
import plan.buildPlan
import search.Search
import search.validationSpec
import validate.validate
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

class SearchTest {
    private fun search(job: String, budget: Double, out: String): Pair<Search, search.SearchResult> {
        val config = TestData.job(job).copy(search = SearchConfig(timeBudgetS = budget, seedsPerSize = 2, threads = 2, seed = 7))
        val plan = buildPlan(TestData.data, config)
        File(out).deleteRecursively()
        val s = Search(TestData.data, config, Tuning(), plan, File(out))
        return s to s.run()
    }

    @Test
    fun `box size never grows across improvements and the best result is written`() {
        val out = "build/search/t3"
        val (_, res) = search("t3", 40.0, out)
        val best = assertNotNull(res.best, "no layout found: ${res.log}")
        val areas = res.improvements.map { it.w * it.h }
        assertTrue(areas.zipWithNext().all { (a, b) -> b <= a }, "areas $areas")
        assertTrue(res.improvements.zipWithNext().all { (a, b) -> b.w * b.h < a.w * a.h || b.score < a.score })
        println("T3 search: ${res.improvements.map { "${it.w}x${it.h} %.1f".format(it.score) }} in ${res.trials} trials")

        // The written blueprint decodes to a layout that still validates.
        val string = File(out, "best_output_string.txt").readText()
        val decoded = decodeBlueprint(string).toLayout(best.w, best.h)
        assertEquals(best.layout!!.entities.size, decoded.entities.size)
        val config = TestData.job("t3")
        val plan = buildPlan(TestData.data, config)
        val outside = decoded.entities.flatMap { it.rect.tiles() }.filter { it.x !in 0 until best.w || it.y !in 0 until best.h }.toSet()
        val spec = validationSpec(TestData.data, plan, config, best.placement!!, best.sources, requirePower = true, outsideAllowed = outside, outputs = best.outputs)
        assertEquals(emptyList(), validate(decoded, spec))
        assertTrue(File(out, "scores.tsv").readLines().size == res.improvements.size + 1)
        assertTrue(File(out, "best.png").exists())
    }

    @Test
    fun `flush rewrites the best result and notes the stop`() {
        val out = "build/search/t1"
        val (s, res) = search("t1", 8.0, out)
        assertNotNull(res.best)
        File(out, "best_output_string.txt").delete()
        s.flush("interrupted")
        assertTrue(File(out, "best_output_string.txt").exists())
        assertTrue(File(out, "log.txt").readText().contains("stopped: interrupted"))
    }

    @Test
    fun `the search starts at max_size and never tries a larger box`() {
        val out = "build/search/t2max"
        val config = TestData.job("t2").copy(box = BoxConfig(maxSize = 12),
            search = SearchConfig(timeBudgetS = 10.0, seedsPerSize = 2, threads = 2, seed = 7))
        File(out).deleteRecursively()
        val res = Search(TestData.data, config, Tuning(), buildPlan(TestData.data, config), File(out)).run()
        assertTrue(res.log.first().contains("start 12, max 12"), res.log.first())
        val sizes = res.log.mapNotNull { Regex("size (\\d+) \\(").find(it)?.groupValues?.get(1)?.toInt() }
        assertTrue(sizes.isNotEmpty() && sizes.all { it <= 12 }, "sizes tried: $sizes")
        assertNotNull(res.best, "T2 fits in 12 x 12: ${res.log}")
        assertTrue(kotlin.runCatching { config.copy(box = BoxConfig(startSize = 20, maxSize = 12)).validate() }.isFailure)
    }

    @Test
    fun `fixed width boxes put the area into the height`() {
        val config = TestData.job("t1").copy(box = BoxConfig(fixedWidth = 12))
        val s = Search(TestData.data, config, Tuning(), buildPlan(TestData.data, config), File("build/search/fw"))
        assertEquals(12 to 12, s.box(12))
        assertEquals(12 to 27, s.box(18))
        val square = Search(TestData.data, TestData.job("t1"), Tuning(), buildPlan(TestData.data, TestData.job("t1")), File("build/search/sq"))
        assertEquals(15 to 15, square.box(15))
    }
}
