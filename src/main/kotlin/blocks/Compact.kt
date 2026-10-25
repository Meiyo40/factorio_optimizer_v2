package blocks

import geom.Dir
import geom.GroundTile
import geom.Kind
import geom.PlacedEntity
import geom.Tile
import geom.dropTile
import geom.outputTile
import geom.pickupTile

/**
 * A layout with some rows and columns of its box taken out, and where every old coordinate went. Coordinates
 * outside the box keep their side: before the box unchanged, after it shifted by what was removed.
 */
class Compaction(
    val entities: List<PlacedEntity>, val tiles: List<GroundTile>, val w: Int, val h: Int,
    private val newX: IntArray, private val newY: IntArray, val rowsRemoved: Int, val colsRemoved: Int,
) {
    fun x(x: Int): Int = when { x < 0 -> x; x >= newX.size -> x - colsRemoved; else -> newX[x] }
    fun y(y: Int): Int = when { y < 0 -> y; y >= newY.size -> y - rowsRemoved; else -> newY[y] }
    fun tile(t: Tile) = Tile(x(t.x), y(t.y))
}

private val PIPES = setOf(Kind.PIPE, Kind.PIPE_UG, Kind.INFINITY_PIPE)

/**
 * Removes every row and column of the box that only straight belts, straight pipes or underground spans cross,
 * one at a time and checking again after each: a line goes when every entity on it is a belt heading straight
 * across it (fed straight from behind, feeding straight on), or a plain pipe joined only to plain pipes before
 * and after it, and no inserter reaches across it; and on its empty tiles no belt points into the gap at
 * something, and no pipe would come to touch a pipe or a machine. Nothing changes but lengths, so whatever
 * worked still works (callers validate anyway). Poles must be removed first and placed again afterwards.
 */
fun compactLayout(entities: List<PlacedEntity>, tiles: List<GroundTile>, w: Int, h: Int): Compaction {
    var ents = entities.toMutableList()
    var ground = tiles
    var cw = w; var ch = h
    val newX = IntArray(w) { it }; val newY = IntArray(h) { it }
    var rows = 0; var cols = 0

    fun occupancy(): HashMap<Tile, Int> {
        val m = HashMap<Tile, Int>()
        ents.forEachIndexed { i, e -> e.rect.tiles().forEach { m[it] = i } }
        return m
    }

    /** Can line [c] go? [row]: the row y = c (crossed vertically), else the column x = c (crossed horizontally). */
    fun removable(c: Int, row: Boolean, occ: Map<Tile, Int>): Boolean {
        val len = if (row) cw else ch
        fun at(i: Int, off: Int) = if (row) Tile(i, c + off) else Tile(c + off, i)
        fun ent(t: Tile) = occ[t]?.let { ents[it] }
        val across = if (row) setOf(Dir.N, Dir.S) else setOf(Dir.E, Dir.W)
        // No inserter may pick or drop across the line (its pickup, itself and its drop span the line).
        for (e in ents) {
            if (e.kind != Kind.INSERTER || e.dir !in across) continue
            val p = e.pickupTile; val d = e.dropTile
            val lo = if (row) minOf(p.y, d.y) else minOf(p.x, d.x)
            val hi = if (row) maxOf(p.y, d.y) else maxOf(p.x, d.x)
            if (c in lo..hi) return false
        }
        for (i in 0 until len) {
            val t = at(i, 0)
            val e = ent(t)
            val a = ent(at(i, -1)); val b = ent(at(i, 1))
            if (e == null) {
                // The tiles before and after the gap come to touch: neither may push into the other, and no
                // pipe may meet another pipe or a machine's connection.
                if (a == null || b == null) continue
                fun pointsInto(x: PlacedEntity) = x.outputTile == t || x.kind == Kind.UG_IN && Tile(x.x, x.y) + x.dir == t ||
                    x.kind == Kind.SPLITTER && x.rect.tiles().any { it + x.dir == t }
                if (pointsInto(a) || pointsInto(b)) return false
                if (a.kind in PIPES && (b.kind in PIPES || b.kind == Kind.MACHINE)) return false
                if (b.kind in PIPES && (a.kind == Kind.MACHINE)) return false
                continue
            }
            when (e.kind) {
                Kind.BELT -> {
                    if (e.dir !in across) return false
                    val behind = ent(t + e.dir.opposite); val ahead = ent(t + e.dir)
                    if (behind == null || !behind.kind.beltFamily || behind.dir != e.dir || (behind.kind != Kind.SPLITTER && behind.outputTile != t)) return false
                    if (ahead == null || (ahead.kind != Kind.BELT && ahead.kind != Kind.UG_IN) || ahead.dir != e.dir) return false
                }
                Kind.PIPE -> {
                    if (a == null || b == null || a.kind != Kind.PIPE || b.kind != Kind.PIPE) return false
                    // Nothing beside it along the line that it could be joined to.
                    for (s in listOf(at(i - 1, 0), at(i + 1, 0))) ent(s)?.let { if (it.kind in PIPES || it.kind == Kind.MACHINE) return false }
                }
                else -> return false
            }
        }
        return true
    }

    fun remove(c: Int, row: Boolean) {
        val gone = ents.filter { e -> if (row) e.y == c && e.h == 1 else e.x == c && e.w == 1 }.toSet()
        ents = ents.filter { it !in gone }.map { e ->
            if (row && e.y > c) e.copy(y = e.y - 1) else if (!row && e.x > c) e.copy(x = e.x - 1) else e
        }.toMutableList()
        ground = ground.filter { g -> if (row) g.y != c else g.x != c }.map { g ->
            if (row && g.y > c) g.copy(y = g.y - 1) else if (!row && g.x > c) g.copy(x = g.x - 1) else g
        }
        if (row) { for (i in newY.indices) if (newY[i] > c) newY[i]-- else if (newY[i] == c) newY[i] = c; ch--; rows++ }
        else { for (i in newX.indices) if (newX[i] > c) newX[i]--; cw--; cols++ }
    }

    var changed = true
    while (changed) {
        changed = false
        for (row in listOf(true, false)) {
            var c = 0
            var occ = occupancy()
            while (c < (if (row) ch else cw)) {
                if (removable(c, row, occ)) { remove(c, row); occ = occupancy(); changed = true } else c++
            }
        }
    }
    return Compaction(ents, ground, cw, ch, newX, newY, rows, cols)
}
