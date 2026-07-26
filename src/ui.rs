//! App state + top-level draw/input. Two screens: ORBIT (the planet view)
//! and MISSION LOG (the flight records), toggled with Tab.

use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};
use ratatui::Frame;

use crate::demo::DemoWorld;
use crate::discover::{ScanConfig, Scanner};
use crate::log_screen::{self, LogFilter};
use crate::model::{Mission, MissionStatus};
use crate::render;
use crate::sim::World;

const SCAN_EVERY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Orbit,
    Log,
}

pub enum Source {
    /// Watching a real ~/.grok/sessions tree.
    Live {
        scanner: Scanner,
        home: std::path::PathBuf,
    },
    /// Scripted demo fleet.
    Demo(DemoWorld),
}

pub struct App {
    pub source: Source,
    pub cfg: ScanConfig,
    pub world: World,
    pub missions: Vec<Mission>,
    pub screen: Screen,
    pub selected: Option<String>,
    pub log_selected: usize,
    pub log_scroll: usize,
    pub filter: LogFilter,
    pub quit: bool,
    last_scan: Option<Instant>,
    /// Cached orbit-canvas rect from the last draw, for mouse hit-testing.
    orbit_area: Rect,
}

impl App {
    pub fn new(source: Source, cfg: ScanConfig, seed: u64) -> Self {
        App {
            source,
            cfg,
            world: World::new(seed),
            missions: Vec::new(),
            screen: Screen::Orbit,
            selected: None,
            log_selected: 0,
            log_scroll: 0,
            filter: LogFilter::All,
            quit: false,
            last_scan: None,
            orbit_area: Rect::default(),
        }
    }

    pub fn is_demo(&self) -> bool {
        matches!(self.source, Source::Demo(_))
    }

