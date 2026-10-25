//! Upgrades and migrations, the matrix of opensuse-migration-tool:
//! - Leap: a newer Leap, Tumbleweed (rolling), Slowroll (batched rolling) or
//!   SUSE Linux Enterprise (official support, from Leap 15.6)
//! - Tumbleweed: a full upgrade, or Slowroll; Slowroll: a full upgrade, or
//!   Tumbleweed; immutable rolling systems: a full upgrade
//!
//! Before any move to another release, every repository that might not work
//! there is disabled (see `repo_fate`); one repository that can't be
//! refreshed makes `zypper dup` fail.

use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use yaist_core::shell_quote as sq;
use yaist_core::{validate, ActionSpec, Context, Distro, ModuleSpec, Output, Param, Registry, Risk, Table};

use crate::{helper, zypper};

const RELEASES_API: &str = "https://get.opensuse.org/api/v0/distributions.json";
/// The "release" used for repository checks when the target rolls.
const ROLLING: &str = "rolling";
const SUSE_SERVER: &str = "https://www.suse.com/products/server/";

fn target_ok(s: &str) -> std::result::Result<(), String> {
    match s.strip_prefix("leap:") {
        Some(v) => validate::release_version(v),
        None if ["dup", "tumbleweed", "slowroll", "sle"].contains(&s) => Ok(()),
        None => Err("expected leap:<version>, tumbleweed, slowroll, sle or dup (see upgrade.targets)".into()),
    }
}

fn email_ok(s: &str) -> std::result::Result<(), String> {
    let ok = s.split_once('@').is_some_and(|(u, d)| !u.is_empty() && d.contains('.'))
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "@._%+-".contains(c))
        && !s.starts_with('-');
    if ok {
        Ok(())
    } else {
        Err("expected an email address".into())
    }
}

fn regcode_ok(s: &str) -> std::result::Result<(), String> {
    if (4..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        Ok(())
    } else {
        Err("expected a SUSE registration code".into())
    }
}

const KEEP: Param = Param::text(
    "keep_enabled",
    "Repository aliases to keep enabled anyway, space separated. Only release-independent ones: a kept repository that can't be refreshed for the target makes zypper dup fail",
)
.default("google-chrome")
.check(validate::names);

const TARGETS: ActionSpec = ActionSpec {
    id: "upgrade.targets",
    title: "Upgrade options",
    description: "Show what this system can upgrade or migrate to (newer Leap, Tumbleweed, Slowroll, SUSE Linux Enterprise, or a full upgrade on rolling releases), what each means, and which repositories it would disable.",
    risk: Risk::Read,
    params: &[],
};

const MIGRATE: ActionSpec = ActionSpec {
    id: "upgrade.migrate",
    title: "Upgrade",
    description: "Upgrade or migrate: target=leap:<version> (newer Leap release), tumbleweed (rolling release), slowroll (Tumbleweed with batched updates) or dup (full upgrade of a rolling release). Disables repositories that may not work on the target. Takes long; reboot afterwards. For SUSE Linux Enterprise use to_sle.",
    risk: Risk::Dangerous,
    params: &[Param::text("target", "What to move to, as listed in the upgrade options").check(target_ok), KEEP],
};

const TO_SLE: ActionSpec = ActionSpec {
    id: "upgrade.to_sle",
    title: "SUSE Linux Enterprise",
    description: "Migrate Leap to SUSE Linux Enterprise Server for official support by SUSE (the same version, e.g. Leap 16.0 to SLES 16.0). Needs a subscription: the email and registration code from the SUSE Customer Center. The migration follows the community opensuse-migration-tool and is not itself supported by SUSE; the resulting SLES is. One-way.",
    risk: Risk::Dangerous,
    params: &[
        Param::text("email", "Email address of your SUSE Customer Center account").check(email_ok),
        Param::text("regcode", "Registration code from the SUSE Customer Center").secret().check(regcode_ok),
        KEEP,
    ],
};

