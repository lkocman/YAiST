//! Packages: search, details, install/remove and system updates. The update
//! command follows the distribution: `zypper up` on Leap, `zypper dup` on
//! rolling releases, `transactional-update` on immutable variants.

use anyhow::Result;
use yaist_core::shell_quote as sq;
use yaist_core::{validate, ActionSpec, Context, ModuleSpec, Output, Param, Registry, Risk, Table};

use crate::{flatpak, zypper};

const PACKAGES_PARAM: Param = Param::text("packages", "Package names, space separated").check(validate::packages);

const UPDATES: ActionSpec = ActionSpec {
    id: "packages.list_updates",
    title: "Available updates",
    description: "List packages with updates available, from the cached repository metadata.",
    risk: Risk::Read,
    params: &[],
};

const SEARCH: ActionSpec = ActionSpec {
    id: "packages.search",
    title: "Search",
    description: "Search packages by name in the enabled repositories, showing whether each is installed.",
    risk: Risk::Read,
    params: &[Param::text("query", "Search term, e.g. nginx or *vim*").check(validate::search_term)],
};

const INFO: ActionSpec = ActionSpec {
    id: "packages.info",
    title: "Details",
    description: "Show a package's version, repository, install status and description.",
    risk: Risk::Read,
    params: &[Param::text("packages", "Package name").check(validate::packages)],
};

const INSTALL: ActionSpec = ActionSpec {
    id: "packages.install",
    title: "Install",
    description: "Install packages (and their dependencies).",
    risk: Risk::Change,
    params: &[PACKAGES_PARAM],
};

const UPDATE: ActionSpec = ActionSpec {
    id: "packages.update",
    title: "Update",
    description: "Update specific packages to their newest version.",
    risk: Risk::Change,
    params: &[PACKAGES_PARAM],
};

const REMOVE: ActionSpec = ActionSpec {
    id: "packages.remove",
    title: "Remove",
    description: "Remove packages. Packages that depend on them are removed too.",
    risk: Risk::Dangerous,
    params: &[PACKAGES_PARAM],
};

const UPDATE_SYSTEM: ActionSpec = ActionSpec {
    id: "packages.update_system",
    title: "Update system",
    description: "Install all available updates: update the whole system the way this distribution expects (Leap: zypper update, Tumbleweed/Slowroll: zypper dup, immutable: transactional-update into a new snapshot).",
    risk: Risk::Dangerous,
    params: &[],
};

const REFRESH: ActionSpec = ActionSpec {
    id: "packages.refresh",
    title: "Refresh",
    description: "Only download fresh repository metadata (zypper refresh); installs nothing. To install updates use update_system.",
    risk: Risk::Change,
    params: &[],
};

const APP_PARAM: Param = Param::text("app", "Flatpak application ID, e.g. org.videolan.VLC").check(validate::flatpak_app);

const FLATPAK_SEARCH: ActionSpec = ActionSpec {
    id: "packages.flatpak_search",
    title: "Search Flatpak apps",
    description: "Search Flatpak apps (e.g. on Flathub) by name. Prefer Flatpak for desktop apps on Leap and on immutable systems.",
    risk: Risk::Read,
    params: &[Param::text("query", "App name, e.g. vlc or steam").check(validate::search_term)],
};

const FLATPAK_LIST: ActionSpec = ActionSpec {
    id: "packages.flatpak_list",
    title: "Flatpak apps",
    description: "List Flatpak apps installed for the current user.",
    risk: Risk::Read,
    params: &[],
};

const FLATPAK_INSTALL: ActionSpec = ActionSpec {
    id: "packages.flatpak_install",
    title: "Install Flatpak app",
    description: "Install a Flatpak app for the current user (flatpak --user, no root). Sets up Flathub for the user if needed.",
    risk: Risk::Change,
    params: &[APP_PARAM, Param::text("remote", "Flatpak remote").default("flathub").check(validate::name)],
};

