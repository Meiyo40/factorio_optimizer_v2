import config.PlaceTuning
import config.RunConfiguration
import place.Annealer
import place.PlaceModel
import place.staticObstacles
import plan.buildPlan
import java.util.SplittableRandom
import kotlin.math.abs
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class PlacerTest {
    private fun model(job: String, size: Int, tuning: PlaceTuning = PlaceTuning()): PlaceModel {
        val config = TestData.job(job)
        val plan = buildPlan(TestData.data, config)
        return PlaceModel(plan, config, tuning, size, size, staticObstacles(config, plan, size, size))
    }

    @Test
    fun `incremental cost matches a full recompute after 10000 random moves`() {
        for (job in listOf("t3", "t4")) {
            val m = model(job, 22)
            m.setHistory(mapOf(geom.Tile(5, 5) to 3.0, geom.Tile(10, 12) to 1.5), 0.7)
            val rnd = SplittableRandom(99)
            val a = Annealer(m, rnd, PlaceTuning())
            assertTrue(a.initialRandom())
            // Anneal at a high fixed temperature so moves are mixed accepts and reverts.
            a.anneal(t0 = 1e6, maxMoves = 10_000)
            val incremental = doubleArrayOf(m.wl, m.cong, m.pin, m.di, m.fb, m.cost)
            m.recomputeAll()
            val full = doubleArrayOf(m.wl, m.cong, m.pin, m.di, m.fb, m.cost)
            for (i in incremental.indices) {
                assertTrue(abs(incremental[i] - full[i]) <= 1e-6 * maxOf(1.0, abs(full[i])), "$job term $i: incremental ${incremental[i]} vs full ${full[i]}")
            }
        }
    }

    @Test
    fun `annealing T3 halves the random initial cost for 5 seeds`() {
        for (seed in 1L..5L) {
            val m = model("t3", 24)
            val a = Annealer(m, SplittableRandom(seed), PlaceTuning())
            assertTrue(a.initialRandom())
            val initial = m.cost
            val stats = a.anneal()
            println("T3 seed $seed: ${"%.1f".format(initial)} -> ${"%.1f".format(m.cost)} in ${stats.moves} moves, ${stats.temperatures} temperatures")
            assertTrue(m.cost < 0.5 * initial, "seed $seed: ${m.cost} vs initial $initial")
        }
    }

    @Test
    fun `T2 places the circuit machine next to a cable machine for direct insertion`() {
        var hits = 0
        for (seed in 1L..5L) {
            val m = model("t2", 14)
            val a = Annealer(m, SplittableRandom(seed), PlaceTuning())
            assertTrue(a.initialRandom())
            a.anneal()
            val fits = m.diPairsFitting()
            println("T2 seed $seed: DI pairs ${fits.map { (p, c) -> "${m.plan.machines[p].recipe}->${m.plan.machines[c].recipe}" }}")
            if (fits.any { (p, c) -> m.plan.machines[p].recipe == "copper-cable" && m.plan.machines[c].recipe == "electronic-circuit" }) hits++
        }
        assertTrue(hits >= 4, "direct insertion in only $hits of 5 seeds")
    }

    /** Checks a placement against the fluid connection rules. */
    private fun assertConnectionsLegal(m: PlaceModel) {
        val p = m.placement()
        val rects = p.machines.map { p.rect(m.plan, it) }
        for (i in rects.indices) for (j in i + 1 until rects.size) assertTrue(!rects[i].overlaps(rects[j]))
        val used = p.machines.flatMap { pm -> p.connections(m.plan, pm).filter { it.fluid != null } }
        val unused = p.machines.flatMap { pm -> p.connections(m.plan, pm).filter { it.fluid == null } }
        used.forEach { c ->
            assertTrue(c.tile.x in 0 until p.w && c.tile.y in 0 until p.h, "pipe end ${c.tile} outside")
            assertTrue(rects.none { c.tile in it }, "pipe end ${c.tile} under a machine")
            assertTrue(unused.none { it.tile == c.tile }, "pipe end ${c.tile} on an unused connection")
        }
        for (a in used) for (b in used) if (a.fluid != b.fluid) {
            assertTrue(a.tile.manhattan(b.tile) > 1, "${a.fluid} at ${a.tile} touches ${b.fluid} at ${b.tile}")
        }
    }

    @Test
    fun `legality keeps fluid connections inside the box and apart from other fluids`() {
        val m = model("t4", 20)
        val a = Annealer(m, SplittableRandom(3), PlaceTuning())
        assertTrue(a.initialRandom())
        a.anneal(maxMoves = 20_000)
        assertConnectionsLegal(m)
        assertEquals(m.plan.machines.size, m.placement().machines.size)
    }

    @Test
    fun `refineries and chemical plants rotate and mirror legally`() {
        val config = RunConfiguration(targets = listOf(config.ConfigTarget("sulfur", 3.0)))
        val plan = buildPlan(TestData.data, config)
        assertTrue(plan.machines.any { it.machine == "oil-refinery" } && plan.machines.any { it.machine == "chemical-plant" })
        for (seed in 1L..3L) {
            val m = PlaceModel(plan, config, PlaceTuning(), 30, 30, emptySet())
            val a = Annealer(m, SplittableRandom(seed), PlaceTuning())
            assertTrue(a.initialRandom())
            a.anneal(maxMoves = 30_000)
            assertConnectionsLegal(m)
            val incremental = m.cost
            m.recomputeAll()
            assertTrue(kotlin.math.abs(incremental - m.cost) < 1e-6 * maxOf(1.0, kotlin.math.abs(m.cost)))
        }
    }

    @Test
    fun `incremental cost stays exact with the path cost and column moves`() {
        val tun = PlaceTuning(beltCost = "path", initial = "columns", columnShare = 0.2)
        for (job in listOf("t3", "t4")) {
            val m = model(job, 24, tun)
            val a = Annealer(m, SplittableRandom(5), tun)
            assertTrue(a.initialRandom())
            a.anneal(t0 = 1e6, maxMoves = 10_000)
            assertTrue(a.initialColumns(buildPlan(TestData.data, TestData.job(job))))
            a.anneal(t0 = 1e6, maxMoves = 10_000)
            val incremental = doubleArrayOf(m.wl, m.cong, m.pin, m.di, m.fb, m.cost)
            m.recomputeAll()
            val full = doubleArrayOf(m.wl, m.cong, m.pin, m.di, m.fb, m.cost)
            for (i in incremental.indices) {
                assertTrue(abs(incremental[i] - full[i]) <= 1e-6 * maxOf(1.0, abs(full[i])), "$job term $i: incremental ${incremental[i]} vs full ${full[i]}")
            }
        }
    }

    @Test
    fun `the path cost charges a scattered net for its detours, a column only for its length`() {
        // Three cable-circuit machines of T2: flush in one column, then at three corners of the same box.
        val tun = PlaceTuning(beltCost = "path")
        val m = model("t2", 20, tun)
        val n = m.nM
        m.setAll(IntArray(n) { 8 }, IntArray(n) { 2 + 3 * it }, IntArray(n))
        val column = m.wl
        m.setAll(intArrayOf(2, 15, 2, 15).copyOf(n), intArrayOf(2, 2, 15, 15).copyOf(n), IntArray(n))
        val scattered = m.wl
        val hp = model("t2", 20, PlaceTuning(beltCost = "hpwl", qTable = listOf(listOf(1.0, 1.0), listOf(50.0, 1.0))))
        hp.setAll(IntArray(n) { 8 }, IntArray(n) { 2 + 3 * it }, IntArray(n))
        val hpColumn = hp.wl
        hp.setAll(intArrayOf(2, 15, 2, 15).copyOf(n), intArrayOf(2, 2, 15, 15).copyOf(n), IntArray(n))
        val hpScattered = hp.wl
        println("path: column $column scattered $scattered; hpwl: column $hpColumn scattered $hpScattered")
        assertTrue(column < scattered)
        // The column's machines share a corridor: no detours, so the path cost adds nothing between them.
        assertTrue(scattered - column > hpScattered - hpColumn, "the path cost should separate them more than the bounding box")
    }

    @Test
    fun `a column start keeps its columns through annealing`() {
        val tun = PlaceTuning(initial = "columns", columnMovesOnly = true)
        val config = TestData.job("t4")
        val plan = buildPlan(TestData.data, config)
        val m = PlaceModel(plan, config, tun, 20, 20, staticObstacles(config, plan, 20, 20))
        val a = Annealer(m, SplittableRandom(3), tun)
        assertTrue(a.initialColumns(plan))
        val xs = (0 until m.nM).map { m.px[it] }.toSet()
        assertTrue(xs.size >= 2, "T4 should need two columns in 20 x 20: $xs")
        a.anneal(maxMoves = 50_000, startRange = 3, refine = true, t0Scale = tun.columnT0Scale)
        // Whole columns move together, so the machines still stand on as many distinct x positions as columns.
        val after = (0 until m.nM).map { m.px[it] }.toSet()
        println("columns before ${xs.sorted()} after ${after.sorted()}")
        assertEquals(xs.size, after.size)
    }
}
