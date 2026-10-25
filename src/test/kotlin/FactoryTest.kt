import blocks.runFactory
import blocks.splitPlan
import config.BlocksConfig
import config.SearchConfig
import config.Tuning
import plan.Role
import plan.buildPlan
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

class FactoryTest {
    @Test
    fun `the split keeps blocks within the limit and balances every item`() {
        val plan = buildPlan(TestData.data, TestData.job("t3"))
        val split = splitPlan(plan, BlocksConfig(maxMachines = 5), 30.0)
        assertTrue(split.blocks.size >= 2)
        assertTrue(split.blocks.all { it.size <= 5 }, "${split.blocks}")
        assertEquals(plan.machines.size, split.blocks.sumOf { it.size })
        // Each block's links in and out balance what its machines make and use.
        plan.nets.forEach { net ->
            split.blocks.indices.forEach { b ->
                val ts = plan.netTerminals(net).filter { it.machine != null && it.machine in split.blocks[b] }
                val made = ts.filter { it.role == Role.PRODUCER }.sumOf { it.rate }
                val used = ts.filter { it.role == Role.CONSUMER }.sumOf { it.rate }
                val inflow = split.links.filter { it.item == net.item && it.to == b }.sumOf { it.rate }
                val outflow = split.links.filter { it.item == net.item && it.from == b }.sumOf { it.rate }
                assertEquals(made + inflow, used + outflow, 1e-6, "${net.item} in block $b")
            }
        }
        assertTrue(split.links.filter { it.inner }.all { it.rate <= 0.9 * 30.0 + 1e-9 })
    }

    @Test
    fun `T3 as two blocks stitches into one valid factory`() {
        listOf(false, true).forEach { test ->
            val config = TestData.job("t3").copy(blocks = BlocksConfig(maxMachines = 5), testMode = test,
                search = SearchConfig(timeBudgetS = 20.0, seedsPerSize = 2, threads = 4, seed = 7))
            val plan = buildPlan(TestData.data, config)
            val out = File("build/factory/t3${if (test) "_test" else ""}")
            out.deleteRecursively()
            val f = assertNotNull(runFactory(TestData.data, config, Tuning.defaults(), plan, out))
            assertTrue(f.success, "${f.errors.take(5)} ${f.notes}")
            assertTrue(File(out, "best_output_string.txt").exists() && File(out, "ports.json").exists())
            assertTrue(File(out, "block_1/best_output_string.txt").exists() && File(out, "block_2/best_output_string.txt").exists())
        }
    }
}
