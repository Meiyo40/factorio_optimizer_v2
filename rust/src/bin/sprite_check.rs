//! Prints where the ink is in the game's belt sheet: for the first frame of the first few direction rows, the
//! bounding box of the non-transparent pixels. Built for one question — which direction a row is, and how much of the
//! 2x2-tile frame the belt actually fills — and kept because a wrong belt frame is impossible to spot by eye in code.
//!
//! ```sh
//! cargo run --features gui --bin sprite_check -- "E:\SteamLibrary\steamapps\common\Factorio" transport-belt
//! ```

#[cfg(not(feature = "gui"))]
fn main() {
    println!("this needs the gui feature, for the png decoder: cargo run --features gui --bin sprite_check");
}

#[cfg(feature = "gui")]
fn main() -> anyhow::Result<()> {
    use anyhow::Context;

    let mut args = std::env::args().skip(1);
    let root = args.next().unwrap_or_else(|| r"E:\SteamLibrary\steamapps\common\Factorio".to_owned());
    let entity = args.next().unwrap_or_else(|| "transport-belt".to_owned());
    let table: serde_json::Value = serde_json::from_str(include_str!("../../sprite-table.json")).context("the sprite table parses")?;
    let entry = table["entities"].get(&entity).with_context(|| format!("no entry for {entity}"))?;
    let layer = entry["sides"][0][0].as_object().context("a first layer")?;
    let file = layer["file"].as_str().context("a file")?;
    let frame_w = layer["w"].as_f64().context("a width")? as u32;
    let frame_h = layer["h"].as_f64().context("a height")? as u32;
    let columns = layer["columns"].as_f64().context("columns")? as u32;
    let frames = layer["frame_count"].as_f64().context("frames")? as u32;
    let scale = layer["scale"].as_f64().context("scale")?;
    println!("{entity}: {file} frame {frame_w}x{frame_h} columns {columns} frames {frames} scale {scale}");

    let path = std::path::Path::new(&root).join("data").join(file);
    let image = image::open(&path).with_context(|| format!("could not open {}", path.display()))?;
    let rgba = image.to_rgba8();
    println!("sheet {}x{}", rgba.width(), rgba.height());
    for row in 0..5 {
        let (sx, sy) = (0, row * frame_h);
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for y in 0..frame_h {
            for x in 0..frame_w {
                let pixel = rgba.get_pixel(sx + x, sy + y);
                if pixel[3] > 8 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        if x0 == u32::MAX {
            println!("  row {row}: empty");
            continue;
        }
        let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
        let tiles_w = f64::from(w) * scale / 32.0;
        let tiles_h = f64::from(h) * scale / 32.0;
        println!(
            "  row {row}: ink {w}x{h} px at ({x0},{y0}) = {tiles_w:.2}x{tiles_h:.2} tiles, offsets {:.2},{:.2} tiles",
            f64::from(x0) * scale / 32.0,
            f64::from(y0) * scale / 32.0
        );
    }
    Ok(())
}