    /// Advance the world; rescan the data source when due.
    pub fn tick(&mut self, dt: f64, now: SystemTime) {
        let scan_due = self
            .last_scan
            .map(|t| t.elapsed() >= SCAN_EVERY)
            .unwrap_or(true);
        match &mut self.source {
            Source::Demo(demo) => {
                self.missions = demo.tick(dt);
                self.world.sync(&self.missions, now);
            }
            Source::Live { scanner, home } => {
                if scan_due {
                    self.missions = scanner.scan(home, now, &self.cfg);
                    self.world.sync(&self.missions, now);
                    self.last_scan = Some(Instant::now());
                }
            }
        }
        self.world.tick(dt);

        // Drop a selection whose rocket left the sky.
        if let Some(sel) = &self.selected {
            if !self.world.rockets.iter().any(|r| &r.mission.id == sel) {
                self.selected = None;
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => self.quit = true,
            KeyCode::Tab => {
                self.screen = match self.screen {
                    Screen::Orbit => Screen::Log,
                    Screen::Log => Screen::Orbit,
                };
            }
            KeyCode::Char('f') | KeyCode::Char('F') => {
                if self.screen == Screen::Log {
                    self.filter = self.filter.next();
                    self.log_selected = 0;
                    self.log_scroll = 0;
                }
            }
            KeyCode::Esc => self.selected = None,
            KeyCode::Left | KeyCode::Right => {
                if self.screen == Screen::Orbit {
                    self.cycle_selection(key.code == KeyCode::Right);
                }
            }
            KeyCode::Up | KeyCode::Down if self.screen == Screen::Log => {
                let count = self.filtered_count();
                if count > 0 {
                    if key.code == KeyCode::Down {
                        self.log_selected = (self.log_selected + 1).min(count - 1);
                    } else {
                        self.log_selected = self.log_selected.saturating_sub(1);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.screen == Screen::Orbit {
                    if let Some(id) =
                        render::rocket_at(&self.world, self.orbit_area, ev.column, ev.row)
                    {
                        self.selected = Some(id);
                    } else {
                        self.selected = None;
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                if self.screen == Screen::Log {
                    let count = self.filtered_count();
                    if count > 0 {
                        self.log_selected = (self.log_selected + 1).min(count - 1);
                    }
                }
            }
            MouseEventKind::ScrollUp if self.screen == Screen::Log => {
                self.log_selected = self.log_selected.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn filtered_count(&self) -> usize {
        self.missions
            .iter()
            .filter(|m| self.filter.keeps(m.status))
            .count()
    }

    fn cycle_selection(&mut self, forward: bool) {
        let rockets = &self.world.rockets;
        if rockets.is_empty() {
            self.selected = None;
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|sel| rockets.iter().position(|r| &r.mission.id == sel));
        let next = match current {
            Some(i) if forward => (i + 1) % rockets.len(),
            Some(i) => (i + rockets.len() - 1) % rockets.len(),
            None => 0,
        };
        self.selected = Some(rockets[next].mission.id.clone());
    }

    pub fn draw(&mut self, frame: &mut Frame, now: SystemTime) {
        let [title_bar, main, help_bar] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        self.draw_title(frame, title_bar);
        match self.screen {
            Screen::Orbit => {
                self.orbit_area = main;
                render::draw_world(
                    frame.buffer_mut(),
                    main,
                    &self.world,
                    self.selected.as_deref(),
                );
                self.draw_inspect_card(frame, main, now);
                if self.missions.is_empty() && !self.is_demo() {
                    self.draw_empty_pad(frame, main);
                }
            }
            Screen::Log => {
                log_screen::draw(
                    frame,
                    main,
                    &self.missions,
                    self.filter,
                    self.log_selected,
                    &mut self.log_scroll,
                    now,
                );
            }
        }
        self.draw_help(frame, help_bar);
    }

    fn draw_title(&self, frame: &mut Frame, area: Rect) {
        let counts = self.status_counts();
        let mut spans = vec![
            Span::styled(
                " âœ¦ GROK-ORBIT ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Rgb(255, 220, 120))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled("mission control", Style::default().fg(Color::Gray)),
            Span::raw("   "),
        ];
        for (label, count, status) in [
            ("burn", counts.0, MissionStatus::Burning),
            ("hold", counts.1, MissionStatus::Holding),
            ("down", counts.2, MissionStatus::Landed),
            ("lost", counts.3, MissionStatus::Lost),
        ] {
            spans.push(Span::styled(
                format!("{count} {label}  "),
                Style::default().fg(render::status_color(status)),
            ));
        }
        if self.is_demo() {
            spans.push(Span::styled(
                " DEMO ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Rgb(120, 190, 255))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let text = match self.screen {
            Screen::Orbit => {
                " tab mission log Â· â† â†’ select rocket Â· click inspect Â· esc clear Â· q quit"
            }
            Screen::Log => " tab orbit Â· â†‘ â†“ browse Â· f filter Â· q quit",
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().fg(Color::DarkGray),
            ))),
            area,
        );
    }

    fn draw_empty_pad(&self, frame: &mut Frame, main: Rect) {
        let msg = vec![
            Line::from(Span::styled(
                "launch pad is quiet",
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "start a grok-build session and it will appear here",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "(or run: grok-orbit --demo)",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let w = 52.min(main.width.saturating_sub(2)).max(10);
        let h = 5;
        let rect = Rect::new(
            main.x + (main.width.saturating_sub(w)) / 2,
            main.y + main.height.saturating_sub(h) / 2 + main.height / 4,
            w,
            h,
        );
        let rect = rect.intersection(main);
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(msg)
                .alignment(ratatui::layout::Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::DarkGray)),
                ),
            rect,
        );
    }

    fn draw_inspect_card(&self, frame: &mut Frame, main: Rect, now: SystemTime) {
        let Some(sel) = &self.selected else { return };
        let Some(rocket) = self.world.rockets.iter().find(|r| &r.mission.id == sel) else {
            return;
        };
        let m = &rocket.mission;
        let w = 44.min(main.width.saturating_sub(2));
        let h = 8.min(main.height.saturating_sub(2));
        if w < 20 || h < 6 {
            return;
        }
        let rect = Rect::new(main.x + main.width - w - 1, main.y + 1, w, h);

        let fuel_left = (1.0 - m.fuel_used).clamp(0.0, 1.0);
        let bar_len = 18usize;
        let filled = (fuel_left * bar_len as f64).round() as usize;
        let fuel_bar = format!(
            "FUEL {}{} {:>3.0}%",
            "â–®".repeat(filled.min(bar_len)),
            "â–¯".repeat(bar_len - filled.min(bar_len)),
            fuel_left * 100.0
        );
        let fuel_color = if fuel_left < 0.15 {
            Color::Rgb(240, 95, 90)
        } else if fuel_left < 0.4 {
            Color::Rgb(255, 180, 60)
        } else {
            Color::Rgb(120, 220, 140)
        };

        let lines = vec![
            Line::from(vec![
                Span::styled(
                    format!("{} ", m.status.label()),
                    Style::default()
                        .fg(render::status_color(m.status))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(m.activity.blurb(), Style::default().fg(Color::Gray)),
            ]),
            Line::from(Span::styled(
                format!("sector  {}", log_screen::shorten_path(&m.cwd)),
                Style::default().fg(Color::Gray),
            )),
            Line::from(Span::styled(
                format!(
                    "model   {}",
                    m.model.clone().unwrap_or_else(|| "unknown".into())
                ),
                Style::default().fg(Color::Gray),
            )),
            Line::from(Span::styled(
                format!(
                    "flight  {} tokens Â· {} turns Â· T-{}",
                    log_screen::human_tokens(m.tokens),
                    m.turns.map(|t| t.to_string()).unwrap_or_else(|| "?".into()),
                    log_screen::human_age(m, now),
                ),
                Style::default().fg(Color::Gray),
            )),
            Line::from(Span::styled(fuel_bar, Style::default().fg(fuel_color))),
        ];
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Rgb(255, 220, 120)))
                    .padding(Padding::horizontal(1))
                    .title(format!(" {} ", render::truncate(&m.title, 36)))
                    .title_style(
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
            ),
            rect,
        );
    }

    fn status_counts(&self) -> (usize, usize, usize, usize) {
        let mut c = (0, 0, 0, 0);
        for m in &self.missions {
            match m.status {
                MissionStatus::Burning => c.0 += 1,
                MissionStatus::Holding => c.1 += 1,
                MissionStatus::Landed => c.2 += 1,
                MissionStatus::Lost => c.3 += 1,
            }
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn demo_app() -> App {
        App::new(
            Source::Demo(DemoWorld::new(42, 256_000)),
            ScanConfig::default(),
            42,
        )
    }

    /// Full pipeline smoke test: demo world -> sim -> render, into a fake
    /// terminal. Catches panics anywhere in the draw path without needing a TTY.
    #[test]
    fn renders_demo_frames_without_panicking() {
        let mut app = demo_app();
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        let now = SystemTime::now();
        for i in 0..120 {
            app.tick(0.1, now);
            if i == 40 {
                app.on_key(KeyEvent::from(KeyCode::Right)); // select something
            }
            if i == 80 {
                app.on_key(KeyEvent::from(KeyCode::Tab)); // mission log
            }
            terminal.draw(|f| app.draw(f, now)).unwrap();
        }
    }

    #[test]
    fn renders_in_a_tiny_terminal() {
        let mut app = demo_app();
        let backend = TestBackend::new(20, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        let now = SystemTime::now();
        for _ in 0..30 {
            app.tick(0.1, now);
            terminal.draw(|f| app.draw(f, now)).unwrap();
        }
        app.on_key(KeyEvent::from(KeyCode::Tab));
        for _ in 0..10 {
            app.tick(0.1, now);
            terminal.draw(|f| app.draw(f, now)).unwrap();
        }
    }

    #[test]
    fn key_handling_cycles_and_quits() {
        let mut app = demo_app();
        let now = SystemTime::now();
        app.tick(0.1, now);
        assert_eq!(app.screen, Screen::Orbit);
        app.on_key(KeyEvent::from(KeyCode::Right));
        assert!(app.selected.is_some());
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert!(app.selected.is_none());
        app.on_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(app.screen, Screen::Log);
        app.on_key(KeyEvent::from(KeyCode::Char('f')));
        assert_eq!(app.filter, LogFilter::Active);
        app.on_key(KeyEvent::from(KeyCode::Char('q')));
        assert!(app.quit);
    }

    #[test]
    fn log_selection_stays_in_bounds() {
        let mut app = demo_app();
        let now = SystemTime::now();
        for _ in 0..50 {
            app.tick(0.2, now);
        }
        app.on_key(KeyEvent::from(KeyCode::Tab));
        for _ in 0..500 {
            app.on_key(KeyEvent::from(KeyCode::Down));
        }
        assert!(app.log_selected < app.missions.len().max(1));
        for _ in 0..600 {
            app.on_key(KeyEvent::from(KeyCode::Up));
        }
        assert_eq!(app.log_selected, 0);
    }
}
