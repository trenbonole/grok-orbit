//! MISSION LOG — the flight-record screen. Every session ever flown (well,
//! the last two weeks of them), with epitaphs for the ones that didn't make it.

use std::time::SystemTime;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Padding, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

use crate::model::{Mission, MissionStatus};
use crate::render::{status_color, truncate};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFilter {
    #[default]
    All,
    Active,
    Landed,
    Lost,
}

impl LogFilter {
    pub fn next(self) -> Self {
        match self {
            LogFilter::All => LogFilter::Active,
            LogFilter::Active => LogFilter::Landed,
            LogFilter::Landed => LogFilter::Lost,
            LogFilter::Lost => LogFilter::All,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            LogFilter::All => "ALL",
            LogFilter::Active => "ACTIVE",
            LogFilter::Landed => "LANDED",
            LogFilter::Lost => "LOST",
        }
    }
    pub fn keeps(self, status: MissionStatus) -> bool {
        match self {
            LogFilter::All => true,
            LogFilter::Active => {
                matches!(status, MissionStatus::Burning | MissionStatus::Holding)
            }
            LogFilter::Landed => status == MissionStatus::Landed,
            LogFilter::Lost => status == MissionStatus::Lost,
        }
    }
}

const LOST_EPITAPHS: &[&str] = &[
    "Rapid unscheduled disassembly.",
    "Ran out of context, and luck.",
    "The context window closed on its fingers.",
    "Last words: \"let me just refactor this real quick\".",
    "Pressed Ctrl+C. Twice.",
    "Auto-compact couldn't save it.",
    "Flew too close to node_modules.",
    "It was rebasing. It is rebasing no more.",
    "Segfaulted into the sun.",
    "Told the linter to be quiet. The linter won.",
];

const LANDED_EPITAPHS: &[&str] = &[
    "Touchdown confirmed. Tests green.",
    "Mission accomplished. PR merged.",
    "Came home with 12 files changed.",
    "Splashdown nominal. Reviewer approved.",
    "The eagle has landed. So has the fix.",
    "Returned safely. Left TODOs behind.",
];

/// Deterministic epitaph per mission (stable across frames).
pub fn epitaph(m: &Mission) -> &'static str {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in m.id.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    match m.status {
        MissionStatus::Lost => LOST_EPITAPHS[(hash % LOST_EPITAPHS.len() as u64) as usize],
        _ => LANDED_EPITAPHS[(hash % LANDED_EPITAPHS.len() as u64) as usize],
    }
}

