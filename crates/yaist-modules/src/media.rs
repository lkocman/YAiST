//! Gaming and multimedia: graphics drivers, Steam, controllers, the SELinux
//! gaming policy and codecs, as a checklist the user can work through.
//!
//! Steam is an RPM on Tumbleweed (non-oss, needs 32-bit Vulkan). Leap has no
//! 32-bit libraries and immutable systems keep apps out of the base system,
//! so there Steam is the Flathub Flatpak, installed per user.

use anyhow::{bail, Result};
use yaist_core::shell_quote as sq;
use yaist_core::{ActionSpec, Context, Distro, ModuleSpec, Output, Param, Registry, Risk, Table};

use crate::{flatpak, helper, packages, zypper};

const STEAM_FLATPAK: &str = "com.valvesoftware.Steam";
const ITEMS: &[&str] = &["graphics", "non-oss", "steam", "controllers", "selinux", "gamemode", "openh264", "flathub"];

const STATUS: ActionSpec = ActionSpec {
    id: "media.status",
    title: "Gaming and media status",
    description: "Show what is set up for gaming and multimedia: graphics driver for the detected GPU, Steam, controllers, the SELinux gaming policy, codecs and Flathub.",
    risk: Risk::Read,
    params: &[],
};

const SET_UP: ActionSpec = ActionSpec {
    id: "media.set_up",
    title: "Set up",
    description: "Set up one item of the gaming and media checklist: graphics (Vulkan driver, or NVIDIA drivers), non-oss (the repository Steam comes from on Tumbleweed), steam, controllers (steam-devices), selinux (gaming policy), gamemode, openh264 (official H.264 codec) or flathub.",
    risk: Risk::Change,
    params: &[Param::choice("item", ITEMS, "What to set up")],
};

const GAMING: ActionSpec = ActionSpec {
    id: "media.set_up_gaming",
    title: "Set up gaming",
    description: "Everything for gaming in one go: graphics driver for the GPU, Steam (RPM on Tumbleweed, Flathub Flatpak for the user on Leap and immutable systems), controller support and the SELinux gaming policy.",
    risk: Risk::Change,
    params: &[],
};

const OPENH264: ActionSpec = ActionSpec {
    id: "media.openh264",
    title: "Install openh264",
    description:
        "Install the official openh264 codec (H.264 for video calls and browsers) from openSUSE's codec repository. Safe default for most people.",
    risk: Risk::Change,
    params: &[],
};

const PACKMAN: ActionSpec = ActionSpec {
    id: "media.packman_codecs",
    title: "Full codecs (Packman)",
    description: "Install full multimedia codecs from Packman, a third-party repository, with opi: switches ffmpeg, GStreamer and VLC codecs to Packman's builds. Not for immutable systems; Flatpak apps bring their own codecs instead.",
    risk: Risk::Dangerous,
    params: &[],
};

