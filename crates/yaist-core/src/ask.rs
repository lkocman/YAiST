//! Questions a running action puts to the user: a package conflict, a
//! license, a signing key. The client answers them; with no client to ask
//! (headless runs, panel loads) the action takes the default answer.

use std::future::Future;
use std::sync::Arc;

use crate::BoxFuture;

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    /// What is sent back to the tool, e.g. `1` or `y`.
    pub value: String,
    pub label: String,
    pub default: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub title: String,
    /// Shown as is: the tool's own description of the problem.
    pub text: String,
    pub choices: Vec<Choice>,
}

impl Question {
    pub fn default_choice(&self) -> Option<&Choice> {
        self.choices.iter().find(|c| c.default).or(self.choices.last())
    }
}

/// Puts a question to the user and resolves to the chosen value; `None` when
/// the question was dropped (the client went away).
pub type Asker = Arc<dyn Fn(Question) -> BoxFuture<Option<String>> + Send + Sync>;

tokio::task_local! {
    static ASKER: Asker;
}

/// Runs `f` with `asker` answering the questions its actions ask.
pub async fn asking<F: Future>(asker: Asker, f: F) -> F::Output {
    ASKER.scope(asker, f).await
}

/// Asks the user, when someone is there to answer. The question and the
/// answer go to the change's log, so it shows later what was decided.
pub async fn ask(q: Question) -> Option<String> {
    let asker = ASKER.try_with(Arc::clone).ok()?;
    crate::log::line(&format!("? {}", q.title));
    for l in q.text.lines() {
        crate::log::line(&format!("  {l}"));
    }
    let answer = asker(q.clone()).await;
    let label = answer.as_ref().and_then(|a| q.choices.iter().find(|c| &c.value == a)).map_or("no answer", |c| c.label.as_str());
    crate::log::line(&format!("> {label}"));
    answer
}
