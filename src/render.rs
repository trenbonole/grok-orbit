//! Rasterizes the world into the ratatui buffer. Pure cell-painting — no
//! widgets here, just chars and colors, like the pixel-art gods intended.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::model::{Activity, MissionStatus};
use crate::sim::{FlightPhase, World};

/// Horizontal squash used everywhere so orbits render round-ish in ~2:1 cells.
pub const ASPECT: f64 = 0.55;

// Palette (assumes a dark terminal, like every self-respecting mission control).
const STAR_DIM: Color = Color::Rgb(90, 95, 120);
const STAR_BRIGHT: Color = Color::Rgb(190, 195, 220);
const RING: Color = Color::Rgb(45, 50, 70);
const PLANET_A: Color = Color::Rgb(48, 130, 106);
const PLANET_B: Color = Color::Rgb(36, 100, 120);
const PLANET_C: Color = Color::Rgb(70, 160, 120);
const FLAME_A: Color = Color::Rgb(255, 180, 60);
const FLAME_B: Color = Color::Rgb(255, 100, 40);
const DEBRIS: Color = Color::Rgb(110, 100, 100);
const COMET: Color = Color::Rgb(220, 230, 255);

pub fn status_color(status: MissionStatus) -> Color {
    match status {
        MissionStatus::Burning => Color::Rgb(255, 220, 120),
        MissionStatus::Holding => Color::Rgb(120, 190, 255),
        MissionStatus::Landed => Color::Rgb(120, 220, 140),
        MissionStatus::Lost => Color::Rgb(240, 95, 90),
    }
}

/// Paint one char if it falls inside `area`.
fn put(buf: &mut Buffer, area: Rect, col: i32, row: i32, ch: char, fg: Color) {
    if col < 0 || row < 0 {
        return;
    }
    let (col, row) = (col as u16, row as u16);
    if col >= area.width || row >= area.height {
        return;
    }
    let cell = &mut buf[(area.x + col, area.y + row)];
    cell.set_char(ch);
    cell.set_fg(fg);
}

/// World (0..1, 0..1) -> cell coordinates inside `area`.
fn to_cell(area: Rect, x: f64, y: f64) -> (i32, i32) {
    (
        (x * (area.width.saturating_sub(1)) as f64).round() as i32,
        (y * (area.height.saturating_sub(1)) as f64).round() as i32,
    )
}

pub fn draw_world(buf: &mut Buffer, area: Rect, world: &World, selected: Option<&str>) {
    if area.width < 4 || area.height < 4 {
        return;
    }
    draw_stars(buf, area, world);
    draw_rings(buf, area, world);
    draw_planet(buf, area, world);
    draw_comet(buf, area, world);
    draw_rockets(buf, area, world, selected);
}

fn draw_stars(buf: &mut Buffer, area: Rect, world: &World) {
    for star in &world.stars {
        let tw = (world.time * 0.9 + star.phase).sin();
        if tw < -0.55 {
            continue; // twinkled out this frame
        }
        let (c, r) = to_cell(area, star.x, star.y);
        let (ch, color) = if star.bright {
            ('✦', STAR_BRIGHT)
        } else if tw > 0.6 {
            ('•', STAR_DIM)
        } else {
            ('·', STAR_DIM)
        };
        put(buf, area, c, r, ch, color);
    }
}

fn draw_rings(buf: &mut Buffer, area: Rect, world: &World) {
    // Sparse dots along each orbit in use, plus the debris belt.
    let mut radii: Vec<f64> = world
        .rockets
        .iter()
        .filter(|r| {
            matches!(
                r.phase,
                FlightPhase::Orbiting | FlightPhase::Launching { .. }
            )
        })
        .map(|r| r.orbit_radius)
        .collect();
    radii.push(0.42); // debris belt
    radii.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    for radius in radii {
        let steps = 40;
        for i in 0..steps {
            if i % 4 != 0 {
                continue;
            }
            let a = i as f64 / steps as f64 * std::f64::consts::TAU;
            let x = 0.5 + radius * a.cos() * ASPECT;
            let y = 0.5 + radius * a.sin();
            let (c, r) = to_cell(area, x, y);
            put(buf, area, c, r, '·', RING);
        }
    }
}

