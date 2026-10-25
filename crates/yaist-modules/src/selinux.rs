//! SELinux: mode, booleans and recent denials. Reading uses selinuxfs and the
//! journal; changes go through setsebool/setenforce as root.

use anyhow::{bail, Result};
use yaist_core::shell_quote as sq;
use yaist_core::{validate, ActionSpec, Context, ModuleSpec, Output, Param, Registry, Risk, Table};

const SELINUXFS: &str = "/sys/fs/selinux";

const BOOLEANS: ActionSpec = ActionSpec {
    id: "selinux.booleans",
    title: "Booleans",
    description: "List SELinux booleans (policy switches such as httpd_can_network_connect) and their values.",
    risk: Risk::Read,
    params: &[Param::text("filter", "Only booleans containing this text").optional().check(validate::search_term)],
};

const STATUS: ActionSpec = ActionSpec {
    id: "selinux.status",
    title: "Status",
    description: "Show whether SELinux is enabled, its current and configured mode, and the policy.",
    risk: Risk::Read,
    params: &[],
};

const DENIALS: ActionSpec = ActionSpec {
    id: "selinux.denials",
    title: "Recent denials",
    description:
        "Show recent SELinux AVC denials from the audit log. Use this when something fails with 'permission denied' and SELinux may be the cause.",
    risk: Risk::Read,
    params: &[Param::integer("limit", "How many denials to show").default("10")],
};

const SET_BOOLEAN: ActionSpec = ActionSpec {
    id: "selinux.set_boolean",
    title: "Set boolean",
    description: "Turn an SELinux boolean on or off.",
    risk: Risk::Change,
    params: &[
        Param::text("boolean", "Boolean name, e.g. httpd_can_network_connect").check(validate::name),
        Param::boolean("value", "On (true) or off (false)").required(),
        Param::boolean("persistent", "Keep the setting after reboot").default("true"),
    ],
};

const SET_MODE: ActionSpec = ActionSpec {
    id: "selinux.set_mode",
    title: "Set mode",
    description: "Switch SELinux between enforcing and permissive until the next reboot. Permissive only logs denials and weakens security.",
    risk: Risk::Dangerous,
    params: &[Param::choice("mode", &["enforcing", "permissive"], "New mode")],
};

pub fn register(reg: &mut Registry) {
    reg.module(ModuleSpec {
        id: "selinux",
        title: "SELinux",
        tagline: "Mode, booleans and denials",
        overview: BOOLEANS.id,
        row_actions: &[SET_BOOLEAN.id],
        overview_needs_admin: false,
    });

    reg.action(BOOLEANS, |_, args| async move { booleans(args.opt_str("filter")) });
    reg.action(STATUS, |_, _| async move { Ok(status()) });
    reg.action(DENIALS, |ctx, args| async move { denials(&ctx, args.int("limit").clamp(1, 50) as usize).await });

    reg.action(SET_BOOLEAN, |ctx, args| async move {
        require_enabled()?;
        let name = args.str("boolean");
        if !std::path::Path::new(SELINUXFS).join("booleans").join(name).exists() {
            bail!("no SELinux boolean called '{name}'");
        }
        let value = if args.bool("value") { "on" } else { "off" };
        let mut argv = vec!["/usr/sbin/setsebool"];
        if args.bool("persistent") {
            argv.push("-P");
        }
        argv.extend([name, value]);
        ctx.run_privileged(&argv).await?;
        Ok(Output::Done(format!("{name} turned {value}{}", if args.bool("persistent") { " (persistent)" } else { " (until reboot)" })))
    });

    reg.action(SET_MODE, |ctx, args| async move {
        require_enabled()?;
        let mode = args.str("mode");
        ctx.run_privileged(&["/usr/sbin/setenforce", if mode == "enforcing" { "1" } else { "0" }]).await?;
        Ok(Output::Done(format!("SELinux is now {mode} (until reboot)")))
    });

    reg.shell(BOOLEANS.id, |a, _| {
        vec![match a.opt_str("filter") {
            Some(f) => format!("getsebool -a | grep {}", sq(f)),
            None => "getsebool -a".into(),
        }]
    });
    reg.shell(STATUS.id, |_, _| vec!["sestatus".into()]);
    reg.shell(DENIALS.id, |_, _| vec!["sudo ausearch --message avc --start recent".into()]);
    reg.shell(SET_BOOLEAN.id, |a, _| {
        let p = if a.bool("persistent") { " -P" } else { "" };
        vec![format!("sudo setsebool{p} {} {}", sq(a.str("boolean")), if a.bool("value") { "on" } else { "off" })]
    });
    reg.shell(SET_MODE.id, |a, _| vec![format!("sudo setenforce {}", if a.str("mode") == "enforcing" { "1" } else { "0" })]);
}

