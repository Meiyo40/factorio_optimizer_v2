package route

/** Binary min-heap of (float key, int value) in flat arrays; duplicates allowed (callers skip stale entries). */
class MinHeap(capacity: Int = 1024) {
    private var keys = FloatArray(capacity)
    private var vals = IntArray(capacity)
    var size = 0
        private set

    fun clear() { size = 0 }
    fun isEmpty() = size == 0

    fun push(key: Float, value: Int) {
        if (size == keys.size) {
            keys = keys.copyOf(size * 2)
            vals = vals.copyOf(size * 2)
        }
        var i = size++
        while (i > 0) {
            val p = (i - 1) ushr 1
            if (keys[p] <= key) break
            keys[i] = keys[p]; vals[i] = vals[p]
            i = p
        }
        keys[i] = key; vals[i] = value
    }

    fun peekKey(): Float = keys[0]

    /** Removes and returns the value with the smallest key. */
    fun pop(): Int {
        val top = vals[0]
        val lastKey = keys[--size]
        val lastVal = vals[size]
        var i = 0
        while (true) {
            val l = 2 * i + 1
            if (l >= size) break
            val r = l + 1
            val c = if (r < size && keys[r] < keys[l]) r else l
            if (keys[c] >= lastKey) break
            keys[i] = keys[c]; vals[i] = vals[c]
            i = c
        }
        if (size > 0) { keys[i] = lastKey; vals[i] = lastVal }
        return top
    }
}
