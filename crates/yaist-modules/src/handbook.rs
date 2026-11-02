//! The openSUSE Handbook's generated parts. Each module has one chapter,
//! `docs/handbook/modules/<module>.adoc`, written by hand except for blocks
//! between `// BEGIN GENERATED <key>` and `// END GENERATED`: one per action
//! (what it does, whether it asks first, its options, the terminal commands)
//! and one per module (the chapter's at-a-glance table). They are rendered
//! from the same [`ActionSpec`]s the panels, /commands and LLM tools use, so a
//! changed action changes the handbook; `yaist docs` rewrites the blocks and a
//! test fails while they are stale.

use std::collections::{BTreeMap, HashSet};

use anyhow::{bail, Result};
use serde_json::{Map, Value};
use yaist_core::{command, ActionSpec, Args, Distro, ModuleSpec, ParamKind, Registry, Risk};

const BEGIN: &str = "// BEGIN GENERATED ";
const END: &str = "// END GENERATED";
/// Marks an action section nobody has described yet.
pub const TODO: &str = "// TODO(handbook):";

/// The flavours whose terminal commands the handbook shows, when they differ.
const FLAVOURS: &[(&str, &str)] = &[
    ("Tumbleweed", include_str!("../../../tests/os-release/tumbleweed")),
    ("Leap 16.0", include_str!("../../../tests/os-release/leap-16.0")),
    ("Leap 16.1 Immutable", include_str!("../../../tests/os-release/leap-16.1-immutable")),
];

/// Stands in for an integer parameter in terminal commands, replaced by its
/// name afterwards.
const INT_PLACEHOLDER: i64 = 987_654_321;

/// The chapter for `module`, with every generated block brought up to date.
/// `text` is the current chapter, empty when there is none yet. Actions
/// without a block get a section stub before the at-a-glance table.
pub fn render(reg: &Registry, module: &ModuleSpec, text: &str) -> Result<String> {
    let actions = reg.module_actions(module.id);
    let mut blocks: BTreeMap<String, String> = actions.iter().map(|a| (a.id.to_owned(), action_block(reg, a))).collect();
    blocks.insert(module.id.to_owned(), module_block(module, &actions));

    let text = if text.trim().is_empty() { skeleton(module) } else { text.to_owned() };
    let mut out = String::new();
    let mut seen = HashSet::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(key) = line.strip_prefix(BEGIN) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let key = key.split_whitespace().next().unwrap_or_default();
        let Some(body) = blocks.get(key) else {
            bail!("{}: a generated block for {key}, which is not an action of this module; remove its section", module.id);
        };
        if !seen.insert(key.to_owned()) {
            bail!("{}: two generated blocks for {key}", module.id);
        }
        if !lines.by_ref().any(|l| l.trim_end() == END) {
            bail!("{}: the block for {key} has no `{END}`", module.id);
        }
        if key == module.id {
            // Sections for actions nobody has placed yet go before the table.
            for a in actions.iter().filter(|a| !text.contains(&format!("{BEGIN}{}", a.id))) {
                out.push_str(&format!("=== {}\n\n{TODO} when and why to use this, then how it looks in YAiST.\n\n", a.title));
                out.push_str(&wrap(a.id, &blocks[a.id]));
                out.push('\n');
                seen.insert(a.id.to_owned());
            }
        }
        out.push_str(&wrap(key, body));
    }
    if !seen.contains(module.id) {
        bail!("{}: the chapter has no `{BEGIN}{}` block for the at-a-glance table", module.id, module.id);
    }
    Ok(out)
}

/// Brings every chapter in `dir/modules` up to date, creating missing ones;
/// with `write` false only reports. Returns the chapters that changed (or
/// would change). Also checks that the book includes each chapter.
pub fn update(reg: &Registry, dir: &std::path::Path, write: bool) -> Result<Vec<String>> {
    let book = std::fs::read_to_string(dir.join(BOOK)).map_err(|e| anyhow::anyhow!("{}: {e}", dir.join(BOOK).display()))?;
    let mut changed = Vec::new();
    for m in &reg.modules {
        let rel = format!("modules/{}.adoc", m.id);
        if !book.contains(&format!("include::{rel}[")) {
            bail!("{BOOK} does not include {rel}");
        }
        let path = dir.join(&rel);
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        let new = render(reg, m, &old)?;
        if new != old {
            if write {
                std::fs::write(&path, &new)?;
            }
            changed.push(rel);
        }
    }
    Ok(changed)
}