const FLATPAK_REMOVE: ActionSpec = ActionSpec {
    id: "packages.flatpak_remove",
    title: "Remove Flatpak app",
    description: "Remove a Flatpak app installed for the current user.",
    risk: Risk::Change,
    params: &[APP_PARAM],
};

pub fn register(reg: &mut Registry) {
    reg.module(ModuleSpec {
        id: "packages",
        title: "Packages",
        tagline: "Install, remove and update",
        overview: UPDATES.id,
        row_actions: &[UPDATE.id, INFO.id],
    });

    reg.action(UPDATES, |ctx, _| async move { updates(&ctx).await });
    reg.action(SEARCH, |ctx, args| async move { search(&ctx, args.str("query")).await });
    reg.action(INFO, |ctx, args| async move { info(&ctx, args.str("packages")).await });

    reg.action(INSTALL, |ctx, args| async move {
        let pkgs: Vec<&str> = args.str("packages").split_whitespace().collect();
        let out = pkg_op(&ctx, "install", &pkgs).await?;
        Ok(Output::Done(format!("Install {}: {out}", pkgs.join(", "))))
    });
    reg.action(UPDATE, |ctx, args| async move {
        let pkgs: Vec<&str> = args.str("packages").split_whitespace().collect();
        let out = pkg_op(&ctx, "update", &pkgs).await?;
        Ok(Output::Done(format!("Update {}: {out}", pkgs.join(", "))))
    });
    reg.action(REMOVE, |ctx, args| async move {
        let pkgs: Vec<&str> = args.str("packages").split_whitespace().collect();
        let out = pkg_op(&ctx, "remove", &pkgs).await?;
        Ok(Output::Done(format!("Remove {}: {out}", pkgs.join(", "))))
    });

    reg.action(FLATPAK_SEARCH, |ctx, args| async move {
        if !flatpak::installed() {
            return Ok(Output::Text("Flatpak is not installed; installing an app sets it up.".into()));
        }
        let out = ctx.run(&[flatpak::BIN, "search", "--columns=application,name,description,remotes", args.str("query")]).await?;
        let mut t = Table::new(&[("app", "App ID"), ("name", "Name"), ("description", "Description"), ("remote", "Remote")]);
        for line in out.lines().filter(|l| l.contains('\t')) {
            let f: Vec<String> = line.split('\t').map(str::to_owned).collect();
            t.push((0..4).map(|i| f.get(i).cloned().unwrap_or_default()).collect());
        }
        t.rows.truncate(100);
        Ok(Output::Table(t))
    });
    reg.action(FLATPAK_LIST, |ctx, _| async move {
        let mut t = Table::new(&[("app", "App ID"), ("name", "Name"), ("version", "Version"), ("remote", "Remote")]);
        if flatpak::installed() {
            let out = ctx.run(&[flatpak::BIN, "list", "--user", "--app", "--columns=application,name,version,origin"]).await?;
            for line in out.lines().filter(|l| l.contains('\t')) {
                let f: Vec<String> = line.split('\t').map(str::to_owned).collect();
                t.push((0..4).map(|i| f.get(i).cloned().unwrap_or_default()).collect());
            }
        }
        Ok(Output::Table(t))
    });
    reg.action(FLATPAK_INSTALL, |ctx, args| async move {
        flatpak::install(&ctx, args.str("remote"), args.str("app")).await?;
        Ok(Output::Done(format!("Installed {} for your user", args.str("app"))))
    });
    reg.action(FLATPAK_REMOVE, |ctx, args| async move {
        flatpak::remove(&ctx, args.str("app")).await?;
        Ok(Output::Done(format!("Removed {}", args.str("app"))))
    });
    reg.shell(FLATPAK_SEARCH.id, |a, _| vec![format!("flatpak search {}", sq(a.str("query")))]);
    reg.shell(FLATPAK_LIST.id, |_, _| vec!["flatpak list --user --app".into()]);
    reg.shell(FLATPAK_INSTALL.id, |a, _| {
        let mut cmds = Vec::new();
        if a.str("remote") == flatpak::FLATHUB {
            cmds.push(format!("flatpak remote-add --user --if-not-exists {} {}", flatpak::FLATHUB, flatpak::FLATHUB_URL));
        }
        cmds.push(format!("flatpak install --user {} {}", sq(a.str("remote")), sq(a.str("app"))));
        cmds
    });
    reg.shell(FLATPAK_REMOVE.id, |a, _| vec![format!("flatpak uninstall --user {}", sq(a.str("app")))]);

    reg.action(UPDATE_SYSTEM, |ctx, _| async move {
        let d = &ctx.distro;
        let verb = if d.rolling { "dup" } else { "update" };
        let disabled = crate::repos::disable_broken_before_update(&ctx).await?;
        let note = if disabled.is_empty() { String::new() } else { format!(" Disabled broken repositories: {}.", disabled.join(", ")) };
        let out = if d.transactional {
            zypper::transactional(&ctx, &[if d.rolling { "dup" } else { "up" }]).await
        } else {
            zypper::interactive(&ctx, &[verb]).await
        };
        // The repositories stay disabled whatever happens to the update.
        let out = out.map_err(|e| anyhow::anyhow!("{e:#}.{note}"))?;
        Ok(Output::Done(format!("System updated ({verb}): {out}.{note}")))
    });

    reg.action(REFRESH, |ctx, _| async move {
        zypper::change(&ctx, &["refresh"]).await?;
        Ok(Output::Done("Repository metadata refreshed".into()))
    });

    reg.shell(UPDATES.id, |_, _| vec!["zypper list-updates".into()]);
    reg.shell(SEARCH.id, |a, _| vec![format!("zypper search {}", sq(a.str("query")))]);
    reg.shell(INFO.id, |a, _| vec![format!("zypper info {}", sq(a.str("packages")))]);
    reg.shell(INSTALL.id, |a, d| pkg_cmd("install", a.str("packages"), d));
    reg.shell(UPDATE.id, |a, d| pkg_cmd("update", a.str("packages"), d));
    reg.shell(REMOVE.id, |a, d| pkg_cmd("remove", a.str("packages"), d));
    reg.shell(UPDATE_SYSTEM.id, |_, d| match (d.transactional, d.rolling) {
        (true, true) => vec!["sudo transactional-update --continue dup".into(), "sudo reboot".into()],
        (true, false) => vec!["sudo transactional-update --continue up".into(), "sudo reboot".into()],
        (false, true) => vec!["sudo zypper dup".into()],
        (false, false) => vec!["sudo zypper update".into()],
    });
    reg.shell(REFRESH.id, |_, _| vec!["sudo zypper refresh".into()]);
}

