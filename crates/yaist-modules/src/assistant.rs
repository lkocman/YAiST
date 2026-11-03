//! What the local AI assistant needs from this computer: a GPU with a working
//! Vulkan driver, and on laptops with two GPUs a way to run it on the
//! discrete one. AMD and Intel work with Mesa as installed; NVIDIA needs its
//! driver (nouveau has no usable Vulkan on most cards, so llama.cpp falls back
//! to the integrated GPU or the CPU). The launcher starts llama-server through
//! `switcherooctl -g 1`, which only takes effect while switcheroo-control runs.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use yaist_core::{ActionSpec, Context, ModuleSpec, Output, Registry, Risk, Table};
use zbus::OwnedVariant;

use crate::media::{self, Vendor};
use crate::{helper, packages, services};

const RUNTIME: &[&str] = &["llamacpp", "libggml-vulkan"];
const SWITCHEROO: &str = "switcheroo-control";
const SWITCHEROO_UNIT: &str = "switcheroo-control.service";
const SWITCHEROO_BUS: &str = "net.hadess.SwitcherooControl";

const STATUS: ActionSpec = ActionSpec {
    id: "assistant.status",
    title: "Assistant readiness",
    description: "Check whether this computer can run the local AI assistant: the GPUs, the NVIDIA driver, GPU switching (switcheroo-control) on computers with two GPUs, and the llama.cpp runtime.",
    risk: Risk::Read,
    params: &[],
};

