//! What the local AI assistant needs from this computer: a GPU with a working
//! Vulkan driver, and on laptops with two GPUs a way to run it on the
//! discrete one. AMD and Intel work with Mesa as installed; NVIDIA needs its
//! driver (nouveau has no usable Vulkan on most cards, so llama.cpp falls back
//! to the integrated GPU or the CPU). The launcher starts llama-server through
//! `switcherooctl -g 1`, which only takes effect while switcheroo-control runs.
//!
//! Without a usable discrete GPU the launcher runs the model on the CPU
//! rather than the integrated GPU (CPU mode): slower, but it works on any
//! computer that meets the minimum below. Below the minimum the assistant
//! isn't offered; YAiST says so once and works without it.

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
    description: "Check whether this computer can run the local AI assistant: the minimum memory and processor, whether it runs on the GPU or in the slower CPU mode, the GPUs, the NVIDIA driver, GPU switching (switcheroo-control) on computers with two GPUs, and the llama.cpp runtime.",
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

/// Minimum for CPU mode with the default model (Qwen3-4B, 4-bit, about
/// 2.5 GB plus the context): 8 GB of RAM, which reports as a bit less, and
/// four threads; on x86-64 also AVX2, without which llama.cpp is far slower.
const MIN_MEM_KIB: u64 = 7 * 1024 * 1024;
const MIN_THREADS: usize = 4;

/// Why this computer can't run the assistant well, in plain words; `None`
/// when it meets the minimum.
pub fn below_minimum() -> Option<String> {
    let mem = yaist_core::total_memory_kib();
    let threads = std::thread::available_parallelism().map(|n| n.get()).ok();
    below_minimum_of(mem, threads, avx2())
}

fn below_minimum_of(mem_kib: Option<u64>, threads: Option<usize>, avx2: Option<bool>) -> Option<String> {
    let mut short = Vec::new();
    if let Some(kib) = mem_kib.filter(|&k| k < MIN_MEM_KIB) {
        short.push(format!("{:.1} GB of memory (8 GB needed)", kib as f64 / 1024.0 / 1024.0));
    }
    if let Some(n) = threads.filter(|&n| n < MIN_THREADS) {
        short.push(format!("{n} processor threads ({MIN_THREADS} needed)"));
    }
    if avx2 == Some(false) {
        short.push("a processor without AVX2".into());
    }
    if short.is_empty() {
        None
    } else {
        Some(format!("This computer has {}, too little for the local AI assistant. Everything else in YAiST works as usual.", short.join(" and ")))
    }
}

/// `None` where AVX2 doesn't apply (not x86-64) or can't be read.
fn avx2() -> Option<bool> {
    if !cfg!(target_arch = "x86_64") {
        return None;
    }
    let info = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    let flags = info.lines().find(|l| l.starts_with("flags"))?;
    Some(flags.split_whitespace().any(|f| f == "avx2"))
}

/// A discrete GPU llama.cpp can use: NVIDIA with its driver loaded, or an
/// AMD card with memory of its own (an APU reports only a small carve-out).
/// Without one the launcher runs in CPU mode. Keep in step with
/// `usable_discrete_gpu` in scripts/llm-server.sh.
///
/// Respects `YAIST_LLM_DEVICE`: `YAIST_LLM_DEVICE=cpu` forces CPU mode even
/// when a usable discrete GPU is present.
fn usable_discrete_gpu() -> Option<Vendor> {
    if std::env::var("YAIST_LLM_DEVICE").is_ok_and(|v| v.eq_ignore_ascii_case("cpu")) {
        return None;
    }
    let dir = std::fs::read_dir("/sys/class/drm").ok()?;
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dev = e.path().join("device");
        let vendor = std::fs::read_to_string(dev.join("vendor")).unwrap_or_default();
        let driver = std::fs::read_link(dev.join("driver")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        match (vendor.trim(), driver.as_deref()) {
            ("0x10de", Some("nvidia")) => return Some(Vendor::Nvidia),
            ("0x1002", Some("amdgpu")) => {
                let vram: u64 = std::fs::read_to_string(dev.join("mem_info_vram_total")).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                if vram >= 4 << 30 {
                    return Some(Vendor::Amd);
                }
            }
            _ => {}
        }
    }
    None
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
            "grep -E 'MemTotal' /proc/meminfo; nproc; grep -c avx2 /proc/cpuinfo".into(),
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
    let short = below_minimum();
    t.push(vec![
        "requirements".into(),
        if short.is_some() { "not met".into() } else { "met".into() },
        short.unwrap_or_else(|| "At least 8 GB of memory and 4 processor threads (AVX2 on x86-64)".into()),
    ]);
    let (mode, detail) = match usable_discrete_gpu() {
        Some(_) => ("gpu", "Discrete GPU available for hardware acceleration"),
        None => (
            "cpu",
            "No usable discrete GPU (or YAIST_LLM_DEVICE=cpu): the assistant runs on the processor, more slowly (the first reply can take up to half a minute). On NVIDIA cards, Set up installs the driver that makes the GPU usable",
        ),
    };
    t.push(vec!["mode".into(), mode.into(), detail.into()]);
    let names: Vec<&str> = r.gpus.iter().map(|(_, n)| n.as_str()).collect();
    t.push(vec![
        "gpu".into(),
        if r.gpus.is_empty() { "none".into() } else { "found".into() },
        if r.gpus.is_empty() { "No GPU found".into() } else { names.join(", ") },
    ]);
    if let Some(d) = r.nvidia {
        let (state, detail) = match d {
            Driver::Loaded => ("loaded", "The NVIDIA driver is running"),
            Driver::NeedsReboot => ("reboot", "Installed; reboot to replace nouveau with it"),
            Driver::Missing => ("missing", "Without it the assistant can't use the NVIDIA card and runs on the processor"),
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

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024;

    #[test]
    fn usable_gpu_respects_env_override() {
        std::env::set_var("YAIST_LLM_DEVICE", "cpu");
        assert_eq!(usable_discrete_gpu(), None);
        std::env::remove_var("YAIST_LLM_DEVICE");
    }

    #[test]
    fn minimum() {
        // Lenovo Legion Y530: i7-8750H (12 threads, AVX2), 16 GB.
        assert_eq!(below_minimum_of(Some(15 * GIB + GIB / 2), Some(12), Some(true)), None);
        // An 8 GB machine reports a bit less.
        assert_eq!(below_minimum_of(Some(7 * GIB + GIB / 2), Some(4), Some(true)), None);
        assert_eq!(below_minimum_of(None, None, None), None);
        let small = below_minimum_of(Some(3 * GIB + GIB / 2), Some(2), Some(false)).unwrap();
        assert!(small.contains("3.5 GB of memory") && small.contains("2 processor threads") && small.contains("AVX2"), "{small}");
        assert!(small.ends_with("Everything else in YAiST works as usual."));
    }
}
