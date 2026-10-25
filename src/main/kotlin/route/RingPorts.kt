package route

import data.GameConstants
import geom.Dir
import geom.Kind
import geom.PlacedEntity
import geom.Tile
import validate.OutputSpec
import validate.SourceSpec

/** A routed layout with its ports moved onto the ring around the box: the entities and the new port tiles. */
class RingedPorts(val entities: List<PlacedEntity>, val sources: List<SourceSpec>, val outputs: List<OutputSpec>)

/**
 * Moves every port on the box edge one tile out, onto the ring around the box, so inputs and outputs stand out:
 * an input belt gets one more belt behind it (inputs enter the box heading straight in, so lanes stay as they
 * are), an output belt one more in front, a fluid port one more pipe unless that pipe would touch a pipe of
 * another fluid. Ports already outside the box, or whose ring tile is taken, stay where they are, and so does a
 * fluid port on a pipe-to-ground (it sits beside a port of another fluid and is fed through its open side).
 */
fun ringPorts(entities: List<PlacedEntity>, sources: List<SourceSpec>, outputs: List<OutputSpec>, w: Int, h: Int, beltName: String): RingedPorts {
    fun inBox(t: Tile) = t.x in 0 until w && t.y in 0 until h
    val list = ArrayList(entities)
    val occ = HashMap<Tile, PlacedEntity>().apply { entities.forEach { e -> e.rect.tiles().forEach { put(it, e) } } }
    fun add(e: PlacedEntity) { list += e; e.rect.tiles().forEach { occ[it] = e } }
    val pipeKinds = setOf(Kind.PIPE, Kind.PIPE_UG, Kind.INFINITY_PIPE)
    // A ring pipe joins every neighbour: none may hold a pipe of another fluid.
    fun pipeSafe(r: Tile, fluid: String) = Dir.entries.all { d -> occ[r + d]?.let { it.kind !in pipeKinds || it.fluid == fluid } ?: true }
    fun pipeOut(t: Tile, fluid: String, net: Int?): Tile? {
        val d = Dir.entries.filter { !inBox(t + it) }.firstOrNull { (t + it) !in occ && pipeSafe(t + it, fluid) } ?: return null
        val r = t + d
        add(PlacedEntity(GameConstants.PIPE, Kind.PIPE, r.x, r.y, fluid = fluid, net = net))
        return r
    }
    // Inputs, one ring tile per port tile (two items sharing a belt move together).
    val moved = HashMap<Tile, Tile>()
    sources.groupBy { it.tile }.forEach { (t, group) ->
        if (!inBox(t)) return@forEach
        val e = occ[t] ?: return@forEach
        if (group.first().fluid) { if (e.kind == Kind.PIPE) pipeOut(t, group.first().item, e.net)?.let { moved[t] = it }; return@forEach }
        if (!e.kind.beltFamily || e.kind == Kind.SPLITTER) return@forEach
        val r = t + e.dir.opposite
        if (inBox(r) || r in occ) return@forEach
        add(PlacedEntity(beltName, Kind.BELT, r.x, r.y, e.dir, net = e.net))
        moved[t] = r
    }
    val newSources = sources.map { s -> moved[s.tile]?.let { s.copy(tile = it) } ?: s }
    val newOutputs = outputs.map { o ->
        if (!inBox(o.tile)) return@map o
        val e = occ[o.tile] ?: return@map o
        if (o.fluid) return@map (if (e.kind == Kind.PIPE) pipeOut(o.tile, o.item, e.net) else null)?.let { o.copy(tile = it) } ?: o
        if (e.kind != Kind.BELT && e.kind != Kind.UG_OUT) return@map o
        val r = o.tile + e.dir
        if (inBox(r) || r in occ) return@map o
        add(PlacedEntity(beltName, Kind.BELT, r.x, r.y, e.dir, net = e.net))
        o.copy(tile = r)
    }
    return RingedPorts(list, newSources, newOutputs)
}
