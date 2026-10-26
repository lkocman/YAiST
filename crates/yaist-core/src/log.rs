//! Output of a running action, line by line, for the client to show while it
//! runs: a distribution upgrade takes long, and a spinner alone says nothing.
//! With no client watching (headless runs, panel loads) lines go nowhere.

use std::future::Future;
use std::sync::Arc;

/// Receives each line a running command prints.
pub type Logger = Arc<dyn Fn(String) + Send + Sync>;

tokio::task_local! {
    static LOGGER: Logger;
}

/// Runs `f` with `logger` receiving what its commands print.
pub async fn logging<F: Future>(logger: Logger, f: F) -> F::Output {
    LOGGER.scope(logger, f).await
}

/// Is a client watching? Commands stream their output only then.
pub fn active() -> bool {
    LOGGER.try_with(|_| ()).is_ok()
}

/// Passes a line on, when someone is watching. Progress redrawn with `\r`
/// keeps only its last state.
pub fn line(text: &str) {
    let text = text.rsplit('\r').find(|s| !s.trim().is_empty()).unwrap_or("").trim_end();
    if text.is_empty() {
        return;
    }
    let _ = LOGGER.try_with(|l| l(text.to_owned()));
}
