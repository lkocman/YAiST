//! The client model shared by the YAiST TUI and GUI: same modules, same
//! panels, same plan cards, same conversation. A feature added here shows up
//! in both.

/// The name users see. YaST with AI inside; commands and crates stay `yaist`.
pub const NAME: &str = "YAiST";

/// The wordmark, piece by piece: (text, in the accent colour).
pub const WORDMARK: [(&str, bool); 3] = [("YA", false), ("i", true), ("ST", false)];

pub mod demo;
pub mod palette;
mod session;
mod state;
pub mod welcome;

pub use session::{Cmd, Session};
pub use state::{Asked, Entry, LlmStatus, ModuleView, Panel, Plan, PlanStatus, State, UpdateStatus};

/// The action behind "Install updates": `zypper update` on Leap, `zypper dup`
/// on rolling releases, transactional-update on immutable systems.
pub const UPDATE_ACTION: &str = "packages.update_system";

/// "Install security updates only" (Leap: zypper patch --category security).
pub const SECURITY_ACTION: &str = "packages.install_security";

/// The action behind "Report a problem…".
pub const REPORT_ACTION: &str = yaist_modules::report::REPORT.id;

/// How urgent the update state is, for colouring the badge and banner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateLevel {
    Unknown,
    UpToDate,
    Available,
    Security,
}

/// How to start the local model here: the service from yaist-offline, the
/// podman script the yaist package ships, or the one in a source checkout.
pub fn llm_start_hint() -> &'static str {
    if std::path::Path::new("/usr/lib/systemd/system/yaist-llm.service").exists() {
        "sudo systemctl enable --now yaist-llm (if it is enabled already: systemctl status yaist-llm)"
    } else if std::path::Path::new("/usr/libexec/yaist/llm-server.sh").exists() {
        "/usr/libexec/yaist/llm-server.sh"
    } else {
        "scripts/llm-server.sh"
    }
}

/// The reboot reminder, when installed changes wait for one: (headline, detail).
pub fn reboot_notice(u: &UpdateStatus, transactional: bool) -> Option<(String, String)> {
    u.reboot_needed().then(|| {
        let detail = if transactional {
            "Changes were installed into a new snapshot. It becomes the running system after a reboot; until then nothing has changed."
        } else {
            "A new kernel or core libraries were installed. Reboot to start using them."
        };
        ("Reboot to finish".to_owned(), detail.to_owned())
    })
}

pub const REBOOT_ACTION: &str = "system.reboot";

/// Text for the update banner and badge: (level, badge, headline, detail).
/// On transactional systems updates go into a new snapshot, and the text
/// says so.
pub fn update_summary(u: &UpdateStatus, transactional: bool) -> (UpdateLevel, String, String, String) {
    let ago = |checked: &std::time::Instant| match checked.elapsed().as_secs() / 60 {
        0 => "Checked just now".to_owned(),
        1 => "Checked a minute ago".to_owned(),
        m => format!("Checked {m} minutes ago"),
    };
    let how = if transactional { " They install into a new snapshot; reboot to use it." } else { "" };
    match u {
        UpdateStatus::Checking => (UpdateLevel::Unknown, "checking updates".into(), "Checking for updates…".into(), String::new()),
        UpdateStatus::Failed(e) => (UpdateLevel::Unknown, "updates unknown".into(), "Could not check for updates".into(), e.clone()),
        UpdateStatus::Known { packages: 0, security: 0, checked, .. } => (
            UpdateLevel::UpToDate,
            "up to date".into(),
            "Your system is up to date".into(),
            format!("{}, from the last repository refresh", ago(checked)),
        ),
        UpdateStatus::Known { packages, security, checked, .. } => {
            let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
            if *security > 0 {
                // Leap counts security fixes as patches, which can cover
                // several packages each, so both numbers are given as such.
                let headline = plural(*security, "security patch pending", "security patches pending");
                let badge = format!("{packages} updates · {security} security");
                let detail = format!(
                    "{}, some of them security fixes: install them soon.{how} {}.",
                    plural(*packages, "package can be updated", "packages can be updated"),
                    ago(checked)
                );
                (UpdateLevel::Security, badge, headline, detail)
            } else {
                let headline = plural(*packages, "update available", "updates available");
                (UpdateLevel::Available, format!("{packages} updates"), headline, format!("Bug fixes and improvements.{how} {}.", ago(checked)))
            }
        }
    }
}

use yaist_core::{ActionSpec, Output, Table};

/// What a status cell means, so both clients colour it the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Warn,
    Bad,
    Faint,
    Plain,
}

