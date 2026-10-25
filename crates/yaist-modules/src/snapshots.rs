//! Snapper snapshots of the root filesystem: list, compare, create, delete
//! and roll back. Snapper only lets configured users read snapshots, so
//! everything goes through yaist-helper.

use anyhow::{anyhow, Result};
use serde_json::Value;
use yaist_core::shell_quote as sq;
use yaist_core::{validate, ActionSpec, Context, ModuleSpec, Output, Param, Registry, Risk, Table};

use crate::helper;

const NUMBER: Param = Param::text("number", "Snapshot number, as listed").check(validate::snapshot_number);

const LIST: ActionSpec = ActionSpec {
    id: "snapshots.list",
    title: "List snapshots",
    description: "List snapper snapshots of the root filesystem, newest first.",
    risk: Risk::Read,
    params: &[],
};

const CHANGES: ActionSpec = ActionSpec {
    id: "snapshots.changes",
    title: "Changes since",
    description: "Show which files changed between a snapshot and the running system.",
    risk: Risk::Read,
    params: &[NUMBER],
};

const CREATE: ActionSpec = ActionSpec {
    id: "snapshots.create",
    title: "Create snapshot",
    description: "Take a snapshot of the root filesystem now, e.g. before risky changes.",
    risk: Risk::Change,
    params: &[Param::text("description", "What the snapshot is for").check(validate::description)],
};

const DELETE: ActionSpec = ActionSpec {
    id: "snapshots.delete",
    title: "Delete",
    description: "Delete a snapshot. It can no longer be rolled back to.",
    risk: Risk::Dangerous,
    params: &[NUMBER],
};

const ROLLBACK: ActionSpec = ActionSpec {
    id: "snapshots.rollback",
    title: "Roll back",
    description:
        "Make a snapshot the system that boots next (snapper rollback). Takes effect after a reboot; the current state stays available as a snapshot.",
    risk: Risk::Dangerous,
    params: &[NUMBER],
};

pub fn register(reg: &mut Registry) {
    reg.module(ModuleSpec {
        id: "snapshots",
        title: "Snapshots",
        tagline: "Snapper: compare and roll back",
        overview: LIST.id,
        row_actions: &[CHANGES.id, ROLLBACK.id, DELETE.id],
        overview_needs_admin: true,
    });

    reg.action(LIST, |ctx, _| async move { list(&ctx).await });

    reg.action(CHANGES, |ctx, args| async move {
        let n = args.str("number");
        let out = helper::run(&ctx, &["snapper-status", n]).await?;
        let lines: Vec<&str> = out.lines().filter(|l| !l.starts_with("==>")).collect();
        if lines.is_empty() {
            return Ok(Output::Text(format!("No files changed since snapshot {n}")));
        }
        let mut text = format!("{} files changed since snapshot {n} (c = content, + = added, - = removed):\n", lines.len());
        text.push_str(&lines.iter().take(200).copied().collect::<Vec<_>>().join("\n"));
        if lines.len() > 200 {
            text.push_str(&format!("\n… {} more", lines.len() - 200));
        }
        Ok(Output::Text(text))
    });

    reg.action(CREATE, |ctx, args| async move {
        let out = helper::run(&ctx, &["snapper-create", args.str("description")]).await?;
        let number = out.lines().rev().find(|l| l.trim().parse::<u32>().is_ok()).unwrap_or("?").trim().to_owned();
        Ok(Output::Done(format!("Created snapshot {number}")))
    });

    reg.action(DELETE, |ctx, args| async move {
        helper::run(&ctx, &["snapper-delete", args.str("number")]).await?;
        Ok(Output::Done(format!("Deleted snapshot {}", args.str("number"))))
    });

    reg.action(ROLLBACK, |ctx, args| async move {
        helper::run(&ctx, &["snapper-rollback", args.str("number")]).await?;
        Ok(Output::Done(format!("Rolled back to snapshot {}; reboot to boot into it", args.str("number"))))
    });

    reg.shell(LIST.id, |_, _| vec!["sudo snapper list".into()]);
    reg.shell(CHANGES.id, |a, _| vec![format!("sudo snapper status {}..0", a.str("number"))]);
    reg.shell(CREATE.id, |a, _| vec![format!("sudo snapper create --description {}", sq(a.str("description")))]);
    reg.shell(DELETE.id, |a, _| vec![format!("sudo snapper delete {}", a.str("number"))]);
    reg.shell(ROLLBACK.id, |a, d| {
        if d.transactional {
            vec![format!("sudo transactional-update rollback {}", a.str("number")), "sudo reboot".into()]
        } else {
            vec![format!("sudo snapper rollback {}", a.str("number")), "sudo reboot".into()]
        }
    });
}

async fn list(ctx: &Context) -> Result<Output> {
    let out = helper::run(ctx, &["snapper-list"]).await?;
    let json = out.find('{').map(|i| &out[i..]).ok_or_else(|| anyhow!("snapper returned no data"))?;
    let v: Value = serde_json::from_str(json).map_err(|e| anyhow!("reading snapper's output: {e}"))?;
    let mut t = Table::new(&[
        ("number", "#"),
        ("type", "Type"),
        ("date", "Date"),
        ("description", "Description"),
        ("state", "State"),
        ("cleanup", "Cleanup"),
    ]);
    let snaps = v.get("root").and_then(Value::as_array).cloned().unwrap_or_default();
    for s in snaps.iter().rev() {
        let n = s["number"].as_u64().unwrap_or(0);
        if n == 0 {
            continue;
        }
        let text = |k: &str| s[k].as_str().unwrap_or("").to_owned();
        let mut state = Vec::new();
        if s["active"].as_bool() == Some(true) {
            state.push("running");
        }
        if s["default"].as_bool() == Some(true) {
            state.push("next boot");
        }
        let description = match s["pre-number"].as_u64() {
            Some(pre) => format!("{} (after {pre})", text("description")).trim().to_owned(),
            None => text("description"),
        };
        t.push(vec![n.to_string(), text("type"), text("date"), description, state.join(", "), text("cleanup")]);
    }
    Ok(Output::Table(t))
}
