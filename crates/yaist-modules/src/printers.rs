//! Printers, like YaST's printer module: CUPS queues, finding printers,
//! driverless (IPP Everywhere) setup, and HP devices through HPLIP.

use anyhow::{bail, Result};
use yaist_core::shell_quote as sq;
use yaist_core::{validate, ActionSpec, Context, ModuleSpec, Output, Param, Registry, Risk, Table};

use crate::packages;

const PRINTER: Param = Param::text("printer", "Printer name, as listed").check(validate::name);
const HPLIP: &[&str] = &["hplip", "hplip-sane"];

const LIST: ActionSpec = ActionSpec {
    id: "printers.list",
    title: "Printers",
    description: "List configured printers with their state, whether they are the default, their device and queued jobs.",
    risk: Risk::Read,
    params: &[],
};

const FIND: ActionSpec = ActionSpec {
    id: "printers.find",
    title: "Find printers",
    description: "Look for printers on the network (IPP/driverless) and connected by USB, and say which are HP devices that work best with HPLIP.",
    risk: Risk::Read,
    params: &[],
};

const JOBS: ActionSpec =
    ActionSpec { id: "printers.jobs", title: "Print jobs", description: "List queued print jobs.", risk: Risk::Read, params: &[] };

const INSTALL_HPLIP: ActionSpec = ActionSpec {
    id: "printers.install_hplip",
    title: "Install HP drivers",
    description: "Install HPLIP, HP's printing and scanning drivers and tools (hplip, hplip-sane), for HP printers and all-in-ones.",
    risk: Risk::Change,
    params: &[],
};

const ADD_HP: ActionSpec = ActionSpec {
    id: "printers.add_hp",
    title: "Add HP printer",
    description:
        "Set up an HP printer or all-in-one with HPLIP (hp-setup), by IP address, hostname or USB serial number. Installs HPLIP first if needed.",
    risk: Risk::Change,
    params: &[
        Param::text("address", "IP address, hostname or USB serial number").check(validate::hp_address),
        Param::text("name", "Name for the printer; HPLIP picks one if empty").optional().check(validate::name),
    ],
};

const ADD: ActionSpec = ActionSpec {
    id: "printers.add",
    title: "Add printer",
    description: "Add a printer by its device URI with the driverless IPP Everywhere driver (works for most network and IPP-over-USB printers made since 2010).",
    risk: Risk::Change,
    params: &[
        Param::text("name", "Name for the printer, e.g. Office").check(validate::name),
        Param::text("uri", "Device URI, e.g. ipp://printer.local/ipp/print (see Find printers)").check(validate::printer_uri),
    ],
};

const TEST_PAGE: ActionSpec =
    ActionSpec { id: "printers.test_page", title: "Test page", description: "Print the CUPS test page.", risk: Risk::Change, params: &[PRINTER] };
const SET_DEFAULT: ActionSpec = ActionSpec {
    id: "printers.set_default",
    title: "Make default",
    description: "Make a printer the system's default.",
    risk: Risk::Change,
    params: &[PRINTER],
};
const ENABLE: ActionSpec =
    ActionSpec { id: "printers.enable", title: "Enable", description: "Enable a printer and accept jobs.", risk: Risk::Change, params: &[PRINTER] };
const DISABLE: ActionSpec = ActionSpec {
    id: "printers.disable",
    title: "Disable",
    description: "Pause a printer and stop accepting jobs.",
    risk: Risk::Change,
    params: &[PRINTER],
};
const CANCEL_JOBS: ActionSpec = ActionSpec {
    id: "printers.cancel_jobs",
    title: "Cancel jobs",
    description: "Cancel all of your queued jobs on a printer.",
    risk: Risk::Change,
    params: &[PRINTER],
};
const REMOVE: ActionSpec =
    ActionSpec { id: "printers.remove", title: "Remove", description: "Delete a printer queue.", risk: Risk::Dangerous, params: &[PRINTER] };
const SHARE: ActionSpec = ActionSpec {
    id: "printers.share",
    title: "Share printers",
    description:
        "Share (or stop sharing) this computer's printers on the local network. Other computers also need the ipp service allowed in the firewall.",
    risk: Risk::Change,
    params: &[Param::boolean("enabled", "Share printers").default("true")],
};

