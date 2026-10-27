//! Made-up conversations for screenshots (`YAIST_DEMO_SCENE`): states that
//! are hard to reach on purpose, like a change that failed. The cards are
//! built from the real actions and arguments, only their outcome is
//! invented. Nothing here runs.

use std::sync::Arc;

use serde_json::{Map, Value};
use yaist_core::{command, Distro, Registry};

use crate::state::{Entry, Plan, PlanStatus};

/// The scenes, for the error message on a typo.
pub const SCENES: &[&str] = &["report-distro", "report-yaist", "upgrade-incomplete"];

/// Plan ids far above the session's own, so the two never meet.
const FIRST_ID: u64 = 1_000_000;

/// The entries of scene `name`, or None for an unknown one.
pub fn scene(name: &str, reg: &Registry, d: &Distro) -> Option<Vec<Entry>> {
    let plan = |id, action: &str, raw: &[(&str, &str)], executed: &[&str], log: &[&str], error: &str| {
        let spec = *reg.get(action).expect("demo action exists");
        let raw: Map<String, Value> = raw.iter().map(|(k, v)| ((*k).to_owned(), Value::String((*v).to_owned()))).collect();
        let args = spec.validate(&raw).expect("demo arguments are valid");
        Entry::Plan(Plan {
            id: FIRST_ID + id,
            spec,
            command: command::format(&spec, &args),
            shell: reg.shell_for(spec.id, &args, d),
            args,
            executed: executed.iter().map(|s| (*s).to_owned()).collect(),
            log: Arc::new(log.iter().map(|s| (*s).to_owned()).collect()),
            log_file: Some(format!("~/.local/state/yaist/logs/20261027-101512-{action}.log")),
            status: PlanStatus::Failed(error.to_owned()),
            from_agent: false,
        })
    };
    let zypper =
        |args: &str| format!("/usr/bin/pkexec /usr/bin/setsid --ctty --wait /usr/bin/env ZYPP_LOCK_TIMEOUT=60 /usr/bin/zypper --xmlout {args}");
    Some(match name {
        // A package's own script failed: the distribution's bug.
        "report-distro" => vec![
            Entry::User("install hplip so I can scan".into()),
            plan(
                1,
                "packages.install",
                &[("packages", "hplip")],
                &[&zypper("install -- hplip")],
                &[
                    "Loading repository data...",
                    "Reading installed packages...",
                    "Resolving package dependencies...",
                    "The following 4 NEW packages are going to be installed:",
                    "  hplip hplip-hpijs hplip-sane python313-PyQt6",
                    "Retrieving: hplip-3.25.2-1.1.x86_64 (Main Repository (OSS)) (4/4), 21.4 MiB",
                    "Checking for file conflicts: ...[done]",
                    "(4/4) Installing: hplip-3.25.2-1.1.x86_64 ...[error]",
                    "warning: %post(hplip-3.25.2-1.1.x86_64) scriptlet failed, exit status 1",
                ],
                "Installation of hplip-3.25.2-1.1.x86_64 failed:\nwarning: %post(hplip-3.25.2-1.1.x86_64) scriptlet failed, exit status 1",
            ),
        ],
        // Something YAiST should have handled: its own issue.
        "report-yaist" => vec![
            Entry::Command("/packages update vim".into()),
            plan(
                1,
                "packages.update",
                &[("packages", "vim")],
                &[&zypper("update -- vim")],
                &["Loading repository data...", "Reading installed packages...", "Resolving package dependencies..."],
                "could not read zypper's output: unexpected end of stream at 41:7",
            ),
        ],
        // An upgrade that stopped part-way: continue it or roll back.
        "upgrade-incomplete" => vec![
            Entry::User("upgrade to Leap 16.1".into()),
            plan(
                1,
                "upgrade.migrate",
                &[("target", "leap:16.1")],
                &["/usr/bin/pkexec /usr/libexec/yaist/yaist-helper migrate leap:16.1 google-chrome"],
                &[
                    "==> snapshot 42 taken before the upgrade",
                    "==> /usr/bin/zypper --releasever 16.1 refresh",
                    "==> /usr/bin/zypper --releasever 16.1 dup --download-in-advance",
                    "Retrieving: 1843 packages, 2.1 GiB",
                    "(1012/1843) Installing: glibc-2.41-1.1.x86_64 ...[done]",
                    "Download (curl) error for 'https://cdn.opensuse.org/distribution/leap/16.1/repo/oss/x86_64/kernel-default-6.12.0-160100.7.1.x86_64.rpm':",
                    "Error code: Bad URL",
                    "==> problem: 831 packages are still from Leap 16.0",
                    "==> verdict: incomplete",
                ],
                "the upgrade stopped part-way: 831 packages are still from Leap 16.0",
            ),
        ],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_builds() {
        let reg = yaist_modules::registry();
        let d = Distro { id: "opensuse-tumbleweed".into(), rolling: true, ..Default::default() };
        for s in SCENES {
            assert!(scene(s, &reg, &d).is_some_and(|e| !e.is_empty()), "{s}");
        }
        assert!(scene("nope", &reg, &d).is_none());
    }
}
