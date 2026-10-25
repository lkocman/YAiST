//! Repositories: RPM (zypper, including community repositories from the Open
//! Build Service) and Flatpak remotes (per user, like Flathub).

use std::time::Duration;

use anyhow::{bail, Result};
use yaist_core::ask::{self, Choice, Question};
use yaist_core::shell_quote as sq;
use yaist_core::{validate, ActionSpec, Context, ModuleSpec, Output, Param, Registry, Risk, Table};

use crate::{flatpak, zypper};

const ALIAS_PARAM: Param = Param::text("alias", "Repository alias, as listed").check(validate::name);
const KIND_PARAM: Param =
    Param::choice("kind", &["rpm", "flatpak-user", "flatpak-system"], "Repository type, as listed (rpm or flatpak)").default("rpm");

const LIST: ActionSpec = ActionSpec {
    id: "repos.list",
    title: "List repositories",
    description: "List configured repositories: RPM repositories (zypper) and Flatpak remotes, with their URL, whether they are enabled, and whether enabled RPM repositories can be reached (status broken: zypper would skip them).",
    risk: Risk::Read,
    params: &[],
};

const ADD: ActionSpec = ActionSpec {
    id: "repos.add",
    title: "Add repository",
    description: "Add a package repository by URL and trust its signing key. Only for sources the user trusts.",
    risk: Risk::Dangerous,
    params: &[
        Param::text("url", "Repository URL (https://…)").check(validate::repo_url),
        Param::text("alias", "Short unique name, e.g. games").check(validate::name),
    ],
};

const ADD_OBS: ActionSpec = ActionSpec {
    id: "repos.add_obs",
    title: "Add OBS project",
    description: "Add a community repository from the Open Build Service (e.g. home:user or devel:languages:rust), built for this distribution, and trust its signing key.",
    risk: Risk::Dangerous,
    params: &[
        Param::text("project", "OBS project, e.g. games or devel:languages:rust").check(validate::obs_project),
        Param::text("alias", "Short unique name; defaults to the project name").optional().check(validate::name),
    ],
};

const ADD_FLATHUB: ActionSpec = ActionSpec {
    id: "repos.add_flathub",
    title: "Add Flathub",
    description: "Add Flathub, the main source of Flatpak apps, for the current user (flatpak --user, no root needed).",
    risk: Risk::Change,
    params: &[],
};

const NON_OSS: ActionSpec = ActionSpec {
    id: "repos.enable_non_oss",
    title: "Enable non-oss",
    description: "Make sure openSUSE's non-oss repository is set up and enabled: freely distributable software that is not open source, such as Steam, some firmware and fonts.",
    risk: Risk::Change,
    params: &[],
};

const ENABLE: ActionSpec = ActionSpec {
    id: "repos.enable",
    title: "Enable",
    description: "Enable a repository (RPM or Flatpak).",
    risk: Risk::Change,
    params: &[ALIAS_PARAM, KIND_PARAM],
};
const DISABLE: ActionSpec = ActionSpec {
    id: "repos.disable",
    title: "Disable",
    description: "Disable a repository (RPM or Flatpak) without removing it.",
    risk: Risk::Change,
    params: &[ALIAS_PARAM, KIND_PARAM],
};
const REMOVE: ActionSpec = ActionSpec {
    id: "repos.remove",
    title: "Remove",
    description: "Remove a repository (RPM or Flatpak).",
    risk: Risk::Dangerous,
    params: &[ALIAS_PARAM, KIND_PARAM],
};

