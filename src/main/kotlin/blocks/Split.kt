package blocks

import config.BlocksConfig
import config.ConfigSource
import config.Tuning
import plan.Net
import plan.Port
import plan.ProductionPlan
import plan.Role
import plan.Terminal
import kotlin.math.abs
import kotlin.math.ceil

/** [rate] of [item] going from block [from] to block [to]; -1 is the factory's outer edge (raw inputs, products). */
data class Link(val id: Int, val item: String, val rate: Double, val from: Int, val to: Int, val fluid: Boolean) {
    /** Between two blocks: one belt, routed through the corridors. Edge links leave or enter a block directly. */
    val inner: Boolean get() = from >= 0 && to >= 0
}

/** A block's place in the factory grid; blocks snake through the rows so neighbours in order sit side by side. */
data class Slot(val row: Int, val col: Int)

/** The plan split into blocks (machine ids of the full plan, upstream first), their slots and the links. */
class Split(val blocks: List<List<Int>>, val slots: List<Slot>, val rows: Int, val cols: Int, val links: List<Link>, val warnings: List<String>)

/**
 * Splits [plan] into blocks of at most [BlocksConfig.maxMachines] machines. Machines joined by a pipe stay
 * together (fluids never cross a block border); then units merge greedily along the heaviest item flow between
 * them (items in [BlocksConfig.splitAt] count [BlocksConfig.splitAtWeight] as much) while the merged block stays within the limit, and small
 * leftovers merge with each other. Blocks are ordered upstream first and snaked through one or two rows, so
 * every block touches the factory's outer edge, where raw inputs enter and products leave. Each item's surplus
 * and deficit per block become links, nearest blocks first; links between blocks are cut into belts of at most
 * [BlocksConfig.linkBeltShare] of the belt tier.
 */
