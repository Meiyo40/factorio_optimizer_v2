pub mod java_hash;
pub mod java_random;
pub mod json;
pub mod splittable_random;
pub mod text;

/// Kotlin's `sum()` and `sumOf` on doubles: left to right from +0.0. Rust's `Iterator::sum` starts from -0.0, so an
/// empty sum would print as "-0.00".
pub fn sum_f64(values: impl IntoIterator<Item = f64>) -> f64 {
    values.into_iter().fold(0.0, |total, v| total + v)
}

/// Kotlin's `Iterable<Double>.average()`: the sum from +0.0 over the count, and (positive) NaN for nothing at all.
pub fn average_f64(values: impl IntoIterator<Item = f64>) -> f64 {
    let (mut sum, mut count) = (0.0, 0usize);
    for v in values {
        sum += v;
        count += 1;
    }
    if count == 0 { f64::NAN } else { sum / count as f64 }
}

/// `Double.compareTo`, which sorts use: a total order with -0.0 below 0.0 and every NaN above everything, equal to each other.
/// Rust's `total_cmp` would put a NaN with the sign bit set (what 0.0 / 0.0 yields on x86) below everything.
pub fn compare_f64(a: f64, b: f64) -> std::cmp::Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        (false, false) => a.total_cmp(&b),
    }
}

/// `Float.compareTo`, see [`compare_f64`].
pub fn compare_f32(a: f32, b: f32) -> std::cmp::Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        (false, false) => a.total_cmp(&b),
    }
}
