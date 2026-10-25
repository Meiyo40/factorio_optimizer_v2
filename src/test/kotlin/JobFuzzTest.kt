import config.ConfigSource
import config.ConfigTarget
import config.RunConfiguration
import config.Tuning
import config.strictJson
import data.GameConstants
import emit.toBlueprint
import emit.toLayout
import plan.ProductionPlan
import plan.buildPlan
import search.TrialResult
import search.runTrial
import viz.RenderOptions
import viz.renderLayout
import java.io.File
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlin.math.ceil
import kotlin.math.sqrt
import kotlin.random.Random
import kotlin.test.Test

/**
 * Random jobs, skipped unless FUZZ=<minutes> is set (FUZZ_THREADS, default 8; FUZZ_SEED, default 1; FUZZ_OUT,
 * default build/fuzz; FUZZ_MAX_MACHINES, default 45). Each job picks one or two targets and a machine set (base
 * game, Space Age machines first, Space Age with assemblers first, or a space platform fed only asteroid chunks),
 * plans it (halving rates while it has too many machines), and runs trials at a roomy size (4x the machine area,
 * then 6x), up to three seeds each. One line per job goes to results.tsv as it finishes; failures also save the
 * job as JSON (replay with DiagTest) and a render. A planner or trial exception, or a layout that fails
 * validation, is a bug; a route failure at a roomy size is worth a look.
 */
class JobFuzzTest {
    private val data get() = TestData.data

    private val baseTargets = listOf(
        "iron-gear-wheel", "electronic-circuit", "advanced-circuit", "processing-unit", "plastic-bar", "sulfur",
        "sulfuric-acid", "battery", "engine-unit", "electric-engine-unit", "flying-robot-frame", "low-density-structure",
        "rocket-fuel", "solid-fuel", "lubricant", "automation-science-pack", "logistic-science-pack",
        "military-science-pack", "chemical-science-pack", "production-science-pack", "utility-science-pack",
        "steel-plate", "stone-brick", "concrete", "rail", "inserter", "fast-inserter", "transport-belt",
        "fast-transport-belt", "express-transport-belt", "pipe", "medium-electric-pole", "explosives", "grenade",
        "firearm-magazine", "piercing-rounds-magazine", "uranium-fuel-cell", "uranium-235", "speed-module",
        "productivity-module", "efficiency-module", "copper-cable", "iron-stick", "steel-chest", "small-lamp",
    )
    private val saTargets = listOf(
        "tungsten-carbide", "tungsten-plate", "metallurgic-science-pack", "holmium-plate", "superconductor",
        "supercapacitor", "electromagnetic-science-pack", "electrolyte", "bioflux", "nutrients",
        "agricultural-science-pack", "carbon-fiber", "lithium-plate", "lithium", "fluoroketone-cold",
        "cryogenic-science-pack", "quantum-processor", "space-science-pack", "carbon", "thruster-fuel",
        "thruster-oxidizer", "iron-plate", "copper-plate", "turbo-transport-belt", "big-mining-drill",
        "foundation", "processing-unit", "low-density-structure", "electronic-circuit",
    )
    private val platformTargets = listOf(
        "iron-gear-wheel", "copper-cable", "electronic-circuit", "space-science-pack", "thruster-fuel",
        "thruster-oxidizer", "steel-plate", "iron-plate", "copper-plate", "carbon", "ice", "calcite", "sulfur",
        "low-density-structure", "rocket-fuel",
    )
    private val saRaw = listOf(
        "iron-ore", "copper-ore", "stone", "coal", "crude-oil", "water", "uranium-ore", "wood", "raw-fish", "calcite",
        "tungsten-ore", "lava", "holmium-ore", "scrap", "heavy-oil", "sulfuric-acid", "lithium-brine", "fluorine",
        "ammoniacal-solution", "yumako", "jellynut", "spoilage", "pentapod-egg",
        "metallic-asteroid-chunk", "carbonic-asteroid-chunk", "oxide-asteroid-chunk",
    )
    private val saMachines = listOf("foundry", "electromagnetic-plant", "cryogenic-plant", "biochamber")
    private val baseMachines = listOf("assembling-machine-3", "electric-furnace", "chemical-plant", "oil-refinery", "centrifuge")