fn enabled() -> bool {
    std::path::Path::new(SELINUXFS).join("enforce").exists()
}

fn require_enabled() -> Result<()> {
    if !enabled() {
        bail!("SELinux is not enabled on this system ({})", lsm_summary());
    }
    Ok(())
}

fn configured(key: &str) -> String {
    std::fs::read_to_string("/etc/selinux/config")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.trim().strip_prefix(key)?.strip_prefix('='))
        .unwrap_or("")
        .trim()
        .to_owned()
}

fn current_mode() -> &'static str {
    match std::fs::read_to_string(format!("{SELINUXFS}/enforce")).unwrap_or_default().trim() {
        "1" => "enforcing",
        "0" => "permissive",
        _ => "disabled",
    }
}

/// "SELinux enforcing", "AppArmor" or "none": for the system overview.
pub fn lsm_summary() -> String {
    if enabled() {
        return format!("SELinux {}", current_mode());
    }
    let lsm = std::fs::read_to_string("/sys/kernel/security/lsm").unwrap_or_default();
    if lsm.contains("apparmor") {
        "AppArmor".into()
    } else {
        "none".into()
    }
}

fn status() -> Output {
    if !enabled() {
        return Output::Facts(vec![("SELinux".into(), "not enabled".into()), ("Security module".into(), lsm_summary())]);
    }
    let count = std::fs::read_dir(format!("{SELINUXFS}/booleans")).map(|d| d.count()).unwrap_or(0);
    Output::Facts(vec![
        ("Mode".into(), current_mode().into()),
        ("Mode at boot".into(), configured("SELINUX")),
        ("Policy".into(), configured("SELINUXTYPE")),
        ("Booleans".into(), count.to_string()),
    ])
}

fn booleans(filter: Option<&str>) -> Result<Output> {
    require_enabled()?;
    let mut t = Table::new(&[("boolean", "Boolean"), ("setting", "Value")]);
    let mut names: Vec<String> =
        std::fs::read_dir(format!("{SELINUXFS}/booleans"))?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    for name in names.into_iter().filter(|n| filter.is_none_or(|f| n.contains(f))) {
        let raw = std::fs::read_to_string(format!("{SELINUXFS}/booleans/{name}")).unwrap_or_default();
        let on = raw.split_whitespace().next() == Some("1");
        t.push(vec![name, if on { "on" } else { "off" }.into()]);
    }
    Ok(Output::Table(t))
}

async fn denials(ctx: &Context, limit: usize) -> Result<Output> {
    require_enabled()?;
    let text =
        ctx.run(&["journalctl", "_TRANSPORT=audit", "--lines", "2000", "--no-pager", "--output", "short-iso", "--quiet"]).await.unwrap_or_default();
    let found: Vec<&str> = text.lines().filter(|l| l.contains("avc:") && l.contains("denied")).collect();
    if found.is_empty() {
        return Ok(Output::Text(
            "No SELinux denials found in the journal. If auditd is running, denials are in /var/log/audit/audit.log, readable by root only.".into(),
        ));
    }
    Ok(Output::Text(found[found.len().saturating_sub(limit)..].join("\n")))
}