pub fn tone_of(value: &str) -> Tone {
    match value {
        "active" | "running" | "enabled" | "yes" | "on" | "installed" | "enforcing" | "idle" | "printing" | "ready" | "ok" => Tone::Good,
        "failed" | "masked" | "broken" => Tone::Bad,
        "activating" | "deactivating" | "reloading" | "permissive" | "missing" | "rc" | "beta" | "alpha" => Tone::Warn,
        "inactive" | "dead" | "disabled" | "no" | "off" | "static" | "-" | "available" | "exited" | "indirect" | "optional" | "not needed"
        | "not available" => Tone::Faint,
        _ => Tone::Plain,
    }
}

/// Columns whose values are states and get a badge.
pub fn is_status_column(key: &str) -> bool {
    matches!(key, "active" | "sub" | "enabled" | "is_admin" | "setting" | "status")
}

/// Whether a failed plan is worth a report at all: not when YAiST refused
/// before running anything (wrong input: a group that doesn't exist), and
/// not when the user cancelled.
pub fn reportable(plan: &Plan) -> bool {
    let PlanStatus::Failed(error) = &plan.status else { return false };
    let cancelled = ["cancelled", "authentication was cancelled"].iter().any(|c| error.starts_with(c));
    !plan.executed.is_empty() && !cancelled && plan.spec.id != REPORT_ACTION
}

/// The snapshot the helper took before an upgrade, as it said in the log
/// (`==> snapshot N taken before the upgrade`): where a rollback goes back to.
pub fn snapshot_before(plan: &Plan) -> Option<u32> {
    if plan.spec.module() != "upgrade" {
        return None;
    }
    plan.log.iter().find_map(|l| l.strip_prefix("==> snapshot ")?.strip_suffix(" taken before the upgrade")?.parse().ok())
}

/// How the helper judged an upgrade (`==> verdict: …`): unchanged,
/// incomplete, migrated or migrated-with-warnings.
pub fn verdict(plan: &Plan) -> Option<&str> {
    plan.log.iter().rev().find_map(|l| l.strip_prefix("==> verdict: "))
}

/// The distribution's short name for "Report a … bug": Leap, Tumbleweed,
/// Slowroll, MicroOS, Aeon, Kalpa, Leap Micro.
pub fn distro_short(d: &yaist_core::Distro) -> &'static str {
    match d.id.as_str() {
        "opensuse-leap" => "Leap",
        "opensuse-leap-micro" => "Leap Micro",
        "opensuse-microos" => "MicroOS",
        "opensuse-aeon" => "Aeon",
        "opensuse-kalpa" => "Kalpa",
        _ if d.id.contains("slowroll") || d.pretty.contains("Slowroll") => "Slowroll",
        "opensuse-tumbleweed" => "Tumbleweed",
        _ => "openSUSE",
    }
}

/// Where a failure most likely belongs ("opensuse" or "yaist"), and why.
pub fn report_suggestion(error: &str) -> (&'static str, &'static str) {
    yaist_modules::report::suggest(error)
}

/// How much of a failed change's output goes into a report; the link has
/// room for little, and report.rs shortens the middle if needed.
const REPORT_LOG_LINES: usize = 30;

/// Arguments for a problem report about a failed plan, from what our code
/// knows: what was asked, what YAiST ran and the error. The user chose where
/// it goes and completes the rest on the report page.
pub fn report_args(plan: &Plan, target: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let PlanStatus::Failed(error) = &plan.status else { return None };
    let first = error.lines().next().unwrap_or("").chars().take(80).collect::<String>();
    let mut observation = format!("YAiST action: {}\n", plan.command);
    if !plan.executed.is_empty() {
        observation.push_str(&format!("\nYAiST ran:\n```\n{}\n```\n", plan.executed.join("\n")));
    }
    observation.push_str(&format!("\nIt failed with:\n```\n{error}\n```"));
    // What the commands printed last says most; the whole of it is on disk.
    if !plan.log.is_empty() {
        let tail = &plan.log[plan.log.len().saturating_sub(REPORT_LOG_LINES)..];
        observation.push_str(&format!("\n\nOutput (last {} lines):\n```\n{}\n```", tail.len(), tail.join("\n")));
    }
    if let Some(f) = &plan.log_file {
        observation.push_str(&format!("\n\nThe full output is in {f} (please attach it)."));
    }
    Some(
        [("target", target.to_owned()), ("title", format!("{} fails: {first}", plan.spec.title)), ("observation", observation)]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), serde_json::Value::String(v)))
            .collect(),
    )
}

/// Raw arguments for a row action, taken from the row's matching columns.
pub fn row_args(spec: &ActionSpec, table: &Table, row: &[String]) -> serde_json::Map<String, serde_json::Value> {
    let mut raw = serde_json::Map::new();
    for p in spec.params {
        if let Some(v) = table.cell(row, p.name) {
            raw.insert(p.name.to_owned(), serde_json::Value::String(v.to_owned()));
        }
    }
    raw
}