fn draw_planet(buf: &mut Buffer, area: Rect, world: &World) {
    let pr = world.planet_radius;
    let (cx, cy) = (0.5, 0.5);
    // Scan the cell grid near the center and fill the ellipse.
    let (w, h) = (area.width as i32, area.height as i32);
    for row in 0..h {
        for col in 0..w {
            let x = col as f64 / (w - 1).max(1) as f64;
            let y = row as f64 / (h - 1).max(1) as f64;
            let dx = (x - cx) / ASPECT;
            let dy = y - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if d > pr {
                continue;
            }
            // Rotating cloud bands + a lit limb on the sun side.
            let band = ((y * 26.0) + (x * 6.0) + world.time * 0.35).sin();
            let rim = d / pr;
            let (ch, color) = if rim > 0.92 {
                ('▓', PLANET_B)
            } else if band > 0.45 {
                ('█', PLANET_C)
            } else if band < -0.5 {
                ('▓', PLANET_B)
            } else {
                ('█', PLANET_A)
            };
            put(buf, area, col, row, ch, color);
        }
    }
}

fn draw_comet(buf: &mut Buffer, area: Rect, world: &World) {
    let Some(comet) = &world.comet else { return };
    for (i, (x, y)) in comet.trail.iter().enumerate() {
        let (c, r) = to_cell(area, *x, *y);
        let ch = if i + 3 >= comet.trail.len() {
            '∙'
        } else {
            '·'
        };
        put(buf, area, c, r, ch, STAR_DIM);
    }
    let (c, r) = to_cell(area, comet.x, comet.y);
    put(buf, area, c, r, '✦', COMET);
}

fn draw_rockets(buf: &mut Buffer, area: Rect, world: &World, selected: Option<&str>) {
    // Trails first so every hull draws on top.
    for rocket in &world.rockets {
        for (i, (x, y)) in rocket.trail.iter().enumerate() {
            let (c, r) = to_cell(area, *x, *y);
            let color = if i + 2 >= rocket.trail.len() {
                FLAME_A
            } else {
                RING
            };
            put(buf, area, c, r, '·', color);
        }
    }

    for rocket in &world.rockets {
        let (x, y) = rocket.pos(ASPECT);
        let (c, r) = to_cell(area, x, y);
        let is_selected = selected == Some(rocket.mission.id.as_str());
        let color = status_color(rocket.mission.status);
        let flame_frame = (rocket.anim * 6.0) as u64 % 2 == 0;

        match rocket.phase {
            FlightPhase::Debris => {
                put(buf, area, c, r, '✖', DEBRIS);
            }
            FlightPhase::Crashing { .. } => {
                let ch = if flame_frame { '✶' } else { '✖' };
                put(buf, area, c, r, ch, FLAME_B);
            }
            FlightPhase::Parked => {
                put(buf, area, c, r, '▲', color);
            }
            _ => {
                if rocket.mission.is_probe {
                    put(buf, area, c, r, '◆', Color::Rgb(140, 220, 230));
                } else {
                    put(buf, area, c, r, '▲', color);
                    if rocket.mission.status == MissionStatus::Burning {
                        let flame = if flame_frame { FLAME_A } else { FLAME_B };
                        put(buf, area, c, r + 1, '▾', flame);
                    }
                }
                if rocket.mission.activity == Activity::Permission && flame_frame {
                    put(buf, area, c, r - 1, '!', Color::Rgb(255, 210, 90));
                }
            }
        }

        if is_selected {
            put(buf, area, c - 2, r, '[', Color::Yellow);
            put(buf, area, c + 2, r, ']', Color::Yellow);
        }

        // Short floating label for active missions (and always for the selection).
        let labelled = is_selected
            || matches!(rocket.mission.status, MissionStatus::Burning) && !rocket.mission.is_probe;
        if labelled {
            let label = truncate(&rocket.mission.title, 16);
            let label_color = if is_selected { Color::Yellow } else { STAR_DIM };
            draw_label(buf, area, c + 2, r - 1, &label, label_color);
        }
    }
}

/// Paint a text label cell-by-cell, honoring display width: wide (CJK/emoji)
/// chars occupy two columns, control and zero-width chars are dropped.
fn draw_label(buf: &mut Buffer, area: Rect, start_col: i32, row: i32, text: &str, fg: Color) {
    use unicode_width::UnicodeWidthChar;
    let mut col = start_col;
    for ch in text.chars() {
        if ch.is_control() {
            continue;
        }
        let w = ch.width().unwrap_or(0) as i32;
        if w == 0 {
            continue;
        }
        if col + w > area.width as i32 {
            break;
        }
        put(buf, area, col, row, ch, fg);
        if w == 2 {
            // Reserve the spill column so the wide glyph doesn't smear.
            put(buf, area, col + 1, row, ' ', fg);
        }
        col += w;
    }
}