pub fn register(reg: &mut Registry) {
    reg.module(ModuleSpec {
        id: "repos",
        title: "Repositories",
        tagline: "RPM and Flatpak repositories",
        overview: LIST.id,
        row_actions: &[ENABLE.id, DISABLE.id, REMOVE.id],
        overview_needs_admin: false,
    });

    reg.action(LIST, |ctx, _| async move { list(&ctx).await });

    reg.action(ADD, |ctx, args| async move { add(&ctx, args.str("url"), args.str("alias")).await });

    reg.action(ADD_OBS, |ctx, args| async move {
        let project = args.str("project");
        let url = obs_url(project, &ctx.distro);
        let alias = args.opt_str("alias").map(str::to_owned).unwrap_or_else(|| project.replace(':', "_"));
        add(&ctx, &url, &alias).await
    });

    reg.action(NON_OSS, |ctx, _| async move {
        Ok(Output::Done(match zypper::ensure_non_oss(&ctx).await? {
            Some(what) => format!("non-oss is ready ({what})"),
            None => "non-oss was already enabled".into(),
        }))
    });
    reg.shell(NON_OSS.id, |_, d| {
        vec![
            "sudo zypper modifyrepo --enable openSUSE:repo-non-oss   # if it exists, disabled".into(),
            format!("sudo zypper addrepo --refresh '{}' repo-non-oss   # if it is missing", zypper::non_oss_url(d)),
        ]
    });

    reg.action(ADD_FLATHUB, |ctx, _| async move {
        flatpak::ensure_flathub(&ctx).await?;
        Ok(Output::Done("Flathub added for your user".into()))
    });

    for (spec, verb) in [(ENABLE, "Enabled"), (DISABLE, "Disabled"), (REMOVE, "Removed")] {
        reg.action(spec, move |ctx, args| async move {
            let alias = args.str("alias");
            match args.str("kind") {
                "rpm" => {
                    let op: &[&str] = match spec.verb() {
                        "enable" => &["modifyrepo", "--enable"],
                        "disable" => &["modifyrepo", "--disable"],
                        _ => &["removerepo"],
                    };
                    let mut zargs = op.to_vec();
                    zargs.extend(["--", alias]);
                    zypper::change(&ctx, &zargs).await?;
                }
                kind => {
                    let user = kind == "flatpak-user";
                    match spec.verb() {
                        "enable" => flatpak::remote_cmd(&ctx, user, &["remote-modify", "--enable", alias]).await?,
                        "disable" => flatpak::remote_cmd(&ctx, user, &["remote-modify", "--disable", alias]).await?,
                        _ => flatpak::remote_cmd(&ctx, user, &["remote-delete", alias]).await?,
                    }
                }
            }
            Ok(Output::Done(format!("{verb} {alias}")))
        });
    }

    reg.shell(LIST.id, |_, _| vec!["zypper repos --uri".into()]);
    reg.shell(ADD.id, |a, _| add_cmds(a.str("url"), a.str("alias")));
    reg.shell(ADD_OBS.id, |a, d| {
        let project = a.str("project");
        let alias = a.opt_str("alias").map(str::to_owned).unwrap_or_else(|| project.replace(':', "_"));
        add_cmds(&obs_url(project, d), &alias)
    });
    reg.shell(ADD_FLATHUB.id, |_, _| vec![format!("flatpak remote-add --user --if-not-exists {} {}", flatpak::FLATHUB, flatpak::FLATHUB_URL)]);
    reg.shell(ENABLE.id, |a, _| repo_cmd(a, "sudo zypper modifyrepo --enable", "flatpak remote-modify --enable"));
    reg.shell(DISABLE.id, |a, _| repo_cmd(a, "sudo zypper modifyrepo --disable", "flatpak remote-modify --disable"));
    reg.shell(REMOVE.id, |a, _| repo_cmd(a, "sudo zypper removerepo", "flatpak remote-delete"));
}

fn repo_cmd(a: &yaist_core::Args, rpm: &str, flatpak: &str) -> Vec<String> {
    let alias = sq(a.str("alias"));
    match a.str("kind") {
        "flatpak-user" => vec![format!("{flatpak} --user {alias}")],
        "flatpak-system" => vec![format!("{flatpak} --system {alias}")],
        _ => vec![format!("{rpm} {alias}")],
    }
}

/// The OBS repository URL for `project` on this distribution.
fn obs_url(project: &str, d: &yaist_core::Distro) -> String {
    format!("https://download.opensuse.org/repositories/{}/{}/", project.replace(':', ":/"), d.obs_repo_name())
}

fn add_cmds(url: &str, alias: &str) -> Vec<String> {
    vec![
        format!("sudo zypper addrepo --refresh {} {}", sq(url), sq(alias)),
        format!("sudo zypper --gpg-auto-import-keys refresh --repo {}", sq(alias)),
    ]
}

/// Adds the repository, then refreshes it once to import its key. A
/// repository that can't be refreshed (wrong URL, no build for this
/// distribution) is removed again rather than left half-configured.
async fn add(ctx: &Context, url: &str, alias: &str) -> Result<Output> {
    zypper::change(ctx, &["addrepo", "--refresh", "--", url, alias]).await?;
    if let Err(e) = zypper::change(ctx, &["--gpg-auto-import-keys", "refresh", "--repo", alias]).await {
        let _ = zypper::change(ctx, &["removerepo", "--", alias]).await;
        anyhow::bail!("{url} could not be refreshed, so it was not added: {e}");
    }
    Ok(Output::Done(format!("Added repository {alias} ({url})")))
}

