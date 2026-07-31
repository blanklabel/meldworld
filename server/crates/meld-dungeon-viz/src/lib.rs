//! Authored-dungeon visualizer (DG-6a).
//!
//! Renders a [`DungeonDef`] to a self-contained top-down **SVG** — every floor as a
//! glyph grid with the walls, entrance, exit, stairs, traps, levers/plates, doors/
//! gates, keys, boss, and treasure marked and labelled — so a designer or agent can
//! *see* what they authored, and eyeball the critical path, without running the
//! game.
//!
//! This is the honest, buildable slice of DG-6: real rendering of exactly the
//! elements the in-game view shows, but from the compiled `DungeonDef` rather than a
//! live server (which needs DG-3b's wire surface, pending SC-3). It doubles as the
//! **reference the Bevy renderer (DG-6b) will match**. Pure: `DungeonDef` → `String`.
//!
//! ```
//! let d = meld_dungeon_content::by_name("verdant_barrow").unwrap();
//! let svg = meld_dungeon_viz::to_svg(d);
//! assert!(svg.starts_with("<svg"));
//! ```

use std::collections::HashMap;
use std::fmt::Write;

use meld_dungeon_content::{DungeonDef, ObjectKind, StairDir};

const CELL: usize = 30;
const MARGIN: usize = 24;
const FLOOR_GAP: usize = 44;
const HEADER: usize = 68;

/// What to draw on top of a cell: a coloured chip + a short glyph + the object id.
struct Marker {
    glyph: &'static str,
    fill: &'static str,
    id: String,
}

