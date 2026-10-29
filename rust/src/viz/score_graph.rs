//! The score-over-time graph of a search: `score_vs_time.svg` next to its other outputs. Kotlin plots it with
//! lets-plot; this is a plain SVG line chart with the same title, axis labels and file name.

use anyhow::{Context, Result};
use std::path::Path;

/// A score at a wall-clock time (milliseconds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScorePoint {
    pub timestamp: i64,
    pub score: f64,
}

const WIDTH: f64 = 720.0;
const HEIGHT: f64 = 440.0;
const LEFT: f64 = 80.0;
const RIGHT: f64 = 24.0;
const TOP: f64 = 56.0;
const BOTTOM: f64 = 64.0;

/// The chart as SVG text: time since the first point along x, the score up y, a line through every point.
pub fn score_graph_svg(points: &[ScorePoint], y_label: &str) -> String {
    let start = points.iter().map(|p| p.timestamp).min().unwrap_or(0);
    let xs: Vec<f64> = points.iter().map(|p| (p.timestamp - start) as f64 / 1000.0).collect();
    let (x_max, y_min, y_max) = (
        xs.iter().copied().fold(0.0, f64::max).max(1e-9),
        points.iter().map(|p| p.score).fold(f64::INFINITY, f64::min),
        points.iter().map(|p| p.score).fold(f64::NEG_INFINITY, f64::max),
    );
    let y_span = (y_max - y_min).max(1e-9);
    let at_x = |x: f64| LEFT + x / x_max * (WIDTH - LEFT - RIGHT);
    let at_y = |y: f64| HEIGHT - BOTTOM - (y - y_min) / y_span * (HEIGHT - TOP - BOTTOM);
    let mut svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{WIDTH}\" height=\"{HEIGHT}\" viewBox=\"0 0 {WIDTH} {HEIGHT}\" font-family=\"sans-serif\">\n");
    svg += &format!("<rect width=\"{WIDTH}\" height=\"{HEIGHT}\" fill=\"white\"/>\n");
    svg += &format!("<text x=\"{LEFT}\" y=\"30\" font-size=\"20\">Score over Time</text>\n");
    svg += &format!(
        "<line x1=\"{LEFT}\" y1=\"{y}\" x2=\"{x2}\" y2=\"{y}\" stroke=\"#444\"/>\n<line x1=\"{LEFT}\" y1=\"{TOP}\" x2=\"{LEFT}\" y2=\"{y}\" stroke=\"#444\"/>\n",
        y = HEIGHT - BOTTOM,
        x2 = WIDTH - RIGHT
    );
    // Five labelled ticks on each axis.
    for i in 0..=4 {
        let f = f64::from(i) / 4.0;
        let (x, y) = (x_max * f, y_min + y_span * f);
        svg += &format!("<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"12\" text-anchor=\"middle\">{x:.0}</text>\n", at_x(x), HEIGHT - BOTTOM + 18.0);
        svg += &format!("<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"12\" text-anchor=\"end\">{y:.1}</text>\n", LEFT - 8.0, at_y(y) + 4.0);
    }
    svg += &format!("<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"13\" text-anchor=\"middle\">Time Elapsed (seconds)</text>\n", (LEFT + WIDTH - RIGHT) / 2.0, HEIGHT - 14.0);
    svg += &format!("<text transform=\"translate(18 {:.1}) rotate(-90)\" font-size=\"13\" text-anchor=\"middle\">{}</text>\n", (TOP + HEIGHT - BOTTOM) / 2.0, escape(y_label));
    let line: Vec<String> = points.iter().zip(&xs).map(|(p, &x)| format!("{:.1},{:.1}", at_x(x), at_y(p.score))).collect();
    svg += &format!("<polyline points=\"{}\" fill=\"none\" stroke=\"#1f77b4\" stroke-width=\"2\" opacity=\"0.7\"/>\n", line.join(" "));
    for (p, &x) in points.iter().zip(&xs) {
        svg += &format!("<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"3\" fill=\"#1f77b4\"/>\n", at_x(x), at_y(p.score));
    }
    svg + "</svg>\n"
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Writes `score_vs_time.svg` into `dir`; nothing for an empty list (the Kotlin graph says so and returns).
pub fn write_score_graph(points: &[ScorePoint], dir: &Path, y_label: &str) -> Result<()> {
    if points.is_empty() {
        return Ok(());
    }
    let path = dir.join("score_vs_time.svg");
    std::fs::write(&path, score_graph_svg(points, y_label)).with_context(|| format!("Could not write {}", path.display()))
}