const SET_UP: ActionSpec = ActionSpec {
    id: "assistant.set_up",
    title: "Set up",
    description: "Prepare this computer for the local AI assistant: the NVIDIA driver on NVIDIA cards (needs a reboot), switcheroo-control on computers with two GPUs so the assistant runs on the discrete GPU rather than the integrated one, and llama.cpp with its Vulkan backend.",
    risk: Risk::Change,
    params: &[],
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Driver {
    Loaded,
    /// Installed but not running yet: nouveau still holds the card.
    NeedsReboot,
    Missing,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Switching {
    /// Running; the GPU `switcherooctl -g 1` picks.
    Running(Option<String>),
    Stopped,
    Missing,
}

struct Readiness {
    gpus: Vec<(Vendor, String)>,
    /// `None` without an NVIDIA card.
    nvidia: Option<Driver>,
    /// `None` with a single GPU: nothing to choose between.
    switching: Option<Switching>,
    runtime: bool,
}

pub fn register(reg: &mut Registry) {
    reg.module(ModuleSpec {
        id: "assistant",
        title: "AI assistant",
        tagline: "GPU and drivers for the local model",
        overview: STATUS.id,
        row_actions: &[],
        overview_needs_admin: false,
    });
    reg.action(STATUS, |ctx, _| async move { status(&ctx).await });
    reg.action(SET_UP, |ctx, _| async move { set_up(&ctx).await });

    reg.shell(STATUS.id, |_, _| {
        vec![
            "lspci | grep -iE 'vga|3d'".into(),
            "lsmod | grep -E '^(nvidia|nouveau)'".into(),
            "switcherooctl list".into(),
            format!("rpm -q {} {SWITCHEROO}", RUNTIME.join(" ")),
        ]
    });
    reg.shell(SET_UP.id, |_, d| {
        let nv = if d.rolling { "openSUSE-repos-Tumbleweed-NVIDIA" } else { "openSUSE-repos-Leap-NVIDIA" };
        let z = if d.transactional { "sudo transactional-update --continue run zypper" } else { "sudo zypper" };
        vec![
            format!("# NVIDIA: {z} install {nv} && {z} refresh-services && {z} install-new-recommends"),
            format!("{z} install {} {SWITCHEROO}", RUNTIME.join(" ")),
            format!("sudo systemctl enable --now {SWITCHEROO_UNIT}   # two GPUs only"),
        ]
    });
}

async fn readiness(ctx: &Context) -> Readiness {
    let gpus = media::gpus();
    let nvidia = if !gpus.iter().any(|(v, _)| *v == Vendor::Nvidia) {
        None
    } else if Path::new("/sys/module/nvidia").exists() {
        Some(Driver::Loaded)
    } else if media::nvidia_installed(ctx).await {
        Some(Driver::NeedsReboot)
    } else {
        Some(Driver::Missing)
    };
    let switching = if gpus.len() < 2 {
        None
    } else if switcheroo_running(ctx).await {
        Some(Switching::Running(discrete_gpu(ctx).await))
    } else if media::rpm_installed(ctx, SWITCHEROO).await {
        Some(Switching::Stopped)
    } else {
        Some(Switching::Missing)
    };
    let mut runtime = Path::new("/usr/bin/llama-server").exists();
    for p in RUNTIME {
        runtime &= media::rpm_installed(ctx, p).await;
    }
    Readiness { gpus, nvidia, switching, runtime }
}

/// switcheroo-control isn't started on demand over D-Bus, only at boot by
/// graphical.target: ask whether it is on the bus now.
async fn switcheroo_running(ctx: &Context) -> bool {
    ctx.call::<_, bool>("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameHasOwner", &(SWITCHEROO_BUS,), false)
        .await
        .unwrap_or(false)
}

/// The GPU `switcherooctl -g 1` launches on: the first that isn't the
/// default, as switcherooctl puts the default one first.
async fn discrete_gpu(ctx: &Context) -> Option<String> {
    let gpus: Vec<HashMap<String, OwnedVariant>> =
        ctx.property(SWITCHEROO_BUS, "/net/hadess/SwitcherooControl", SWITCHEROO_BUS, "GPUs").await.ok()?;
    let is_default = |g: &HashMap<String, OwnedVariant>| g.get("Default").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
    let gpu = gpus.iter().find(|g| !is_default(g))?;
    gpu.get("Name").and_then(|v| v.try_clone().ok()).and_then(|v| String::try_from(v).ok())
}

async fn status(ctx: &Context) -> Result<Output> {
    let r = readiness(ctx).await;
    let mut t = Table::new(&[("item", "Item"), ("status", "Status"), ("detail", "Detail")]);
    let names: Vec<&str> = r.gpus.iter().map(|(_, n)| n.as_str()).collect();
    t.push(vec![
        "gpu".into(),
        if r.gpus.is_empty() { "none".into() } else { "found".into() },
        if r.gpus.is_empty() { "No GPU found: the assistant would run on the CPU, slowly".into() } else { names.join(", ") },
    ]);
    if let Some(d) = r.nvidia {
        let (state, detail) = match d {
            Driver::Loaded => ("loaded", "The NVIDIA driver is running"),
            Driver::NeedsReboot => ("reboot", "Installed; reboot to replace nouveau with it"),
            Driver::Missing => ("missing", "Without it the assistant can't use the NVIDIA card and falls back to the integrated GPU or the CPU"),
        };
        t.push(vec!["nvidia driver".into(), state.into(), detail.into()]);
    }
    if let Some(s) = &r.switching {
        let (state, detail) = match s {
            Switching::Running(Some(name)) => ("running", format!("The assistant runs on {name}")),
            Switching::Running(None) => ("running", "switcheroo-control reports a single GPU".into()),
            Switching::Stopped => ("stopped", "switcheroo-control is installed but not running: the assistant starts on the integrated GPU".into()),
            Switching::Missing => ("missing", "Two GPUs; switcheroo-control lets the assistant start on the discrete one".into()),
        };
        t.push(vec!["gpu switching".into(), state.into(), detail]);
    }
    t.push(vec![
        "runtime".into(),
        if r.runtime { "installed".into() } else { "missing".into() },
        "llama.cpp with the Vulkan backend (llamacpp, libggml-vulkan)".into(),
    ]);
    Ok(Output::Table(t))
}

async fn set_up(ctx: &Context) -> Result<Output> {
    let d = &ctx.distro;
    let r = readiness(ctx).await;
    let mut done = Vec::new();
    let mut reboot = r.nvidia == Some(Driver::NeedsReboot);
    if r.nvidia == Some(Driver::Missing) {
        helper::run(ctx, &["nvidia-setup", if d.rolling { "tumbleweed" } else { "leap" }]).await?;
        done.push("NVIDIA drivers".to_owned());
        reboot = true;
    }
    let mut rpms: Vec<&str> = Vec::new();
    if !r.runtime {
        rpms.extend(RUNTIME);
    }
    if r.switching == Some(Switching::Missing) {
        rpms.push(SWITCHEROO);
    }
    if !rpms.is_empty() {
        packages::install_rpms(ctx, &rpms).await?;
        done.push(rpms.join(", "));
    }
    if matches!(r.switching, Some(Switching::Stopped | Switching::Missing)) {
        if d.transactional && r.switching == Some(Switching::Missing) {
            // In the new snapshot; the preset enables it there.
            reboot = true;
        } else {
            services::enable(ctx, SWITCHEROO_UNIT, true).await?;
            done.push(format!("started {SWITCHEROO}"));
        }
    }
    if done.is_empty() && !reboot {
        return Ok(Output::Done("Nothing to do: this computer is ready for the assistant".into()));
    }
    let mut msg = if done.is_empty() { "Nothing new to install".to_owned() } else { format!("Set up {}", done.join("; ")) };
    if reboot || d.transactional {
        msg.push_str(". Reboot to finish, then start the assistant");
    }
    Ok(Output::Done(msg))
}

/// Preflight check for whether the local AI assistant can safely run.
/// Returns `None` if ready, or `Some(reason)` if disabled.
pub fn preflight() -> Option<String> {
    if let Some(kib) = yaist_core::total_memory_kib() {
        if kib < 3_800_000 {
            let gib = kib as f64 / 1024.0 / 1024.0;
            return Some(format!("requires ≥4 GiB RAM; found {gib:.1} GiB"));
        }
    }
    let gpus = media::gpus();
    let has_nvidia = gpus.iter().any(|(v, _)| *v == Vendor::Nvidia);
    if gpus.len() >= 2 && has_nvidia && !Path::new("/sys/module/nvidia").exists() {
        return Some("dual GPU: NVIDIA driver not active".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_low_memory() {
        std::env::set_var("YAIST_TOTAL_MEM_KIB", (2 * 1024 * 1024).to_string());
        let res = preflight();
        std::env::remove_var("YAIST_TOTAL_MEM_KIB");
        assert_eq!(res.as_deref(), Some("requires ≥4 GiB RAM; found 2.0 GiB"));
    }
}
