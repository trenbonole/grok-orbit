//! grok-orbit — mission control for your Grok Build agents.
//!
//! Watches `~/.grok/sessions` (read-only) and renders every session as a
//! pixel-art rocket orbiting a planet in your terminal. No grok-build? Run
//! with `--demo` and enjoy the show anyway.

mod demo;
mod discover;
mod log_screen;
mod model;
mod render;
mod sim;
mod ui;

use std::io;
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{self, Event};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::demo::DemoWorld;
use crate::discover::{grok_home, ScanConfig, Scanner};
use crate::ui::{App, Source};

const FRAME: Duration = Duration::from_millis(50); // ~20 fps

struct Args {
    demo: bool,
    seed: u64,
    grok_home: Option<std::path::PathBuf>,
    context_window: Option<u64>,
    snapshot: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        demo: false,
        seed: 0x5EED, // deterministic by default so demo GIFs are reproducible
        grok_home: None,
        context_window: None,
        snapshot: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--demo" | "-d" => args.demo = true,
            "--snapshot" => args.snapshot = true,
            "--seed" => {
                let v = it.next().ok_or("--seed needs a value")?;
                args.seed = v.parse().map_err(|_| format!("bad seed: {v}"))?;
            }
            "--grok-home" => {
                let v = it.next().ok_or("--grok-home needs a path")?;
                args.grok_home = Some(v.into());
            }
            "--context-window" => {
                let v = it.next().ok_or("--context-window needs a number")?;
                args.context_window =
                    Some(v.parse().map_err(|_| format!("bad context window: {v}"))?);
            }
            "--help" | "-h" => {
                println!("{HELP}");
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("grok-orbit {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => return Err(format!("unknown flag: {other} (try --help)")),
        }
    }
    Ok(args)
}

const HELP: &str = "\
grok-orbit — mission control for your Grok Build agents 🚀

Every grok-build session becomes a pixel-art rocket orbiting a planet in
your terminal. Working agents burn, waiting agents hold, finished agents
land, crashed agents join the debris belt. Read-only: it never touches
your sessions.

USAGE:
    grok-orbit [FLAGS]

FLAGS:
    -d, --demo               fly a scripted demo fleet (no grok-build needed)
        --snapshot           print one plain-text scan of the fleet and exit
        --seed <N>           demo/starfield RNG seed (default: fixed, reproducible)
        --grok-home <PATH>   grok-build home to watch (default: $GROK_HOME or ~/.grok;
                             when the default home doesn't exist, the demo fleet flies)
        --context-window <N> assumed context window for the fuel gauge (default 256000)
    -h, --help               this
    -V, --version            version

KEYS:
    tab      switch between ORBIT and MISSION LOG
    ← →      select a rocket (or click one)
    esc      clear selection
    ↑ ↓      browse the mission log
    f        cycle mission-log filter (all/active/landed/lost)
    q        quit";

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };

    let cfg = ScanConfig {
        context_window: args.context_window.unwrap_or(model::DEFAULT_CONTEXT_WINDOW),
        ..ScanConfig::default()
    };

    // Pick the data source: --demo wins; an explicit --grok-home must exist
    // (fail loudly, never silently demo); the default home is watched if the
    // directory exists at all (empty is fine — the pad just starts quiet);
    // only when there's no grok-build home whatsoever do we fall back to the
    // demo fleet so the first run is never a blank screen.
    let explicit = args.grok_home.is_some();
    let home = args.grok_home.clone().or_else(grok_home);
    let source = if args.demo {
        Source::Demo(DemoWorld::new(args.seed, cfg.context_window))
    } else {
        match home {
            Some(h) if h.is_dir() => Source::Live {
                scanner: Scanner::new(),
                home: h,
            },
            Some(h) if explicit => {
                eprintln!("error: --grok-home {} does not exist", h.display());
                std::process::exit(2);
            }
            _ => {
                eprintln!("no grok-build home found — launching demo fleet (see --help)");
                std::thread::sleep(Duration::from_millis(900));
                Source::Demo(DemoWorld::new(args.seed, cfg.context_window))
            }
        }
    };

    if args.snapshot {
        snapshot(source, &cfg);
        return;
    }

    let mut app = App::new(source, cfg, args.seed);
    if let Err(e) = run(&mut app) {
        eprintln!("grok-orbit crashed on re-entry: {e}");
        std::process::exit(1);
    }
}

/// `--snapshot`: one scan, plain-text mission table on stdout, exit. Useful
/// for scripting and for checking what the scanner sees without the TUI.
fn snapshot(source: Source, cfg: &discover::ScanConfig) {
    let now = SystemTime::now();
    let missions = match source {
        Source::Live { mut scanner, home } => {
            let m = scanner.scan(&home, now, cfg);
            println!("grok home: {}", home.display());
            m
        }
        Source::Demo(mut demo) => {
            println!("grok home: (demo fleet)");
            demo.tick(0.1)
        }
    };
    println!("{} mission(s)\n", missions.len());
    for m in &missions {
        println!(
            "[{:^6}] {:<44} {:>8} tok  {:>3} turns  fuel {:>3.0}%  {:<12} {}",
            m.status.label(),
            crate::render::truncate(&m.title, 44),
            m.tokens,
            m.turns.map(|t| t.to_string()).unwrap_or_else(|| "?".into()),
            (1.0 - m.fuel_used) * 100.0,
            m.model.clone().unwrap_or_else(|| "?".into()),
            m.cwd,
        );
    }
}

fn run(app: &mut App) -> io::Result<()> {
    // Restore the terminal even if we panic mid-frame.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        default_hook(info);
    }));

    enable_raw_mode()?;
    let setup = (|| -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
        let mut stdout = io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        stdout.execute(event::EnableMouseCapture)?;
        Terminal::new(CrosstermBackend::new(stdout))
    })();
    let mut terminal = match setup {
        Ok(t) => t,
        Err(e) => {
            // Undo whatever half of the setup succeeded.
            let _ = restore_terminal();
            return Err(e);
        }
    };

    let result = event_loop(app, &mut terminal);
    let restored = restore_terminal();
    result.and(restored)
}

/// Best-effort teardown: attempt every step even if an earlier one fails, so
/// a hiccup in one call can't strand the user in a mouse-captured alt screen.
fn restore_terminal() -> io::Result<()> {
    let r1 = disable_raw_mode();
    let mut stdout = io::stdout();
    let r2 = stdout.execute(event::DisableMouseCapture).map(|_| ());
    let r3 = stdout.execute(LeaveAlternateScreen).map(|_| ());
    r1.and(r2).and(r3)
}

fn event_loop(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> io::Result<()> {
    let mut last_frame = Instant::now();
    loop {
        let timeout = FRAME.saturating_sub(last_frame.elapsed());
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind != event::KeyEventKind::Release => app.on_key(key),
                Event::Mouse(m) => app.on_mouse(m),
                _ => {}
            }
        }
        if last_frame.elapsed() >= FRAME {
            let dt = last_frame.elapsed().as_secs_f64();
            last_frame = Instant::now();
            let now = SystemTime::now();
            app.tick(dt, now);
            terminal.draw(|f| app.draw(f, now))?;
        }
        if app.quit {
            return Ok(());
        }
    }
}
