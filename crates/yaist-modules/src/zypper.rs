//! Talking to zypper: queries run as the user with `--xmlout` against cached
//! metadata; changes run as root (or through transactional-update on
//! read-only-root systems). Package changes run interactively: zypper's
//! prompts (dependency conflicts, licenses, keys) go to the user.
//!
//! Later this moves to a libzypp binding (see PLAN.md).

use std::collections::HashMap;

use anyhow::{anyhow, bail, Result};
use yaist_core::ask::{self, Choice, Question};
use yaist_core::{last_lines, Context};

/// Exit codes zypper uses for "succeeded, with something to report":
/// updates/patches available (100-103), nothing found (104), repos skipped (106).
const OK: &[i32] = &[0, 100, 101, 102, 103, 104, 106];

/// Runs a read-only zypper command and returns its XML. It waits for the
/// package lock when another zypper (a running change) holds it, which only
/// happens when we run as root; failures report zypper's own error messages.
pub async fn query(ctx: &Context, args: &[&str]) -> Result<String> {
    let mut argv = vec!["/usr/bin/env", "ZYPP_LOCK_TIMEOUT=60", "/usr/bin/zypper", "--xmlout", "--no-refresh", "--non-interactive"];
    argv.extend_from_slice(args);
    ctx.run_ok(&argv, OK).await.map_err(|e| {
        let msg = e.to_string();
        let errors: Vec<String> = msg
            .find("<?xml")
            .and_then(|start| items(&msg[start..]).ok())
            .into_iter()
            .flatten()
            .filter_map(|i| if let Item::Error(text) = i { Some(text) } else { None })
            .collect();
        if errors.is_empty() {
            e
        } else {
            anyhow!("zypper: {}", errors.join("\n"))
        }
    })
}

/// Runs a zypper command as root and returns the tail of its output. Also on
/// transactional systems for repository changes: /etc is writable there (an
/// overlay), only package changes need a transaction.
pub async fn change(ctx: &Context, args: &[&str]) -> Result<String> {
    let mut argv = vec!["/usr/bin/zypper", "--non-interactive"];
    argv.extend_from_slice(args);
    let out = ctx.run_privileged_ok(&argv, OK).await?;
    Ok(last_lines(out.trim(), 3))
}

/// Changes on transactional systems go into a new snapshot that becomes
/// active after a reboot. `--continue` builds on the snapshot that boots next,
/// so several changes before a reboot all land; without it each call would
/// start again from the running system and drop the previous one.
pub async fn transactional(ctx: &Context, args: &[&str]) -> Result<String> {
    let mut argv = vec!["/usr/sbin/transactional-update", "--non-interactive", "--continue"];
    argv.extend_from_slice(args);
    let out = ctx.run_privileged(&argv).await?;
    Ok(format!("{}\nReboot to switch to the new snapshot.", last_lines(out.trim(), 3)))
}

/// Whether a reboot is needed to finish installed changes: a pending
/// transactional snapshot, or updated core libraries/kernel elsewhere.
pub async fn reboot_needed(ctx: &Context) -> bool {
    if std::path::Path::new("/run/reboot-needed").exists() {
        return true;
    }
    !ctx.distro.transactional && ctx.exit_code(&["/usr/bin/zypper", "needs-rebooting"]).await.ok() == Some(102)
}

/// openSUSE's non-oss repository (freely distributable, not open source:
/// Steam, some firmware and fonts): its alias and whether it is enabled.
/// Debug and source variants don't count.
pub async fn non_oss(ctx: &Context) -> Result<Option<(String, bool)>> {
    let xml = query(ctx, &["repos"]).await?;
    let doc = parse(&xml)?;
    let found = doc
        .descendants()
        .filter(|n| n.has_tag_name("repo"))
        .filter_map(|r| {
            let alias = r.attribute("alias")?.to_owned();
            let url = r.children().find(|c| c.has_tag_name("url")).and_then(|u| u.text()).unwrap_or("");
            let is_non_oss = alias.contains("non-oss") || url.contains("/repo/non-oss");
            let variant = alias.contains("debug") || alias.contains("source") || url.contains("/debug/") || url.contains("/source/");
            (is_non_oss && !variant).then(|| (alias, r.attribute("enabled") == Some("1")))
        })
        .max_by_key(|(_, enabled)| *enabled);
    Ok(found)
}