pub fn register(reg: &mut Registry) {
    reg.module(ModuleSpec {
        id: "media",
        title: "Gaming & media",
        tagline: "Steam, graphics drivers, codecs",
        overview: STATUS.id,
        row_actions: &[SET_UP.id],
        overview_needs_admin: false,
    });
    reg.action(STATUS, |ctx, _| async move { status(&ctx).await });
    reg.action(SET_UP, |ctx, args| async move { set_up(&ctx, &[args.str("item")]).await });
    reg.action(GAMING, |ctx, _| async move { set_up(&ctx, &["graphics", "steam", "controllers", "selinux"]).await });
    reg.action(OPENH264, |ctx, _| async move { set_up(&ctx, &["openh264"]).await });
    reg.action(PACKMAN, |ctx, _| async move {
        helper::run(&ctx, &["codecs-packman"]).await?;
        Ok(Output::Done("Installed full codecs from Packman".into()))
    });

    reg.shell(STATUS.id, |_, _| {
        vec!["lspci | grep -iE 'vga|3d'".into(), "rpm -q steam steam-devices selinux-policy-targeted-gaming".into(), "flatpak remotes --user".into()]
    });
    reg.shell(SET_UP.id, |a, d| shell(&[a.str("item")], d));
    reg.shell(GAMING.id, |_, d| shell(&["graphics", "steam", "controllers", "selinux"], d));
    reg.shell(OPENH264.id, |_, d| shell(&["openh264"], d));
    reg.shell(PACKMAN.id, |_, _| vec!["sudo zypper install opi".into(), "opi codecs".into()]);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Vendor {
    Amd,
    Intel,
    Nvidia,
}

/// GPUs from /sys (vendor) and, when available, lspci (model names).
fn gpus() -> Vec<(Vendor, String)> {
    let mut found = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/sys/class/drm") {
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            let vendor = std::fs::read_to_string(e.path().join("device/vendor")).unwrap_or_default();
            let v = match vendor.trim() {
                "0x1002" => Vendor::Amd,
                "0x8086" => Vendor::Intel,
                "0x10de" => Vendor::Nvidia,
                _ => continue,
            };
            if !found.iter().any(|(f, _)| *f == v) {
                found.push((v, String::new()));
            }
        }
    }
    let lspci =
        std::process::Command::new("/usr/bin/lspci").arg("-mm").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    for (v, desc) in found.iter_mut() {
        let key = match v {
            Vendor::Amd => "AMD",
            Vendor::Intel => "Intel",
            Vendor::Nvidia => "NVIDIA",
        };
        *desc = lspci
            .lines()
            .filter(|l| l.contains("VGA") || l.contains("3D controller") || l.contains("Display controller"))
            .filter_map(|l| {
                let f: Vec<&str> = l.split('"').filter(|x| !x.trim().is_empty()).collect();
                (f.len() >= 4 && f[2].contains(key)).then(|| format!("{key} {}", f[3]))
            })
            .next()
            .unwrap_or_else(|| key.to_owned());
    }
    found
}

/// Steam comes as an RPM only on standard Tumbleweed.
fn steam_rpm(d: &Distro) -> bool {
    d.rolling && !d.transactional
}

fn vulkan_rpms(v: Vendor, d: &Distro) -> Vec<&'static str> {
    let base = match v {
        Vendor::Amd => "libvulkan_radeon",
        Vendor::Intel => "libvulkan_intel",
        Vendor::Nvidia => return Vec::new(),
    };
    if steam_rpm(d) {
        // The Steam RPM is 32-bit underneath.
        vec![base, if base == "libvulkan_radeon" { "libvulkan_radeon-32bit" } else { "libvulkan_intel-32bit" }, "libvulkan1-32bit"]
    } else {
        vec![base]
    }
}

async fn rpm_installed(ctx: &Context, pkg: &str) -> bool {
    ctx.exit_code(&["/usr/bin/rpm", "-q", "--quiet", pkg]).await.ok() == Some(0)
}

async fn nvidia_installed(ctx: &Context) -> bool {
    ctx.run(&["/usr/bin/rpm", "-qa", "nvidia-*G0[0-9]*"]).await.map(|o| !o.trim().is_empty()).unwrap_or(false)
}

fn selinux_enabled() -> bool {
    std::path::Path::new("/sys/fs/selinux/enforce").exists()
}

