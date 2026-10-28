use serde::Serialize;

/// Pretty-printed with four spaces, like kotlinx's `prettyPrint` and the committed tuning.json.
pub fn to_pretty_string<T: Serialize>(value: &T) -> String {
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    value.serialize(&mut serde_json::Serializer::with_formatter(&mut out, formatter)).expect("the value serializes");
    String::from_utf8(out).expect("JSON is UTF-8")
}