/// The official non-oss URL for this distribution, with zypper's variables so
/// it keeps working after upgrades.
pub fn non_oss_url(d: &yaist_core::Distro) -> String {
    if d.id.contains("slowroll") || d.pretty.contains("Slowroll") {
        "http://cdn.opensuse.org/slowroll/repo/non-oss/".into()
    } else if d.rolling {
        "http://cdn.opensuse.org/tumbleweed/repo/non-oss/".into()
    } else {
        "http://cdn.opensuse.org/distribution/leap/${releasever}/repo/non-oss/$basearch".into()
    }
}

/// Makes sure non-oss is configured and enabled; returns what was done.
pub async fn ensure_non_oss(ctx: &Context) -> Result<Option<String>> {
    match non_oss(ctx).await? {
        Some((_, true)) => Ok(None),
        Some((alias, false)) => {
            change(ctx, &["modifyrepo", "--enable", "--", &alias]).await?;
            Ok(Some(format!("enabled {alias}")))
        }
        None => {
            let url = non_oss_url(&ctx.distro);
            change(ctx, &["addrepo", "--refresh", "--", &url, "repo-non-oss"]).await?;
            change(ctx, &["refresh", "--repo", "repo-non-oss"]).await?;
            Ok(Some("added repo-non-oss".into()))
        }
    }
}

pub fn parse(xml: &str) -> Result<roxmltree::Document<'_>> {
    roxmltree::Document::parse(xml).map_err(|e| anyhow!("could not read zypper's output: {e}"))
}

/// zypper's `<message type="info">` texts, in order.
pub fn info_messages(doc: &roxmltree::Document<'_>) -> Vec<String> {
    doc.descendants()
        .filter(|n| n.has_tag_name("message") && n.attribute("type") == Some("info"))
        .filter_map(|n| n.text().map(str::to_owned))
        .collect()
}

/// zypper's prompt ids (`src/output/prompt.h`).
const PROMPT_CONTINUE: u32 = 0;
const PROMPT_DEP_RESOLVE: u32 = 1;

/// Exit codes for a finished package change: done, done with a reboot or
/// zypper restart needed, some repositories skipped, an RPM scriptlet failed.
/// Unlike queries, 104 (a package not found) is a failure here.
const CHANGED: &[i32] = &[0, 100, 101, 102, 103, 106, 107];

/// Runs a package change (`install`, `remove`, `update`, `dup`) with zypper's
/// prompts put to the user: zypper reads answers from its terminal, so it
/// gets a pseudo-terminal, and its `--xmlout` stream says what it asks.
/// Several conflicts come as one prompt each, and answering one can raise
/// new ones; every prompt is just answered in turn until zypper is done.
/// With nobody to ask, zypper's default answers apply, as with
/// `--non-interactive` (a conflict cancels).
pub async fn interactive(ctx: &Context, args: &[&str]) -> Result<String> {
    // Wait for the package lock instead of failing on it: a panel query run
    // as root or PackageKit may hold it for a moment. pkexec clears the
    // environment, so the variable goes in through env.
    let mut argv = vec!["/usr/bin/env", "ZYPP_LOCK_TIMEOUT=60", "/usr/bin/zypper", "--xmlout"];
    argv.extend_from_slice(args);
    let mut proc = ctx.spawn_privileged_tty(&argv)?;
    let mut session = Session::default();
    let mut chunk = String::new();
    while let Some(line) = proc.stdout.next_line().await? {
        chunk.push_str(&line);
        chunk.push('\n');
        // zypper waits after each prompt, so everything before one is complete.
        if line.trim_end().ends_with("</prompt>") {
            for item in items(&std::mem::take(&mut chunk))? {
                if let Some(answer) = session.take(item).await {
                    proc.answer(&answer)?;
                }
            }
        }
    }
    for item in items(&chunk)? {
        session.take(item).await;
    }
    let code = proc.wait().await?;
    session.finish(code)
}

/// One element of zypper's XML stream that matters to us.
#[derive(Debug, PartialEq)]
enum Item {
    Info(String),
    Warning(String),
    Error(String),
    /// A step or download zypper marked as failed (`done="0"`).
    Failed(String),
    Summary(Summary),
    Prompt(Prompt),
}

#[derive(Debug, Default, PartialEq)]
struct Prompt {
    id: u32,
    /// Empty when zypper repeats a prompt after an answer it didn't take.
    description: String,
    text: String,
    options: Vec<Choice>,
}

/// What a transaction changes (`<install-summary>`), by package name.
#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    pub install: Vec<String>,
    pub remove: Vec<String>,
    pub upgrade: Vec<String>,
    pub downgrade: Vec<String>,
    /// Reinstalls and architecture changes.
    pub other: Vec<String>,
    pub download_bytes: u64,
    pub reboot: bool,
}