pub fn human_age(m: &Mission, now: SystemTime) -> String {
    let Some(t) = m.last_activity else {
        return "?".into();
    };
    let secs = now.duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

pub fn human_tokens(tokens: u64) -> String {
    match tokens {
        0 => "—".into(),
        1..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{:.1}k", tokens as f64 / 1_000.0),
        _ => format!("{:.1}M", tokens as f64 / 1_000_000.0),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    frame: &mut Frame,
    area: Rect,
    missions: &[Mission],
    filter: LogFilter,
    selected: usize,
    scroll_offset: &mut usize,
    now: SystemTime,
) {
    let rows_area_height = area.height.saturating_sub(3) as usize; // borders + header
    let filtered: Vec<&Mission> = missions.iter().filter(|m| filter.keeps(m.status)).collect();
    let selected = selected.min(filtered.len().saturating_sub(1));

    // Keep the selection in view.
    if selected < *scroll_offset {
        *scroll_offset = selected;
    }
    let visible_rows = rows_area_height.saturating_sub(6).max(3);
    if selected >= *scroll_offset + visible_rows {
        *scroll_offset = selected + 1 - visible_rows;
    }

    let [table_area, detail_area] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(6)]).areas(area);

    let header = Row::new(vec![
        "", "MISSION", "SECTOR", "MODEL", "TOKENS", "TURNS", "AGE",
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .skip(*scroll_offset)
        .take(visible_rows)
        .map(|(i, m)| {
            let marker = match m.status {
                MissionStatus::Burning => "▲",
                MissionStatus::Holding => "◦",
                MissionStatus::Landed => "✓",
                MissionStatus::Lost => "✖",
            };
            let style = if i == selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(status_color(m.status))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(status_color(m.status))
            };
            Row::new(vec![
                Cell::from(marker),
                Cell::from(truncate(&m.title, 34)),
                Cell::from(truncate(&shorten_path(&m.cwd), 22)),
                Cell::from(m.model.clone().unwrap_or_else(|| "—".into())),
                Cell::from(human_tokens(m.tokens)),
                Cell::from(m.turns.map(|t| t.to_string()).unwrap_or_else(|| "—".into())),
                Cell::from(human_age(m, now)),
            ])
            .style(style)
        })
        .collect();

    let title = format!(
        " MISSION LOG ── {} of {} ── filter: {} (f) ",
        filtered.len().min(1 + selected).max(filtered.len().min(1)),
        filtered.len(),
        filter.label()
    );
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Min(20),
            Constraint::Length(22),
            Constraint::Length(14),
            Constraint::Length(7),
            Constraint::Length(5),
            Constraint::Length(5),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(title)
            .title_style(
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
    );
    frame.render_widget(table, table_area);

    // Detail card for the selected mission.
    let detail = filtered.get(selected).map(|m| detail_lines(m, now));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .padding(Padding::horizontal(1))
        .title(" FLIGHT RECORD ");
    let para = match detail {
        Some(lines) => Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        None => Paragraph::new("No missions on file. Fly something.").block(block),
    };
    frame.render_widget(para, detail_area);
}

fn detail_lines(m: &Mission, now: SystemTime) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("{} ", m.status.label()),
            Style::default()
                .fg(status_color(m.status))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            m.title.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  ({})", truncate(&m.id, 20)),
            Style::default().fg(Color::DarkGray),
        ),
    ])];
    let launched = m
        .created_at
        .and_then(|t| now.duration_since(t).ok())
        .map(|d| match d.as_secs() {
            0..=3599 => format!("{}m", d.as_secs().max(60) / 60),
            3600..=86_399 => format!("{}h", d.as_secs() / 3600),
            s => format!("{}d", s / 86_400),
        })
        .unwrap_or_else(|| "?".into());
    lines.push(Line::from(Span::styled(
        format!(
            "sector {} · {} tokens · {} turns · launched {} ago · last contact {} ago",
            shorten_path(&m.cwd),
            human_tokens(m.tokens),
            m.turns.map(|t| t.to_string()).unwrap_or_else(|| "?".into()),
            launched,
            human_age(m, now),
        ),
        Style::default().fg(Color::Gray),
    )));
    lines.push(Line::from(Span::styled(
        m.path.display().to_string(),
        Style::default().fg(Color::DarkGray),
    )));
    if matches!(m.status, MissionStatus::Landed | MissionStatus::Lost) {
        lines.push(Line::from(Span::styled(
            format!("“{}”", epitaph(m)),
            Style::default()
                .fg(Color::Rgb(180, 170, 140))
                .add_modifier(Modifier::ITALIC),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            m.activity.blurb().to_string(),
            Style::default().fg(Color::Rgb(140, 180, 220)),
        )));
    }
    lines
}

/// Compress a long path to its interesting tail: `…/parent/leaf`.
pub fn shorten_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= 2 {
        return path.to_string();
    }
    format!("…/{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_cycles_through_all_states() {
        let mut f = LogFilter::All;
        let mut seen = vec![f];
        for _ in 0..3 {
            f = f.next();
            seen.push(f);
        }
        assert_eq!(f.next(), LogFilter::All);
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn human_units_read_well() {
        assert_eq!(human_tokens(0), "—");
        assert_eq!(human_tokens(950), "950");
        assert_eq!(human_tokens(94_000), "94.0k");
        assert_eq!(human_tokens(1_200_000), "1.2M");
    }

    #[test]
    fn shorten_path_keeps_the_tail() {
        assert_eq!(shorten_path("C:\\Users\\me\\code\\app"), "…/code/app");
        assert_eq!(shorten_path("/home/y/cape/stable"), "…/cape/stable");
        assert_eq!(shorten_path("~/code"), "~/code");
    }

    #[test]
    fn epitaph_is_stable_per_mission() {
        let mut m = crate::sim::tests_support::mission("epitaph-test");
        m.status = MissionStatus::Lost;
        assert_eq!(epitaph(&m), epitaph(&m));
    }
}
