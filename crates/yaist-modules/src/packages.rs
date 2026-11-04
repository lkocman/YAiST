//! Packages: search, details, install/remove and system updates. The update
//! command follows the distribution: `zypper up` on Leap, `zypper dup` on
//! rolling releases, `transactional-update` on immutable variants.

use anyhow::{bail, Result};
use yaist_core::ask::{self, Choice, Question};
use yaist_core::shell_quote as sq;
use yaist_core::{validate, ActionSpec, Context, ModuleSpec, Output, Param, Registry, Risk, Table};

use crate::{flatpak, zypper};

const PACKAGES_PARAM: Param = Param::text("packages", "Package names, space separated").check(validate::packages);

const UPDATES: ActionSpec = ActionSpec {
    id: "packages.list_updates",
    title: "Available updates",
    description: "List available updates from the cached repository metadata: on Leap the needed patches (security fixes first), on rolling releases the packages with a newer version.",
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
    description: "Install packages (and their dependencies). Where openSUSE has a pattern of the same name (e.g. hyprland, gnome, kde), the user picks between that complete setup and just the package.",
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

const INSTALL_PATCH: ActionSpec = ActionSpec {
    id: "packages.install_patch",
    title: "Install",
    description: "Install one patch (Leap): the fix for one issue, with exactly the package updates it needs (zypper install -t patch).",
    risk: Risk::Change,
    params: &[Param::text("patch", "Patch name, e.g. openSUSE-Leap-16.0-2026-123").check(validate::name)],
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
    description: "Install all available updates: update the whole system the way this distribution expects (Leap: zypper update, or zypper dup while the release is in Alpha/Beta/RC; Tumbleweed/Slowroll: zypper dup; immutable: transactional-update into a new snapshot).",
    risk: Risk::Dangerous,
    params: &[],
};

const INSTALL_SECURITY: ActionSpec = ActionSpec {
    id: "packages.install_security",
    title: "Install security updates",
    description: "Install only the pending security patches (Leap: zypper patch --category security). Rolling releases have no separate security updates; there use update_system.",
    risk: Risk::Dangerous,
    params: &[],
};

const CLEAN: ActionSpec = ActionSpec {
    id: "packages.clean",
    title: "Clean caches",
    description: "Delete downloaded packages and cached repository metadata, then download fresh metadata (zypper clean --all, zypper refresh). Fixes failures from a mirror that was still syncing or a damaged download.",
    risk: Risk::Change,
    params: &[],
};

const REFRESH: ActionSpec = ActionSpec {
    id: "packages.refresh",
    title: "Refresh repositories",
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
        // Each table offers the ones it has the columns for: patches on Leap,
        // packages on rolling releases.
        row_actions: &[INSTALL_PATCH.id, INFO.id, UPDATE.id],
        overview_needs_admin: false,
    });

    reg.action(UPDATES, |ctx, _| async move { updates(&ctx).await });
    reg.action(SEARCH, |ctx, args| async move { search(&ctx, args.str("query")).await });
    reg.action(INFO, |ctx, args| async move { info(&ctx, args.str("packages")).await });

    reg.action(INSTALL, |ctx, args| async move {
        let pkgs: Vec<&str> = args.str("packages").split_whitespace().collect();
        let targets = with_patterns(&ctx, &pkgs).await?;
        let targets: Vec<&str> = targets.iter().map(String::as_str).collect();
        let out = pkg_op(&ctx, "install", &targets).await?;
        Ok(Output::Done(format!("Install {}: {out}", targets.join(", "))))
    });
    reg.action(UPDATE, |ctx, args| async move {
        let pkgs: Vec<&str> = args.str("packages").split_whitespace().collect();
        let out = pkg_op(&ctx, "update", &pkgs).await?;
        Ok(Output::Done(format!("Update {}: {out}", pkgs.join(", "))))
    });
    reg.action(INSTALL_PATCH, |ctx, args| async move {
        let patch = args.str("patch");
        if ctx.distro.rolling {
            bail!("{} is a rolling release: it has no patches. Install all updates instead.", ctx.distro.pretty);
        }
        if ctx.distro.transactional {
            bail!("on this system updates go into a new snapshot together: install all updates instead.");
        }
        let out = zypper::interactive(&ctx, &["install", "-t", "patch", "--", patch]).await?;
        Ok(Output::Done(format!("Patch {patch}: {out}")))
    });
    reg.shell(INSTALL_PATCH.id, |a, _| vec![format!("sudo zypper install -t patch {}", sq(a.str("patch")))]);
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
        let phase = if d.rolling { None } else { crate::upgrade::prerelease_phase(d).await };
        let dup = d.rolling || phase.is_some();
        let verb = if dup { "dup" } else { "update" };
        let disabled = crate::repos::disable_broken_before_update(&ctx).await?;
        let note = if disabled.is_empty() { String::new() } else { format!(" Disabled broken repositories: {}.", disabled.join(", ")) };
        let out = if d.transactional {
            zypper::transactional(&ctx, &[if dup { "dup" } else { "up" }]).await
        } else {
            // Everything downloaded before anything changes: a network
            // problem stops the update cleanly, and the install itself needs
            // no network (the update may restart it).
            zypper::interactive(&ctx, &[verb, "--download-in-advance"]).await
        };
        // The repositories stay disabled whatever happens to the update.
        let out = out.map_err(|e| anyhow::anyhow!("{e:#}.{note}"))?;
        let why = phase.map(|p| format!(", {} is in {p}", d.pretty)).unwrap_or_default();
        Ok(Output::Done(format!("System updated ({verb}{why}): {out}.{note}")))
    });

    reg.action(INSTALL_SECURITY, |ctx, _| async move {
        let d = &ctx.distro;
        if d.rolling {
            anyhow::bail!("{} is a rolling release: it has no separate security updates. Install all updates instead.", d.pretty);
        }
        if d.transactional {
            anyhow::bail!("transactional-update can't install only security patches. Install all updates instead.");
        }
        let disabled = crate::repos::disable_broken_before_update(&ctx).await?;
        let note = if disabled.is_empty() { String::new() } else { format!(" Disabled broken repositories: {}.", disabled.join(", ")) };
        let out = zypper::interactive(&ctx, &["patch", "--category", "security"]).await.map_err(|e| anyhow::anyhow!("{e:#}.{note}"))?;
        Ok(Output::Done(format!("Security updates installed: {out}.{note}")))
    });
    reg.shell(INSTALL_SECURITY.id, |_, _| vec!["sudo zypper patch --category security".into()]);

    reg.action(CLEAN, |ctx, _| async move {
        zypper::clean_and_refresh(&ctx).await?;
        Ok(Output::Done("Caches cleaned, repository metadata refreshed".into()))
    });
    reg.shell(CLEAN.id, |_, _| vec!["sudo zypper clean --all".into(), "sudo zypper refresh".into()]);

    reg.action(REFRESH, |ctx, _| async move {
        zypper::change(&ctx, &["refresh"]).await?;
        Ok(Output::Done("Repository metadata refreshed".into()))
    });

    reg.shell(UPDATES.id, |_, d| vec![if d.rolling { "zypper list-updates" } else { "zypper list-patches" }.into()]);
    reg.shell(SEARCH.id, |a, _| vec![format!("zypper search {}", sq(a.str("query")))]);
    reg.shell(INFO.id, |a, _| vec![format!("zypper info {}", sq(a.str("packages")))]);
    reg.shell(INSTALL.id, |a, d| pkg_cmd("install", a.str("packages"), d));
    reg.shell(UPDATE.id, |a, d| pkg_cmd("update", a.str("packages"), d));
    reg.shell(REMOVE.id, |a, d| pkg_cmd("remove", a.str("packages"), d));
    reg.shell(UPDATE_SYSTEM.id, |_, d| match (d.transactional, d.rolling) {
        (true, true) => vec!["sudo transactional-update --continue dup".into(), "sudo reboot".into()],
        (true, false) => vec!["sudo transactional-update --continue up".into(), "sudo reboot".into()],
        (false, true) => vec!["sudo zypper dup --download-in-advance".into()],
        (false, false) => vec!["sudo zypper update --download-in-advance".into()],
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

/// One package with a newer version waiting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingUpdate {
    pub name: String,
    pub installed: String,
    pub available: String,
    pub repo: String,
}

impl PendingUpdate {
    /// Updating it only takes full effect after a reboot: the kernel,
    /// firmware and what everything else runs on. A short list on purpose;
    /// zypper's own reboot hint comes after the update, from the packages.
    pub fn needs_reboot(&self) -> bool {
        let n = self.name.as_str();
        let kernel = n.starts_with("kernel-")
            && !["-devel", "-source", "-docs", "-macros", "-syms"].iter().any(|s| n.ends_with(s) || n.contains(&format!("{s}-")));
        kernel || n.starts_with("ucode-") || matches!(n, "glibc" | "systemd" | "udev" | "dbus-1" | "dbus-broker" | "dracut")
    }
}

/// What updating the whole system is about to change, for the plan card
/// before the user confirms.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdatePreview {
    /// Packages with a newer version, reboot-needing ones first.
    pub packages: Vec<PendingUpdate>,
    /// The update runs `zypper dup`: it can also install, remove or switch
    /// packages, which zypper's list of updates doesn't show (and a dry run
    /// would need root). zypper asks before removing anything.
    pub dup: bool,
}