impl Summary {
    /// Changes beyond what was asked for, worth a second look.
    fn needs_review(&self) -> bool {
        !self.remove.is_empty() || !self.downgrade.is_empty() || !self.other.is_empty()
    }

    fn groups(&self) -> [(&'static str, &[String]); 5] {
        [("remove", &self.remove), ("downgrade", &self.downgrade), ("change", &self.other), ("install", &self.install), ("upgrade", &self.upgrade)]
    }

    /// One line, e.g. "12 installed, 3 removed".
    pub fn headline(&self) -> String {
        let done = |verb: &str| match verb {
            "remove" => "removed",
            "downgrade" => "downgraded",
            "change" => "changed",
            "install" => "installed",
            _ => "upgraded",
        };
        let parts: Vec<String> = self.groups().iter().filter(|(_, p)| !p.is_empty()).map(|(v, p)| format!("{} {}", p.len(), done(v))).collect();
        if parts.is_empty() {
            "nothing to do".into()
        } else {
            parts.join(", ")
        }
    }

    /// Every change, removals first, for the confirmation question.
    pub fn describe(&self) -> String {
        const SHOWN: usize = 12;
        let mut out = Vec::new();
        for (verb, pkgs) in self.groups() {
            if pkgs.is_empty() {
                continue;
            }
            let mut names = pkgs.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
            if pkgs.len() > SHOWN {
                names.push_str(&format!(" and {} more", pkgs.len() - SHOWN));
            }
            let mut verb = verb.to_owned();
            verb[..1].make_ascii_uppercase();
            out.push(format!("{verb} {}: {names}", pkgs.len()));
        }
        if self.download_bytes > 0 {
            out.push(format!("Download: {:.1} MiB", self.download_bytes as f64 / (1024.0 * 1024.0)));
        }
        if self.reboot {
            out.push("A reboot is needed afterwards.".into());
        }
        out.join("\n")
    }
}

/// Parses a piece of the stream: everything up to and including a prompt,
/// or the rest at the end. The `<stream>` wrapper is dropped wherever it
/// falls; zypper escapes text, so the tags can't appear inside it.
fn items(chunk: &str) -> Result<Vec<Item>> {
    let body = chunk.replace("<?xml version='1.0'?>", "").replace("<stream>", "").replace("</stream>", "");
    let wrapped = format!("<chunk>{body}</chunk>");
    let doc = parse(&wrapped)?;
    let child_text =
        |n: roxmltree::Node<'_, '_>, tag: &str| n.children().find(|c| c.has_tag_name(tag)).and_then(|c| c.text()).unwrap_or("").to_owned();
    let mut out = Vec::new();
    for n in doc.root_element().children().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "message" => {
                let text = n.text().unwrap_or("").to_owned();
                out.push(match n.attribute("type") {
                    Some("error") => Item::Error(text),
                    Some("warning") => Item::Warning(text),
                    _ => Item::Info(text),
                });
            }
            // done="1" is success, done="0" failure (the schema's comment
            // has it backwards).
            "progress" if n.attribute("done") == Some("0") => out.push(Item::Failed(n.attribute("name").unwrap_or("").to_owned())),
            "download" if n.attribute("done") == Some("0") => out.push(Item::Failed(format!("downloading {}", n.attribute("url").unwrap_or("")))),
            "prompt" => out.push(Item::Prompt(Prompt {
                id: n.attribute("id").and_then(|i| i.parse().ok()).unwrap_or(u32::MAX),
                description: child_text(n, "description"),
                text: child_text(n, "text"),
                options: n
                    .children()
                    .filter(|c| c.has_tag_name("option"))
                    .map(|o| Choice {
                        value: o.attribute("value").unwrap_or("").to_owned(),
                        label: o.attribute("desc").unwrap_or("").to_owned(),
                        default: o.attribute("default") == Some("1"),
                    })
                    .collect(),
            })),
            "install-summary" => {
                let mut s = Summary {
                    download_bytes: n.attribute("download-size").and_then(|d| d.parse().ok()).unwrap_or(0),
                    reboot: n.attribute("need-reboot") == Some("1"),
                    ..Default::default()
                };
                for group in n.children().filter(|c| c.is_element()) {
                    let names = group.children().filter(|c| c.has_tag_name("solvable")).filter_map(|p| p.attribute("name")).map(str::to_owned);
                    match group.tag_name().name() {
                        "to-install" => s.install.extend(names),
                        "to-remove" => s.remove.extend(names),
                        "to-upgrade" => s.upgrade.extend(names),
                        "to-downgrade" | "to-downgrade-change-arch" => s.downgrade.extend(names),
                        "to-upgrade-change-arch" | "to-reinstall" | "to-change-arch" => s.other.extend(names),
                        _ => {}
                    }
                }
                out.push(Item::Summary(s));
            }
            _ => {}
        }
    }
    Ok(out)
}

