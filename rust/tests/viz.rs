//! The render's own facts, needing no Kotlin pictures: the size formula, the colours that are flat and exact, the
//! per-net and per-chain belt colours (Java's HSB conversion), the legend, the file it writes, and the score graph.

use factorio_optimizer::geom::{Dir, GroundTile, Kind, Layout, PlacedEntity, Tile};
use factorio_optimizer::validate::{ErrorType, ValidationError};
use factorio_optimizer::viz::{PortLabel, RenderOptions, ScorePoint, image_size, render_image, render_layout, score_graph_svg, write_score_graph};
use tiny_skia::Pixmap;

fn entity(name: &str, kind: Kind, x: i32, y: i32, dir: Dir) -> PlacedEntity {
    PlacedEntity::new(name, kind, x, y, dir)
}

fn belt(x: i32, y: i32, dir: Dir, net: Option<i32>) -> PlacedEntity {
    PlacedEntity { net, ..entity("fast-transport-belt", Kind::Belt, x, y, dir) }
}

fn pixel(p: &Pixmap, x: i32, y: i32) -> (u8, u8, u8) {
    let c = p.pixel(x as u32, y as u32).expect("inside the picture");
    (c.red(), c.green(), c.blue())
}

fn near(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
    [(a.0, b.0), (a.1, b.1), (a.2, b.2)].iter().all(|&(p, q)| (i32::from(p) - i32::from(q)).abs() <= 1)
}

fn error(x: i32, y: i32) -> ValidationError {
    ValidationError { kind: ErrorType::Overlap, tile: Tile::new(x, y), message: "two on one tile".to_owned() }
}

fn port(input: bool) -> PortLabel {
    PortLabel { tile: Tile::new(0, 0), item: "iron-plate".to_owned(), rate: 1.5, input, lane: -1, fluid: false }
}

#[test]
fn the_picture_is_the_layout_plus_a_margin_plus_the_legend() {
    let layout = Layout::new(10, 6, vec![entity("steel-chest", Kind::Chest, 2, 2, Dir::N)]);
    // No legend: just the box and a tile of margin on every side.
    assert_eq!(image_size(&layout, &RenderOptions::default()), (12 * 32, 8 * 32 + 8));
    // A title makes it at least 720 * lineH / 14 wide and adds a line; the line is 14 px at 16 px tiles, 28 at 32.
    let titled = |px| RenderOptions { px, title: Some("t".to_owned()), ..RenderOptions::default() };
    assert_eq!(image_size(&layout, &titled(16)), (720, 8 * 16 + 14 + 8));
    assert_eq!(image_size(&layout, &titled(32)), (1440, 8 * 32 + 28 + 8));
    // Errors are listed up to 30, ports all; a wide box beats the legend's minimum.
    let many = RenderOptions { px: 16, errors: (0..40).map(|i| error(i, 0)).collect(), ports: vec![port(true), port(false)], ..RenderOptions::default() };
    assert_eq!(image_size(&layout, &many), (720, 8 * 16 + 32 * 14 + 8));
    let wide = Layout::new(60, 4, vec![]);
    assert_eq!(image_size(&wide, &titled(16)), (62 * 16, 6 * 16 + 14 + 8));
    // Entities outside the box (ring ports, harness) widen the bounds.
    let ring = Layout::new(10, 6, vec![entity("steel-chest", Kind::Chest, -2, 7, Dir::N)]);
    assert_eq!(image_size(&ring, &RenderOptions::default()), (14 * 32, 10 * 32 + 8));
}

#[test]
fn flat_areas_have_their_exact_colours() {
    let marker = GroundTile { name: "hazard-concrete-left".to_owned(), x: 5, y: 1 };
    let layout = Layout {
        tiles: vec![marker],
        ..Layout::new(
            8,
            4,
            vec![
                entity("steel-chest", Kind::Chest, 1, 1, Dir::N),
                entity("infinity-chest", Kind::InfinityChest, 2, 1, Dir::N),
                entity("assembling-machine-2", Kind::Machine, 0, 2, Dir::N),
            ],
        )
    };
    let img = render_image(&layout, &RenderOptions::default()).unwrap();
    let centre = |x: i32, y: i32| pixel(&img, (x + 1) * 32 + 16, (y + 1) * 32 + 16);
    assert_eq!(pixel(&img, 3, 3), (235, 235, 235), "the margin");
    assert_eq!(centre(7, 0), (255, 255, 255), "an empty tile of the box");
    assert_eq!(centre(1, 1), (150, 100, 50), "a chest");
    assert_eq!(centre(2, 1), (230, 90, 200), "an infinity chest");
    assert_eq!(pixel(&img, 6 * 32 + 8, 2 * 32 + 8), (250, 214, 120), "a marker tile");
    // The machine's body, away from its text.
    assert_eq!(pixel(&img, 32 + 90, 3 * 32 + 90), (200, 210, 230), "a machine");
}