impl UpdatePreview {
    pub fn reboot(&self) -> bool {
        self.packages.iter().any(PendingUpdate::needs_reboot)
    }
}

/// What the system update would bring, from the cached metadata (no root).
pub async fn update_preview(ctx: &Context) -> Result<UpdatePreview> {
    let d = &ctx.distro;
    let dup = d.rolling || crate::upgrade::prerelease_phase(d).await.is_some();
    let xml = zypper::query(ctx, &["list-updates"]).await?;
    let mut packages = pending_updates(&xml)?;
    packages.sort_by_key(|p| !p.needs_reboot());
    Ok(UpdatePreview { packages, dup })
}

/// The packages in `zypper --xmlout list-updates`.
fn pending_updates(xml: &str) -> Result<Vec<PendingUpdate>> {
    let doc = zypper::parse(xml)?;
    Ok(doc
        .descendants()
        .filter(|n| n.has_tag_name("update") && n.attribute("kind").is_none_or(|k| k == "package"))
        .map(|u| {
            let attr = |k| u.attribute(k).unwrap_or("").to_owned();
            let repo = u.children().find(|c| c.has_tag_name("source")).and_then(|s| s.attribute("alias")).unwrap_or("");
            PendingUpdate { name: attr("name"), installed: attr("edition-old"), available: attr("edition"), repo: repo.to_owned() }
        })
        .collect())
}