pub fn register(reg: &mut Registry) {
    reg.module(ModuleSpec {
        id: "upgrade",
        title: "Upgrade",
        tagline: "New releases and migrations",
        overview: TARGETS.id,
        row_actions: &[MIGRATE.id],
    });
    reg.action(TARGETS, |ctx, _| async move { targets(&ctx).await });
    reg.action(MIGRATE, |ctx, args| async move {
        let keep: Vec<String> = args.str("keep_enabled").split_whitespace().map(str::to_owned).collect();
        migrate(&ctx, args.str("target"), &keep).await
    });
    reg.action(TO_SLE, |ctx, args| async move {
        let keep: Vec<String> = args.str("keep_enabled").split_whitespace().map(str::to_owned).collect();
        to_sle(&ctx, args.str("email"), args.str("regcode"), &keep).await
    });
    reg.shell(TARGETS.id, |_, _| vec!["cat /etc/os-release".into(), "zypper repos --uri".into()]);
    reg.shell(MIGRATE.id, |a, d| migrate_shell(a.str("target"), a.str("keep_enabled"), d));
    reg.shell(TO_SLE.id, |a, _| {
        vec![
            format!("sudo zypper modifyrepo --disable <every repository except openSUSE:* and {}>", a.str("keep_enabled")),
            "sudo zypper install suseconnect-ng snapper grub2-snapper-plugin".into(),
            "sudo cp /etc/os-release /etc/os-release.backup   # then write a SLES os-release and product".into(),
            "sudo rpm -e --nodeps openSUSE-release openSUSE-repos".into(),
            format!("sudo suseconnect -e {} -r REGCODE", sq(a.str("email"))),
            "sudo suseconnect -p PackageHub/$VERSION_ID/$(uname -m)".into(),
            "sudo zypper dup --force-resolution --allow-vendor-change --download-in-advance".into(),
            "sudo zypper install branding-SLE-<major> grub2-branding-SLE wallpaper-branding-SLE-<major> plymouth-branding-SLE systemd-presets-branding-SLE"
                .into(),
            "sudo reboot".into(),
        ]
    });
}

fn migrate_shell(target: &str, keep: &str, d: &Distro) -> Vec<String> {
    let z = if d.transactional { "sudo transactional-update --continue run zypper" } else { "sudo zypper" };
    if target == "dup" {
        return vec![
            if d.transactional { "sudo transactional-update --continue dup".into() } else { "sudo zypper dup".into() },
            "sudo reboot".into(),
        ];
    }
    let mut cmds = vec![format!("{z} modifyrepo --disable <every repository except openSUSE:*, NVIDIA:* and {keep}>")];
    let (url, pkg, extra) = match target {
        "tumbleweed" => ("https://download.opensuse.org/tumbleweed/repo/oss/".to_owned(), "openSUSE-repos-Tumbleweed", ""),
        "slowroll" => ("https://download.opensuse.org/slowroll/repo/oss/".to_owned(), "openSUSE-repos-Slowroll", " --allow-downgrade"),
        t => {
            let v = sq(t.strip_prefix("leap:").unwrap_or("<version>"));
            let (pkg, url) = if d.id == "opensuse-leap-micro" {
                (
                    "openSUSE-repos-LeapMicro",
                    format!("https://download.opensuse.org/distribution/leap-micro/{v}/product/repo/openSUSE-Leap-Micro-{v}-$(uname -m)/"),
                )
            } else {
                ("openSUSE-repos-Leap", format!("https://download.opensuse.org/distribution/leap/{v}/repo/oss"))
            };
            cmds.extend([
                format!("{z} install --force-resolution {pkg}"),
                format!("{z} refresh-services"),
                format!("{z} addrepo --refresh {url} tmp"),
                format!("{z} install --repo tmp --from tmp --force-resolution {pkg}"),
                format!("{z} removerepo tmp"),
                format!("{z} refresh-services"),
                format!("{z} --releasever {v} dup --force-resolution --allow-vendor-change --download-in-advance"),
                format!(
                    "{z} --releasever {v} install --force-resolution {}   # the target's release package",
                    if d.id == "opensuse-leap-micro" {
                        "<release package>"
                    } else if v.starts_with("15.") {
                        "openSUSE-release"
                    } else {
                        "Leap-release"
                    }
                ),
                "sudo reboot".into(),
            ]);
            return cmds;
        }
    };
    cmds.extend([
        format!("{z} addrepo --refresh {url} tmp"),
        format!("{z} install --repo tmp --from tmp --force-resolution{extra} {pkg}"),
        format!("{z} removerepo tmp"),
        format!("{z} refresh-services"),
        format!("{z} dup --force-resolution --allow-vendor-change --download-in-advance{extra}"),
        format!("{z} install --force-resolution{extra} openSUSE-release   # the target's release package"),
        "sudo reboot".into(),
    ]);
    cmds
}

