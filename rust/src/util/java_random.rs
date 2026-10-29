//! `java.util.Random` (a 48-bit linear congruential generator) and `Collections.shuffle`, for the places where the
//! Kotlin code seeds one and the result depends on it (the router's endgame shuffles).

const MULTIPLIER: i64 = 0x5DEECE66D;
const MASK: i64 = (1 << 48) - 1;

pub struct JavaRandom {
    seed: i64,
}

impl JavaRandom {
    pub fn new(seed: i64) -> Self {
        Self { seed: (seed ^ MULTIPLIER) & MASK }
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self.seed.wrapping_mul(MULTIPLIER).wrapping_add(0xB)) & MASK;
        (self.seed >> (48 - bits)) as i32
    }

    pub fn next_int(&mut self) -> i32 {
        self.next(32)
    }

    /// Uniform in `0..bound`: the power-of-two shortcut and the rejection loop of `Random.nextInt(int)`.
    pub fn next_int_bound(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "bound must be positive");
        let mut r = self.next(31);
        let m = bound - 1;
        if bound & m == 0 {
            return ((i64::from(bound) * i64::from(r)) >> 31) as i32;
        }
        let mut u = r;
        loop {
            r = u % bound;
            if u.wrapping_sub(r).wrapping_add(m) >= 0 {
                return r;
            }
            u = self.next(31);
        }
    }
}

/// `Collections.shuffle(list, rnd)` for a random-access list: walk down from the end, swapping each element with a
/// random one at or before it.
pub fn java_shuffle<T>(list: &mut [T], rnd: &mut JavaRandom) {
    for i in (2..=list.len()).rev() {
        list.swap(i - 1, rnd.next_int_bound(i as i32) as usize);
    }
}