/// The needed patches in `zypper --xmlout list-patches`, security fixes
/// first, then by severity.
fn needed_patches(xml: &str) -> Result<Table> {
    let doc = zypper::parse(xml)?;
    let mut rows: Vec<Vec<String>> = doc
        .descendants()
        .filter(|n| n.has_tag_name("update") && n.attribute("kind") == Some("patch") && n.attribute("status") == Some("needed"))
        .map(|u| {
            let attr = |k| u.attribute(k).unwrap_or("").to_owned();
            let summary = u.children().find(|c| c.has_tag_name("summary")).and_then(|s| s.text()).unwrap_or("").trim().to_owned();
            vec![summary, attr("category"), attr("severity"), attr("name")]
        })
        .collect();
    let rank = |r: &Vec<String>| {
        let category = if r[1] == "security" { 0 } else { 1 };
        let severity = ["critical", "important", "moderate", "low"].iter().position(|s| *s == r[2]).unwrap_or(4);
        (category, severity)
    };
    rows.sort_by_key(rank);
    let mut t = Table::new(&[("summary", "Patch"), ("category", "Category"), ("severity", "Severity"), ("patch", "Name")])
        .essential(&["summary", "category"]);
    t.rows = rows;
    Ok(t)
}

async fn updates(ctx: &Context) -> Result<Output> {
    // Leap's updates come as patches, one per fixed issue: what to pick
    // from. Rolling releases have none; there the packages say what the
    // update brings.
    if !ctx.distro.rolling {
        let xml = zypper::query(ctx, &["list-patches"]).await?;
        return Ok(Output::Table(needed_patches(&xml)?));
    }
    let xml = zypper::query(ctx, &["list-updates"]).await?;
    let mut t = Table::new(&[("packages", "Package"), ("current", "Installed"), ("new", "Available"), ("repo", "Repository")])
        .essential(&["packages", "new"]);
    for u in pending_updates(&xml)? {
        t.push(vec![u.name, u.installed, u.available, u.repo]);
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

/// A pattern named like a requested package: what openSUSE puts together
/// around it (Hyprland with a bar, launcher, terminal, file manager…).
#[derive(Debug, PartialEq)]
struct PatternOffer {
    name: String,
    summary: String,
    /// There is also a package of that name; without one only the pattern
    /// can be installed.
    package: bool,
}

/// Patterns among zypper's exact search results that aren't installed yet.
fn pattern_offers(xml: &str) -> Result<Vec<PatternOffer>> {
    let doc = zypper::parse(xml)?;
    let solvables: Vec<_> = doc.descendants().filter(|n| n.has_tag_name("solvable")).collect();
    Ok(solvables
        .iter()
        .filter(|s| s.attribute("kind") == Some("pattern") && s.attribute("status") != Some("installed"))
        .map(|s| {
            let name = s.attribute("name").unwrap_or("").to_owned();
            let package = solvables.iter().any(|p| p.attribute("kind") == Some("package") && p.attribute("name") == Some(&name));
            PatternOffer { summary: s.attribute("summary").unwrap_or("").to_owned(), package, name }
        })
        .collect())
}

/// From `zypper info -t pattern`: the description, and the packages the
/// pattern brings besides the one named like it.
fn pattern_details(info: &str, name: &str) -> (String, Vec<String>) {
    let mut description = Vec::new();
    let mut packages = Vec::new();
    let mut section = "";
    for line in info.lines() {
        // Blank lines separate paragraphs of the description.
        if line.trim().is_empty() {
            if section == "Description" {
                description.push("");
            }
            continue;
        }
        if !line.starts_with(' ') {
            section = line.split(" : ").next().unwrap_or("").trim();
            continue;
        }
        match section {
            "Description" => description.push(line.trim()),
            "Contents" => {
                let cols: Vec<&str> = line.split('|').map(str::trim).collect();
                if let [_, pkg, "package", _] = cols[..] {
                    if pkg != name {
                        packages.push(pkg.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    (description.join("\n").trim().to_owned(), packages)
}

/// The question for an offer; the answers are `pattern`, `package` and
/// `cancel`. Without anyone to ask, the request stands as it was made.
fn pattern_question(offer: &PatternOffer, description: &str, packages: &[String]) -> Question {
    let name = &offer.name;
    let mut text = if description.is_empty() { offer.summary.clone() } else { description.to_owned() };
    if !packages.is_empty() {
        text.push_str(&format!("\n\nThe pattern brings, besides {name}: {}.", packages.join(", ")));
    }
    let pattern = Choice { value: "pattern".into(), label: format!("The complete setup: the {name} pattern"), default: !offer.package };
    let other = if offer.package {
        Choice { value: "package".into(), label: format!("Only the {name} package"), default: true }
    } else {
        Choice { value: "cancel".into(), label: "Cancel, install nothing".into(), default: false }
    };
    let title = if offer.package { format!("{name}: just the package, or the complete setup?") } else { format!("{name} is a pattern") };
    Question { title, text, choices: vec![pattern, other] }
}

/// The install targets: each requested name, or `pattern:<name>` where the
/// user picks the pattern of the same name.
async fn with_patterns(ctx: &Context, pkgs: &[&str]) -> Result<Vec<String>> {
    let mut args = vec!["search", "--match-exact", "-t", "package", "-t", "pattern", "--"];
    args.extend_from_slice(pkgs);
    // Only a suggestion: if the search fails, the install goes ahead as asked.
    let offers = match zypper::query(ctx, &args).await {
        Ok(xml) => pattern_offers(&xml)?,
        Err(_) => Vec::new(),
    };
    let mut targets: Vec<String> = pkgs.iter().map(|p| (*p).to_owned()).collect();
    for offer in offers {
        let info = zypper::query(ctx, &["info", "-t", "pattern", "--", &offer.name]).await.unwrap_or_default();
        let messages = zypper::parse(&info).map(|d| zypper::info_messages(&d).join("\n")).unwrap_or_default();
        let (description, packages) = pattern_details(&messages, &offer.name);
        let q = pattern_question(&offer, &description, &packages);
        let default = q.default_choice().map(|c| c.value.clone()).unwrap_or_default();
        match ask::ask(q).await.unwrap_or(default).as_str() {
            "pattern" => {
                for t in targets.iter_mut().filter(|t| **t == offer.name) {
                    *t = format!("pattern:{}", offer.name);
                }
            }
            "cancel" => bail!("cancelled, nothing was installed"),
            _ => {}
        }
    }
    Ok(targets)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of `zypper --xmlout list-updates` on Leap.
    const LIST_UPDATES: &str = "<?xml version='1.0'?>
<stream>
<message type=\"info\">Loading repository data...</message>
<update-status version=\"0.6\">
<update-list>
<update kind=\"package\" name=\"vim\" edition=\"9.1.2-1.1\" arch=\"x86_64\" edition-old=\"9.1.1-1.1\"><summary>Vi IMproved</summary><source url=\"http://cdn.opensuse.org/x\" alias=\"repo-oss\"/></update>
<update kind=\"package\" name=\"kernel-default\" edition=\"6.12.0-9.1\" arch=\"x86_64\" edition-old=\"6.12.0-8.1\"><summary>The Standard Kernel</summary><source url=\"http://cdn.opensuse.org/x\" alias=\"repo-oss\"/></update>
<update kind=\"package\" name=\"kernel-default-devel\" edition=\"6.12.0-9.1\" arch=\"x86_64\" edition-old=\"6.12.0-8.1\"><summary>Development files</summary><source url=\"http://cdn.opensuse.org/x\" alias=\"repo-oss\"/></update>
</update-list>
</update-status>
</stream>";

    /// The shape of `zypper --xmlout list-patches` on Leap.
    const LIST_PATCHES: &str = "<?xml version='1.0'?>
<stream>
<update-status version=\"0.6\">
<update-list>
<update kind=\"patch\" name=\"openSUSE-Leap-16.0-2026-41\" edition=\"1\" arch=\"noarch\" status=\"needed\" category=\"recommended\" severity=\"moderate\"><summary>Recommended update for crypto-policies</summary></update>
<update kind=\"patch\" name=\"openSUSE-Leap-16.0-2026-12\" edition=\"1\" arch=\"noarch\" status=\"applied\" category=\"security\" severity=\"important\"><summary>Security update for curl</summary></update>
<update kind=\"patch\" name=\"openSUSE-Leap-16.0-2026-57\" edition=\"1\" arch=\"noarch\" status=\"needed\" category=\"security\" severity=\"low\"><summary>Security update for vim</summary></update>
<update kind=\"patch\" name=\"openSUSE-Leap-16.0-2026-60\" edition=\"1\" arch=\"noarch\" status=\"needed\" category=\"security\" severity=\"important\"><summary>Security update for the Linux Kernel</summary></update>
</update-list>
</update-status>
</stream>";

    #[test]
    fn needed_patches_security_first() {
        let t = needed_patches(LIST_PATCHES).unwrap();
        let names: Vec<&str> = t.rows.iter().map(|r| t.cell(r, "patch").unwrap()).collect();
        assert_eq!(names, ["openSUSE-Leap-16.0-2026-60", "openSUSE-Leap-16.0-2026-57", "openSUSE-Leap-16.0-2026-41"]);
        assert_eq!(t.cell(&t.rows[0], "summary"), Some("Security update for the Linux Kernel"));
    }

    #[test]
    fn pending_updates_and_which_need_a_reboot() {
        let u = pending_updates(LIST_UPDATES).unwrap();
        let names: Vec<&str> = u.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["vim", "kernel-default", "kernel-default-devel"]);
        assert_eq!((u[1].installed.as_str(), u[1].available.as_str(), u[1].repo.as_str()), ("6.12.0-8.1", "6.12.0-9.1", "repo-oss"));
        let reboot: Vec<bool> = u.iter().map(PendingUpdate::needs_reboot).collect();
        assert_eq!(reboot, [false, true, false]);
        let p = |n: &str| PendingUpdate { name: n.into(), installed: String::new(), available: String::new(), repo: String::new() };
        assert!(p("glibc").needs_reboot() && p("ucode-intel").needs_reboot() && p("kernel-firmware-intel").needs_reboot());
        assert!(!p("glibc-locale").needs_reboot() && !p("kernel-macros").needs_reboot());
    }

    /// Recorded from `zypper --xmlout search --match-exact -t package -t pattern -- hyprland gnome fish`
    /// on Tumbleweed.
    const SEARCH: &str = "<?xml version='1.0'?>
<stream>
<message type=\"info\">Loading repository data...</message>
<search-result version=\"0.0\">
<solvable-list>
<solvable status=\"not-installed\" name=\"fish\" summary=\"The &quot;friendly interactive shell&quot;\" kind=\"package\"/>
<solvable status=\"not-installed\" name=\"gnome\" summary=\"GNOME Desktop Environment (Wayland)\" kind=\"pattern\"/>
<solvable status=\"not-installed\" name=\"hyprland\" summary=\"Dynamic tiling Wayland compositor\" kind=\"package\"/>
<solvable status=\"not-installed\" name=\"hyprland\" summary=\"Hyprland (Wayland compositor)\" kind=\"pattern\"/>
<solvable status=\"installed\" name=\"base\" summary=\"Base System\" kind=\"pattern\"/>
</solvable-list>
</search-result>
</stream>";

    /// The info message of `zypper --xmlout info -t pattern hyprland`, shortened.
    const INFO: &str = "Repository      : openSUSE-Tumbleweed-Oss
Name            : hyprland
Summary         : Hyprland (Wayland compositor)
Description     : 
    Hyprland is a dynamic tiling Wayland compositor.

    This pattern installs a minimal Hyprland session with common essentials.
Contents        : 
    S  | Name                        | Type    | Dependency
    ---+-----------------------------+---------+------------
       | pattern:base                | pattern | Required
       | hyprland                    | package | Required
       | xdg-desktop-portal-hyprland | package | Required
       | kitty                       | package | Recommended
       | rofi-wayland                | package | Recommended";

    #[test]
    fn patterns_named_like_the_request_are_offered() {
        let offers = pattern_offers(SEARCH).unwrap();
        assert_eq!(
            offers,
            [
                PatternOffer { name: "gnome".into(), summary: "GNOME Desktop Environment (Wayland)".into(), package: false },
                PatternOffer { name: "hyprland".into(), summary: "Hyprland (Wayland compositor)".into(), package: true },
            ]
        );
    }

    #[test]
    fn the_question_shows_what_the_pattern_brings() {
        let (description, packages) = pattern_details(INFO, "hyprland");
        assert_eq!(
            description,
            "Hyprland is a dynamic tiling Wayland compositor.\n\nThis pattern installs a minimal Hyprland session with common essentials."
        );
        assert_eq!(packages, ["xdg-desktop-portal-hyprland", "kitty", "rofi-wayland"]);
        let offers = pattern_offers(SEARCH).unwrap();
        let q = pattern_question(&offers[1], &description, &packages);
        assert!(q.text.ends_with("besides hyprland: xdg-desktop-portal-hyprland, kitty, rofi-wayland."), "{}", q.text);
        // Nobody to ask: what was asked for.
        assert_eq!(q.default_choice().unwrap().value, "package");
        // No package of that name: the pattern, or nothing.
        let q = pattern_question(&offers[0], "", &[]);
        assert_eq!(q.choices.iter().map(|c| c.value.as_str()).collect::<Vec<_>>(), ["pattern", "cancel"]);
        assert_eq!(q.default_choice().unwrap().value, "pattern");
    }
}