fn is_slowroll(d: &Distro) -> bool {
    d.id.contains("slowroll") || d.pretty.contains("Slowroll")
}

/// The Leap line this system follows: (get.opensuse.org key, helper flavour,
/// display name). Immutable Leap is `opensuse-leap` too, with
/// `VARIANT_ID=transactional`.
fn leap_line(d: &yaist_core::Distro) -> Option<(&'static str, &'static str, &'static str)> {
    match d.id.as_str() {
        "opensuse-leap" => Some(("Leap", "leap", "openSUSE Leap")),
        "opensuse-leap-micro" => Some(("LeapMicro", "leap-micro", "openSUSE Leap Micro")),
        _ => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Fate {
    /// openSUSE's own, switched to the new release by openSUSE-repos.
    Official,
    /// Release-independent, asked to be kept.
    Keep,
    /// Follows $releasever; kept only if it already has the new release.
    CheckRelease,
    Disable,
}

/// What happens to an enabled repository in a Leap upgrade, following
/// opensuse-migration-tool: openSUSE's own repositories switch with
/// openSUSE-repos, release-independent ones the user keeps stay, repositories
/// using $releasever stay if they already have the new release (checked
/// online), and everything else is disabled: hard-coded old releases, OBS
/// leftovers that would ask for impossible upgrades, vendor repositories.
fn repo_fate(alias: &str, raw_url: &str, keep: &[String]) -> Fate {
    if alias.starts_with("openSUSE:") || alias.starts_with("NVIDIA:") {
        Fate::Official
    } else if keep.iter().any(|k| k == alias) {
        Fate::Keep
    } else if raw_url.contains("releasever") {
        Fate::CheckRelease
    } else {
        Fate::Disable
    }
}

/// `baseurl` of a repository as written in /etc/zypp/repos.d, variables
/// unexpanded.
fn raw_baseurl(alias: &str) -> String {
    let header = format!("[{alias}]");
    std::fs::read_dir("/etc/zypp/repos.d")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .find_map(|text| {
            let mut in_section = false;
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_section = line == header;
                } else if in_section {
                    if let Some(url) = line.strip_prefix("baseurl=") {
                        return Some(url.trim().to_owned());
                    }
                }
            }
            None
        })
        .unwrap_or_default()
}

/// Does the repository already have metadata for `target`?
async fn has_release(client: &reqwest::Client, raw_url: &str, target: &str) -> bool {
    let arch = std::env::consts::ARCH;
    let url = raw_url.replace("${releasever}", target).replace("$releasever", target).replace("${basearch}", arch).replace("$basearch", arch);
    let probe = format!("{}/repodata/repomd.xml", url.trim_end_matches('/'));
    client.head(&probe).send().await.map(|r| r.status().is_success()).unwrap_or(false)
}

/// Repositories to disable before upgrading to `target`, and those kept with
/// the reason.
async fn repos_to_disable(ctx: &Context, keep: &[String], target: &str) -> Result<(Vec<String>, Vec<String>)> {
    let xml = zypper::query(ctx, &["repos"]).await?;
    let doc = zypper::parse(&xml)?;
    let client = reqwest::Client::builder().timeout(Duration::from_secs(8)).build()?;
    let mut disable = Vec::new();
    let mut kept = Vec::new();
    let mut checks = Vec::new();
    for r in doc.descendants().filter(|n| n.has_tag_name("repo") && n.attribute("enabled") == Some("1")) {
        let Some(alias) = r.attribute("alias") else { continue };
        let raw = raw_baseurl(alias);
        match repo_fate(alias, &raw, keep) {
            Fate::Official => {}
            Fate::Keep => kept.push(format!("{alias} (kept)")),
            Fate::Disable => disable.push(alias.to_owned()),
            Fate::CheckRelease => checks.push((alias.to_owned(), raw)),
        }
    }
    // Moving to a rolling release, $releasever has no meaning for these
    // repositories (Packman's openSUSE_Leap_$releasever has no Tumbleweed
    // tree), so they are disabled. Otherwise probe them concurrently.
    if target == ROLLING {
        disable.extend(checks.into_iter().map(|(alias, _)| alias));
        return Ok((disable, kept));
    }
    let tasks: Vec<_> = checks
        .into_iter()
        .map(|(alias, raw)| {
            let (client, target) = (client.clone(), target.to_owned());
            tokio::spawn(async move {
                let ready = has_release(&client, &raw, &target).await;
                (alias, ready)
            })
        })
        .collect();
    for task in tasks {
        let (alias, ready) = task.await?;
        if ready {
            kept.push(format!("{alias} (has {target})"));
        } else {
            disable.push(alias);
        }
    }
    Ok((disable, kept))
}

