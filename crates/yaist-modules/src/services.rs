//! systemd services over D-Bus. systemd checks polkit itself, so changes ask
//! for authentication through the desktop's polkit agent.

use std::collections::BTreeMap;

use anyhow::Result;
use yaist_core::shell_quote as sq;
use yaist_core::{last_lines, validate, ActionSpec, Context, ModuleSpec, Output, Param, Registry, Risk, Table};
use zbus::zvariant::OwnedObjectPath;

const SYSTEMD: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const UNIT: &str = "org.freedesktop.systemd1.Unit";
const SERVICE: &str = "org.freedesktop.systemd1.Service";

const UNIT_PARAM: Param = Param::text("unit", "Service name, e.g. sshd or sshd.service").check(validate::unit_name);

const LIST: ActionSpec = ActionSpec {
    id: "services.list",
    title: "List services",
    description: "List systemd services with their running and enabled state.",
    risk: Risk::Read,
    params: &[
        Param::choice("state", &["all", "running", "failed", "enabled"], "Which services to show").default("all"),
        Param::text("filter", "Only services whose name contains this text").optional().check(validate::search_term),
    ],
};

const STATUS: ActionSpec = ActionSpec {
    id: "services.status",
    title: "Status",
    description: "Show one service's state, main process and recent log lines. Use this to diagnose a failing service.",
    risk: Risk::Read,
    params: &[UNIT_PARAM],
};

const START: ActionSpec =
    ActionSpec { id: "services.start", title: "Start", description: "Start a service now.", risk: Risk::Change, params: &[UNIT_PARAM] };
const STOP: ActionSpec =
    ActionSpec { id: "services.stop", title: "Stop", description: "Stop a running service.", risk: Risk::Change, params: &[UNIT_PARAM] };
const RESTART: ActionSpec =
    ActionSpec { id: "services.restart", title: "Restart", description: "Restart a service.", risk: Risk::Change, params: &[UNIT_PARAM] };

const ENABLE: ActionSpec = ActionSpec {
    id: "services.enable",
    title: "Enable",
    description: "Start a service automatically at boot, and optionally start it now.",
    risk: Risk::Change,
    params: &[UNIT_PARAM, Param::boolean("now", "Also start it now").default("true")],
};

const DISABLE: ActionSpec = ActionSpec {
    id: "services.disable",
    title: "Disable",
    description: "Stop a service from starting at boot, and optionally stop it now.",
    risk: Risk::Change,
    params: &[UNIT_PARAM, Param::boolean("now", "Also stop it now").default("true")],
};

pub fn register(reg: &mut Registry) {
    reg.module(ModuleSpec {
        id: "services",
        title: "Services",
        tagline: "Start, stop and enable services",
        overview: LIST.id,
        row_actions: &[STATUS.id, START.id, STOP.id, RESTART.id, ENABLE.id, DISABLE.id],
        overview_needs_admin: false,
    });

    reg.action(LIST, |ctx, args| async move { list(&ctx, args.str("state"), args.opt_str("filter")).await });
    reg.action(STATUS, |ctx, args| async move { status(&ctx, &unit(args.str("unit"))).await });

    for (spec, method, verb) in [(START, "StartUnit", "Started"), (STOP, "StopUnit", "Stopped"), (RESTART, "RestartUnit", "Restarted")] {
        reg.action(spec, move |ctx, args| async move {
            let u = unit(args.str("unit"));
            job(&ctx, method, &u).await?;
            Ok(Output::Done(format!("{verb} {u}")))
        });
    }

    reg.action(ENABLE, |ctx, args| async move {
        let u = unit(args.str("unit"));
        ctx.call::<_, (bool, Vec<(String, String, String)>)>(
            SYSTEMD,
            SYSTEMD_PATH,
            MANAGER,
            "EnableUnitFiles",
            &(vec![u.as_str()], false, false),
            true,
        )
        .await?;
        ctx.call::<_, ()>(SYSTEMD, SYSTEMD_PATH, MANAGER, "Reload", &(), true).await?;
        if args.bool("now") {
            job(&ctx, "StartUnit", &u).await?;
            return Ok(Output::Done(format!("Enabled and started {u}")));
        }
        Ok(Output::Done(format!("Enabled {u}")))
    });

    reg.action(DISABLE, |ctx, args| async move {
        let u = unit(args.str("unit"));
        ctx.call::<_, Vec<(String, String, String)>>(SYSTEMD, SYSTEMD_PATH, MANAGER, "DisableUnitFiles", &(vec![u.as_str()], false), true).await?;
        ctx.call::<_, ()>(SYSTEMD, SYSTEMD_PATH, MANAGER, "Reload", &(), true).await?;
        if args.bool("now") {
            job(&ctx, "StopUnit", &u).await?;
            return Ok(Output::Done(format!("Disabled and stopped {u}")));
        }
        Ok(Output::Done(format!("Disabled {u}")))
    });

    reg.shell(LIST.id, |a, _| {
        vec![match a.str("state") {
            "running" => "systemctl list-units --type=service --state=running".into(),
            "failed" => "systemctl list-units --type=service --state=failed".into(),
            "enabled" => "systemctl list-unit-files --type=service --state=enabled".into(),
            _ => "systemctl list-units --type=service --all".into(),
        }]
    });
    reg.shell(STATUS.id, |a, _| vec![format!("systemctl status {}", sq(&unit(a.str("unit"))))]);
    reg.shell(START.id, |a, _| vec![format!("sudo systemctl start {}", sq(&unit(a.str("unit"))))]);
    reg.shell(STOP.id, |a, _| vec![format!("sudo systemctl stop {}", sq(&unit(a.str("unit"))))]);
    reg.shell(RESTART.id, |a, _| vec![format!("sudo systemctl restart {}", sq(&unit(a.str("unit"))))]);
    reg.shell(ENABLE.id, |a, _| vec![format!("sudo systemctl enable{} {}", if a.bool("now") { " --now" } else { "" }, sq(&unit(a.str("unit"))))]);
    reg.shell(DISABLE.id, |a, _| vec![format!("sudo systemctl disable{} {}", if a.bool("now") { " --now" } else { "" }, sq(&unit(a.str("unit"))))]);
}