/// Whether a row action can be proposed straight from the row: every
/// required parameter comes from a column. Otherwise a form opens first.
pub fn row_action_is_direct(spec: &ActionSpec, table: &Table) -> bool {
    spec.params.iter().filter(|p| p.required).all(|p| table.columns.iter().any(|c| c.key == p.name))
}

pub fn output_rows(out: &Output) -> usize {
    match out {
        Output::Table(t) => t.rows.len(),
        Output::Facts(f) => f.len(),
        _ => 1,
    }
}

/// A form generated from an action's parameters; both clients edit `values`
/// with their own widgets.
#[derive(Clone, Debug)]
pub struct Form {
    pub spec: ActionSpec,
    /// One per parameter, as text; booleans are "true"/"false".
    pub values: Vec<String>,
    /// The plan this form edits, cancelled when the form is submitted.
    pub replaces: Option<u64>,
}

impl Form {
    pub fn new(spec: ActionSpec, prefill: &serde_json::Map<String, serde_json::Value>) -> Self {
        let values = spec
            .params
            .iter()
            .map(|p| match prefill.get(p.name) {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => match p.kind {
                    yaist_core::ParamKind::Choice(opts) => p.default.unwrap_or(opts[0]).to_owned(),
                    _ => p.default.unwrap_or("").to_owned(),
                },
            })
            .map(|v| match v.as_str() {
                "on" | "yes" => "true".to_owned(),
                "off" | "no" => "false".to_owned(),
                _ => v,
            })
            .collect();
        Self { spec, values, replaces: None }
    }

    /// Editing an existing plan: start from its arguments.
    pub fn edit(plan: &Plan) -> Self {
        Self { replaces: Some(plan.id), ..Self::new(plan.spec, &plan.args.0) }
    }

    pub fn toggle(&mut self, i: usize) {
        let v = &mut self.values[i];
        *v = if v == "true" { "false".into() } else { "true".into() };
    }

    /// Steps a choice parameter forwards or backwards.
    pub fn cycle(&mut self, i: usize, forward: bool) {
        if let yaist_core::ParamKind::Choice(opts) = self.spec.params[i].kind {
            let cur = opts.iter().position(|o| *o == self.values[i]).unwrap_or(0);
            let next = if forward { (cur + 1) % opts.len() } else { (cur + opts.len() - 1) % opts.len() };
            self.values[i] = opts[next].to_owned();
        }
    }

    pub fn raw(&self) -> serde_json::Map<String, serde_json::Value> {
        self.spec
            .params
            .iter()
            .zip(&self.values)
            .filter(|(p, v)| p.required || !v.trim().is_empty())
            .map(|(p, v)| (p.name.to_owned(), serde_json::Value::String(v.clone())))
            .collect()
    }

    /// Sends the form: cancels the plan it edits and proposes the new one.
    pub fn submit(&self, session: &Session) {
        if let Some(old) = self.replaces {
            session.send(Cmd::Cancel(old));
        }
        session.send(Cmd::Propose { action: self.spec.id, raw: self.raw() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaist_core::{ActionSpec, Args, Risk};

    fn plan(id: &'static str, log: &[&str]) -> Plan {
        let spec = ActionSpec { id, title: "", description: "", risk: Risk::Dangerous, params: &[] };
        Plan {
            id: 1,
            spec,
            args: Args::default(),
            command: String::new(),
            shell: Vec::new(),
            executed: Vec::new(),
            log: std::sync::Arc::new(log.iter().map(|l| l.to_string()).collect()),
            log_file: None,
            status: PlanStatus::Failed("x".into()),
            from_agent: false,
        }
    }

    #[test]
    fn rollback_target_from_the_helper_log() {
        let log = ["==> /usr/bin/snapper -c root create …", "==> snapshot 412 taken before the upgrade", "==> /usr/bin/zypper …"];
        assert_eq!(snapshot_before(&plan("upgrade.migrate", &log)), Some(412));
        assert_eq!(snapshot_before(&plan("upgrade.migrate", &["==> /usr/bin/zypper …"])), None);
        // Only upgrades offer it; another action's output can't fake one.
        assert_eq!(snapshot_before(&plan("packages.install", &log)), None);
    }

    #[test]
    fn verdict_from_the_helper_log() {
        let log = ["==> warning: %post(foo-2-1.noarch) scriptlet failed, exit status 1", "==> verdict: migrated-with-warnings"];
        assert_eq!(verdict(&plan("upgrade.migrate", &log)), Some("migrated-with-warnings"));
        assert_eq!(verdict(&plan("upgrade.migrate", &["==> /usr/bin/zypper …"])), None);
    }
}