    private class Job(val id: Int, val kind: String, val config: RunConfiguration)

    private fun job(id: Int, rnd: Random): Job {
        val kind = listOf("base", "base", "sa", "sa-am-first", "platform").random(rnd)
        val pool = when (kind) {
            "base" -> baseTargets
            "platform" -> platformTargets
            else -> saTargets + baseTargets
        }.filter { it in data.items || it in data.fluids }
        val targets = pool.shuffled(rnd).take(if (rnd.nextDouble() < 0.25) 2 else 1)
            .map { ConfigTarget(it, listOf(0.25, 0.5, 1.0, 2.0, 4.0).random(rnd)) }
        val machines = when (kind) {
            "base" -> null
            "sa" -> saMachines + baseMachines + "crusher"
            "sa-am-first" -> baseMachines.take(1) + saMachines + baseMachines.drop(1) + "crusher"
            else -> listOf("crusher", "assembling-machine-3", "electric-furnace", "chemical-plant", "foundry")
        }
        val sources = if (kind == "platform") listOf(
            ConfigSource("metallic-asteroid-chunk", edge = "west"),
            ConfigSource("carbonic-asteroid-chunk", edge = "west"),
            ConfigSource("oxide-asteroid-chunk", edge = "north"),
        ) else emptyList()
        val config = RunConfiguration(
            targets = targets,
            machines = machines,
            rawResources = when (kind) { "base" -> null; "platform" -> emptyList(); else -> saRaw },
            sources = sources,
            beltTier = (if (kind == "base") listOf("fast", "express") else listOf("fast", "express", "turbo")).random(rnd),
            laneSharing = rnd.nextDouble() < 0.3,
            forbiddenRecipes = if (kind == "platform") listOf("metallic-asteroid-reprocessing", "carbonic-asteroid-reprocessing", "oxide-asteroid-reprocessing") else emptyList(),
        )
        return Job(id, kind, config)
    }

    /** Plans [config], halving its rates while it needs more than [maxMachines] machines. */
    private fun plan(config: RunConfiguration, maxMachines: Int): Pair<RunConfiguration, ProductionPlan> {
        var c = config
        repeat(6) {
            val p = buildPlan(data, c)
            if (p.machines.size <= maxMachines) return c to p
            val f = maxOf(0.1, maxMachines * 0.8 / p.machines.size)
            c = c.copy(targets = c.targets.map { it.copy(amount = it.amount * f) })
        }
        error("too big")
    }

    @Test
    fun fuzz() {
        val minutes = System.getenv("FUZZ")?.toDoubleOrNull() ?: return
        val threads = System.getenv("FUZZ_THREADS")?.toIntOrNull() ?: 8
        val seed = System.getenv("FUZZ_SEED")?.toLongOrNull() ?: 1L
        val maxMachines = System.getenv("FUZZ_MAX_MACHINES")?.toIntOrNull() ?: 45
        val out = File(System.getenv("FUZZ_OUT") ?: "build/fuzz").apply { mkdirs() }
        val tsv = File(out, "results.tsv")
        val stop = System.currentTimeMillis() + (minutes * 60_000).toLong()
        val rnd = Random(seed)
        val lock = Any()
        fun write(line: String) = synchronized(lock) { tsv.appendText(line + "\n") }
        val pool = Executors.newFixedThreadPool(threads)
        // Jobs are made up front from one random stream, so a run is repeatable; workers take them in order.
        val jobs = (1..5000).map { job(it, rnd) }.iterator()
        repeat(threads) {
            pool.submit {
                while (System.currentTimeMillis() < stop) {
                    val j = synchronized(lock) { if (jobs.hasNext()) jobs.next() else null } ?: break
                    write(runJob(j, maxMachines, stop, out))
                }
            }
        }
        pool.shutdown()
        pool.awaitTermination(24, TimeUnit.HOURS)
    }