/// Render `def` to a standalone SVG document (light + dark friendly via a neutral
/// palette). Floors are stacked top-to-bottom, floor 0 first.
pub fn to_svg(def: &DungeonDef) -> String {
    let markers = collect_markers(def);

    let grid_w = def.grids.iter().map(|g| g.width).max().unwrap_or(0);
    let width = MARGIN * 2 + grid_w * CELL;
    let width = width.max(360);

    // Vertical layout: header, then each floor (label + grid), then a legend.
    let mut body = String::new();
    let mut y = HEADER;
    for (fi, g) in def.grids.iter().enumerate() {
        let _ = write!(
            body,
            r#"<text x="{}" y="{}" class="flabel">Floor {}</text>"#,
            MARGIN,
            y - 8,
            fi
        );
        for cy in 0..g.height {
            for cx in 0..g.width {
                let cell = g.at(cx, cy);
                let px = MARGIN + cx * CELL;
                let py = y + cy * CELL;
                draw_cell(&mut body, cell, px, py);
                if let Some(m) = markers.get(&(fi, cx, cy)) {
                    draw_marker(&mut body, m, px, py);
                }
            }
        }
        y += g.height * CELL + FLOOR_GAP;
    }

    let legend_h = draw_legend(&mut body, MARGIN, y, width - MARGIN * 2);
    let height = y + legend_h + MARGIN;

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" font-family="ui-sans-serif, system-ui, sans-serif">
<style>
  .bg {{ fill: #f4f1ea; }}
  .wall {{ fill: #2b2f3a; }}
  .floor {{ fill: #e8e4d8; }}
  .grid {{ stroke: #cfc9ba; stroke-width: 0.5; }}
  .title {{ fill: #1c1f26; font-size: 20px; font-weight: 700; }}
  .subtitle {{ fill: #6b6456; font-size: 12px; }}
  .flabel {{ fill: #4a4636; font-size: 13px; font-weight: 600; }}
  .mid {{ fill: #ffffff; font-size: 12px; font-weight: 700; text-anchor: middle; dominant-baseline: central; }}
  .oid {{ fill: #3a3630; font-size: 8px; text-anchor: middle; }}
  .lg {{ fill: #3a3630; font-size: 11px; dominant-baseline: central; }}
</style>
<rect class="bg" x="0" y="0" width="{width}" height="{height}"/>
<text class="title" x="{MARGIN}" y="26">{name}</text>
<text class="subtitle" x="{MARGIN}" y="44">biome: {biome}  ·  {floors} floor(s)  ·  authored dungeon (WG-1)</text>
{body}
</svg>
"##,
        name = esc(&def.name),
        biome = esc(&def.biome),
        floors = def.grids.len(),
    )
}

fn draw_cell(out: &mut String, cell: &meld_dungeon_content::Cell, px: usize, py: usize) {
    use meld_dungeon_content::Tile::*;
    let class = match cell.tile {
        Wall => "wall",
        Floor => "floor",
        Void => return, // void = nothing drawn (negative space)
    };
    let _ = write!(
        out,
        r#"<rect class="{class} grid" x="{px}" y="{py}" width="{CELL}" height="{CELL}"/>"#
    );
}

fn draw_marker(out: &mut String, m: &Marker, px: usize, py: usize) {
    let cx = px + CELL / 2;
    let cy = py + CELL / 2;
    let r = CELL * 36 / 100;
    let _ = write!(
        out,
        r##"<circle cx="{cx}" cy="{cy}" r="{r}" fill="{fill}" stroke="#1c1f26" stroke-width="1"/>"##,
        fill = m.fill
    );
    let _ = write!(out, r#"<text class="mid" x="{cx}" y="{cy}">{}</text>"#, esc(m.glyph));
    let _ = write!(
        out,
        r#"<text class="oid" x="{cx}" y="{ly}">{}</text>"#,
        esc(&m.id),
        ly = py + CELL - 2
    );
}

/// The chip style for each object kind (+ entrance/exit + stair direction).
fn style(kind: &ObjectKind, dir: Option<StairDir>) -> (&'static str, &'static str) {
    match kind {
        ObjectKind::Lever => ("L", "#e08a1e"),
        ObjectKind::Plate { .. } => ("P", "#c8a415"),
        ObjectKind::Key => ("K", "#b8860b"),
        ObjectKind::Pedestal => ("Pd", "#a1785a"),
        ObjectKind::Trap { .. } => ("T", "#c0392b"),
        ObjectKind::Door { .. } => ("D", "#8a5a2b"),
        ObjectKind::Gate { .. } => ("G", "#6b4423"),
        ObjectKind::Boss { .. } => ("B", "#7b2d8e"),
        ObjectKind::Chest { .. } => ("$", "#148f77"),
        ObjectKind::Stair => match dir {
            Some(StairDir::Up) => ("▲", "#2d6cdf"),
            _ => ("▼", "#2d6cdf"),
        },
    }
}

fn collect_markers(def: &DungeonDef) -> HashMap<(usize, usize, usize), Marker> {
    let mut m = HashMap::new();
    for e in &def.entrances {
        m.insert((e.floor, e.x, e.y), Marker { glyph: "IN", fill: "#3aa35a", id: String::new() });
    }
    for e in &def.exits {
        m.insert((e.floor, e.x, e.y), Marker { glyph: "OUT", fill: "#c0392b", id: String::new() });
    }
    for p in &def.placements {
        if let Some(kind) = def.objects.get(&p.id) {
            let (glyph, fill) = style(kind, p.dir);
            m.insert((p.floor, p.x, p.y), Marker { glyph, fill, id: p.id.clone() });
        }
    }
    m
}

/// The full legend of marker kinds, wrapped to the available width. Returns the
/// vertical space it consumed.
fn draw_legend(out: &mut String, x: usize, y: usize, w: usize) -> usize {
    const ITEMS: [(&str, &str, &str); 12] = [
        ("IN", "#3aa35a", "entrance"),
        ("OUT", "#c0392b", "exit"),
        ("▼", "#2d6cdf", "stairs"),
        ("L", "#e08a1e", "lever"),
        ("P", "#c8a415", "plate"),
        ("K", "#b8860b", "key"),
        ("Pd", "#a1785a", "pedestal"),
        ("T", "#c0392b", "trap"),
        ("D", "#8a5a2b", "door"),
        ("G", "#6b4423", "gate"),
        ("B", "#7b2d8e", "boss"),
        ("$", "#148f77", "chest"),
    ];
    let _ = write!(out, r#"<text class="flabel" x="{x}" y="{y}">Legend</text>"#);
    let (mut lx, mut ly) = (x, y + 18);
    for (glyph, fill, label) in ITEMS {
        if lx + 120 > x + w.max(120) {
            lx = x;
            ly += 24;
        }
        let _ = write!(
            out,
            r##"<circle cx="{}" cy="{}" r="8" fill="{fill}" stroke="#1c1f26" stroke-width="1"/><text x="{}" y="{}" style="font-size:9px;font-weight:700;fill:#fff;text-anchor:middle;dominant-baseline:central">{}</text><text class="lg" x="{}" y="{}">{}</text>"##,
            lx + 8, ly, lx + 8, ly, esc(glyph), lx + 22, ly, esc(label)
        );
        lx += 120;
    }
    (ly - y) + 24
}

/// Minimal XML text escaping.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_valid_svg_for_the_reference_dungeon() {
        let d = meld_dungeon_content::by_name("verdant_barrow").unwrap();
        let svg = to_svg(d);
        assert!(svg.starts_with("<svg"));
        assert!(svg.trim_end().ends_with("</svg>"));
        assert!(svg.contains("verdant_barrow"));
        assert!(svg.contains(">IN<"), "entrance is marked");
        assert!(svg.contains(">OUT<"), "exit is marked");
        assert!(svg.contains("Floor 0") && svg.contains("Floor 1"), "both floors drawn");
    }

    #[test]
    fn every_embedded_dungeon_renders() {
        for d in meld_dungeon_content::all() {
            let svg = to_svg(d);
            assert!(svg.contains(&d.name), "renders {}", d.name);
            assert!(svg.len() > 500);
        }
    }
}