/// `zypper <op> <packages>`, or its transactional-update form.
fn pkg_cmd(op: &str, packages: &str, d: &yaist_core::Distro) -> Vec<String> {
    let pkgs: Vec<String> = packages.split_whitespace().map(sq).collect();
    if d.transactional {
        vec![format!("sudo transactional-update --continue pkg {op} {}", pkgs.join(" ")), "sudo reboot".into()]
    } else {
        vec![format!("sudo zypper {op} {}", pkgs.join(" "))]
    }
}

/// Installs RPM packages the way this system wants it (transactional or not).
pub(crate) async fn install_rpms(ctx: &Context, pkgs: &[&str]) -> Result<String> {
    pkg_op(ctx, "install", pkgs).await
}

async fn pkg_op(ctx: &Context, op: &str, pkgs: &[&str]) -> Result<String> {
    if ctx.distro.transactional {
        // Package names are validated not to start with '-', and
        // transactional-update would take a "--" as its own.
        let mut targs = vec!["pkg", op];
        targs.extend_from_slice(pkgs);
        zypper::transactional(ctx, &targs).await
    } else {
        let mut args = vec![op, "--"];
        args.extend_from_slice(pkgs);
        zypper::interactive(ctx, &args).await
    }
}

/// How many updates are waiting, for the update banner and header badge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UpdateCounts {
    /// Packages with a newer version available.
    pub packages: usize,
    /// Needed security patches (Leap; rolling releases have no patches).
    pub security: usize,
    /// Installed changes wait for a reboot (a pending transactional
    /// snapshot, or a new kernel or core libraries).
    pub reboot: bool,
}

