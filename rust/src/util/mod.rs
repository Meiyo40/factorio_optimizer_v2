pub mod java_hash;
pub mod json;
pub mod splittable_random;
pub mod text;

/// Kotlin's `sum()` and `sumOf` on doubles: left to right from +0.0. Rust's `Iterator::sum` starts from -0.0, so an
/// empty sum would print as "-0.00".
pub fn sum_f64(values: impl IntoIterator<Item = f64>) -> f64 {
    values.into_iter().fold(0.0, |total, v| total + v)
}
