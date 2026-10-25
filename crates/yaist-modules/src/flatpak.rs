//! Flatpak, always per user (`--user`): no root needed, and apps don't touch
//! the system. System-wide remotes are shown too, since they exist.

use std::path::Path;

use anyhow::Result;
use yaist_core::Context;

pub const FLATHUB: &str = "flathub";
pub const FLATHUB_URL: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";
const FLATPAK: &str = "/usr/bin/flatpak";

pub fn installed() -> bool {
    Path::new(FLATPAK).exists()
}

/// A configured remote: (name, title, url, user scope, enabled).
pub struct Remote {
    pub name: String,
    pub title: String,
    pub url: String,
    pub user: bool,
    pub enabled: bool,
}

pub async fn remotes(ctx: &Context) -> Vec<Remote> {
    if !installed() {
        return Vec::new();
    }
    let out = ctx.run(&[FLATPAK, "remotes", "--show-disabled", "--columns=name,title,url,options"]).await.unwrap_or_default();
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let options = f.get(3).copied().unwrap_or("");
            (!f.is_empty() && !f[0].is_empty()).then(|| Remote {
                name: f[0].to_owned(),
                title: f.get(1).copied().unwrap_or("").to_owned(),
                url: f.get(2).copied().unwrap_or("").to_owned(),
                user: options.split(',').any(|o| o == "user"),
                enabled: !options.split(',').any(|o| o == "disabled"),
            })
        })
        .collect()
}

pub async fn has_user_remote(ctx: &Context, name: &str) -> bool {
    remotes(ctx).await.iter().any(|r| r.user && r.name == name)
}

/// Installs flatpak itself when missing (an RPM, so root, or a transaction).
pub async fn ensure_installed(ctx: &Context) -> Result<()> {
    if installed() {
        return Ok(());
    }
    if ctx.distro.transactional {
        crate::zypper::transactional(ctx, &["pkg", "install", "flatpak"]).await?;
        anyhow::bail!("flatpak was installed into a new snapshot; reboot, then try again");
    }
    crate::zypper::interactive(ctx, &["install", "--", "flatpak"]).await?;
    Ok(())
}

/// Adds Flathub for the current user if it isn't there yet.
pub async fn ensure_flathub(ctx: &Context) -> Result<()> {
    ensure_installed(ctx).await?;
    if !has_user_remote(ctx, FLATHUB).await {
        ctx.run(&[FLATPAK, "remote-add", "--user", "--if-not-exists", FLATHUB, FLATHUB_URL]).await?;
    }
    Ok(())
}

pub async fn app_installed(ctx: &Context, app: &str) -> bool {
    installed() && ctx.run(&[FLATPAK, "info", "--user", app]).await.is_ok()
}

pub async fn install(ctx: &Context, remote: &str, app: &str) -> Result<()> {
    if remote == FLATHUB {
        ensure_flathub(ctx).await?;
    } else {
        ensure_installed(ctx).await?;
    }
    ctx.run(&[FLATPAK, "install", "--user", "--noninteractive", "--assumeyes", remote, app]).await?;
    Ok(())
}

pub async fn remove(ctx: &Context, app: &str) -> Result<()> {
    ctx.run(&[FLATPAK, "uninstall", "--user", "--noninteractive", "--assumeyes", app]).await?;
    Ok(())
}

/// Flatpak runs as the user; remotes in system scope ask polkit themselves.
pub async fn remote_cmd(ctx: &Context, user: bool, args: &[&str]) -> Result<()> {
    let mut argv = vec![FLATPAK];
    argv.extend_from_slice(args);
    argv.push(if user { "--user" } else { "--system" });
    ctx.run(&argv).await?;
    Ok(())
}

pub const BIN: &str = FLATPAK;