/// Counts available updates from the cached repository metadata (no root).
pub async fn update_counts(ctx: &Context) -> Result<UpdateCounts> {
    let xml = zypper::query(ctx, &["list-updates"]).await?;
    let doc = zypper::parse(&xml)?;
    let packages = doc.descendants().filter(|n| n.has_tag_name("update") && n.attribute("kind") == Some("package")).count();
    let security = if ctx.distro.rolling {
        0
    } else {
        // `--category security` doesn't filter the XML, so count ourselves.
        let xml = zypper::query(ctx, &["list-patches"]).await?;
        let doc = zypper::parse(&xml)?;
        doc.descendants()
            .filter(|n| n.has_tag_name("update") && n.attribute("kind") == Some("patch"))
            .filter(|n| n.attribute("category") == Some("security") && n.attribute("status") == Some("needed"))
            .count()
    };
    Ok(UpdateCounts { packages, security, reboot: zypper::reboot_needed(ctx).await })
}

async fn updates(ctx: &Context) -> Result<Output> {
    let xml = zypper::query(ctx, &["list-updates"]).await?;
    let doc = zypper::parse(&xml)?;
    let mut t = Table::new(&[("packages", "Package"), ("current", "Installed"), ("new", "Available"), ("repo", "Repository")]);
    for u in doc.descendants().filter(|n| n.has_tag_name("update")) {
        let repo = u.children().find(|c| c.has_tag_name("source")).and_then(|s| s.attribute("alias")).unwrap_or("");
        let attr = |k| u.attribute(k).unwrap_or("").to_owned();
        t.push(vec![attr("name"), attr("edition-old"), attr("edition"), repo.to_owned()]);
    }
    Ok(Output::Table(t))
}

async fn search(ctx: &Context, query: &str) -> Result<Output> {
    let xml = zypper::query(ctx, &["search", "--", query]).await?;
    let doc = zypper::parse(&xml)?;
    let mut t = Table::new(&[("packages", "Package"), ("status", "Status"), ("summary", "Summary")]);
    for s in doc.descendants().filter(|n| n.has_tag_name("solvable")) {
        if s.attribute("kind").is_some_and(|k| k != "package") {
            continue;
        }
        let status = if s.attribute("status") == Some("installed") { "installed" } else { "available" };
        t.push(vec![s.attribute("name").unwrap_or("").into(), status.into(), s.attribute("summary").unwrap_or("").into()]);
    }
    t.rows.truncate(200);
    Ok(Output::Table(t))
}

/// `zypper info` has no structured output, only "Key : value" lines.
async fn info(ctx: &Context, package: &str) -> Result<Output> {
    let xml = zypper::query(ctx, &["info", "--", package]).await?;
    let doc = zypper::parse(&xml)?;
    let mut facts: Vec<(String, String)> = Vec::new();
    for msg in zypper::info_messages(&doc).iter().filter(|m| m.contains(" : ")) {
        for line in msg.lines() {
            match line.split_once(" : ") {
                Some((k, v)) if !k.starts_with(' ') && !k.trim().is_empty() => facts.push((k.trim().to_owned(), v.trim().to_owned())),
                _ => {
                    if let Some(last) = facts.last_mut() {
                        last.1.push('\n');
                        last.1.push_str(line.trim());
                    }
                }
            }
        }
    }
    if facts.is_empty() {
        return Ok(Output::Text(format!("No package called '{package}' found in the enabled repositories")));
    }
    Ok(Output::Facts(facts))
}