/// The book, relative to the handbook directory.
pub const BOOK: &str = "opensuse-handbook.adoc";

fn wrap(key: &str, body: &str) -> String {
    format!("{BEGIN}{key} -- written by `yaist docs` from the module's code; edit the code, not this block\n{body}{END}\n")
}

/// A new chapter: the opening a writer fills in, and the table.
fn skeleton(m: &ModuleSpec) -> String {
    format!(
        "[[{id}]]\n== {title}\n\n{TODO} what this part of the system is for and why it matters, then a list of what the chapter covers.\n\n\
         {BEGIN}{id}\n{END}\n",
        id = m.id,
        title = m.title,
    )
}

/// The table closing a chapter: every action, its command and whether it asks.
fn module_block(m: &ModuleSpec, actions: &[ActionSpec]) -> String {
    if actions.is_empty() {
        return String::new();
    }
    let mut s = format!("[[{}-at-a-glance]]\n=== {} at a glance\n\n", m.id, m.title);
    s.push_str(&format!(".{}: every operation\n[cols=\"3,4,2\",options=\"header\"]\n|===\n|Operation |Command |Runs\n", m.title));
    for a in actions {
        s.push_str(&format!("|<<{},{}>> |`{}` |{}\n", a.id, cell(a.title), cell(&command::format(a, &placeholders(a).0)), runs(a.risk)));
    }
    s.push_str("|===\n");
    s
}

/// One action: what it does, how it asks, its options and the terminal commands.
fn action_block(reg: &Registry, a: &ActionSpec) -> String {
    let (args, ints) = placeholders(a);
    let mut s = format!(
        "[[{id}]]\n[.operation]\n****\n{desc}\n\n*Command:* `{cmd}` +\n*Runs:* {risk}\n",
        id = a.id,
        desc = link_actions(reg, a),
        cmd = command::format(a, &args),
        risk = risk_sentence(a.risk),
    );
    let shown: Vec<_> = a.params.iter().filter(|p| !p.secret || p.required).collect();
    if !shown.is_empty() {
        s.push_str("\n[horizontal]\n");
        for p in shown {
            s.push_str(&format!("`{}`:: {}{}\n", p.name, p.help.trim_end_matches('.'), param_note(p)));
        }
    }

    // The terminal equivalent per flavour; identical ones are shown once.
    let mut variants: Vec<(Vec<&str>, Vec<String>)> = Vec::new();
    for (name, os_release) in FLAVOURS {
        let distro = Distro::from_os_release(os_release, false);
        let cmds: Vec<String> = reg.shell_for(a.id, &args, &distro).into_iter().map(|c| unplaceholder(&c, &ints)).collect();
        match variants.iter_mut().find(|(_, c)| *c == cmds) {
            Some((names, _)) => names.push(name),
            None => variants.push((vec![name], cmds)),
        }
    }
    variants.retain(|(_, c)| !c.is_empty());
    for (names, cmds) in &variants {
        let title = if variants.len() == 1 { "In the terminal".to_owned() } else { format!("In the terminal, on {}", names.join(" and ")) };
        s.push_str(&format!("\n.{title}\n[source,shell]\n----\n{}\n----\n", cmds.join("\n")));
    }
    s.push_str("****\n");
    s
}

