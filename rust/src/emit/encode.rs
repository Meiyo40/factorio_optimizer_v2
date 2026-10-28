use anyhow::{Context, Result, ensure};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use std::io::{Read, Write};

/// Blueprint string: version byte "0", then the zlib-compressed JSON in base64.
pub fn compress_and_encode(json: &str) -> String {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(json.as_bytes()).expect("writing to a Vec cannot fail");
    let compressed = encoder.finish().expect("writing to a Vec cannot fail");
    format!("0{}", STANDARD.encode(compressed))
}

/// Inverse of `compress_and_encode`: strips the version byte, base64-decodes and inflates to JSON.
pub fn decode_to_json(blueprint_string: &str) -> Result<String> {
    let trimmed = blueprint_string.trim();
    ensure!(trimmed.starts_with('0'), "Unknown blueprint string version '{}'", trimmed.chars().next().unwrap_or(' '));
    let compressed = STANDARD.decode(&trimmed[1..]).context("Blueprint string is not base64")?;
    let mut json = String::new();
    ZlibDecoder::new(compressed.as_slice()).read_to_string(&mut json).context("Truncated or corrupt blueprint string")?;
    Ok(json)
}