fun splitPlan(plan: ProductionPlan, cfg: BlocksConfig, beltThroughput: Double): Split {
    val n = plan.machines.size
    val warnings = ArrayList<String>()
    // 1. Units: machines joined through fluid nets.
    val parent = IntArray(n) { it }
    fun find(x: Int): Int { var a = x; while (parent[a] != a) { parent[a] = parent[parent[a]]; a = parent[a] }; return a }
    plan.nets.filter { it.fluid }.forEach { net ->
        val ms = plan.netTerminals(net).mapNotNull { it.machine }
        ms.drop(1).forEach { m -> val a = find(ms[0]); val b = find(m); if (a != b) parent[a] = b }
    }
    val clusters = HashMap<Int, MutableList<Int>>()
    (0 until n).forEach { clusters.getOrPut(find(it)) { mutableListOf() } += it }
    val clusterOf = IntArray(n)
    val members = HashMap<Int, MutableList<Int>>()
    clusters.values.forEachIndexed { i, ms -> members[i] = ms; ms.forEach { clusterOf[it] = i } }
    members.values.filter { it.size > cfg.maxMachines }.forEach {
        warnings += "${it.size} machines are joined by pipes and stay one block, above the ${cfg.maxMachines}-machine limit"
    }

    // 2. Flow between clusters.
    val weight = HashMap<Long, Double>()
    fun key(a: Int, b: Int) = if (a < b) a.toLong() * 1_000_000 + b else b.toLong() * 1_000_000 + a
    plan.nets.filter { !it.fluid }.forEach { net ->
        val ts = plan.netTerminals(net)
        val prod = ts.filter { it.role == Role.PRODUCER && it.machine != null }
        val cons = ts.filter { it.role == Role.CONSUMER && it.machine != null }
        val total = cons.sumOf { it.rate }
        if (total < 1e-12) return@forEach
        val f = if (net.item in cfg.splitAt) cfg.splitAtWeight else 1.0
        prod.forEach { p -> cons.forEach { c ->
            val a = clusterOf[p.machine!!]; val b = clusterOf[c.machine!!]
            if (a != b) weight.merge(key(a, b), f * p.rate * c.rate / total, Double::plus)
        } }
    }

    // 3. Greedy merging along the heaviest flow, within the size limit; then small leftovers together.
    fun size(c: Int) = members.getValue(c).size
    fun merge(a: Int, b: Int) {
        members.getValue(a) += members.getValue(b)
        members.remove(b)
        members.getValue(a).forEach { clusterOf[it] = a }
        val moved = weight.keys.filter { (it / 1_000_000).toInt() == b || (it % 1_000_000).toInt() == b }
        moved.forEach { k ->
            val w = weight.remove(k)!!
            val o = if ((k / 1_000_000).toInt() == b) (k % 1_000_000).toInt() else (k / 1_000_000).toInt()
            if (o != a) weight.merge(key(a, o), w, Double::plus)
        }
    }
    while (true) {
        val best = weight.entries.filter { (k, _) -> size((k / 1_000_000).toInt()) + size((k % 1_000_000).toInt()) <= cfg.maxMachines }
            .maxByOrNull { it.value } ?: break
        merge((best.key / 1_000_000).toInt(), (best.key % 1_000_000).toInt())
    }
    while (true) {
        val small = members.keys.sortedBy { size(it) }
        if (small.size < 2 || size(small[0]) + size(small[1]) > cfg.maxMachines) break
        merge(small[0], small[1])
    }

    // 4. Order upstream first: production depth of each machine (longest chain from inputs, capped for loops).
    val depth = DoubleArray(n)
    repeat(40) {
        val itemDepth = HashMap<String, Double>()
        plan.machines.forEach { m -> m.products.keys.forEach { p -> itemDepth.merge(p, depth[m.id], ::maxOf) } }
        plan.machines.forEach { m ->
            val ins = m.demand.keys + m.fluidDemand.keys
            depth[m.id] = 1 + (ins.maxOfOrNull { itemDepth[it] ?: 0.0 } ?: 0.0).coerceAtMost(60.0)
        }
    }
    val blocks = members.values.map { ms -> ms.sorted() }.sortedBy { ms -> ms.map { depth[it] }.average() }
    val k = blocks.size
    val rows = cfg.rows?.coerceAtMost(k) ?: if (k <= 3) 1 else 2
    val cols = (k + rows - 1) / rows
    val slots = (0 until k).map { i -> val r = i / cols; Slot(r, if (r == 0) i % cols else cols - 1 - i % cols) }
    val blockOf = IntArray(n).also { a -> blocks.forEachIndexed { b, ms -> ms.forEach { a[it] = b } } }

    // 5. Links: per item, block surpluses to block deficits (nearest first), the edge supplying raw inputs
    //    and taking products.
    val links = ArrayList<Link>()
    fun dist(a: Int, b: Int): Int = if (a < 0 || b < 0) 0 else abs(slots[a].row - slots[b].row) * 2 + abs(slots[a].col - slots[b].col)
    plan.nets.forEach { net ->
        val ts = plan.netTerminals(net)
        val balance = DoubleArray(k)
        ts.forEach { t ->
            val m = t.machine ?: return@forEach
            when (t.role) {
                Role.PRODUCER -> balance[blockOf[m]] += t.rate
                Role.CONSUMER -> balance[blockOf[m]] -= t.rate
                else -> {}
            }
        }
        val give = ArrayList<Pair<Int, Double>>()
        val take = ArrayList<Pair<Int, Double>>()
        (0 until k).forEach { b -> if (balance[b] > 1e-9) give += b to balance[b] else if (balance[b] < -1e-9) take += b to -balance[b] }
        ts.filter { it.role == Role.SOURCE }.sumOf { it.rate }.takeIf { it > 1e-9 }?.let { give += -1 to it }
        ts.filter { it.role == Role.OUTPUT }.sumOf { it.rate }.takeIf { it > 1e-9 }?.let { take += -1 to it }
        val left = HashMap<Pair<Int, Int>, Double>()
        val pairs = give.flatMap { g -> take.map { t -> g.first to t.first } }.filter { (a, b) -> a != b }.sortedBy { (a, b) -> dist(a, b) }
        val giveLeft = give.toMap().toMutableMap(); val takeLeft = take.toMap().toMutableMap()
        pairs.forEach { (a, b) ->
            val r = minOf(giveLeft.getValue(a), takeLeft.getValue(b))
            if (r < 1e-9) return@forEach
            giveLeft[a] = giveLeft.getValue(a) - r; takeLeft[b] = takeLeft.getValue(b) - r
            left.merge(a to b, r, Double::plus)
        }
        left.forEach { (ab, r) ->
            val (a, b) = ab
            if (net.fluid && a >= 0 && b >= 0) {
                warnings += "${net.item} would have to be piped between blocks ${a + 1} and ${b + 1}; fluids are not stitched yet"
            }
            val pieces = if (a >= 0 && b >= 0 && !net.fluid) maxOf(1, ceil(r / (cfg.linkBeltShare * beltThroughput) - 1e-9).toInt()) else 1
            repeat(pieces) { links += Link(links.size, net.item, r / pieces, a, b, net.fluid) }
        }
    }
    return Split(blocks, slots, rows, cols, links, warnings)
}

/**
 * [split] with blocks [a] and [b] (side by side in one row) made one block in [a]'s place: the links between
 * them disappear (their items now flow inside the block), the blocks after them in the row move up one slot,
 * and link ids are renumbered to stay list indices.
 */