pub fn register(reg: &mut Registry) {
    reg.module(ModuleSpec {
        id: "printers",
        title: "Printers",
        tagline: "CUPS printers and HP devices",
        overview: LIST.id,
        row_actions: &[TEST_PAGE.id, SET_DEFAULT.id, ENABLE.id, DISABLE.id, CANCEL_JOBS.id, REMOVE.id],
        overview_needs_admin: false,
    });

    reg.action(LIST, |ctx, _| async move { list(&ctx).await });
    reg.action(FIND, |ctx, _| async move { find(&ctx).await });
    reg.action(JOBS, |ctx, _| async move { jobs(&ctx).await });

    reg.action(INSTALL_HPLIP, |ctx, _| async move {
        packages::install_rpms(&ctx, HPLIP).await?;
        Ok(Output::Done("Installed HPLIP (hplip, hplip-sane)".into()))
    });
    reg.action(ADD_HP, |ctx, args| async move {
        if !hplip_installed(&ctx).await {
            packages::install_rpms(&ctx, HPLIP).await?;
            if ctx.distro.transactional {
                bail!("HPLIP was installed into a new snapshot; reboot, then add the printer");
            }
        }
        let mut argv = vec!["/usr/bin/hp-setup", "-i", "-a", "-x"];
        let name = args.opt_str("name").map(|n| format!("--printer={n}"));
        if let Some(n) = &name {
            argv.push(n);
        }
        argv.push(args.str("address"));
        ctx.run_privileged(&argv).await?;
        Ok(Output::Done(format!("Added the HP printer at {}", args.str("address"))))
    });
    reg.action(ADD, |ctx, args| async move {
        let (name, uri) = (args.str("name"), args.str("uri"));
        ctx.run_privileged(&["/usr/sbin/lpadmin", "-p", name, "-E", "-v", uri, "-m", "everywhere"]).await?;
        Ok(Output::Done(format!("Added printer {name}")))
    });
    reg.action(TEST_PAGE, |ctx, args| async move {
        let p = args.str("printer");
        ctx.run(&["/usr/bin/lp", "-d", p, "-t", "YAiST test page", "/usr/share/cups/data/testprint"]).await?;
        Ok(Output::Done(format!("Sent a test page to {p}")))
    });
    reg.action(SET_DEFAULT, |ctx, args| async move {
        ctx.run_privileged(&["/usr/sbin/lpadmin", "-d", args.str("printer")]).await?;
        Ok(Output::Done(format!("{} is now the default printer", args.str("printer"))))
    });
    reg.action(ENABLE, |ctx, args| async move {
        ctx.run_privileged(&["/usr/sbin/cupsenable", args.str("printer")]).await?;
        ctx.run_privileged(&["/usr/sbin/cupsaccept", args.str("printer")]).await?;
        Ok(Output::Done(format!("Enabled {}", args.str("printer"))))
    });
    reg.action(DISABLE, |ctx, args| async move {
        ctx.run_privileged(&["/usr/sbin/cupsdisable", args.str("printer")]).await?;
        ctx.run_privileged(&["/usr/sbin/cupsreject", args.str("printer")]).await?;
        Ok(Output::Done(format!("Disabled {}", args.str("printer"))))
    });
    reg.action(CANCEL_JOBS, |ctx, args| async move {
        ctx.run(&["/usr/bin/cancel", "-a", args.str("printer")]).await?;
        Ok(Output::Done(format!("Cancelled your jobs on {}", args.str("printer"))))
    });
    reg.action(REMOVE, |ctx, args| async move {
        ctx.run_privileged(&["/usr/sbin/lpadmin", "-x", args.str("printer")]).await?;
        Ok(Output::Done(format!("Removed {}", args.str("printer"))))
    });
    reg.action(SHARE, |ctx, args| async move {
        let on = args.bool("enabled");
        ctx.run_privileged(&["/usr/sbin/cupsctl", if on { "--share-printers" } else { "--no-share-printers" }]).await?;
        Ok(Output::Done(
            if on { "Printers are shared on the network; allow the ipp service in the firewall" } else { "Printers are no longer shared" }.into(),
        ))
    });

    reg.shell(LIST.id, |_, _| vec!["lpstat -p -d -v".into()]);
    reg.shell(FIND.id, |_, _| vec!["ippfind --timeout 5".into(), "sudo lpinfo -v".into()]);
    reg.shell(JOBS.id, |_, _| vec!["lpstat -o".into()]);
    reg.shell(INSTALL_HPLIP.id, |_, d| install_cmd(d));
    reg.shell(ADD_HP.id, |a, d| {
        let mut cmds = install_cmd(d);
        let name = a.opt_str("name").map(|n| format!(" --printer={}", sq(n))).unwrap_or_default();
        cmds.push(format!("sudo hp-setup -i -a -x{name} {}", sq(a.str("address"))));
        cmds
    });
    reg.shell(ADD.id, |a, _| vec![format!("sudo lpadmin -p {} -E -v {} -m everywhere", sq(a.str("name")), sq(a.str("uri")))]);
    reg.shell(TEST_PAGE.id, |a, _| vec![format!("lp -d {} /usr/share/cups/data/testprint", sq(a.str("printer")))]);
    reg.shell(SET_DEFAULT.id, |a, _| vec![format!("sudo lpadmin -d {}", sq(a.str("printer")))]);
    reg.shell(ENABLE.id, |a, _| vec![format!("sudo cupsenable {p} && sudo cupsaccept {p}", p = sq(a.str("printer")))]);
    reg.shell(DISABLE.id, |a, _| vec![format!("sudo cupsdisable {p} && sudo cupsreject {p}", p = sq(a.str("printer")))]);
    reg.shell(CANCEL_JOBS.id, |a, _| vec![format!("cancel -a {}", sq(a.str("printer")))]);
    reg.shell(REMOVE.id, |a, _| vec![format!("sudo lpadmin -x {}", sq(a.str("printer")))]);
    reg.shell(SHARE.id, |a, _| vec![format!("sudo cupsctl {}", if a.bool("enabled") { "--share-printers" } else { "--no-share-printers" })]);
}

