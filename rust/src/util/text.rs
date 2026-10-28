//! Text the way the Kotlin (Java) code prints it, for messages that must match.

use std::fmt::Display;

/// `"%.2f".format(d)`. Java's Formatter rounds half up on the shortest decimal digits of the double (the ones
/// `Double.toString` shows), so 0.285 gives 0.29. C and Rust round the exact binary value instead, which is
/// slightly below 0.285 and gives 0.28.
pub fn fmt2(d: f64) -> String {
    if !d.is_finite() {
        return format!("{d}");
    }
    let shortest = format!("{}", d.abs()); // the shortest decimal that round-trips, never in exponent form
    let (whole, frac) = shortest.split_once('.').unwrap_or((&shortest, ""));
    let digits: Vec<u8> = frac.bytes().map(|b| b - b'0').collect();
    let digit = |i: usize| digits.get(i).copied().unwrap_or(0);
    let Ok(whole) = whole.parse::<u128>() else { return format!("{d:.2}") };
    let mut hundredths = whole * 100 + u128::from(digit(0)) * 10 + u128::from(digit(1));
    if digit(2) >= 5 {
        hundredths += 1;
    }
    let sign = if d.is_sign_negative() { "-" } else { "" }; // Java keeps it: -0.001 prints -0.00
    format!("{sign}{}.{:02}", hundredths / 100, hundredths % 100)
}

/// Kotlin's `List.toString()`: `[a, b, c]`.
pub fn fmt_list<T: Display>(items: impl IntoIterator<Item = T>) -> String {
    let parts: Vec<String> = items.into_iter().map(|i| i.to_string()).collect();
    format!("[{}]", parts.join(", "))
}