#[test]
fn belts_take_their_colour_from_the_net_and_without_nets_from_their_chain() {
    // Java's Color.getHSBColor(0.618..., 0.75, 0.8) for net 1: (51, 96, 204).
    let one = Layout::new(6, 3, vec![belt(1, 1, Dir::E, Some(1))]);
    let img = render_image(&one, &RenderOptions::default()).unwrap();
    assert!(near(pixel(&img, 2 * 32 + 16, 2 * 32 + 16), (51, 96, 204)), "{:?}", pixel(&img, 80, 80));
    // Net 0 is hue 0: (204, 51, 51).
    let zero = Layout::new(6, 3, vec![belt(1, 1, Dir::E, Some(0))]);
    assert!(near(pixel(&render_image(&zero, &RenderOptions::default()).unwrap(), 80, 80), (204, 51, 51)));
    // A belt of no net and no chain is grey; no net ids at all colours by chain: two joined belts alike, a third apart.
    let chains = Layout::new(
        12,
        3,
        vec![belt(1, 1, Dir::E, None), belt(2, 1, Dir::E, None), belt(8, 1, Dir::E, None)],
    );
    let img = render_image(&chains, &RenderOptions::default()).unwrap();
    let at = |x: i32| pixel(&img, (x + 1) * 32 + 16, 2 * 32 + 16);
    assert_eq!(at(1), at(2), "one chain");
    assert_ne!(at(1), at(8), "another chain");
    assert_ne!(at(1), (110, 110, 110));
}

#[test]
fn the_legend_lists_the_title_the_ports_and_the_errors_in_their_colours() {
    let layout = Layout::new(6, 3, vec![entity("steel-chest", Kind::Chest, 1, 1, Dir::N)]);
    let opt = RenderOptions { px: 16, title: Some("a title".to_owned()), ports: vec![port(true), port(false)], errors: vec![error(2, 1)], ..RenderOptions::default() };
    let img = render_image(&layout, &opt).unwrap();
    let map_bottom = 5 * 16;
    // The pixels of a legend line (the lines are 14 px apart under the map) that satisfy `is`.
    let in_line = |n: i32, is: &dyn Fn((u8, u8, u8)) -> bool| -> usize {
        (0..img.width() as i32).flat_map(|x| (map_bottom + n * 14..map_bottom + (n + 1) * 14).map(move |y| (x, y))).filter(|&(x, y)| is(pixel(&img, x, y))).count()
    };
    let dark = |c: (u8, u8, u8)| c.0 < 170 && c.1 < 170 && c.2 < 170;
    let green = |c: (u8, u8, u8)| i32::from(c.1) > i32::from(c.0) + 40 && i32::from(c.1) > i32::from(c.2) + 40;
    let red = |c: (u8, u8, u8)| i32::from(c.0) > i32::from(c.1) + 60 && i32::from(c.0) > i32::from(c.2) + 60;
    assert!(in_line(0, &dark) > 15, "the title is dark text");
    assert!(in_line(1, &green) > 15, "the input line is green");
    assert!(in_line(2, &red) > 15, "the output line is red");
    assert!(in_line(3, &dark) > 15, "the error line is dark text");
    // The tile of the error is ringed in red on the map.
    assert!((0..16).any(|d| near(pixel(&img, 3 * 16 + d, 2 * 16 - 1), (230, 0, 0)) || near(pixel(&img, 3 * 16 + d, 2 * 16), (230, 0, 0))), "the error square");
}

#[test]
fn a_render_goes_to_a_png_file_in_a_new_folder() {
    let layout = Layout::new(6, 3, vec![entity("steel-chest", Kind::Chest, 1, 1, Dir::N)]);
    let dir = std::env::temp_dir().join(format!("factorio-viz-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("deeper/best.png");
    render_layout(&layout, &file, &RenderOptions::default()).unwrap();
    let back = Pixmap::load_png(&file).unwrap();
    assert_eq!((back.width(), back.height()), (8 * 32, 5 * 32 + 8));
    assert_eq!(pixel(&back, 2 * 32 + 16, 2 * 32 + 16), (150, 100, 50));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_score_graph_has_a_point_per_score_and_a_line_through_them() {
    let points = [ScorePoint { timestamp: 1_000, score: 900.0 }, ScorePoint { timestamp: 3_000, score: 500.0 }, ScorePoint { timestamp: 11_000, score: 420.5 }];
    let svg = score_graph_svg(&points, "Score <area> & more");
    assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
    assert_eq!(svg.matches("<circle").count(), 3);
    assert!(svg.contains("Score over Time") && svg.contains("Time Elapsed (seconds)") && svg.contains("Score &lt;area&gt; &amp; more"));
    let line = svg.lines().find(|l| l.starts_with("<polyline")).unwrap();
    assert_eq!(line.split("points=\"").nth(1).unwrap().split('"').next().unwrap().split(' ').count(), 3);
    // The best score is the lowest point of the chart, the first the highest.
    let ys: Vec<f64> = svg.lines().filter(|l| l.starts_with("<circle")).map(|l| l.split("cy=\"").nth(1).unwrap().split('"').next().unwrap().parse().unwrap()).collect();
    assert!(ys[0] < ys[1] && ys[1] < ys[2], "a lower score is drawn lower: {ys:?}");

    let dir = std::env::temp_dir().join(format!("factorio-score-graph-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    write_score_graph(&[], &dir, "y").unwrap();
    assert!(!dir.join("score_vs_time.svg").exists(), "nothing to plot, nothing written");
    write_score_graph(&points, &dir, "y").unwrap();
    assert!(dir.join("score_vs_time.svg").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
