//! piwiplay — console (TUI) DSD audio player over PipeWire.
//!
//! This binary is a thin frontend over `piwiplay-engine`: it owns the terminal
//! and input, translates keys to engine [`Command`]s, and renders engine
//! [`Event`]s. All playback logic lives in the engine (see its crate docs).

use piwiplay::{app, theme, ui};

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event as CtEvent, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use piwiplay_engine::config::{Config, Paths};
use piwiplay_engine::{Command, Engine};

use app::App;
use theme::Theme;

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn main() -> Result<()> {
    // Non-interactive flags so packaging (e.g. Homebrew `test do`) and users can
    // query the binary without entering the TUI.
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "-V" | "--version" => {
                println!("piwiplay {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-h" | "--help" => {
                println!(
                    "piwiplay {} — console DSD audio player over PipeWire\n\n\
                     Usage: piwiplay [FILE|DIR]...\n\n\
                     With no arguments, starts in the current directory. Files and\n\
                     folders are queued; folders are scanned recursively for .dsf/.dff.\n\
                     Press ? inside the app for keybindings.",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            _ => {}
        }
    }

    let paths = Paths::get();
    paths.ensure_dirs();
    let _log_guard = init_logging(&paths);

    let cfg = Config::load();
    let theme = Theme::from_config(&cfg.theme);

    // CLI: any args are files/dirs to enqueue; first dir seeds the browser.
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let start_dir = args
        .iter()
        .find(|p| p.is_dir())
        .cloned()
        .or_else(|| args.first().and_then(|p| p.parent().map(|q| q.to_path_buf())))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let engine = Engine::start();
    if !args.is_empty() {
        engine.command(Command::Enqueue(args.clone()));
    }

    let mut app = App::new(engine, cfg, theme, start_dir, paths.playlists_dir.clone());

    let mut terminal = setup_terminal()?;
    install_panic_hook();
    let res = run(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    res
}

fn run(terminal: &mut Tui, app: &mut App) -> Result<()> {
    let frame = Duration::from_millis((1000 / app.cfg.ui.fps.max(1)).max(8) as u64);
    loop {
        // Drain engine events into the view-model.
        while let Ok(ev) = app.engine.events().try_recv() {
            app.apply_event(ev);
        }
        app.tick();

        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(frame)? {
            match event::read()? {
                CtEvent::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                CtEvent::Resize(_, _) => {} // redraw next loop
                _ => {}
            }
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

fn setup_terminal() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Ensure the terminal is restored even if a panic unwinds through the UI.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        default(info);
    }));
}

fn init_logging(paths: &Paths) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::EnvFilter;
    let file = tracing_appender::rolling::never(&paths.state_dir, "piwiplay.log");
    let (nb, guard) = tracing_appender::non_blocking(file);
    let filter = EnvFilter::try_from_env("PIWIPLAY_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(nb).with_ansi(false).try_init();
    Some(guard)
}