fn install_cmd(d: &yaist_core::Distro) -> Vec<String> {
    vec![if d.transactional {
        "sudo transactional-update --continue pkg install hplip hplip-sane".into()
    } else {
        "sudo zypper install hplip hplip-sane".into()
    }]
}

async fn hplip_installed(ctx: &Context) -> bool {
    ctx.exit_code(&["/usr/bin/rpm", "-q", "--quiet", "hplip"]).await.ok() == Some(0)
}

async fn list(ctx: &Context) -> Result<Output> {
    let out = ctx.run_ok(&["/usr/bin/lpstat", "-p", "-d", "-v"], &[0, 1]).await.unwrap_or_default();
    let jobs = ctx.run_ok(&["/usr/bin/lpstat", "-o"], &[0, 1]).await.unwrap_or_default();
    let default = out.lines().find_map(|l| l.strip_prefix("system default destination: ")).unwrap_or("").trim().to_owned();
    let mut t = Table::new(&[("printer", "Printer"), ("status", "State"), ("default", "Default"), ("device", "Device"), ("jobs", "Jobs")]);
    for line in out.lines() {
        let Some(rest) = line.strip_prefix("printer ") else { continue };
        let name = rest.split_whitespace().next().unwrap_or("").to_owned();
        let state = if rest.contains("disabled") {
            "disabled"
        } else if rest.contains("now printing") {
            "printing"
        } else {
            "idle"
        };
        let device = out.lines().find_map(|l| l.strip_prefix(&format!("device for {name}: "))).unwrap_or("").trim().to_owned();
        let queued = jobs.lines().filter(|j| j.starts_with(&format!("{name}-"))).count();
        let is_default = if name == default { "yes" } else { "no" };
        t.push(vec![name, state.into(), is_default.into(), device, queued.to_string()]);
    }
    Ok(Output::Table(t))
}

async fn find(ctx: &Context) -> Result<Output> {
    let mut t = Table::new(&[("uri", "Device URI"), ("kind", "Connection"), ("info", "Printer"), ("hint", "How to add")]);
    // Network printers announcing IPP (driverless).
    let net = ctx.run_ok(&["/usr/bin/ippfind", "--timeout", "5", "--exec", "echo", "{}\t{txt_ty}", ";"], &[0, 1]).await.unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    for line in net.lines() {
        let (uri, model) = line.split_once('\t').unwrap_or((line, ""));
        if uri.is_empty() || !seen.insert(uri.to_owned()) {
            continue;
        }
        let hp = model.to_ascii_uppercase().contains("HP ") || model.starts_with("HP") || model.contains("Hewlett");
        let host = uri.split("://").nth(1).and_then(|h| h.split(['/', ':']).next()).unwrap_or("");
        let hint = if hp { format!("Add HP printer with address {host}, or Add printer (driverless)") } else { "Add printer (driverless)".into() };
        t.push(vec![uri.to_owned(), "network".into(), model.to_owned(), hint]);
    }
    // USB devices with a printer interface (class 07).
    if let Ok(dir) = std::fs::read_dir("/sys/bus/usb/devices") {
        for e in dir.flatten() {
            let p = e.path();
            let read = |f: &str| std::fs::read_to_string(p.join(f)).unwrap_or_default().trim().to_owned();
            if read("bInterfaceClass") != "07" {
                continue;
            }
            let dev = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
            let dread = |f: &str| std::fs::read_to_string(dev.join(f)).unwrap_or_default().trim().to_owned();
            let (vendor, product, serial) = (dread("idVendor"), dread("product"), dread("serial"));
            if !seen.insert(format!("usb:{vendor}:{serial}")) {
                continue;
            }
            let hp = vendor == "03f0";
            let hint =
                if hp { format!("Add HP printer with address {serial}") } else { "Plugged in printers are usually set up automatically".into() };
            t.push(vec![format!("usb ({vendor})"), "USB".into(), format!("{} {product}", if hp { "HP" } else { "" }).trim().to_owned(), hint]);
        }
    }
    if t.rows.is_empty() {
        return Ok(Output::Text("No printers found on the network or USB. Check that the printer is on and on the same network.".into()));
    }
    Ok(Output::Table(t))
}

async fn jobs(ctx: &Context) -> Result<Output> {
    let out = ctx.run_ok(&["/usr/bin/lpstat", "-o"], &[0, 1]).await.unwrap_or_default();
    let mut t = Table::new(&[("job", "Job"), ("user", "User"), ("size", "Size"), ("date", "Queued")]);
    for line in out.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 4 {
            t.push(vec![f[0].into(), f[1].into(), f[2].into(), f[3..].join(" ")]);
        }
    }
    Ok(Output::Table(t))
}
