//! `java.util.SplittableRandom` with the JDK 17+ bounded-draw algorithms, so a seed gives the JVM's draws.
//! Only the calls the Kotlin code makes are ported (no `split`).

const GOLDEN_GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;
const DOUBLE_UNIT: f64 = 1.0 / (1u64 << 53) as f64;

fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn mix32(mut z: u64) -> i32 {
    z = (z ^ (z >> 33)).wrapping_mul(0x62a9_d9ed_7997_05f5);
    ((z ^ (z >> 28)).wrapping_mul(0xcb24_d0a5_c88c_35b3) >> 32) as i32
}

#[derive(Clone, Debug)]
pub struct SplittableRandom {
    seed: u64,
}

impl SplittableRandom {
    pub fn new(seed: i64) -> Self {
        Self { seed: seed as u64 }
    }

    fn next_seed(&mut self) -> u64 {
        self.seed = self.seed.wrapping_add(GOLDEN_GAMMA);
        self.seed
    }

    pub fn next_int(&mut self) -> i32 {
        mix32(self.next_seed())
    }

    pub fn next_long(&mut self) -> i64 {
        mix64(self.next_seed()) as i64
    }

    pub fn next_double(&mut self) -> f64 {
        (mix64(self.next_seed()) >> 11) as f64 * DOUBLE_UNIT
    }

    pub fn next_boolean(&mut self) -> bool {
        self.next_int() < 0
    }

    /// Uniform in `0 until bound`.
    pub fn next_int_bound(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "bound must be positive");
        let m = bound - 1;
        let mut r = self.next_int();
        if bound & m == 0 {
            return r & m;
        }
        let mut u = ((r as u32) >> 1) as i32;
        loop {
            r = u % bound;
            if u.wrapping_add(m).wrapping_sub(r) >= 0 {
                return r;
            }
            u = ((self.next_int() as u32) >> 1) as i32;
        }
    }

    /// Uniform in `origin until bound`.
    pub fn next_int_range(&mut self, origin: i32, bound: i32) -> i32 {
        assert!(origin < bound, "bound must be greater than origin");
        let mut r = self.next_int();
        let n = bound.wrapping_sub(origin);
        let m = n.wrapping_sub(1);
        if n & m == 0 {
            (r & m) + origin
        } else if n > 0 {
            let mut u = ((r as u32) >> 1) as i32;
            loop {
                r = u % n;
                if u.wrapping_add(m).wrapping_sub(r) >= 0 {
                    return r + origin;
                }
                u = ((self.next_int() as u32) >> 1) as i32;
            }
        } else {
            while r < origin || r >= bound {
                r = self.next_int();
            }
            r
        }
    }
}