    private fun runJob(j: Job, maxMachines: Int, stop: Long, out: File): String {
        val t0 = System.currentTimeMillis()
        val label = j.config.targets.joinToString("+") { "${it.name}@${"%.3g".format(it.amount)}" }
        fun line(outcome: String, plan: ProductionPlan?, size: Int, seed: Long, detail: String) =
            listOf(j.id, j.kind, label, plan?.machines?.size ?: -1, plan?.nets?.size ?: -1, j.config.beltTier,
                j.config.laneSharing, outcome, size, seed, System.currentTimeMillis() - t0, detail.replace("\t", " ").replace("\n", " ").take(400))
                .joinToString("\t")
        fun save(config: RunConfiguration, r: TrialResult?, name: String) {
            File(out, "job_${j.id}.json").writeText(strictJson.encodeToString(RunConfiguration.serializer(), config))
            r?.layout?.let { renderLayout(it, File(out, "job_${j.id}_$name.png"), RenderOptions(errors = r.errors, title = "job ${j.id} $label ${r.stage}")) }
        }
        val (config, plan) = try {
            plan(j.config, maxMachines)
        } catch (e: Throwable) {
            val msg = e.message ?: e.toString()
            val outcome = when {
                msg == "too big" -> "too-big"
                msg.startsWith("No production plan") -> "noplan"
                else -> "plan-error"
            }
            if (outcome == "plan-error") save(j.config, null, "plan")
            return line(outcome, null, 0, 0, if (outcome == "plan-error") e.stackTraceToString().take(400) else msg)
        }
        if (plan.machines.isEmpty()) return line("empty", plan, 0, 0, "")
        val area = plan.machines.sumOf { it.w * it.h }
        val failures = ArrayList<String>()
        var last: TrialResult? = null
        for (factor in listOf(4.0, 6.0)) {
            val size = maxOf(10, ceil(sqrt(area * factor)).toInt())
            for (seed in 1L..3L) {
                if (System.currentTimeMillis() > stop + 10 * 60_000) return line("timeout", plan, size, seed, failures.joinToString("; "))
                val r = try {
                    runTrial(data, plan, config, Tuning.defaults(), size, size, seed, deadline = System.currentTimeMillis() + 6 * 60_000)
                } catch (e: Throwable) {
                    save(config, null, "trial")
                    return line("trial-error", plan, size, seed, e.stackTraceToString().take(400))
                }
                last = r
                if (r.stage == "validate" || r.stage == "power") {
                    // An invalid layout is a bug whatever later seeds do.
                    save(config, r, "${size}_$seed")
                    return line(r.stage, plan, size, seed, r.errors.take(4).joinToString("; "))
                }
                if (r.success) {
                    // The layout must also survive a blueprint round trip unchanged.
                    val l = r.layout!!
                    val back = l.toBlueprint().toLayout(l.w, l.h)
                    // Furnaces take their recipe from what they are fed and pipes their fluid from what they touch, so
                    // blueprints leave both out.
                    fun norm(e: geom.PlacedEntity) = e.stripped().let {
                        when {
                            it.kind == geom.Kind.MACHINE && !GameConstants.takesRecipe(it.name) -> it.copy(recipe = null)
                            it.kind == geom.Kind.PIPE || it.kind == geom.Kind.PIPE_UG -> it.copy(fluid = null)
                            else -> it
                        }
                    }
                    val diff = l.entities.map(::norm).zip(back.entities.map(::norm)).firstOrNull { (a, b) -> a != b }
                    if (diff != null || back.entities.size != l.entities.size) {
                        save(config, r, "roundtrip")
                        return line("roundtrip", plan, size, seed, "blueprint round trip changed ${diff?.first} into ${diff?.second}")
                    }
                    return line(if (failures.isEmpty()) "ok" else "ok-retry", plan, size, seed, failures.joinToString("; "))
                }
                failures += "${size}/$seed ${r.stage} ${r.route?.failedNets?.map { plan.nets[it].item }?.distinct() ?: ""}"
            }
        }
        save(config, last, "route")
        return line("fail", plan, last?.w ?: 0, last?.seed ?: 0, failures.joinToString("; ") + " " + (last?.notes?.lastOrNull()?.take(150) ?: ""))
    }
}