/// Newer, supported releases of a Leap line from get.opensuse.org: (version, state).
async fn leap_releases(key: &str, current: &str) -> Result<Vec<(String, String)>> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?;
    let v: Value = client.get(RELEASES_API).send().await?.error_for_status()?.json().await.map_err(|e| anyhow!("reading {RELEASES_API}: {e}"))?;
    let mut out: Vec<(String, String)> = v[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| Some((r["version"].as_str()?.to_owned(), r["state"].as_str().unwrap_or("").to_owned())))
        .filter(|(ver, state)| state != "EOL" && validate::release_version(ver).is_ok() && validate::version_newer(ver, current))
        .collect();
    out.sort_by(|a, b| if validate::version_newer(&a.0, &b.0) { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less });
    Ok(out)
}

/// This Leap release's phase on get.opensuse.org while it isn't out yet
/// (Alpha, Beta, RC); `None` once it is stable, and when unknown or offline.
/// Pre-releases are updated with dup, as their packages may go backwards.
pub(crate) async fn prerelease_phase(d: &Distro) -> Option<String> {
    let (key, _, _) = leap_line(d)?;
    // As long as leap_releases waits: with broken IPv6 the answer can take
    // seconds.
    let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().ok()?;
    let v: Value = client.get(RELEASES_API).send().await.ok()?.json().await.ok()?;
    phase_of(&v, key, &d.version)
}

fn phase_of(releases: &Value, key: &str, version: &str) -> Option<String> {
    let state = releases[key].as_array()?.iter().find(|r| r["version"].as_str() == Some(version))?["state"].as_str()?;
    matches!(state.to_ascii_lowercase().as_str(), "alpha" | "beta" | "rc").then(|| state.to_owned())
}

fn x86_64_v2() -> bool {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let flags: Vec<&str> = cpuinfo.lines().find(|l| l.starts_with("flags")).map(|l| l.split_whitespace().collect()).unwrap_or_default();
    ["cx16", "sse4_1", "sse4_2", "popcnt"].iter().all(|f| flags.contains(f))
}

