//! Text the way the Kotlin (Java) code prints it, for messages that must match.

use std::fmt::Display;

/// `"%.{digits}f".format(d)`. Java's Formatter rounds half up on the shortest decimal digits of the double (the ones
/// `Double.toString` shows), so 0.285 gives 0.29 and 12.5 gives 13. C and Rust round the exact binary value instead:
/// 0.285 is slightly below, and 12.5 goes to even.
pub fn fmt_fixed(d: f64, digits: usize) -> String {
    if !d.is_finite() {
        return format!("{d}");
    }
    let shortest = format!("{}", d.abs()); // the shortest decimal that round-trips, never in exponent form
    let (whole, frac) = shortest.split_once('.').unwrap_or((&shortest, ""));
    let digit = |i: usize| frac.as_bytes().get(i).map_or(0, |b| b - b'0');
    let Ok(whole) = whole.parse::<u128>() else { return format!("{d:.digits$}") };
    let unit = 10u128.pow(digits as u32);
    let mut scaled = whole * unit + (0..digits).fold(0u128, |acc, i| acc * 10 + u128::from(digit(i)));
    if digit(digits) >= 5 {
        scaled += 1;
    }
    let sign = if d.is_sign_negative() { "-" } else { "" }; // Java keeps it: -0.001 prints -0.00
    if digits == 0 { format!("{sign}{scaled}") } else { format!("{sign}{}.{:0digits$}", scaled / unit, scaled % unit) }
}

/// `"%.2f".format(d)`, see [`fmt_fixed`].
pub fn fmt2(d: f64) -> String {
    fmt_fixed(d, 2)
}

/// Kotlin's `List.toString()`: `[a, b, c]`.
pub fn fmt_list<T: Display>(items: impl IntoIterator<Item = T>) -> String {
    let parts: Vec<String> = items.into_iter().map(|i| i.to_string()).collect();
    format!("[{}]", parts.join(", "))
}