fun mergeBlocks(split: Split, a: Int, b: Int): Split {
    require(split.slots[a].row == split.slots[b].row && kotlin.math.abs(split.slots[a].col - split.slots[b].col) == 1) { "blocks $a and $b are not neighbours" }
    val (keep, gone) = if (split.slots[a].col < split.slots[b].col) a to b else b to a
    val row = split.slots[keep].row
    val goneCol = split.slots[gone].col
    // New block indices: everything but [gone], in order.
    val index = IntArray(split.blocks.size) { -1 }
    var n = 0
    split.blocks.indices.forEach { i -> if (i != gone) index[i] = n++ }
    index[gone] = index[keep]
    val blocks = split.blocks.indices.filter { it != gone }.map { i -> if (i == keep) (split.blocks[keep] + split.blocks[gone]).sorted() else split.blocks[i] }
    val slots = split.blocks.indices.filter { it != gone }.map { i ->
        val s = split.slots[i]
        if (s.row == row && s.col > goneCol) Slot(s.row, s.col - 1) else s
    }
    val links = split.links.filter { l -> !(l.from >= 0 && l.to >= 0 && index[l.from] == index[l.to]) }
        .mapIndexed { i, l -> l.copy(id = i, from = if (l.from >= 0) index[l.from] else -1, to = if (l.to >= 0) index[l.to] else -1) }
    val cols = slots.maxOf { it.col } + 1
    return Split(blocks, slots, split.rows, cols, links, split.warnings)
}

/** The edge of block [b] that a link's port goes on: towards its partner block, or the factory's outer edge. */
fun edgeFor(split: Split, link: Link, b: Int): String {
    val me = split.slots[b]
    val other = if (link.from == b) link.to else link.from
    if (other < 0) return if (split.rows == 1 || me.row == 0) "north" else "south"
    val o = split.slots[other]
    if (o.row == me.row && abs(o.col - me.col) == 1) return if (o.col > me.col) "east" else "west"
    // Not side by side: through the corridor along the rows (below a single row, between two rows).
    return if (split.rows == 1 || me.row == 0) "south" else "north"
}

/** One block as a plan of its own: local machine ids, a port per link, and which link each port serves. */
class BlockPlan(val index: Int, val plan: ProductionPlan, val globalIds: List<Int>, val linkOfPort: Map<Int, Int>)

/** The plan of block [b]: its machines, their terminals, and an input or output port per link it is part of. */
fun blockPlan(full: ProductionPlan, split: Split, b: Int, tuning: Tuning): BlockPlan {
    val ids = split.blocks[b]
    val local = ids.withIndex().associate { (i, g) -> g to i }
    val machines = ids.map { full.machines[it].copy(id = local.getValue(it)) }
    val terminals = ArrayList<Terminal>()
    full.terminals.filter { it.machine != null && it.machine in local }.forEach { t ->
        terminals += t.copy(id = terminals.size, machine = local.getValue(t.machine!!), port = null)
    }
    val ports = ArrayList<Port>()
    val linkOfPort = HashMap<Int, Int>()
    split.links.filter { it.from == b || it.to == b }.forEach { l ->
        val out = l.from == b
        val port = Port(ports.size, l.item, listOf(ConfigSource(l.item, edge = edgeFor(split, l, b))), fluid = l.fluid, output = out, single = l.inner)
        ports += port
        linkOfPort[port.id] = l.id
        terminals += Terminal(terminals.size, if (out) Role.OUTPUT else Role.SOURCE, l.item, l.rate, 1, port = port.id, fluid = l.fluid)
    }
    val tier = data.GameConstants.beltTier(full.beltTier)
    val nets = ArrayList<Net>()
    terminals.filter { it.role != Role.SINK }.groupBy { it.item }.forEach { (item, ts) ->
        val takers = ts.filter { it.role == Role.CONSUMER || it.role == Role.OUTPUT }
        if (ts.size < 2 || takers.isEmpty()) return@forEach
        val fluid = ts.first().fluid
        val flow = takers.sumOf { it.rate }
        val belts = if (fluid) 1 else maxOf(1, ceil(flow / (tuning.net.beltUtilisation * tier.throughput) - 1e-9).toInt())
        nets += Net(nets.size, item, flow, ts.map { it.id }, belts, fluid = fluid)
    }
    val inputs = split.links.filter { it.to == b && it.from < 0 }.groupBy { it.item }.mapValues { (_, ls) -> ls.sumOf { it.rate } }
    val outputs = split.links.filter { it.from == b && it.to < 0 }.groupBy { it.item }.mapValues { (_, ls) -> ls.sumOf { it.rate } }
    val plan = ProductionPlan(machines, ports, terminals, nets, inputs, inputs.filterKeys { k -> split.links.any { it.item == k && it.fluid } },
        emptyMap(), full.beltTier, emptyList(), outputs)
    return BlockPlan(b, plan, ids, linkOfPort)
}