async fn status(ctx: &Context) -> Result<Output> {
    let d = &ctx.distro;
    let mut t = Table::new(&[("item", "Item"), ("status", "Status"), ("detail", "Detail")]);
    let row = |t: &mut Table, item: &str, status: &str, detail: String| t.push(vec![item.into(), status.into(), detail]);

    let gpus = gpus();
    let mut graphics_ok = !gpus.is_empty();
    let mut names = Vec::new();
    for (v, desc) in &gpus {
        names.push(desc.clone());
        let ok = if *v == Vendor::Nvidia {
            nvidia_installed(ctx).await
        } else {
            let mut all = true;
            for p in vulkan_rpms(*v, d) {
                all &= rpm_installed(ctx, p).await;
            }
            all
        };
        graphics_ok &= ok;
    }
    let driver = if gpus.iter().any(|(v, _)| *v == Vendor::Nvidia) { "NVIDIA drivers" } else { "Vulkan (Mesa)" };
    row(
        &mut t,
        "graphics",
        if graphics_ok { "installed" } else { "missing" },
        format!("{}: {driver}", if names.is_empty() { "no GPU found".into() } else { names.join(", ") }),
    );

    let (steam_ok, how) = if steam_rpm(d) {
        (rpm_installed(ctx, "steam").await, "RPM from the openSUSE non-oss repository")
    } else {
        (flatpak::app_installed(ctx, STEAM_FLATPAK).await, "Flatpak from Flathub, for your user")
    };
    if steam_rpm(d) {
        let non_oss = zypper::non_oss(ctx).await.ok().flatten();
        let (state, detail) = match &non_oss {
            Some((alias, true)) => ("enabled", format!("{alias}: Steam comes from here")),
            Some((alias, false)) => ("missing", format!("{alias} is disabled; Steam comes from here")),
            None => ("missing", "openSUSE's non-oss repository is not set up; Steam comes from here".into()),
        };
        row(&mut t, "non-oss", state, detail);
    }
    row(&mut t, "steam", if steam_ok { "installed" } else { "missing" }, how.into());
    row(
        &mut t,
        "controllers",
        if rpm_installed(ctx, "steam-devices").await { "installed" } else { "missing" },
        "steam-devices: rules for game controllers and VR headsets".into(),
    );
    if selinux_enabled() {
        let ok = rpm_installed(ctx, "selinux-policy-targeted-gaming").await;
        row(
            &mut t,
            "selinux",
            if ok { "installed" } else { "missing" },
            "selinux-policy-targeted-gaming: SELinux changes so games and Proton run".into(),
        );
    } else {
        row(&mut t, "selinux", "not needed", "SELinux is not enabled".into());
    }
    row(
        &mut t,
        "gamemode",
        if rpm_installed(ctx, "gamemode").await { "installed" } else { "optional" },
        "Feral GameMode: performance tweaks while a game runs".into(),
    );
    let h264 = rpm_installed(ctx, "libopenh264-8").await && rpm_installed(ctx, "mozilla-openh264").await;
    row(&mut t, "openh264", if h264 { "installed" } else { "missing" }, "Official H.264 codec for video calls and browsers".into());
    let packman = zypper::query(ctx, &["repos"]).await.map(|x| x.contains("packman")).unwrap_or(false);
    let packman_detail = if d.transactional {
        "not for immutable systems: use Flatpak apps, they bring their own codecs"
    } else {
        "Full codecs from the third-party Packman repository (Full codecs button)"
    };
    row(&mut t, "packman", if packman { "enabled" } else { "optional" }, packman_detail.into());
    let flathub = flatpak::has_user_remote(ctx, flatpak::FLATHUB).await;
    row(&mut t, "flathub", if flathub { "enabled" } else { "missing" }, "Flathub for your user (flatpak --user)".into());
    Ok(Output::Table(t))
}

