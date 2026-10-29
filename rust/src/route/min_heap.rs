/// Binary min-heap of (f32 key, i32 value) in flat arrays; duplicates are allowed (callers skip stale entries).
/// Ties are decided by the sift rules alone (no sequence number), so they are copied from Kotlin exactly:
/// `<=` stops a push, and a pop takes the right child only when it is strictly smaller.
pub struct MinHeap {
    keys: Vec<f32>,
    vals: Vec<i32>,
}

impl MinHeap {
    pub fn new(capacity: usize) -> Self {
        Self { keys: Vec::with_capacity(capacity), vals: Vec::with_capacity(capacity) }
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.vals.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn push(&mut self, key: f32, value: i32) {
        let mut i = self.keys.len();
        self.keys.push(key);
        self.vals.push(value);
        while i > 0 {
            let p = (i - 1) >> 1;
            if self.keys[p] <= key {
                break;
            }
            self.keys[i] = self.keys[p];
            self.vals[i] = self.vals[p];
            i = p;
        }
        self.keys[i] = key;
        self.vals[i] = value;
    }

    /// Removes and returns the value with the smallest key. The heap must not be empty.
    pub fn pop(&mut self) -> i32 {
        let top = self.vals[0];
        let last_key = self.keys.pop().expect("pop on an empty heap");
        let last_val = self.vals.pop().expect("pop on an empty heap");
        let size = self.keys.len();
        let mut i = 0;
        loop {
            let l = 2 * i + 1;
            if l >= size {
                break;
            }
            let r = l + 1;
            let c = if r < size && self.keys[r] < self.keys[l] { r } else { l };
            if self.keys[c] >= last_key {
                break;
            }
            self.keys[i] = self.keys[c];
            self.vals[i] = self.vals[c];
            i = c;
        }
        if size > 0 {
            self.keys[i] = last_key;
            self.vals[i] = last_val;
        }
        top
    }
}
