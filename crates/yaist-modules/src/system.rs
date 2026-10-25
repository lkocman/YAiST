//! Hostname, time zone, time sync and an overview of the machine, via
//! systemd's hostnamed and timedated.

use yaist_core::shell_quote as sq;
use yaist_core::{validate, ActionSpec, Context, ModuleSpec, Output, Param, Registry, Risk};

const HOSTNAME1: &str = "org.freedesktop.hostname1";
const HOSTNAME1_PATH: &str = "/org/freedesktop/hostname1";
const TIMEDATE1: &str = "org.freedesktop.timedate1";
const TIMEDATE1_PATH: &str = "/org/freedesktop/timedate1";

const OVERVIEW: ActionSpec = ActionSpec {
    id: "system.overview",
    title: "Overview",
    description: "Show hostname, operating system, kernel, time zone, uptime, memory and security module.",
    risk: Risk::Read,
    params: &[],
};

const SET_HOSTNAME: ActionSpec = ActionSpec {
    id: "system.set_hostname",
    title: "Set hostname",
    description: "Set the machine's static hostname.",
    risk: Risk::Change,
    params: &[Param::text("hostname", "New hostname, e.g. workstation").check(validate::hostname)],
};

const SET_TIMEZONE: ActionSpec = ActionSpec {
    id: "system.set_timezone",
    title: "Set time zone",
    description: "Set the system time zone (an IANA zone such as Europe/Prague or America/New_York).",
    risk: Risk::Change,
    params: &[Param::text("timezone", "IANA time zone, e.g. Europe/Prague").check(validate::timezone)],
};

const SET_NTP: ActionSpec = ActionSpec {
    id: "system.set_ntp",
    title: "Network time",
    description: "Turn automatic time synchronisation (NTP) on or off.",
    risk: Risk::Change,
    params: &[Param::boolean("enabled", "Synchronise the clock over the network").default("true")],
};

const REBOOT: ActionSpec = ActionSpec {
    id: "system.reboot",
    title: "Reboot",
    description: "Restart the computer now, e.g. to finish updates installed into a new snapshot. Running programs are closed.",
    risk: Risk::Dangerous,
    params: &[],
};

pub fn register(reg: &mut Registry) {
    reg.module(ModuleSpec {
        id: "system",
        title: "System",
        tagline: "Hostname, time and overview",
        overview: OVERVIEW.id,
        row_actions: &[],
        overview_needs_admin: false,
    });

    reg.action(OVERVIEW, |ctx, _| async move { overview(&ctx).await });

    reg.action(SET_HOSTNAME, |ctx, args| async move {
        let name = args.str("hostname").to_owned();
        ctx.call::<_, ()>(HOSTNAME1, HOSTNAME1_PATH, HOSTNAME1, "SetStaticHostname", &(name.as_str(), true), true).await?;
        Ok(Output::Done(format!("Hostname set to {name}")))
    });

    reg.action(SET_TIMEZONE, |ctx, args| async move {
        let tz = args.str("timezone").to_owned();
        ctx.call::<_, ()>(TIMEDATE1, TIMEDATE1_PATH, TIMEDATE1, "SetTimezone", &(tz.as_str(), true), true).await?;
        Ok(Output::Done(format!("Time zone set to {tz}")))
    });

    reg.action(REBOOT, |ctx, _| async move {
        ctx.call::<_, ()>("org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", "Reboot", &(true,), true).await?;
        Ok(Output::Done("Rebooting".into()))
    });
    reg.shell(REBOOT.id, |_, _| vec!["sudo systemctl reboot".into()]);

    reg.action(SET_NTP, |ctx, args| async move {
        let on = args.bool("enabled");
        ctx.call::<_, ()>(TIMEDATE1, TIMEDATE1_PATH, TIMEDATE1, "SetNTP", &(on, true), true).await?;
        Ok(Output::Done(format!("Network time synchronisation turned {}", if on { "on" } else { "off" })))
    });

    reg.shell(OVERVIEW.id, |_, _| vec!["hostnamectl".into(), "timedatectl".into()]);
    reg.shell(SET_HOSTNAME.id, |a, _| vec![format!("sudo hostnamectl set-hostname {}", sq(a.str("hostname")))]);
    reg.shell(SET_TIMEZONE.id, |a, _| vec![format!("sudo timedatectl set-timezone {}", sq(a.str("timezone")))]);
    reg.shell(SET_NTP.id, |a, _| vec![format!("sudo timedatectl set-ntp {}", a.bool("enabled"))]);
}

async fn overview(ctx: &Context) -> anyhow::Result<Output> {
    let host: String =
        ctx.property(HOSTNAME1, HOSTNAME1_PATH, HOSTNAME1, "Hostname").await.unwrap_or_else(|_| read_trim("/proc/sys/kernel/hostname"));
    let kernel: String =
        ctx.property(HOSTNAME1, HOSTNAME1_PATH, HOSTNAME1, "KernelRelease").await.unwrap_or_else(|_| read_trim("/proc/sys/kernel/osrelease"));
    let tz: String = ctx.property(TIMEDATE1, TIMEDATE1_PATH, TIMEDATE1, "Timezone").await.unwrap_or_default();
    let ntp: bool = ctx.property(TIMEDATE1, TIMEDATE1_PATH, TIMEDATE1, "NTP").await.unwrap_or(false);
    let synced: bool = ctx.property(TIMEDATE1, TIMEDATE1_PATH, TIMEDATE1, "NTPSynchronized").await.unwrap_or(false);

    let d = &ctx.distro;
    let updates = if d.transactional {
        "transactional-update (reboot to apply)"
    } else if d.rolling {
        "rolling: zypper dup"
    } else {
        "zypper update / patch"
    };
    Ok(Output::Facts(vec![
        ("Hostname".into(), host),
        ("Operating system".into(), if d.pretty.is_empty() { d.id.clone() } else { d.pretty.clone() }),
        ("Kernel".into(), kernel),
        ("Mode".into(), if d.transactional { "immutable".into() } else { "standard".into() }),
        ("Support".into(), support_fact(d)),
        ("Update model".into(), updates.into()),
        ("Time zone".into(), tz),
        ("Network time".into(), format!("{}{}", if ntp { "on" } else { "off" }, if synced { ", synchronised" } else { "" })),
        ("Uptime".into(), uptime()),
        ("Memory".into(), memory()),
        ("Security module".into(), crate::selinux::lsm_summary()),
    ]))
}

fn support_fact(d: &yaist_core::Distro) -> String {
    use yaist_core::date;
    match d.end_of_life.as_deref().and_then(|e| date::parse(e).map(|days| (e, days))) {
        _ if d.rolling => "rolling release, no end of life".into(),
        Some((e, days)) if days < date::today() => format!("END OF LIFE since {e}"),
        Some((e, days)) => format!("until {e} ({} days left)", days - date::today()),
        None => "unknown".into(),
    }
}

fn read_trim(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default().trim().to_owned()
}

fn uptime() -> String {
    let secs = read_trim("/proc/uptime").split('.').next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let (d, h, m) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
    if d > 0 {
        format!("{d} d {h} h {m} min")
    } else {
        format!("{h} h {m} min")
    }
}

fn memory() -> String {
    let info = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb = |key: &str| {
        info.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0)
    };
    let gib = |k: f64| k / 1024.0 / 1024.0;
    let total = kb("MemTotal:");
    let avail = kb("MemAvailable:");
    format!("{:.1} GiB used of {:.1} GiB", gib(total - avail), gib(total))
}