/// `sshd` → `sshd.service`.
fn unit(name: &str) -> String {
    if name.contains('.') {
        name.to_owned()
    } else {
        format!("{name}.service")
    }
}

async fn job(ctx: &Context, method: &'static str, unit: &str) -> Result<()> {
    ctx.call::<_, OwnedObjectPath>(SYSTEMD, SYSTEMD_PATH, MANAGER, method, &(unit, "replace"), true).await?;
    Ok(())
}

type UnitRow = (String, String, String, String, String, String, OwnedObjectPath, u32, String, OwnedObjectPath);

async fn list(ctx: &Context, state: &str, filter: Option<&str>) -> Result<Output> {
    let loaded: Vec<UnitRow> = ctx.call(SYSTEMD, SYSTEMD_PATH, MANAGER, "ListUnits", &(), false).await?;
    let files: Vec<(String, String)> = ctx.call(SYSTEMD, SYSTEMD_PATH, MANAGER, "ListUnitFiles", &(), false).await?;

    let mut enablement: BTreeMap<String, String> = BTreeMap::new();
    for (path, st) in files {
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        if name.ends_with(".service") && !name.ends_with("@.service") {
            enablement.insert(name, st);
        }
    }

    // name -> (active, sub, description)
    let mut rows: BTreeMap<String, (String, String, String)> = BTreeMap::new();
    for (name, desc, load, active, sub, ..) in loaded {
        if name.ends_with(".service") && load != "not-found" {
            rows.insert(name, (active, sub, desc));
        }
    }
    for name in enablement.keys() {
        rows.entry(name.clone()).or_insert_with(|| ("inactive".into(), "dead".into(), String::new()));
    }

    let mut t = Table::new(&[("unit", "Service"), ("active", "State"), ("sub", "Detail"), ("enabled", "Boot"), ("description", "Description")])
        .essential(&["unit", "active", "enabled"]);
    for (name, (active, sub, desc)) in rows {
        let enabled = enablement.get(&name).cloned().unwrap_or_else(|| "-".into());
        let keep = match state {
            "running" => active == "active",
            "failed" => active == "failed",
            "enabled" => enabled == "enabled",
            _ => true,
        };
        if keep && filter.is_none_or(|f| name.contains(f)) {
            t.push(vec![name, active, sub, enabled, desc]);
        }
    }
    // Failed first, then running, then the rest: what needs attention leads.
    t.rows.sort_by_key(|r| (r[1] != "failed", r[1] != "active", r[0].clone()));
    Ok(Output::Table(t))
}

async fn status(ctx: &Context, unit: &str) -> Result<Output> {
    let path: OwnedObjectPath = ctx.call(SYSTEMD, SYSTEMD_PATH, MANAGER, "LoadUnit", &(unit,), false).await?;
    let p = path.as_str();
    let prop = |name: &'static str| async move { ctx.property::<String>(SYSTEMD, p, UNIT, name).await.unwrap_or_default() };
    let load = prop("LoadState").await;
    if load == "not-found" {
        return Ok(Output::Text(format!("{unit} does not exist on this system")));
    }
    let pid: u32 = ctx.property(SYSTEMD, p, SERVICE, "MainPID").await.unwrap_or(0);
    let restarts: u32 = ctx.property(SYSTEMD, p, SERVICE, "NRestarts").await.unwrap_or(0);
    let result = ctx.property::<String>(SYSTEMD, p, SERVICE, "Result").await.unwrap_or_default();

    let log = match ctx.run(&["journalctl", "--unit", unit, "--lines", "15", "--no-pager", "--output", "short-iso", "--quiet"]).await {
        Ok(text) if text.trim().is_empty() => "(no log entries)".to_owned(),
        Ok(text) => last_lines(&text, 15),
        Err(e) => format!("(log unavailable: {e})"),
    };

    Ok(Output::Facts(vec![
        ("Service".into(), unit.to_owned()),
        ("Description".into(), prop("Description").await),
        ("State".into(), format!("{} ({})", prop("ActiveState").await, prop("SubState").await)),
        ("Boot".into(), prop("UnitFileState").await),
        ("Last result".into(), result),
        ("Main PID".into(), if pid == 0 { "-".into() } else { pid.to_string() }),
        ("Restarts".into(), restarts.to_string()),
        ("Unit file".into(), prop("FragmentPath").await),
        ("Recent log".into(), log),
    ]))
}