async fn targets(ctx: &Context) -> Result<Output> {
    let d = &ctx.distro;
    let arch = std::env::consts::ARCH;
    let keep = ["google-chrome".to_owned()];
    let mut t = Table::new(&[
        ("target", "Target"),
        ("title", "Move to"),
        ("kind", "Kind"),
        ("status", "Status"),
        ("description", "What it means"),
        ("notes", "What happens"),
    ]);
    let repo_notes = |disable: &[String], kept: &[String]| {
        let mut n = Vec::new();
        if !disable.is_empty() {
            n.push(format!("disables {} repositories: {}", disable.len(), disable.join(", ")));
        }
        if !kept.is_empty() {
            n.push(format!("keeps {}", kept.join(", ")));
        }
        n.join("; ")
    };
    let rolling_repos = if leap_line(d).is_some() || d.rolling { repos_to_disable(ctx, &keep, ROLLING).await.ok() } else { None };
    let rolling_notes = rolling_repos.as_ref().map(|(dis, kept)| repo_notes(dis, kept)).unwrap_or_default();

    if d.rolling {
        let how = if d.transactional {
            "transactional-update dup into a new snapshot; reboot to use it"
        } else {
            "zypper dup: the normal way to update a rolling release"
        };
        t.push(vec![
            "dup".into(),
            format!("Full upgrade of {}", d.pretty),
            "upgrade".into(),
            "ready".into(),
            "Bring the system up to the latest snapshot.".into(),
            how.into(),
        ]);
        if !d.transactional {
            if is_slowroll(d) {
                let status = if matches!(arch, "x86_64" | "aarch64") { "ready" } else { "not available" };
                t.push(vec![
                    "tumbleweed".into(),
                    "openSUSE Tumbleweed".into(),
                    "migrate".into(),
                    status.into(),
                    "Rolling release with daily updates: always the latest software.".into(),
                    rolling_notes.clone(),
                ]);
            } else {
                let status = if arch == "x86_64" { "ready" } else { "not available" };
                t.push(vec![
                    "slowroll".into(),
                    "openSUSE Slowroll".into(),
                    "migrate".into(),
                    status.into(),
                    "Tumbleweed with batched updates about once a month: rolling, but calmer.".into(),
                    rolling_notes.clone(),
                ]);
            }
        }
        return Ok(Output::Table(t));
    }

    let Some((key, flavour, line)) = leap_line(d) else {
        return Ok(Output::Text(format!("Upgrades from {} are not supported yet.", if d.pretty.is_empty() { &d.id } else { &d.pretty })));
    };
    match leap_releases(key, &d.version).await {
        Ok(releases) => {
            for (ver, state) in releases {
                let (disable, kept) = repos_to_disable(ctx, &keep, &ver).await?;
                let mut notes = vec![format!("switch openSUSE repos to {ver}, then zypper --releasever {ver} dup")];
                if d.transactional {
                    notes.push("inside a new snapshot; reboot to use it".into());
                }
                let r = repo_notes(&disable, &kept);
                if !r.is_empty() {
                    notes.push(r);
                }
                let major: u32 = ver.split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0);
                let status = if flavour == "leap" && arch == "x86_64" && major >= 16 && !x86_64_v2() {
                    notes.insert(0, "needs an x86-64-v2 CPU".into());
                    "not available".to_owned()
                } else if state.eq_ignore_ascii_case("stable") {
                    "ready".to_owned()
                } else {
                    state.to_lowercase()
                };
                t.push(vec![
                    format!("leap:{ver}"),
                    format!("{line} {ver}"),
                    "upgrade".into(),
                    status,
                    "A newer release of what you run now: same experience, newer software.".into(),
                    notes.join("; "),
                ]);
            }
        }
        Err(e) => t.push(vec![
            "leap:?".into(),
            format!("Newer {line}"),
            "upgrade".into(),
            "not available".into(),
            "Could not fetch the release list from get.opensuse.org.".into(),
            format!("{e:#}"),
        ]),
    }
    if flavour == "leap" && !d.transactional {
        let tw = if matches!(arch, "x86_64" | "aarch64") { "ready" } else { "not available" };
        t.push(vec![
            "tumbleweed".into(),
            "openSUSE Tumbleweed".into(),
            "migrate".into(),
            tw.into(),
            "Rolling release: always the latest software, updated continuously. Going back to Leap later means reinstalling.".into(),
            rolling_notes.clone(),
        ]);
        let sr = if arch == "x86_64" { "ready" } else { "not available" };
        t.push(vec![
            "slowroll".into(),
            "openSUSE Slowroll".into(),
            "migrate".into(),
            sr.into(),
            "Tumbleweed with batched updates about once a month: rolling, but calmer.".into(),
            rolling_notes.clone(),
        ]);
        let title = match d.version.split_once('.') {
            Some((major, minor)) if major.parse::<u32>().unwrap_or(0) < 16 => format!("SUSE Linux Enterprise Server {major} SP{minor}"),
            _ => format!("SUSE Linux Enterprise Server {}", d.version),
        };
        let notes = format!(
            "needs a subscription (email and registration code from the SUSE Customer Center); the migration is community-made (opensuse-migration-tool steps), not supported by SUSE; {rolling_notes}"
        );
        t.push(vec![
            "sle".into(),
            title,
            "support".into(),
            "ready".into(),
            "Official support by SUSE for the resulting system, with a subscription. One-way.".into(),
            notes,
        ]);
    }
    Ok(Output::Table(t))
}

