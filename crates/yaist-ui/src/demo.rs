//! Made-up conversations for screenshots and demos (`/demo`, or
//! `YAIST_DEMO_SCENE` at start): states that are hard to reach on purpose,
//! like a change that failed or an upgrade half-way. The cards are built
//! from the real actions and arguments, only their outcome is invented.
//! Nothing here runs; buttons on the cards do what they always do.

use std::sync::Arc;

use serde_json::{Map, Value};
use yaist_core::{command, Distro, Registry};

use crate::state::{Entry, Plan, PlanStatus};
use yaist_modules::release_notes::{NoteEntry, Notes};

/// The scenes, for `/demo` without one and the error message on a typo.
pub const SCENES: &[&str] = &["report-distro", "report-yaist", "upgrade-incomplete", "upgrade-running", "whats-new"];

/// The entries of scene `name`, or None for an unknown one. Its plans are
/// numbered from `first_id`.
pub fn scene(name: &str, reg: &Registry, d: &Distro, first_id: u64) -> Option<Vec<Entry>> {
    let card = |id, action: &str, raw: &[(&str, &str)], executed: &[&str], log: &[&str], status: PlanStatus| {
        let spec = *reg.get(action).expect("demo action exists");
        let raw: Map<String, Value> = raw.iter().map(|(k, v)| ((*k).to_owned(), Value::String((*v).to_owned()))).collect();
        let args = spec.validate(&raw).expect("demo arguments are valid");
        Entry::Plan(Plan {
            id: first_id + id,
            spec,
            command: command::format(&spec, &args),
            shell: reg.shell_for(spec.id, &args, d),
            args,
            executed: executed.iter().map(|s| (*s).to_owned()).collect(),
            log: Arc::new(log.iter().map(|s| (*s).to_owned()).collect()),
            log_file: Some(format!("~/.local/state/yaist/logs/20261027-101512-{action}.log")),
            status,
            from_agent: false,
        })
    };
    let plan = |id, action: &str, raw: &[(&str, &str)], executed: &[&str], log: &[&str], error: &str| {
        card(id, action, raw, executed, log, PlanStatus::Failed(error.to_owned()))
    };
    let zypper =
        |args: &str| format!("/usr/bin/pkexec /usr/bin/setsid --ctty --wait /usr/bin/env ZYPP_LOCK_TIMEOUT=60 /usr/bin/zypper --xmlout {args}");
    Some(match name {
        // A package's own script failed: the distribution's bug.
        "report-distro" => vec![
            Entry::User("install hplip so I can scan".into()),
            plan(
                0,
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
                0,
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
                0,
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
        // An upgrade under way, offering to tell what's new meanwhile.
        "upgrade-running" => vec![
            Entry::User("upgrade to Leap 16.1".into()),
            card(0, "upgrade.migrate", &[("target", "leap:16.1")], &[], &UPGRADE_LOG[..6], PlanStatus::Running),
        ],
        // What it tells: the entries for this system, then a summary.
        "whats-new" => vec![
            Entry::User("upgrade to Leap 16.1".into()),
            card(0, "upgrade.migrate", &[("target", "leap:16.1")], &[], UPGRADE_LOG, PlanStatus::Running),
            Entry::User("What's new in Leap 16.1?".into()),
            Entry::Notes(sample_notes()),
            Entry::Assistant { text: SAMPLE_SUMMARY.into(), elapsed: std::time::Duration::from_millis(9400) },
        ],
        _ => return None,
    })
}

const UPGRADE_LOG: &[&str] = &[
    "==> snapshot 42 taken before the upgrade",
    "==> disabling repositories that may not work on Leap 16.1: google-chrome-beta",
    "==> /usr/bin/zypper --releasever 16.1 refresh",
    "==> /usr/bin/zypper --releasever 16.1 dup --download-in-advance",
    "Retrieving: 1843 packages, 2.1 GiB",
    "(611/1843) Retrieving: libQt6Core6-6.10.1-1.1.x86_64 (Main Repository (OSS)), 2.4 MiB",
    "(1012/1843) Retrieving: kernel-default-6.12.0-160100.7.1.x86_64 (Main Repository (OSS)), 158.2 MiB",
];

/// What `release_notes::load` gives for Leap 16.1 from the installed
/// release-notes-openSUSE-Leap-16.1, on a GNOME desktop.
fn sample_notes() -> Notes {
    let page = "/usr/share/doc/release-notes/openSUSE/Leap-16.1/RELEASE-NOTES.en.html";
    let entry = |title: &str, why: &str, id: &str, excerpt: &str| NoteEntry {
        title: title.into(),
        why: why.into(),
        link: format!("file://{page}#{id}"),
        excerpt: excerpt.into(),
    };
    Notes {
        release: "Leap 16.1".into(),
        source: "Leap 16.1 release notes".into(),
        caveat: None,
        online: "https://doc.opensuse.org/release-notes/".into(),
        local: Some(page.into()),
        entries: vec![
            entry("Upgrade from Leap 16.0", "about upgrading", "leap-upgrade-from-160", "Use the openSUSE migration tool; preview with --dry-run first."),
            entry("GDM no longer uses the xdm integration", "you have gdm installed", "leap-features-306", "A systemd service from gdm-systemd starts GDM; DISPLAYMANAGER no longer selects it."),
            entry("PipeWire replaces PulseAudio", "you have pipewire installed", "id-pipewire-replaces-pulseaudio", "Upgrades should move from PulseAudio automatically."),
            entry("Security update and changes in mount behavior for util-linux", "you have util-linux installed", "jsc-PED-16741", "Non-root mount operations must always use path canonicalization."),
            entry("Unified systemd service presets between standard and immutable modes", "you have systemd installed", "jsc-PED-16083", "Several service presets are newly enabled by default."),
            entry("Distrobox pre-installed by default", "you have distrobox installed", "jsc-PED-16282", "distrobox replaces the older toolbox utility."),
        ],
        total: 107,
    }
}

const SAMPLE_SUMMARY: &str = "- GDM now starts from its own systemd service. Setting DISPLAYMANAGER no longer picks it; if GDM should stay your login screen, nothing to do.
- Sound moves from PulseAudio to PipeWire on its own. If audio is silent after the reboot, check that the wireplumber-video-only profile isn't in use.
- Some services are now enabled by default, the same way on standard and immutable systems. Check that nothing starts that you don't want.
- Mounting as a normal user is stricter (a util-linux security fix). Scripts that mount through symlinks may need full paths.
- Distrobox replaces toolbox for running containers.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_builds() {
        let reg = yaist_modules::registry();
        let d = Distro { id: "opensuse-tumbleweed".into(), rolling: true, ..Default::default() };
        for s in SCENES {
            assert!(scene(s, &reg, &d, 1).is_some_and(|e| !e.is_empty()), "{s}");
        }
        assert!(scene("nope", &reg, &d, 1).is_none());
    }
}
