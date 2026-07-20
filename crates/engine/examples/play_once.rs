//! Headless engine smoke test: play a DSD file to the real PipeWire sink and
//! exit when it ends (or after a timeout). Verifies the full engine path
//! outside the TUI. Usage: `play_once <file.dsf> [max_secs]`

use std::time::{Duration, Instant};

use piwiplay_engine::{Command, Engine, Event};

fn main() {
    let path = std::env::args().nth(1).expect("usage: play_once <file.dsf> [max_secs]");
    let max = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(10u64);

    let engine = Engine::start();
    engine.command(Command::OpenAndPlay(path.into()));

    let deadline = Instant::now() + Duration::from_secs(max);
    let mut negotiated = false;
    while Instant::now() < deadline {
        match engine.events().recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Status { transport, track, mode }) => {
                println!("status: {transport:?} mode={} track={:?}", mode.label(), track.map(|t| t.display_title()));
            }
            Ok(Event::Position { elapsed, total }) => {
                if !negotiated {
                    negotiated = true;
                }
                print!("\rpos {:.1}/{:.1}s   ", elapsed.as_secs_f64(), total.as_secs_f64());
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            Ok(Event::Message(m)) => println!("\nmessage: {m}"),
            Ok(_) => {}
            Err(_) => {}
        }
        // Stop when the engine reports Stopped after having played.
        if negotiated && matches!(latest_transport(&engine), Some(piwiplay_engine::Transport::Stopped)) {
            println!("\ntrack ended");
            break;
        }
    }
    println!("\ndone");
}

/// Peek any pending Status transport (best-effort, non-blocking).
fn latest_transport(engine: &Engine) -> Option<piwiplay_engine::Transport> {
    let mut t = None;
    while let Ok(ev) = engine.events().try_recv() {
        if let Event::Status { transport, .. } = ev {
            t = Some(transport);
        }
    }
    t
}