/// The description, with the actions it names (written for the model, as
/// `allow_service` or `firewall.allow_service`) turned into links.
fn link_actions(reg: &Registry, a: &ActionSpec) -> String {
    let words: Vec<&str> = a.description.split_inclusive(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.')).collect();
    words
        .iter()
        .map(|w| {
            let (word, rest) = w.split_at(w.trim_end_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.')).len());
            let (word, dot) = match word.strip_suffix('.') {
                Some(stripped) => (stripped, "."),
                None => (word, ""),
            };
            let target = reg.get(word).or_else(|| reg.get(&format!("{}.{word}", a.module()))).filter(|_| word.contains(['_', '.']));
            match target {
                Some(t) if t.id != a.id => format!("<<{},{}>>{dot}{rest}", t.id, cell(t.title)),
                _ => w.to_string(),
            }
        })
        .collect()
}

/// Example arguments: required text and numbers as their names in capitals
/// (`PORT`), choices and switches at their defaults; optional ones left out.
/// Also returns the integer placeholders to put names back in afterwards.
fn placeholders(a: &ActionSpec) -> (Args, Vec<(String, String)>) {
    let mut m = Map::new();
    let mut ints = Vec::new();
    for (i, p) in a.params.iter().enumerate() {
        let upper = p.name.to_uppercase();
        let v = match (p.kind, p.default) {
            (_, Some(d)) => match p.kind {
                ParamKind::Bool => Value::Bool(d == "true"),
                ParamKind::Integer => d.parse::<i64>().map(Value::from).unwrap_or(Value::Null),
                _ => Value::String(d.into()),
            },
            _ if !p.required => continue,
            (ParamKind::Text, None) => Value::String(upper),
            (ParamKind::Integer, None) => {
                let n = INT_PLACEHOLDER + i as i64;
                ints.push((n.to_string(), upper));
                Value::from(n)
            }
            (ParamKind::Choice(opts), None) => Value::String(opts[0].into()),
            (ParamKind::Bool, None) => Value::Bool(false),
        };
        m.insert(p.name.into(), v);
    }
    (Args(m), ints)
}

fn unplaceholder(s: &str, ints: &[(String, String)]) -> String {
    ints.iter().fold(s.to_owned(), |s, (n, name)| s.replace(n, name))
}

fn param_note(p: &yaist_core::Param) -> String {
    let choices = match p.kind {
        ParamKind::Choice(opts) => format!(": {}", opts.iter().map(|o| format!("`{o}`")).collect::<Vec<_>>().join(", ")),
        _ => String::new(),
    };
    let when = match (p.required, p.default) {
        (true, _) => " (required)".to_owned(),
        (false, Some(d)) if !d.is_empty() => format!(" (default: `{d}`)"),
        _ => " (optional)".to_owned(),
    };
    format!("{choices}{when}")
}

fn runs(r: Risk) -> &'static str {
    match r {
        Risk::Read => "straight away",
        Risk::Change => "after you confirm",
        Risk::Dangerous => "after you confirm (hard to undo)",
    }
}

fn risk_sentence(r: Risk) -> &'static str {
    match r {
        Risk::Read => "straight away; it only reads the system.",
        Risk::Change => "after you confirm. YAiST, or the assistant, first shows a plan with the commands it stands for.",
        Risk::Dangerous => {
            "after you confirm. The plan is marked in red: this is hard to undo or changes what the system trusts, \
             so read it before you agree."
        }
    }
}

/// A table cell: `|` would end it.
fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendering_is_stable_and_keeps_prose() {
        let reg = crate::registry();
        let m = reg.find_module("firewall").unwrap();
        let first = render(&reg, m, "").unwrap();
        assert!(first.contains("=== Open port") && first.contains("sudo firewall-cmd --permanent --add-port=PORT/tcp"), "{first}");
        let edited = first.replacen("// TODO(handbook): when and why", "Open a port when", 1);
        assert_eq!(render(&reg, m, &edited).unwrap(), edited);
        assert!(first.contains("Prefer <<firewall.allow_service,Allow service>> when"), "{first}");
    }

    #[test]
    fn unknown_blocks_are_refused() {
        let reg = crate::registry();
        let m = reg.find_module("firewall").unwrap();
        let text = format!("{}{BEGIN}firewall.gone\n{END}\n", render(&reg, m, "").unwrap());
        assert!(render(&reg, m, &text).is_err());
    }
}
