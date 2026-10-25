import blocks.compactLayout
import geom.Dir
import geom.Kind
import geom.PlacedEntity
import kotlin.test.Test
import kotlin.test.assertEquals

class CompactTest {
    private fun belt(x: Int, y: Int, d: Dir) = PlacedEntity("fast-transport-belt", Kind.BELT, x, y, d)

    @Test
    fun `rows a belt only passes straight through go, and empty columns beside it`() {
        // A belt down column 1 of a 3 x 6 box: its first and last tiles stay (fed from nothing, feeding nothing).
        val c = compactLayout((0..5).map { belt(1, it, Dir.S) }, emptyList(), 3, 6)
        assertEquals(2, c.h)
        assertEquals(1, c.w)
        assertEquals(listOf(0 to 0, 0 to 1), c.entities.map { it.x to it.y })
        assertEquals(1, c.y(5)); assertEquals(0, c.x(1))
    }

    @Test
    fun `a gap a belt points into stays, so it cannot start feeding what lies beyond`() {
        // A belt ending above an empty row, another belt below it: closing the row would join them.
        val ents = listOf(belt(0, 0, Dir.S), belt(0, 2, Dir.E), belt(1, 2, Dir.E))
        val c = compactLayout(ents, emptyList(), 2, 3)
        assertEquals(3, c.h)
    }

    @Test
    fun `no row goes that an inserter reaches across`() {
        val ents = listOf(
            PlacedEntity("steel-chest", Kind.CHEST, 0, 0),
            PlacedEntity("long-handed-inserter", Kind.INSERTER, 0, 2, Dir.N),   // picks at (0,0), drops at (0,4)
            PlacedEntity("steel-chest", Kind.CHEST, 0, 4),
        )
        val c = compactLayout(ents, emptyList(), 1, 5)
        assertEquals(5, c.h)
    }
}