/// A dependency problem as zypper words it: the problem, then one line per
/// solution ("Solution 2: do not install postfix-3.11…"), a solution of
/// several actions spread over indented lines.
#[derive(Debug, PartialEq)]
struct Problem {
    number: Option<usize>,
    text: String,
    solutions: Vec<String>,
}

fn split_problem(description: &str) -> Problem {
    let mut text = Vec::new();
    let mut solutions: Vec<Vec<String>> = Vec::new();
    for line in description.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Solution ").and_then(|r| r.split_once(": ")).map(|(_, r)| r) {
            solutions.push(vec![rest.to_owned()]);
        } else if let Some(sol) = solutions.last_mut() {
            if !trimmed.is_empty() {
                sol.push(trimmed.to_owned());
            }
        } else {
            text.push(line);
        }
    }
    let text = text.join("\n").trim().to_owned();
    let (number, text) = match text.strip_prefix("Problem: ").and_then(|r| r.split_once(": ")) {
        Some((n, rest)) if n.parse::<usize>().is_ok() => (n.parse().ok(), rest.to_owned()),
        _ => (None, text),
    };
    let solutions = solutions
        .into_iter()
        .map(|mut parts| {
            if parts.len() > 1 && parts[0].ends_with(':') {
                parts.remove(0);
            }
            parts.join("; ")
        })
        .collect();
    Problem { number, text, solutions }
}

fn prompt_title(id: u32) -> &'static str {
    match id {
        4 => "Download problem",
        5 => "Repository problem",
        6 => "Package removal problem",
        7 => "Package installation problem",
        8 => "Media problem",
        9 => "Change media",
        10 => "License agreement",
        11 => "Unsigned file",
        13 => "Unknown signing key",
        14 => "Trust this signing key?",
        15 => "Signature check failed",
        16..=18 => "Checksum problem",
        19 => "Remove the lock?",
        20 => "Patch message",
        21 => "Patch script problem",
        24 => "File conflicts",
        _ => "zypper asks",
    }
}

/// Follows one zypper run: turns its prompts into questions and remembers
/// what it needs to report at the end.
#[derive(Default)]
struct Session {
    /// From "3 Problems:", for numbering the conflict questions.
    problems: usize,
    /// The last description of each prompt, for repeated prompts.
    descriptions: HashMap<u32, String>,
    summary: Option<Summary>,
    /// Errors, warnings and failed steps in order: why a transaction failed.
    trouble: Vec<String>,
    /// How much of `trouble` came before the last prompt; the rest explains
    /// the next one ("File … not found on medium", then "Abort, retry, ignore?").
    trouble_asked: usize,
    /// The user picked a solution, so the result differs from the request.
    resolved: bool,
    /// The conflict that was cancelled, if one was.
    unresolved: Option<String>,
    declined: bool,
    /// Repositories zypper couldn't refresh and went on without.
    skipped_repos: Vec<String>,
}

impl Session {
    /// Takes in one item; returns the answer when it is a prompt.
    async fn take(&mut self, item: Item) -> Option<String> {
        match item {
            Item::Info(text) => {
                if let Some(n) = text.strip_suffix(" Problems:").and_then(|n| n.trim().parse().ok()) {
                    self.problems = n;
                }
                None
            }
            Item::Warning(text) => {
                if let Some(repo) = text.strip_prefix("Skipping repository '").and_then(|r| r.split_once('\'')).map(|(r, _)| r) {
                    self.skipped_repos.push(repo.to_owned());
                }
                self.trouble.push(text);
                None
            }
            Item::Error(text) => {
                self.trouble.push(text);
                None
            }
            Item::Failed(what) => {
                self.trouble.push(format!("failed: {what}"));
                None
            }
            Item::Summary(s) => {
                self.summary = Some(s);
                None
            }
            Item::Prompt(p) => Some(self.answer(p).await),
        }
    }

