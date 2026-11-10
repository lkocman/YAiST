//! Tab completion in the conversation input, the same in both clients and
//! shell-like: Tab finishes the word when only one fits (and adds a space),
//! or as far as all candidates agree. When Tab can add nothing, it lists the
//! candidates; Tab again goes through them one by one, Shift+Tab backwards.
//! Typing anything else ends the round.

use yaist_core::command::{self, ClientCommand};
use yaist_core::Registry;

/// Commands the session handles itself (see `Session::submit`), with the
/// words their argument can be.
pub const CLIENT_COMMANDS: &[ClientCommand] = &[("/clear", &[]), ("/demo", crate::demo::SCENES), ("/zoom", &["in", "out", "reset", "150%"])];

/// Completes `line` (the text before the cursor) against the registry and the
/// session's own commands.
pub fn complete(registry: &Registry, line: &str) -> command::Completion {
    command::complete(registry, line, CLIENT_COMMANDS)
}

/// One input's completion round.
#[derive(Debug, Default)]
pub struct Completer {
    /// The input as the last Tab left it; any other text starts afresh.
    last: String,
    /// The line up to the word being completed.
    base: String,
    /// The candidates on show, after a Tab that could add nothing.
    listed: Vec<String>,
    /// The candidate in the input, when going through them.
    pick: Option<usize>,
}

impl Completer {
    /// Whether Tab belongs to completion here: on a /command. Elsewhere it
    /// moves the focus as usual.
    pub fn wants_tab(input: &str) -> bool {
        input.trim_start().starts_with('/')
    }

    /// Handles Tab (`forward`) or Shift+Tab on `input`. Returns whether the
    /// input changed.
    pub fn tab(&mut self, registry: &Registry, input: &mut String, forward: bool) -> bool {
        if *input != self.last {
            *self = Self::default();
        }
        if !self.listed.is_empty() {
            let n = self.listed.len();
            let i = match (self.pick, forward) {
                (None, true) => 0,
                (None, false) => n - 1,
                (Some(i), true) => (i + 1) % n,
                (Some(i), false) => (i + n - 1) % n,
            };
            self.pick = Some(i);
            *input = format!("{}{}", self.base, self.listed[i]);
            self.last = input.clone();
            return true;
        }
        let c = complete(registry, input);
        let word = &input[c.start..];
        let new = match c.candidates.as_slice() {
            [] => None,
            [only] if only.ends_with('=') => Some(only.clone()),
            [only] => Some(format!("{only} ")),
            many => {
                let common = common_prefix(many);
                if common.len() > word.len() {
                    Some(common.to_owned())
                } else {
                    self.base = input[..c.start].to_owned();
                    self.listed = c.candidates.clone();
                    None
                }
            }
        };
        let changed = new.as_deref().is_some_and(|w| w != word);
        if let Some(w) = new {
            input.truncate(c.start);
            input.push_str(&w);
        }
        self.last = input.clone();
        changed
    }

    /// Puts listed candidate `i` in the input, as Tab would (a click on it).
    pub fn pick(&mut self, i: usize, input: &mut String) {
        if let Some(w) = self.listed.get(i) {
            self.pick = Some(i);
            *input = format!("{}{w}", self.base);
            self.last = input.clone();
        }
    }

    /// The candidates to show by the input and the one picked, while
    /// `input` is still what Tab left.
    pub fn shown(&self, input: &str) -> Option<(&[String], Option<usize>)> {
        (!self.listed.is_empty() && input == self.last).then_some((self.listed.as_slice(), self.pick))
    }
}

/// The longest start all `words` share, on a character boundary.
fn common_prefix(words: &[String]) -> &str {
    let first = words[0].as_str();
    let mut end = first.len();
    for w in &words[1..] {
        end = first.char_indices().zip(w.chars()).take_while(|((_, a), b)| a == b).last().map_or(0, |((i, a), _)| i + a.len_utf8()).min(end);
    }
    &first[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_finishes_lists_then_cycles() {
        let reg = yaist_modules::registry();
        let mut c = Completer::default();
        let mut input = String::from("/de");
        assert!(c.tab(&reg, &mut input, true));
        assert_eq!(input, "/demo ");
        // Nothing to add: the scenes are listed, the input stays.
        assert!(!c.tab(&reg, &mut input, true));
        assert_eq!(c.shown(&input).map(|(l, p)| (l.len(), p)), Some((crate::demo::SCENES.len(), None)));
        assert!(c.tab(&reg, &mut input, true));
        assert_eq!(input, format!("/demo {}", crate::demo::SCENES[0]));
        // Back from the first goes round to the last.
        assert!(c.tab(&reg, &mut input, false));
        assert_eq!(input, format!("/demo {}", crate::demo::SCENES.last().unwrap()));
        // Typing ends the round.
        input.push('x');
        assert!(c.shown(&input).is_none());
    }

    #[test]
    fn tab_extends_to_the_common_start() {
        let reg = yaist_modules::registry();
        let mut c = Completer::default();
        let mut input = String::from("/demo upd");
        assert!(c.tab(&reg, &mut input, true));
        assert_eq!(input, "/demo update-");
        let mut input = String::from("ask me something");
        assert!(!c.tab(&reg, &mut input, true));
        assert!(c.shown(&input).is_none());
    }

    #[test]
    fn common_prefix_respects_characters() {
        assert_eq!(common_prefix(&["/snapshots".into(), "/services".into(), "/selinux".into()]), "/s");
        assert_eq!(common_prefix(&["ab".into(), "abc".into()]), "ab");
        assert_eq!(common_prefix(&["é1".into(), "é2".into()]), "é");
    }
}
