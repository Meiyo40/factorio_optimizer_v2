package geom

import kotlinx.serialization.Serializable

/** Cardinal direction. [value16] is the 2.0 blueprint value (16-way defines.direction: N 0, E 4, S 8, W 12). */
enum class Dir(val dx: Int, val dy: Int, val value16: Int) {
    N(0, -1, 0), E(1, 0, 4), S(0, 1, 8), W(-1, 0, 12);

    val opposite: Dir get() = entries[(ordinal + 2) % 4]
    val cw: Dir get() = entries[(ordinal + 1) % 4]
    val ccw: Dir get() = entries[(ordinal + 3) % 4]
    /** 0 for E/W travel, 1 for N/S travel. */
    val axis: Int get() = if (dx != 0) 0 else 1

    companion object {
        fun from16(value: Int): Dir = entries.firstOrNull { it.value16 == value }
            ?: error("Direction $value is not cardinal")
        fun fromName(s: String): Dir = when (s.lowercase()) {
            "n", "north" -> N; "e", "east" -> E; "s", "south" -> S; "w", "west" -> W
            else -> error("Unknown direction '$s'")
        }
    }
}

@Serializable
data class Tile(val x: Int, val y: Int) {
    operator fun plus(d: Dir) = Tile(x + d.dx, y + d.dy)
    fun step(d: Dir, n: Int) = Tile(x + d.dx * n, y + d.dy * n)
    fun manhattan(o: Tile) = kotlin.math.abs(x - o.x) + kotlin.math.abs(y - o.y)
    override fun toString() = "($x,$y)"
}

/** Axis-aligned rectangle of tiles with top-left (x, y). */
@Serializable
data class Rect(val x: Int, val y: Int, val w: Int, val h: Int) {
    val x2: Int get() = x + w - 1
    val y2: Int get() = y + h - 1
    operator fun contains(t: Tile) = t.x in x..x2 && t.y in y..y2
    fun contains(tx: Int, ty: Int) = tx in x..x2 && ty in y..y2
    fun overlaps(o: Rect) = x <= o.x2 && o.x <= x2 && y <= o.y2 && o.y <= y2
    fun tiles(): List<Tile> = (y..y2).flatMap { ty -> (x..x2).map { tx -> Tile(tx, ty) } }

    /**
     * Tiles at distance [d] outside side [side] (d = 1 is the first row outside), along the whole side.
     * Corners are not included.
     */
    fun sideTiles(side: Dir, d: Int): List<Tile> = when (side) {
        Dir.N -> (x..x2).map { Tile(it, y - d) }
        Dir.S -> (x..x2).map { Tile(it, y2 + d) }
        Dir.W -> (y..y2).map { Tile(x - d, it) }
        Dir.E -> (y..y2).map { Tile(x2 + d, it) }
    }

    /** The tile of this rectangle's edge row on [side] that lies next to the outside tile [outside]. */
    fun edgeTileFacing(side: Dir, outside: Tile): Tile = when (side) {
        Dir.N -> Tile(outside.x, y)
        Dir.S -> Tile(outside.x, y2)
        Dir.W -> Tile(x, outside.y)
        Dir.E -> Tile(x2, outside.y)
    }

    /** Middle tile at distance [d] outside [side] (for odd sizes). */
    fun sideMiddle(side: Dir, d: Int): Tile = when (side) {
        Dir.N -> Tile(x + w / 2, y - d)
        Dir.S -> Tile(x + w / 2, y2 + d)
        Dir.W -> Tile(x - d, y + h / 2)
        Dir.E -> Tile(x2 + d, y + h / 2)
    }
}

/**
 * Index arithmetic for a box of [w] x [h] tiles surrounded by a ring of [margin] tiles. Box tiles have
 * coordinates 0 until w / 0 until h; ring tiles have negative or >= w/h coordinates.
 */
class GridDims(val w: Int, val h: Int, val margin: Int) {
    val fullW = w + 2 * margin
    val fullH = h + 2 * margin
    val size = fullW * fullH

    fun inFull(x: Int, y: Int) = x >= -margin && y >= -margin && x < w + margin && y < h + margin
    fun inBox(x: Int, y: Int) = x >= 0 && y >= 0 && x < w && y < h
    fun idx(x: Int, y: Int) = (y + margin) * fullW + (x + margin)
    fun idx(t: Tile) = idx(t.x, t.y)
    fun x(i: Int) = i % fullW - margin
    fun y(i: Int) = i / fullW - margin
    fun tile(i: Int) = Tile(x(i), y(i))
}