async fn migrate(ctx: &Context, target: &str, keep: &[String]) -> Result<Output> {
    let d = &ctx.distro;
    match target {
        "dup" if d.rolling => {
            if d.transactional {
                zypper::transactional(ctx, &["dup"]).await?;
            } else {
                zypper::change(ctx, &["dup"]).await?;
            }
            Ok(Output::Done(format!("{} is fully upgraded; reboot if the kernel or core libraries changed", d.pretty)))
        }
        "dup" => bail!("a full dup is for rolling releases; on {} pick a newer release", d.pretty),
        "sle" => bail!("migrating to SUSE Linux Enterprise needs a registration code: use upgrade.to_sle"),
        "tumbleweed" | "slowroll" => {
            let (aliases, _) = repos_to_disable(ctx, keep, ROLLING).await?;
            let mut args = vec!["migrate-rolling", target];
            args.extend(aliases.iter().map(String::as_str));
            helper::run(ctx, &args).await?;
            Ok(Output::Done(done(&format!("openSUSE {}", if target == "slowroll" { "Slowroll" } else { "Tumbleweed" }), &aliases)))
        }
        t => {
            let version = t.strip_prefix("leap:").ok_or_else(|| anyhow!("unknown target {t}"))?;
            let Some((key, flavour, line)) = leap_line(d) else {
                bail!("upgrades from {} are not supported yet", d.id);
            };
            if !leap_releases(key, &d.version).await?.iter().any(|(v, _)| v == version) {
                bail!("{version} is not an available upgrade from {}", d.version);
            }
            let (aliases, _) = repos_to_disable(ctx, keep, version).await?;
            let mut args = vec!["migrate-leap", flavour, version];
            args.extend(aliases.iter().map(String::as_str));
            helper::run(ctx, &args).await?;
            Ok(Output::Done(done(&format!("{line} {version}"), &aliases)))
        }
    }
}

fn done(what: &str, disabled: &[String]) -> String {
    let mut msg = format!("Moved to {what}; reboot to finish");
    if !disabled.is_empty() {
        msg.push_str(&format!(
            ". Disabled {}: enable them again (Repositories) once they work for {what}. Packages they provided may be orphaned now: zypper packages --orphaned",
            disabled.join(", ")
        ));
    }
    msg
}

/// The registration code reaches yaist-helper through a file only the user
/// can read, never on a command line.
async fn to_sle(ctx: &Context, email: &str, regcode: &str, keep: &[String]) -> Result<Output> {
    let d = &ctx.distro;
    if d.id != "opensuse-leap" || d.transactional {
        bail!("migrating to SUSE Linux Enterprise works from (non-immutable) Leap; see {SUSE_SERVER}");
    }
    let (aliases, _) = repos_to_disable(ctx, keep, ROLLING).await?;
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let path = format!("{dir}/yaist-regcode-{}", std::process::id());
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        f.write_all(regcode.as_bytes())?;
    }
    let mut args = vec!["migrate-sle", email, path.as_str()];
    args.extend(aliases.iter().map(String::as_str));
    let result = helper::run(ctx, &args).await;
    let _ = std::fs::remove_file(&path);
    result?;
    Ok(Output::Done(done(&format!("SUSE Linux Enterprise Server {}", d.version), &aliases)))
}

#[cfg(test)]
mod tests {
    use super::{phase_of, repo_fate, Fate};
    use serde_json::Value;

    #[test]
    fn prerelease_phase_from_the_release_list() {
        let v: Value = serde_json::json!({ "Leap": [
            { "version": "16.1", "state": "RC" },
            { "version": "16.0", "state": "Stable" },
            { "version": "15.5", "state": "EOL" },
        ]});
        assert_eq!(phase_of(&v, "Leap", "16.1").as_deref(), Some("RC"));
        assert_eq!(phase_of(&v, "Leap", "16.0"), None);
        assert_eq!(phase_of(&v, "Leap", "16.2"), None);
        assert_eq!(phase_of(&v, "LeapMicro", "6.2"), None);
    }

    #[test]
    fn repositories_in_an_upgrade() {
        let keep = vec!["google-chrome".to_owned()];
        let fate = |alias: &str, url: &str| repo_fate(alias, url, &keep);
        assert_eq!(fate("openSUSE:repo-oss", "http://cdn.opensuse.org/distribution/leap/${releasever}/repo/oss/$basearch"), Fate::Official);
        assert_eq!(fate("NVIDIA:repo-non-free", "https://download.nvidia.com/opensuse/leap/$releasever"), Fate::Official);
        assert_eq!(fate("google-chrome", "https://dl.google.com/linux/chrome/rpm/stable/x86_64"), Fate::Keep);
        assert_eq!(fate("packman", "https://ftp.fau.de/packman/suse/openSUSE_Leap_$releasever/"), Fate::CheckRelease);
        assert_eq!(fate("home_lkocman_kudos", "https://download.opensuse.org/repositories/home:/lkocman:/kudos/16.0/"), Fate::Disable);
        assert_eq!(fate("old_obs", "https://download.opensuse.org/repositories/games/15.6/"), Fate::Disable);
        assert_eq!(fate("code", "https://packages.microsoft.com/yumrepos/vscode"), Fate::Disable);
    }
}