async fn list(ctx: &Context) -> Result<Output> {
    let repos = rpm_repos(ctx).await?;
    let health = check(&repos).await;
    let mut t = Table::new(&[
        ("alias", "Alias"),
        ("name", "Name"),
        ("kind", "Type"),
        ("enabled", "Enabled"),
        ("status", "Status"),
        ("problem", "Problem"),
        ("priority", "Priority"),
        ("url", "URL"),
    ])
    .essential(&["alias", "enabled", "status"]);
    for (r, h) in repos.iter().zip(&health) {
        let (status, problem) = match h {
            Health::Unchecked => ("-", String::new()),
            Health::Ok => ("ok", String::new()),
            Health::Broken(p) | Health::Unreachable(p) => ("broken", p.clone()),
        };
        let enabled = if r.enabled { "yes" } else { "no" };
        t.push(vec![r.alias.clone(), r.name.clone(), "rpm".into(), enabled.into(), status.into(), problem, r.priority.clone(), r.url.clone()]);
    }
    for remote in flatpak::remotes(ctx).await {
        let kind = if remote.user { "flatpak-user" } else { "flatpak-system" };
        let enabled = if remote.enabled { "yes" } else { "no" };
        t.push(vec![remote.name, remote.title, kind.into(), enabled.into(), "-".into(), String::new(), "-".into(), remote.url]);
    }
    Ok(Output::Table(t))
}

/// An RPM repository as zypper lists it (URLs with variables expanded).
struct RpmRepo {
    alias: String,
    name: String,
    url: String,
    priority: String,
    enabled: bool,
    /// `rpm-md`, `yast2`, `plaindir`…; empty until zypper could fetch it.
    kind: String,
}

async fn rpm_repos(ctx: &Context) -> Result<Vec<RpmRepo>> {
    let xml = zypper::query(ctx, &["repos"]).await?;
    let doc = zypper::parse(&xml)?;
    Ok(doc
        .descendants()
        .filter(|n| n.has_tag_name("repo"))
        .map(|r| {
            let attr = |k| r.attribute(k).unwrap_or("").to_owned();
            RpmRepo {
                alias: attr("alias"),
                name: attr("name"),
                url: r.children().find(|c| c.has_tag_name("url")).and_then(|u| u.text()).unwrap_or("").to_owned(),
                priority: attr("priority"),
                enabled: r.attribute("enabled") == Some("1"),
                kind: attr("type"),
            }
        })
        .collect())
}

#[derive(Clone, Debug, PartialEq)]
enum Health {
    /// Disabled, local, or a type without repodata (yast2, plaindir).
    Unchecked,
    Ok,
    /// The server answered, but there is no repository there.
    Broken(String),
    /// No answer at all (offline, DNS, timeout).
    Unreachable(String),
}

/// Checks enabled online repositories the way zypper's refresh starts, by
/// fetching repodata/repomd.xml: no root needed, all at once. A 404 is
/// zypper's "Repository type can't be determined".
async fn check(repos: &[RpmRepo]) -> Vec<Health> {
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(8)).build() else {
        return repos.iter().map(|_| Health::Unchecked).collect();
    };
    let tasks: Vec<_> = repos
        .iter()
        .map(|r| {
            // No type means zypper never managed to fetch the repository:
            // exactly the ones worth checking.
            let rpm_md = matches!(r.kind.as_str(), "rpm-md" | "" | "NONE");
            let checked = r.enabled && rpm_md && (r.url.starts_with("https://") || r.url.starts_with("http://"));
            let probe = format!("{}/repodata/repomd.xml", r.url.trim_end_matches('/'));
            let client = client.clone();
            tokio::spawn(async move {
                if !checked {
                    return Health::Unchecked;
                }
                match client.get(&probe).send().await {
                    Ok(resp) => health_of(resp.status().as_u16()),
                    Err(e) if e.is_timeout() => Health::Unreachable("no answer".into()),
                    Err(_) => Health::Unreachable("unreachable".into()),
                }
            })
        })
        .collect();
    let mut out = Vec::new();
    for task in tasks {
        out.push(task.await.unwrap_or(Health::Unchecked));
    }
    out
}

fn health_of(status: u16) -> Health {
    match status {
        200..=299 => Health::Ok,
        404 | 410 => Health::Broken(format!("no repository at this address (HTTP {status})")),
        s => Health::Broken(format!("HTTP {s}")),
    }
}

