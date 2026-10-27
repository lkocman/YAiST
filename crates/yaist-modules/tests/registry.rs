//! Integrity of the action registry: the contract between the modules, the
//! panels, the /commands and the LLM tools.

use std::collections::HashSet;

use yaist_core::{ParamKind, Risk};

#[test]
fn action_ids_are_unique_and_belong_to_a_module() {
    let reg = yaist_modules::registry();
    let modules: HashSet<_> = reg.modules.iter().map(|m| m.id).collect();
    let mut seen = HashSet::new();
    for spec in reg.specs() {
        assert!(seen.insert(spec.id), "duplicate action {}", spec.id);
        assert!(modules.contains(spec.module()), "{} has no module", spec.id);
        assert!(spec.verb().chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'), "{}: verb must be snake_case", spec.id);
        // LLM tool names: [a-zA-Z0-9_-], at most 64 characters ('.' becomes '__').
        assert!(spec.id.len() < 64, "{} is too long for a tool name", spec.id);
        assert!(!spec.description.is_empty() && !spec.title.is_empty(), "{} needs a title and description", spec.id);
    }
}

#[test]
fn modules_point_at_real_actions() {
    let reg = yaist_modules::registry();
    for m in &reg.modules {
        let overview = reg.get(m.overview).unwrap_or_else(|| panic!("{}: overview {} missing", m.id, m.overview));
        assert_eq!(overview.risk, Risk::Read, "{}: the overview runs automatically, so it must be read-only", m.id);
        assert!(overview.params.iter().all(|p| !p.required), "{}: the overview must run without arguments", m.id);
        for id in m.row_actions {
            let a = reg.get(id).unwrap_or_else(|| panic!("{}: row action {id} missing", m.id));
            assert_eq!(a.module(), m.id, "{id} is offered by module {}", m.id);
        }
    }
}

#[test]
fn parameters_are_well_formed() {
    let reg = yaist_modules::registry();
    for spec in reg.specs() {
        let mut names = HashSet::new();
        for p in spec.params {
            assert!(names.insert(p.name), "{}: duplicate parameter {}", spec.id, p.name);
            if let ParamKind::Choice(opts) = p.kind {
                assert!(!opts.is_empty(), "{}.{}: empty choice", spec.id, p.name);
            }
            if let Some(d) = p.default {
                // A default must pass the parameter's own validation.
                let mut raw = serde_json::Map::new();
                raw.insert(p.name.into(), d.into());
                let only = yaist_core::ActionSpec { params: std::slice::from_ref(p), ..*spec };
                only.validate(&raw).unwrap_or_else(|e| panic!("{}.{}: default fails validation: {e}", spec.id, p.name));
            }
        }
        assert_eq!(spec.json_schema()["type"], "object");
    }
}

#[test]
fn changes_are_never_marked_read_only() {
    // Read actions run without confirmation, so anything that sounds like a
    // change must not be one.
    let reg = yaist_modules::registry();
    let changing = [
        "set_", "add", "remove", "delete", "create", "install", "update", "enable", "disable", "start", "stop", "restart", "open", "allow", "refresh",
    ];
    for spec in reg.specs() {
        if changing.iter().any(|w| spec.verb().starts_with(w)) {
            assert_ne!(spec.risk, Risk::Read, "{} looks like a change but is marked read-only", spec.id);
        }
    }
}

#[test]
fn every_action_teaches_its_terminal_command() {
    // Plan cards and results show the equivalent command so users learn the
    // tools underneath; an action without one is a gap in that.
    let reg = yaist_modules::registry();
    let distros = [
        yaist_core::Distro { id: "opensuse-leap".into(), version: "16.0".into(), ..Default::default() },
        yaist_core::Distro { id: "opensuse-tumbleweed".into(), rolling: true, ..Default::default() },
        yaist_core::Distro { id: "opensuse-aeon".into(), rolling: true, transactional: true, ..Default::default() },
        yaist_core::Distro {
            id: "opensuse-leap".into(),
            version: "16.1".into(),
            transactional: true,
            variant_id: "transactional".into(),
            ..Default::default()
        },
        yaist_core::Distro { id: "opensuse-leap-micro".into(), version: "6.2".into(), transactional: true, ..Default::default() },
    ];
    for spec in reg.specs() {
        // Time zones are checked against /usr/share/zoneinfo, which a build
        // root may not have.
        if spec.id == "system.set_timezone" && !std::path::Path::new("/usr/share/zoneinfo/UTC").exists() {
            continue;
        }
        // Fill required parameters with something valid-looking.
        let mut raw = serde_json::Map::new();
        for p in spec.params.iter().filter(|p| p.required) {
            let v = match p.kind {
                ParamKind::Bool => "true",
                ParamKind::Integer => "1",
                ParamKind::Choice(opts) => opts[0],
                ParamKind::Text => match p.name {
                    "port" => "8080",
                    "number" => "42",
                    "rule" => "8080/tcp",
                    "url" => "https://example.org/repo",
                    "timezone" => "UTC",
                    "project" => "games",
                    "app" => "org.videolan.VLC",
                    "address" => "192.168.1.20",
                    "uri" => "ipp://printer.local/ipp/print",
                    "link" => "https://chat.opensuse.org/",
                    "target" => "tumbleweed",
                    "email" => "geeko@example.org",
                    "regcode" => "ABCD-1234",
                    "version" => "16.1",
                    _ => "example",
                },
            };
            raw.insert(p.name.into(), v.into());
        }
        let args = spec.validate(&raw).unwrap_or_else(|e| panic!("{}: sample arguments invalid: {e}", spec.id));
        for d in &distros {
            let cmds = reg.shell_for(spec.id, &args, d);
            assert!(!cmds.is_empty(), "{} has no terminal equivalent", spec.id);
            if spec.risk != Risk::Read {
                // Root changes show sudo; per-user Flatpak changes need none.
                // Root changes show sudo; per-user ones (Flatpak, printing
                // your own jobs) need none.
                let user_level = ["flatpak ", "# ", "lp ", "cancel "];
                let shows = cmds.iter().any(|c| c.starts_with("sudo ") || user_level.iter().any(|p| c.starts_with(p)));
                assert!(shows, "{}: a change should show the sudo (or flatpak --user) command", spec.id);
            }
        }
    }
}