async fn set_up(ctx: &Context, items: &[&str]) -> Result<Output> {
    let d = &ctx.distro;
    let gpus = gpus();
    let mut rpms: Vec<&str> = Vec::new();
    let mut nvidia = false;
    let mut steam_flatpak = false;
    let mut flathub = false;
    let mut non_oss = false;
    for item in items {
        match *item {
            "graphics" => {
                for (v, _) in &gpus {
                    match v {
                        Vendor::Nvidia => nvidia = true,
                        v => rpms.extend(vulkan_rpms(*v, d)),
                    }
                }
                if gpus.is_empty() {
                    bail!("no AMD, Intel or NVIDIA GPU found");
                }
            }
            "steam" if steam_rpm(d) => {
                non_oss = true;
                rpms.push("steam");
            }
            "steam" => steam_flatpak = true,
            "non-oss" => non_oss = true,
            "controllers" => rpms.push("steam-devices"),
            "selinux" if selinux_enabled() => rpms.push("selinux-policy-targeted-gaming"),
            "selinux" => {}
            "gamemode" => rpms.push("gamemode"),
            "openh264" => {
                rpms.extend(["libopenh264-8", "mozilla-openh264"]);
                if d.rolling {
                    rpms.push("gstreamer-plugin-openh264");
                }
            }
            "flathub" => flathub = true,
            other => bail!("unknown item {other}"),
        }
    }
    let mut done = Vec::new();
    if nvidia {
        helper::run(ctx, &["nvidia-setup", if d.rolling { "tumbleweed" } else { "leap" }]).await?;
        done.push("NVIDIA drivers".to_owned());
    }
    if non_oss {
        // Steam lives in non-oss; enable (or add) it before installing.
        if let Some(what) = zypper::ensure_non_oss(ctx).await? {
            done.push(format!("non-oss ({what})"));
        }
    }
    if !rpms.is_empty() {
        packages::install_rpms(ctx, &rpms).await?;
        done.push(rpms.join(", "));
    }
    if flathub || steam_flatpak {
        flatpak::ensure_flathub(ctx).await?;
        done.push("Flathub for your user".into());
    }
    if steam_flatpak {
        flatpak::install(ctx, flatpak::FLATHUB, STEAM_FLATPAK).await?;
        done.push("Steam (Flatpak)".into());
    }
    let mut msg = format!("Set up {}", done.join("; "));
    if d.transactional || nvidia {
        msg.push_str(". Reboot to finish");
    }
    Ok(Output::Done(msg))
}

/// Terminal equivalent of setting up `items` on distribution `d`.
fn shell(items: &[&str], d: &Distro) -> Vec<String> {
    let mut rpms: Vec<&str> = Vec::new();
    let mut cmds = Vec::new();
    for item in items {
        match *item {
            "graphics" => {
                // Shown for every vendor: the GPU is only known at run time.
                cmds.push(format!("# AMD: {}", vulkan_rpms(Vendor::Amd, d).join(" ")));
                cmds.push(format!("# Intel: {}", vulkan_rpms(Vendor::Intel, d).join(" ")));
                let nv = if d.rolling { "openSUSE-repos-Tumbleweed-NVIDIA" } else { "openSUSE-repos-Leap-NVIDIA" };
                let z = if d.transactional { "sudo transactional-update --continue run zypper" } else { "sudo zypper" };
                cmds.push(format!("# NVIDIA: {z} install {nv} && {z} refresh-services && {z} install-new-recommends"));
            }
            "steam" if steam_rpm(d) => {
                cmds.push("sudo zypper modifyrepo --enable openSUSE:repo-non-oss   # Steam is in non-oss".into());
                rpms.push("steam");
            }
            "non-oss" => cmds.push("sudo zypper modifyrepo --enable openSUSE:repo-non-oss".into()),
            "steam" => {
                cmds.push(format!("flatpak remote-add --user --if-not-exists {} {}", flatpak::FLATHUB, flatpak::FLATHUB_URL));
                cmds.push(format!("flatpak install --user {} {STEAM_FLATPAK}", flatpak::FLATHUB));
            }
            "controllers" => rpms.push("steam-devices"),
            "selinux" => rpms.push("selinux-policy-targeted-gaming"),
            "gamemode" => rpms.push("gamemode"),
            "openh264" => {
                rpms.extend(["libopenh264-8", "mozilla-openh264"]);
                if d.rolling {
                    rpms.push("gstreamer-plugin-openh264");
                }
            }
            "flathub" => cmds.push(format!("flatpak remote-add --user --if-not-exists {} {}", flatpak::FLATHUB, flatpak::FLATHUB_URL)),
            _ => {}
        }
    }
    if !rpms.is_empty() {
        let list = rpms.iter().map(|p| sq(p)).collect::<Vec<_>>().join(" ");
        let cmd =
            if d.transactional { format!("sudo transactional-update --continue pkg install {list}") } else { format!("sudo zypper install {list}") };
        cmds.insert(0, cmd);
    }
    cmds
}