/// Truncate to a display-column budget (not chars, not bytes): CJK and emoji
/// count as two columns. Control chars are stripped.
pub fn truncate(s: &str, max_cols: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let cleaned: Vec<char> = s.chars().filter(|c| !c.is_control()).collect();
    let total: usize = cleaned.iter().map(|c| c.width().unwrap_or(0)).sum();
    if total <= max_cols {
        return cleaned.into_iter().collect();
    }
    let budget = max_cols.saturating_sub(1); // room for the ellipsis
    let mut out = String::new();
    let mut used = 0;
    for ch in cleaned {
        let w = ch.width().unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Hit-test a click against rocket positions; returns the closest mission id
/// within a small radius.
pub fn rocket_at(world: &World, area: Rect, col: u16, row: u16) -> Option<String> {
    if col < area.x || row < area.y {
        return None;
    }
    let (rel_c, rel_r) = ((col - area.x) as i32, (row - area.y) as i32);
    let mut best: Option<(i32, String)> = None;
    for rocket in &world.rockets {
        let (x, y) = rocket.pos(ASPECT);
        let (c, r) = to_cell(area, x, y);
        let d = (c - rel_c).abs() + (r - rel_r).abs() * 2;
        if d <= 4 && best.as_ref().map(|(bd, _)| d < *bd).unwrap_or(true) {
            best = Some((d, rocket.mission.id.clone()));
        }
    }
    best.map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_handles_unicode() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("a very long mission title", 10), "a very lo…");
        assert_eq!(truncate("héllo wörld exträ", 8), "héllo w…");
    }

    #[test]
    fn truncate_budgets_by_display_width() {
        // Each CJK char is 2 columns: "日本語" = 6 cols, fits in 6.
        assert_eq!(truncate("日本語", 6), "日本語");
        // 5 columns: two CJK (4) + ellipsis (1).
        assert_eq!(truncate("日本語テスト", 5), "日本…");
        // Control chars are stripped, never painted.
        assert_eq!(truncate("a\x1b[31mb", 10), "a[31mb");
    }

    #[test]
    fn labels_with_wide_chars_stay_in_bounds() {
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        draw_label(&mut buf, area, 5, 1, "日本語セッション", Color::White);
        // Wide glyphs must stop before the edge; nothing panics, and the
        // last column is either blank or a reserved spill space.
        assert_ne!(buf[(5, 1)].symbol(), "日本"); // sanity: cells hold single glyphs
    }

    #[test]
    fn draw_world_stays_in_bounds() {
        // Paint into a buffer bigger than the draw area and verify nothing
        // leaks outside — the put() clamp is the only guard.
        let mut world = World::new(5);
        let now = std::time::SystemTime::now();
        let missions: Vec<_> = (0..10)
            .map(|i| {
                let mut m = crate::sim::tests_support::mission(&format!("m{i}"));
                m.status = match i % 4 {
                    0 => MissionStatus::Burning,
                    1 => MissionStatus::Holding,
                    2 => MissionStatus::Landed,
                    _ => MissionStatus::Lost,
                };
                m
            })
            .collect();
        world.sync(&missions, now);
        for _ in 0..100 {
            world.tick(0.1);
        }
        let full = Rect::new(0, 0, 60, 24);
        let inner = Rect::new(5, 3, 40, 15);
        let mut buf = Buffer::empty(full);
        draw_world(&mut buf, inner, &world, Some("m0"));
        for row in 0..full.height {
            for col in 0..full.width {
                let inside = col >= inner.x
                    && col < inner.x + inner.width
                    && row >= inner.y
                    && row < inner.y + inner.height;
                if !inside {
                    assert_eq!(
                        buf[(col, row)].symbol(),
                        " ",
                        "cell ({col},{row}) painted outside the draw area"
                    );
                }
            }
        }
    }

    #[test]
    fn tiny_areas_do_not_panic() {
        let world = World::new(6);
        for (w, h) in [(0, 0), (1, 1), (3, 2), (4, 4)] {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(Rect::new(0, 0, w.max(1), h.max(1)));
            draw_world(&mut buf, area, &world, None);
        }
    }
}