    async fn answer(&mut self, mut p: Prompt) -> String {
        if p.description.is_empty() {
            p.description = self.descriptions.get(&p.id).cloned().unwrap_or_default();
        } else {
            self.descriptions.insert(p.id, p.description.clone());
        }
        let Some(q) = self.question(&p) else {
            // Nothing worth asking: accept what zypper proposes.
            return p.options.iter().find(|o| o.default).map(|o| o.value.clone()).unwrap_or_default();
        };
        self.trouble_asked = self.trouble.len();
        let default = q.default_choice().map(|c| c.value.clone()).unwrap_or_default();
        // Only ever send what zypper offered; anything else makes it ask again.
        let value = ask::ask(q.clone()).await.filter(|v| q.choices.iter().any(|c| &c.value == v)).unwrap_or(default);
        match p.id {
            PROMPT_DEP_RESOLVE if value.parse::<usize>().is_ok() => self.resolved = true,
            PROMPT_DEP_RESOLVE if value == "c" => self.unresolved = Some(split_problem(&p.description).text),
            PROMPT_CONTINUE if value == "n" => self.declined = true,
            _ => {}
        }
        value
    }

    /// The question for a prompt, or `None` to take zypper's default.
    fn question(&self, p: &Prompt) -> Option<Question> {
        // Display toggles and the pager are for terminals, and a new base URI
        // would have to be typed.
        let offered = || {
            p.options.iter().filter(|o| {
                !o.label.starts_with("Toggle") && !o.label.starts_with("View") && !o.label.starts_with("Change current base URI") && o.value != "?"
            })
        };
        match p.id {
            PROMPT_CONTINUE => {
                let summary = self.summary.as_ref();
                if !self.resolved && !summary.is_some_and(Summary::needs_review) {
                    return None;
                }
                let choices = vec![
                    Choice { value: "y".into(), label: "Apply these changes".into(), default: false },
                    Choice { value: "n".into(), label: "Cancel, change nothing".into(), default: true },
                ];
                Some(Question { title: "Apply these changes?".into(), text: summary.map(Summary::describe).unwrap_or_default(), choices })
            }
            PROMPT_DEP_RESOLVE => {
                let problem = split_problem(&p.description);
                let title = match (problem.number, self.problems) {
                    (Some(n), total) if total > 1 => format!("Package conflict {n} of {total}"),
                    _ => "Package conflict".into(),
                };
                let choices = offered()
                    .filter_map(|o| {
                        let label = match o.value.as_str() {
                            v if v.parse::<usize>().is_ok() => {
                                let sol = problem.solutions.get(v.parse::<usize>().ok()? - 1).cloned().unwrap_or_else(|| o.label.clone());
                                if sol.contains("by ignoring some of its dependencies") {
                                    format!("{sol} (not recommended)")
                                } else {
                                    sol
                                }
                            }
                            "s" => "Skip this conflict for now".into(),
                            "c" => "Cancel, change nothing".into(),
                            // Retrying gives the same answer.
                            _ => return None,
                        };
                        Some(Choice { label, ..o.clone() })
                    })
                    .collect();
                Some(Question { title, text: problem.text, choices })
            }
            id => {
                let choices: Vec<Choice> = offered().cloned().collect();
                if choices.is_empty() {
                    return None;
                }
                let recent = self.trouble[self.trouble_asked..].join("\n");
                let text =
                    [recent.trim(), p.description.trim(), p.text.trim()].iter().filter(|t| !t.is_empty()).copied().collect::<Vec<_>>().join("\n\n");
                Some(Question { title: prompt_title(id).into(), text, choices })
            }
        }
    }

