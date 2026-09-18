//! `jev` — an interactive playground for learning TypeSafe AI System One questions.
//!
//! Run it with a `TYPESAFE_API_KEY` for live answers, or without one to explore offline with
//! simulated answers. `:help` inside lists every command; `:lesson` starts the guided track.

use std::time::Duration;

use ratatui::crossterm::event;
use tokio::sync::mpsc;

use jev_repl::app::{App, Msg};
use jev_repl::ui;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!(
            "jev — a REPL for TypeSafe AI System One questions\n\n\
             Set TYPESAFE_API_KEY for live answers; without one, answers are simulated locally.\n\
             Inside: :help for commands, :lesson for the guided track, :sketch to write a request\n\
             as one page of text, :quit to leave."
        );
        return Ok(());
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_input(tx.clone());
    spawn_ticker(tx.clone());

    let mut terminal = ratatui::init();
    let mut app = App::new(tx);
    let mut redraw = true;
    let result = loop {
        if redraw && let Err(e) = terminal.draw(|frame| ui::render(frame, &mut app)) {
            break Err(e);
        }
        let Some(msg) = rx.recv().await else {
            break Ok(());
        };
        // Ticks only matter while the spinner is turning; otherwise an idle REPL redraws nothing.
        redraw = !matches!(msg, Msg::Tick) || app.pending;
        app.handle(msg);
        if app.quit {
            break Ok(());
        }
    };
    ratatui::restore();
    result
}

/// Terminal events come from a blocking thread so the async side stays free for API calls.
fn spawn_input(tx: mpsc::UnboundedSender<Msg>) {
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if tx.send(Msg::Term(ev)).is_err() {
                break;
            }
        }
    });
}

/// Drives the spinner while a request is in flight.
fn spawn_ticker(tx: mpsc::UnboundedSender<Msg>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(120));
        loop {
            interval.tick().await;
            if tx.send(Msg::Tick).is_err() {
                break;
            }
        }
    });
}
