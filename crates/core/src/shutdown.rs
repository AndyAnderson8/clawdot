//! Orderly stop on a signal (REQ-CFG-085).
//!
//! A service manager stops the program with SIGTERM, and a person at a terminal with Ctrl-C (SIGINT). Neither
//! should lose a change that was made a moment earlier but is still queued for the background writer, or the
//! usage counters. [`requested`] is a future that completes when either arrives; the loop then does the same
//! orderly stop as the console's `quit` (`App::run`).

use std::future::Future;

/// How long the loop waits for queued writes after a signal before it gives up and leaves anyway, so that a
/// disk that does not answer cannot keep the program from stopping.
pub const FLUSH_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// A future that completes when the program is asked to stop: SIGTERM or SIGINT on Unix, Ctrl-C on Windows. The
/// handlers are installed when this is called, not when the future is first polled, and only then: until it is
/// called a signal ends the program at once, as it does by default. If a handler cannot be installed the problem
/// is reported and the future never completes.
#[cfg(unix)]
pub fn requested() -> impl Future<Output = ()> {
    use tokio::signal::unix::{SignalKind, signal};
    let term = signal(SignalKind::terminate());
    let int = signal(SignalKind::interrupt());
    async move {
        match (term, int) {
            (Ok(mut term), Ok(mut int)) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = int.recv() => {}
                }
            }
            (term, int) => {
                for e in [term.err(), int.err()].into_iter().flatten() {
                    crate::diag::error(&format!("cannot watch for a stop signal: {e}"));
                }
                std::future::pending::<()>().await;
            }
        }
    }
}

/// See the Unix version.
#[cfg(windows)]
pub fn requested() -> impl Future<Output = ()> {
    async {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {}
            Err(e) => {
                crate::diag::error(&format!("cannot watch for Ctrl-C: {e}"));
                std::future::pending::<()>().await;
            }
        }
    }
}

/// See the Unix version. On a system with neither, there is no signal to wait for.
#[cfg(not(any(unix, windows)))]
pub fn requested() -> impl Future<Output = ()> {
    std::future::pending()
}
