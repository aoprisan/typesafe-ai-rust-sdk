//! The MCP server on a pipe: newline-delimited JSON-RPC in on stdin, the same out on stdout.
//!
//! Nothing but the protocol may be written to stdout — a stray `println!` is what breaks a
//! hand-written MCP server — so everything the server wants to say goes to stderr.

use std::io::{BufRead, Write};

use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::mcp::{self, Host};

/// Answer messages until stdin closes.
///
/// Requests are answered as they finish rather than in the order they arrived: a `jev_eval` over a
/// hundred cases must not hold up the `jev_check` behind it, and JSON-RPC matches replies by id.
/// Stdin is read on a blocking thread, since a pipe that never closes would otherwise hold a
/// runtime worker for the life of the process.
pub async fn serve(host: Host) {
    let (lines, mut incoming) = mpsc::unbounded_channel::<String>();
    let reader = tokio::task::spawn_blocking(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { return };
            if lines.send(line).is_err() {
                return;
            }
        }
    });

    let mut answering = JoinSet::new();
    let mut open = true;
    // A reply goes out the moment it is ready, not when the next line happens to arrive: the
    // client sends `initialize` and waits on the answer before it says anything else.
    while open || !answering.is_empty() {
        tokio::select! {
            line = incoming.recv(), if open => match line {
                Some(line) => {
                    let host = host.clone();
                    answering.spawn(async move { mcp::handle_line(&line, &host).await });
                }
                None => open = false,
            },
            Some(done) = answering.join_next(), if !answering.is_empty() => {
                write(done.ok().flatten());
            }
        }
    }
    let _ = reader.await;
}

/// One reply on stdout, flushed: the client is waiting on a line, not on a buffer.
fn write(reply: Option<String>) {
    let Some(reply) = reply else { return };
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{reply}");
    let _ = out.flush();
}
