package power

import config.RunConfiguration
import data.GameConstants
import geom.Dir
import geom.Kind
import geom.PlacedEntity
import geom.Tile
import plan.inserterCount
import plan.inserterRate
import validate.OutputSpec
import validate.SourceSpec

data class Harness(val entities: List<PlacedEntity>, val tiles: Set<Tile>, val warnings: List<String>)

/**
 * H1 (test mode): feed every source belt from outside the box with a straight feeder belt whose sides
 * carry inserters taking from infinity chests, enough inserters for the source's rate. Every output belt
 * runs on outside the box past inserters that empty it into void infinity chests; fluid ports get an
 * infinity pipe (a drain for outputs).
 */
fun buildHarness(
    sources: List<SourceSpec>, entities: List<PlacedEntity>, w: Int, h: Int, config: RunConfiguration,
    outputs: List<OutputSpec> = emptyList(),
): Harness {
    val tier = GameConstants.beltTier(config.beltTier)
    val occupied = HashSet<Tile>()
    entities.forEach { e -> e.rect.tiles().forEach { occupied += it } }
    val byTile = entities.associateBy { it.tile }
    val insName = config.inserters.allowed.filter { it != "long-handed-inserter" }
        .maxByOrNull { inserterRate(it, config, fromBelt = false) } ?: "inserter"
    val cap = inserterRate(insName, config, fromBelt = false)
    val out = ArrayList<PlacedEntity>()
    val tiles = HashSet<Tile>()
    val warnings = ArrayList<String>()
    fun inBox(t: Tile) = t.x in 0 until w && t.y in 0 until h
    fun free(t: Tile) = !inBox(t) && t !in occupied && t !in tiles

    fun fluidPort(at: Tile, fluid: String, drain: Boolean) {
        // A fluid port gets an infinity pipe just outside the box, next to its pipe.
        val o = Dir.entries.firstOrNull { d -> !inBox(at + d) && free(at + d) }
        if (o == null) { warnings += "no room for a fluid harness at $at ($fluid)"; return }
        val t = at + o
        out += PlacedEntity(GameConstants.INFINITY_PIPE, Kind.INFINITY_PIPE, t.x, t.y, fluid = fluid, drain = drain)
        tiles += t
    }
    sources.filter { it.fluid }.forEach { fluidPort(it.tile, it.item, drain = false) }
    outputs.filter { it.fluid }.forEach { fluidPort(it.tile, it.item, drain = true) }
    // Item sources, grouped by tile: a shared belt gets one feeder with each item's inserters on the side that
    // drops onto its lane (an inserter drops on the far lane).
    sources.filter { !it.fluid }.groupBy { it.tile }.forEach { (tile, group) ->
        val first = byTile[tile]
        val laned = group.any { it.lane >= 0 }
        // Outward direction: off the box, and not the way the first belt faces (it cannot take items head-on);
        // a laned belt must be fed straight from behind so its lanes stay as they are.
        val dirs = if (laned && first != null) listOf(first.dir.opposite).filter { !inBox(tile + it) }
            else Dir.entries.filter { d -> !inBox(tile + d) && first?.dir != d }
        val o = dirs.firstOrNull { d -> free(tile + d) }
        if (o == null) { warnings += "no room for a harness at $tile (${group.map { it.item }})"; return@forEach }
        // Feeder heading o.opposite: inserters on its o.ccw side drop on the left lane, on o.cw the right lane.
        val pending = group.map { s -> s to inserterCount(s.rate, cap) }.toMutableList()
        fun sidesFor(s: SourceSpec) = when (s.lane) { 0 -> listOf(o.ccw); 1 -> listOf(o.cw); else -> listOf(o.cw, o.ccw) }
        val total = pending.sumOf { it.second }
        var i = 1
        while (pending.any { it.second > 0 } && i <= total + 4) {
            val f = tile.step(o, i)
            if (!free(f)) { warnings += "harness at $tile blocked at $f"; break }
            out += PlacedEntity(tier.belt, Kind.BELT, f.x, f.y, o.opposite)
            tiles += f
            for (side in listOf(o.cw, o.ccw)) {
                val k = pending.indexOfFirst { (s, n) -> n > 0 && side in sidesFor(s) }
                if (k < 0) continue
                val insT = f + side
                val chestT = f.step(side, 2)
                if (!free(insT) || !free(chestT)) continue
                out += PlacedEntity(insName, Kind.INSERTER, insT.x, insT.y, side)   // picks from the chest side
                out += PlacedEntity(GameConstants.INFINITY_CHEST, Kind.INFINITY_CHEST, chestT.x, chestT.y, item = pending[k].first.item)
                tiles += insT; tiles += chestT
                pending[k] = pending[k].first to pending[k].second - 1
            }
            i++
        }
        pending.filter { it.second > 0 }.forEach { (s, n) -> warnings += "harness for ${s.item} is short of $n inserters" }
    }
    // Outputs after inputs: an input's feeder must run straight out, an output's void inserters can go either side.
    val pickCap = inserterRate(insName, config, fromBelt = true)
    outputs.filter { !it.fluid }.forEach { o ->
        val d = byTile[o.tile]?.dir
        if (d == null || inBox(o.tile + d)) { warnings += "output of ${o.item} at ${o.tile} does not leave the box"; return@forEach }
        val k = inserterCount(o.rate, pickCap)
        var placedIns = 0
        var i = 1
        while (placedIns < k && i <= k + 4) {
            val f = o.tile.step(d, i)
            if (!free(f)) { warnings += "output harness for ${o.item} blocked at $f"; break }
            out += PlacedEntity(tier.belt, Kind.BELT, f.x, f.y, d)
            tiles += f
            for (side in listOf(d.cw, d.ccw)) {
                if (placedIns >= k) break
                val insT = f + side
                val chestT = f.step(side, 2)
                if (!free(insT) || !free(chestT)) continue
                out += PlacedEntity(insName, Kind.INSERTER, insT.x, insT.y, side.opposite)   // picks from the belt
                out += PlacedEntity(GameConstants.INFINITY_CHEST, Kind.INFINITY_CHEST, chestT.x, chestT.y)
                tiles += insT; tiles += chestT
                placedIns++
            }
            i++
        }
        if (placedIns < k) warnings += "output harness for ${o.item} has $placedIns of $k inserters"
    }
    return Harness(out, tiles, warnings)
}