    fn finish(self, code: i32) -> Result<String> {
        if self.declined {
            bail!("cancelled, no packages were changed");
        }
        if let Some(problem) = self.unresolved {
            bail!("unresolved package conflict, no packages were changed: {problem}");
        }
        if !CHANGED.contains(&code) {
            // zypper's closing "Installation has completed with error." says
            // nothing; what failed comes before it.
            let detail = if self.trouble.is_empty() { format!("zypper exited with code {code}") } else { last_lines(&self.trouble.join("\n"), 10) };
            bail!("{detail}");
        }
        let mut out = self.summary.map(|s| s.headline()).unwrap_or_else(|| "nothing to do".into());
        if !self.skipped_repos.is_empty() {
            out.push_str(&format!("; without the repositories that could not be refreshed: {} (see Repositories)", self.skipped_repos.join(", ")));
        }
        if code == 107 {
            out.push_str("; a package script failed, see /var/log/zypp/history");
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Recorded from `zypper --xmlout install postfix sendmail exim
    /// pipewire-pulseaudio pulseaudio busybox-which which` on Tumbleweed.
    const THREE_CONFLICTS: &str = "<?xml version='1.0'?>
<stream>
<message type=\"info\">Resolving package dependencies...</message>
<message type=\"info\">3 Problems:</message>
<message type=\"info\">Problem: 1: the to be installed busybox-which-1.38.0-42.2.noarch conflicts with &apos;which&apos; provided by the to be installed which-2.25-1.3.x86_64</message>
<prompt id=\"1\">
<description>Problem: 1: the to be installed busybox-which-1.38.0-42.2.noarch conflicts with &apos;which&apos; provided by the to be installed which-2.25-1.3.x86_64
 Solution 1: do not install which-2.25-1.3.x86_64
 Solution 2: do not install busybox-which-1.38.0-42.2.noarch
</description>
<text>Choose from above solutions by number or skip, retry or cancel</text>
<option value=\"1\" desc=\"Choose solution 1\"/>
<option value=\"2\" desc=\"Choose solution 2\"/>
<option value=\"s\" desc=\"Skip problem and continue.\"/>
<option value=\"r\" desc=\"Retry solving immediately.\"/>
<option default=\"1\" value=\"c\" desc=\"Choose no solution and cancel.\"/>
<option value=\"d\" desc=\"Toggle show detailed conflict information.\"/>
</prompt>
";

    const SECOND_CONFLICT: &str = "<prompt id=\"1\">
<description>Problem: 2: the to be installed exim-4.98.2-4.6.x86_64 conflicts with &apos;sendmail&apos; provided by the to be installed sendmail-8.18.2-3.3.x86_64
 Solution 1: Following actions will be done:
  do not install exim-4.98.2-4.6.x86_64
  do not install postfix-3.11.7-2.1.x86_64
 Solution 2: Following actions will be done:
  do not install sendmail-8.18.2-3.3.x86_64
  do not install postfix-3.11.7-2.1.x86_64
</description>
<text>Choose from above solutions by number or skip, retry or cancel</text>
<option value=\"1\" desc=\"Choose solution 1\"/>
<option value=\"2\" desc=\"Choose solution 2\"/>
<option value=\"s\" desc=\"Skip problem and continue.\"/>
<option value=\"r\" desc=\"Retry solving immediately.\"/>
<option default=\"1\" value=\"c\" desc=\"Choose no solution and cancel.\"/>
<option value=\"d\" desc=\"Toggle show detailed conflict information.\"/>
</prompt>
";

    const SUMMARY: &str = "<message type=\"info\">Resolving dependencies...</message>
<install-summary download-size=\"2097152\" space-usage-diff=\"1\" space-usage-installed=\"1\" space-usage-removed=\"0\" packages-to-change=\"3\" need-restart=\"0\" need-reboot=\"0\">
<to-install>
<solvable type=\"package\" name=\"sendmail\" edition=\"8.18.2-3.3\" arch=\"x86_64\" repository=\"repo-oss\" summary=\"BSD Sendmail\">
<description>The &quot;Unix System Administration Handbook&quot;
calls sendmail...</description></solvable>
<solvable type=\"package\" name=\"procmail\" edition=\"3.24-5.5\" arch=\"x86_64\" repository=\"repo-oss\" summary=\"Local delivery\"/>
</to-install>
<to-remove>
<solvable type=\"package\" name=\"postfix\" edition=\"3.11.7-2.1\" arch=\"x86_64\" repository=\"@System\"/>
</to-remove>
</install-summary>
<prompt id=\"0\">
<text>Continue?</text>
<option default=\"1\" value=\"y\" desc=\"Yes, accept the summary and proceed with installation/removal of packages.\"/>
<option value=\"n\" desc=\"No, cancel the operation.\"/>
<option value=\"v\" desc=\"Toggle display of package versions.\"/>
<option value=\"g\" desc=\"View the summary in pager.\"/>
</prompt>
";

    fn prompt(chunk: &str) -> Prompt {
        items(chunk).unwrap().into_iter().find_map(|i| if let Item::Prompt(p) = i { Some(p) } else { None }).unwrap()
    }

    async fn feed(session: &mut Session, chunk: &str) -> Vec<String> {
        let mut answers = Vec::new();
        for item in items(chunk).unwrap() {
            answers.extend(session.take(item).await);
        }
        answers
    }

    #[test]
    fn error_messages_are_read_from_the_stream() {
        let failed = "/usr/bin/env failed (exit status: 7): <?xml version='1.0'?>\n<stream>\n<message type=\"error\">System management is locked.</message>\n</stream>";
        let start = failed.find("<?xml").unwrap();
        assert_eq!(items(&failed[start..]).unwrap(), [Item::Error("System management is locked.".into())]);
    }

    #[test]
    fn parses_the_stream_up_to_a_prompt() {
        let items = items(THREE_CONFLICTS).unwrap();
        assert_eq!(items[1], Item::Info("3 Problems:".into()));
        let Item::Prompt(p) = &items[3] else { panic!("{items:?}") };
        assert_eq!(p.id, PROMPT_DEP_RESOLVE);
        assert!(p.description.contains("conflicts with 'which'"));
        assert_eq!(p.options.iter().map(|o| o.value.as_str()).collect::<Vec<_>>(), ["1", "2", "s", "r", "c", "d"]);
        assert!(p.options[4].default);
    }

    #[test]
    fn splits_problems_and_multi_action_solutions() {
        let p = split_problem(&prompt(SECOND_CONFLICT).description);
        assert_eq!(p.number, Some(2));
        assert!(p.text.starts_with("the to be installed exim"));
        assert_eq!(
            p.solutions,
            [
                "do not install exim-4.98.2-4.6.x86_64; do not install postfix-3.11.7-2.1.x86_64",
                "do not install sendmail-8.18.2-3.3.x86_64; do not install postfix-3.11.7-2.1.x86_64",
            ]
        );
    }

    #[test]
    fn conflict_questions_are_numbered_and_offer_only_real_choices() {
        let s = Session { problems: 3, ..Default::default() };
        let q = s.question(&prompt(SECOND_CONFLICT)).unwrap();
        assert_eq!(q.title, "Package conflict 2 of 3");
        let values: Vec<&str> = q.choices.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(values, ["1", "2", "s", "c"]);
        assert!(q.choices[0].label.starts_with("do not install exim"));
        assert_eq!(q.default_choice().unwrap().value, "c");
    }

    #[test]
    fn summary_lists_removals_first() {
        let Some(Item::Summary(sum)) = items(SUMMARY).unwrap().into_iter().find(|i| matches!(i, Item::Summary(_))) else { panic!() };
        assert_eq!(sum.install, ["sendmail", "procmail"]);
        assert_eq!(sum.remove, ["postfix"]);
        assert!(sum.needs_review());
        assert_eq!(sum.headline(), "1 removed, 2 installed");
        assert_eq!(sum.describe(), "Remove 1: postfix\nInstall 2: sendmail, procmail\nDownload: 2.0 MiB");
    }

    #[tokio::test]
    async fn plain_installs_go_ahead_without_a_second_question() {
        let mut s = Session { summary: Some(Summary { install: vec!["fish".into()], ..Default::default() }), ..Default::default() };
        let p = prompt(&SUMMARY[SUMMARY.find("<prompt").unwrap()..]);
        assert!(s.question(&p).is_none());
        assert_eq!(s.answer(p).await, "y");
        assert_eq!(s.finish(0).unwrap(), "1 installed");
    }

    #[tokio::test]
    async fn with_nobody_to_ask_a_conflict_cancels() {
        let mut s = Session::default();
        assert_eq!(feed(&mut s, THREE_CONFLICTS).await, ["c"]);
        let err = s.finish(4).unwrap_err().to_string();
        assert!(err.starts_with("unresolved package conflict"), "{err}");
        assert!(err.contains("busybox-which"), "{err}");
    }

    #[tokio::test]
    async fn the_user_works_through_each_conflict_then_confirms() {
        // Answers: solution 2, then solution 1, then apply.
        let script = Arc::new(Mutex::new(vec!["y", "1", "2"]));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (script2, asked2) = (script.clone(), asked.clone());
        let asker: ask::Asker = Arc::new(move |q: Question| {
            asked2.lock().unwrap().push(q.title.clone());
            let a = script2.lock().unwrap().pop().map(str::to_owned);
            Box::pin(async move { a })
        });
        let (answers, s) = ask::asking(asker, async {
            let mut s = Session::default();
            let mut answers = feed(&mut s, THREE_CONFLICTS).await;
            answers.extend(feed(&mut s, SECOND_CONFLICT).await);
            answers.extend(feed(&mut s, SUMMARY).await);
            (answers, s)
        })
        .await;
        assert_eq!(answers, ["2", "1", "y"]);
        assert_eq!(*asked.lock().unwrap(), ["Package conflict 1 of 3", "Package conflict 2 of 3", "Apply these changes?"]);
        assert_eq!(s.finish(0).unwrap(), "1 removed, 2 installed");
    }

    #[tokio::test]
    async fn answers_zypper_did_not_offer_fall_back_to_the_default() {
        let asker: ask::Asker = Arc::new(|_| Box::pin(async { Some("r".to_owned()) }));
        let answer = ask::asking(asker, async { Session::default().answer(prompt(THREE_CONFLICTS)).await }).await;
        assert_eq!(answer, "c");
    }

    #[tokio::test]
    async fn repeated_prompts_keep_their_description() {
        let mut s = Session::default();
        s.answer(prompt(THREE_CONFLICTS)).await;
        let mut again = prompt(THREE_CONFLICTS);
        again.description.clear();
        again.text = ": Invalid answer 'x'.".into();
        s.answer(again).await;
        assert!(s.unresolved.unwrap().contains("busybox-which"));
    }

    /// Recorded from `zypper --xmlout update` with an OBS repository that has
    /// no build for this distribution.
    const BROKEN_REPO: &str = "<message type=\"error\">Error building the cache:
[home_lkocman_branches_graphics|https://download.opensuse.org/repositories/home:/lkocman:/branches:/graphics/16.0/] Failed to retrieve new repository metadata.
 - [home_lkocman_branches_graphics|https://download.opensuse.org/repositories/home:/lkocman:/branches:/graphics/16.0/] Repository type can&apos;t be determined.
</message>
<message type=\"warning\">Skipping repository &apos;home_lkocman_branches_graphics&apos; because of the above error.</message>
<message type=\"error\">Some of the repositories have not been refreshed because of an error.</message>
";

    #[tokio::test]
    async fn skipped_repositories_and_failed_scripts_are_reported() {
        let mut s = Session::default();
        feed(&mut s, BROKEN_REPO).await;
        assert_eq!(s.skipped_repos, ["home_lkocman_branches_graphics"]);
        let out = s.finish(107).unwrap();
        assert!(out.contains("could not be refreshed: home_lkocman_branches_graphics"), "{out}");
        assert!(out.ends_with("a package script failed, see /var/log/zypp/history"), "{out}");
    }

    /// Recorded from `zypper --xmlout install hello` with the package file
    /// gone from a local repository.
    const MISSING_FILE: &str =
        "<message type=\"error\">File &apos;./hello-2.12.3-1.4.x86_64.rpm&apos; not found on medium &apos;dir:/tmp/repo&apos;</message>
<prompt id=\"8\">
<text>Abort, retry, ignore?</text>
<option default=\"1\" value=\"a\" desc=\"Skip retrieval of the file and abort current operation.\"/>
<option value=\"r\" desc=\"Try to retrieve the file again.\"/>
<option value=\"i\" desc=\"Skip retrieval of the file and try to continue with the operation without the file.\"/>
<option value=\"u\" desc=\"Change current base URI and try retrieving the file again.\"/>
</prompt>
";

    #[test]
    fn media_problems_say_what_failed() {
        let mut s = Session::default();
        let mut items = items(MISSING_FILE).unwrap().into_iter();
        let Some(Item::Error(e)) = items.next() else { panic!() };
        s.trouble.push(e);
        let Some(Item::Prompt(p)) = items.next() else { panic!() };
        let q = s.question(&p).unwrap();
        assert_eq!(q.title, "Media problem");
        assert!(q.text.starts_with("File './hello-2.12.3-1.4.x86_64.rpm' not found on medium"), "{}", q.text);
        assert_eq!(q.choices.iter().map(|c| c.value.as_str()).collect::<Vec<_>>(), ["a", "r", "i"]);
    }

    #[tokio::test]
    async fn a_failed_commit_explains_itself() {
        let mut s = Session::default();
        let stream = "<download url=\"https://example.org/x86_64/foo-1.rpm\" rate=\"0\" done=\"0\"/>
<progress id=\"\" name=\"(1/2) Installing: bar-2\" done=\"0\"/>
<message type=\"warning\">bar-2: %post scriptlet failed</message>
<message type=\"error\">Installation has completed with error.</message>
";
        feed(&mut s, stream).await;
        let err = s.finish(8).unwrap_err().to_string();
        assert_eq!(
            err,
            "failed: downloading https://example.org/x86_64/foo-1.rpm\nfailed: (1/2) Installing: bar-2\nbar-2: %post scriptlet failed\nInstallation has completed with error."
        );
    }

    #[tokio::test]
    async fn declining_the_summary_changes_nothing() {
        let asker: ask::Asker = Arc::new(|_| Box::pin(async { Some("n".to_owned()) }));
        let s = ask::asking(asker, async {
            let mut s = Session::default();
            feed(&mut s, SUMMARY).await;
            s
        })
        .await;
        assert!(s.finish(0).unwrap_err().to_string().contains("no packages were changed"));
    }
}