/// openSUSE's or SUSE's own repositories: the distribution itself, its
/// updates and debug/source trees, wherever the alias came from. OBS
/// projects (/repositories/) and vendors are not.
fn is_distribution(alias: &str, url: &str) -> bool {
    if alias.starts_with("openSUSE:") || alias.starts_with("SUSE:") || alias.starts_with("SLE") {
        return true;
    }
    let Some(rest) = url.split_once("://").map(|(_, r)| r) else { return false };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let opensuse = matches!(host, "download.opensuse.org" | "cdn.opensuse.org" | "downloadcontent.opensuse.org");
    let suse = host == "updates.suse.com" || host.ends_with(".scc.suse.com");
    suse || (opensuse && !path.starts_with("repositories/"))
}

/// Before a system update: zypper skips repositories it can't refresh and
/// plans without their packages, so a distribution upgrade (dup) could
/// downgrade or remove what came from them. Asks to disable broken ones
/// that aren't the distribution's; returns the ones disabled. Offline (every
/// repository unreachable) or with nobody to ask, nothing changes.
pub(crate) async fn disable_broken_before_update(ctx: &Context) -> Result<Vec<String>> {
    let repos = rpm_repos(ctx).await?;
    let health = check(&repos).await;
    let checked = health.iter().filter(|h| **h != Health::Unchecked).count();
    if checked == 0 || health.iter().filter(|h| matches!(h, Health::Unreachable(_))).count() == checked {
        return Ok(Vec::new());
    }
    let broken: Vec<(&RpmRepo, &str)> = repos
        .iter()
        .zip(&health)
        .filter(|(r, _)| !is_distribution(&r.alias, &r.url))
        .filter_map(|(r, h)| match h {
            Health::Broken(p) | Health::Unreachable(p) => Some((r, p.as_str())),
            _ => None,
        })
        .collect();
    if broken.is_empty() {
        return Ok(Vec::new());
    }
    let list = broken.iter().map(|(r, p)| format!("{} ({}): {p}", r.alias, r.url)).collect::<Vec<_>>().join("\n");
    let (title, them) =
        if broken.len() == 1 { ("Broken repository".to_owned(), "it") } else { (format!("{} broken repositories", broken.len()), "them") };
    let q = Question {
        title,
        text: format!(
            "{list}\n\nzypper would skip {them} and update without {them}; a distribution upgrade (dup) could then downgrade or remove what was installed from {them}. Disabled repositories stay configured and can be enabled again in Repositories."
        ),
        choices: vec![
            Choice { value: "disable".into(), label: format!("Disable {them} and update"), default: true },
            Choice { value: "keep".into(), label: "Update without disabling".into(), default: false },
            Choice { value: "cancel".into(), label: "Cancel, change nothing".into(), default: false },
        ],
    };
    match ask::ask(q).await.as_deref() {
        Some("disable") => {
            let mut disabled = Vec::new();
            for (r, _) in broken {
                zypper::change(ctx, &["modifyrepo", "--disable", "--", &r.alias]).await?;
                disabled.push(r.alias.clone());
            }
            Ok(disabled)
        }
        Some("cancel") => bail!("cancelled, nothing was changed"),
        _ => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_repositories_by_url() {
        assert!(is_distribution("https-download.opensuse.org-f0a9b0b6", "https://download.opensuse.org/distribution/leap/16.0/repo/oss/x86_64"));
        assert!(is_distribution("repo-non-oss", "http://cdn.opensuse.org/distribution/leap/16.0/repo/non-oss/x86_64"));
        assert!(is_distribution("openSUSE:repo-oss", "http://example.org/"));
        assert!(!is_distribution(
            "home_lkocman_branches_graphics",
            "https://download.opensuse.org/repositories/home:/lkocman:/branches:/graphics/16.0/"
        ));
        assert!(!is_distribution("google-chrome", "https://dl.google.com/linux/chrome/rpm/stable/x86_64"));
        assert!(!is_distribution("openh264", "https://codecs.opensuse.org/openh264/openSUSE_Leap_16"));
    }

    #[test]
    fn health_from_http_status() {
        assert_eq!(health_of(200), Health::Ok);
        assert_eq!(health_of(404), Health::Broken("no repository at this address (HTTP 404)".into()));
        assert_eq!(health_of(503), Health::Broken("HTTP 503".into()));
    }
}
