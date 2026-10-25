//! firewalld over D-Bus. openSUSE's polkit policy wants admin authentication
//! even to read the configuration, so reads are interactive too (cached by
//! polkit for a few minutes); yaistd will do them as root. Every change is
//! applied to both the running
//! configuration and the permanent one, so it takes effect now and survives a
//! reboot.

use std::collections::HashMap;

use anyhow::Result;
use yaist_core::shell_quote as sq;
use yaist_core::{validate, ActionSpec, Context, ModuleSpec, Output, Param, Registry, Risk, Table};
use zbus::zvariant::OwnedObjectPath;

const FIREWALLD: &str = "org.fedoraproject.FirewallD1";
const PATH: &str = "/org/fedoraproject/FirewallD1";
const CONFIG_PATH: &str = "/org/fedoraproject/FirewallD1/config";
const ZONE: &str = "org.fedoraproject.FirewallD1.zone";
const CONFIG: &str = "org.fedoraproject.FirewallD1.config";
const CONFIG_ZONE: &str = "org.fedoraproject.FirewallD1.config.zone";

const ZONE_PARAM: Param = Param::text("zone", "Firewall zone; leave empty for the default zone").optional().check(validate::name);

const OVERVIEW: ActionSpec = ActionSpec {
    id: "firewall.overview",
    title: "Rules",
    description: "List the open ports and allowed services in each active firewall zone.",
    risk: Risk::Read,
    params: &[],
};

const AVAILABLE: ActionSpec = ActionSpec {
    id: "firewall.available_services",
    title: "Known services",
    description: "List the service names firewalld knows (e.g. ssh, http, https, samba). Use it to find the right name for allow_service.",
    risk: Risk::Read,
    params: &[Param::text("filter", "Only names containing this text").optional().check(validate::search_term)],
};

const OPEN_PORT: ActionSpec = ActionSpec {
    id: "firewall.open_port",
    title: "Open port",
    description: "Open a port (or port range) in a firewall zone. Prefer allow_service when a known service fits.",
    risk: Risk::Change,
    params: &[
        Param::text("port", "Port or range, e.g. 8080 or 60000-61000").check(validate::port),
        Param::choice("protocol", &["tcp", "udp"], "Protocol").default("tcp"),
        ZONE_PARAM,
    ],
};

const ALLOW_SERVICE: ActionSpec = ActionSpec {
    id: "firewall.allow_service",
    title: "Allow service",
    description: "Allow a known service (e.g. ssh, http, https) through the firewall.",
    risk: Risk::Change,
    params: &[Param::text("service", "firewalld service name, e.g. https").check(validate::name), ZONE_PARAM],
};

const REMOVE: ActionSpec = ActionSpec {
    id: "firewall.remove",
    title: "Remove",
    description: "Close a port or stop allowing a service. rule is either port/protocol (8080/tcp) or a service name (http).",
    risk: Risk::Change,
    params: &[Param::text("rule", "8080/tcp or a service name").check(validate::firewall_rule), ZONE_PARAM],
};

pub fn register(reg: &mut Registry) {
    reg.module(ModuleSpec {
        id: "firewall",
        title: "Firewall",
        tagline: "Ports, services and zones",
        overview: OVERVIEW.id,
        row_actions: &[REMOVE.id],
        overview_needs_admin: true,
    });

    reg.action(OVERVIEW, |ctx, _| async move { overview(&ctx).await });

    reg.action(AVAILABLE, |ctx, args| async move {
        let mut names: Vec<String> = ctx.call(FIREWALLD, PATH, FIREWALLD, "listServices", &(), true).await?;
        if let Some(f) = args.opt_str("filter") {
            names.retain(|n| n.contains(f));
        }
        names.sort();
        Ok(Output::Text(names.join(", ")))
    });

    reg.action(OPEN_PORT, |ctx, args| async move {
        let zone = zone_or_default(&ctx, args.opt_str("zone")).await?;
        let (port, proto) = (args.str("port"), args.str("protocol"));
        tolerate(ctx.call::<_, String>(FIREWALLD, PATH, ZONE, "addPort", &(zone.as_str(), port, proto, 0i32), true).await)?;
        let zpath = config_zone(&ctx, &zone).await?;
        tolerate(ctx.call::<_, ()>(FIREWALLD, zpath.as_str(), CONFIG_ZONE, "addPort", &(port, proto), true).await)?;
        Ok(Output::Done(format!("Opened {port}/{proto} in zone {zone}")))
    });

    reg.action(ALLOW_SERVICE, |ctx, args| async move {
        let zone = zone_or_default(&ctx, args.opt_str("zone")).await?;
        let service = args.str("service");
        let known: Vec<String> = ctx.call(FIREWALLD, PATH, FIREWALLD, "listServices", &(), true).await?;
        if !known.iter().any(|k| k == service) {
            anyhow::bail!("firewalld has no service called '{service}' (see firewall.available_services)");
        }
        tolerate(ctx.call::<_, String>(FIREWALLD, PATH, ZONE, "addService", &(zone.as_str(), service, 0i32), true).await)?;
        let zpath = config_zone(&ctx, &zone).await?;
        tolerate(ctx.call::<_, ()>(FIREWALLD, zpath.as_str(), CONFIG_ZONE, "addService", &(service,), true).await)?;
        Ok(Output::Done(format!("Allowed {service} in zone {zone}")))
    });

    reg.action(REMOVE, |ctx, args| async move {
        let zone = zone_or_default(&ctx, args.opt_str("zone")).await?;
        let rule = args.str("rule");
        let zpath = config_zone(&ctx, &zone).await?;
        match rule.split_once('/') {
            Some((port, proto)) => {
                tolerate(ctx.call::<_, String>(FIREWALLD, PATH, ZONE, "removePort", &(zone.as_str(), port, proto), true).await)?;
                tolerate(ctx.call::<_, ()>(FIREWALLD, zpath.as_str(), CONFIG_ZONE, "removePort", &(port, proto), true).await)?;
            }
            None => {
                tolerate(ctx.call::<_, String>(FIREWALLD, PATH, ZONE, "removeService", &(zone.as_str(), rule), true).await)?;
                tolerate(ctx.call::<_, ()>(FIREWALLD, zpath.as_str(), CONFIG_ZONE, "removeService", &(rule,), true).await)?;
            }
        }
        Ok(Output::Done(format!("Removed {rule} from zone {zone}")))
    });

    reg.shell(OVERVIEW.id, |_, _| vec!["sudo firewall-cmd --list-all-zones".into()]);
    reg.shell(AVAILABLE.id, |_, _| vec!["firewall-cmd --get-services".into()]);
    reg.shell(OPEN_PORT.id, |a, _| firewall_cmd(a.opt_str("zone"), &format!("--add-port={}/{}", sq(a.str("port")), a.str("protocol"))));
    reg.shell(ALLOW_SERVICE.id, |a, _| firewall_cmd(a.opt_str("zone"), &format!("--add-service={}", sq(a.str("service")))));
    reg.shell(REMOVE.id, |a, _| {
        let rule = a.str("rule");
        let op = if rule.contains('/') { format!("--remove-port={}", sq(rule)) } else { format!("--remove-service={}", sq(rule)) };
        firewall_cmd(a.opt_str("zone"), &op)
    });
}

/// `firewall-cmd` for the running and the permanent configuration.
fn firewall_cmd(zone: Option<&str>, op: &str) -> Vec<String> {
    let zone = zone.filter(|z| *z != "default").map(|z| format!(" --zone={}", sq(z))).unwrap_or_default();
    vec![format!("sudo firewall-cmd{zone} {op}"), format!("sudo firewall-cmd{zone} --permanent {op}")]
}

/// Adding what is already there, or removing what isn't, is not a failure.
fn tolerate<T>(r: Result<T>) -> Result<()> {
    match r {
        Ok(_) => Ok(()),
        Err(e) if ["ALREADY_ENABLED", "NOT_ENABLED"].iter().any(|k| e.to_string().contains(k)) => Ok(()),
        Err(e) => Err(e),
    }
}

async fn default_zone(ctx: &Context) -> Result<String> {
    ctx.call(FIREWALLD, PATH, FIREWALLD, "getDefaultZone", &(), true).await
}

async fn zone_or_default(ctx: &Context, zone: Option<&str>) -> Result<String> {
    match zone {
        // Models like to spell out "default" instead of leaving it empty.
        Some(z) if z != "default" => Ok(z.to_owned()),
        _ => default_zone(ctx).await,
    }
}

async fn config_zone(ctx: &Context, zone: &str) -> Result<OwnedObjectPath> {
    ctx.call(FIREWALLD, CONFIG_PATH, CONFIG, "getZoneByName", &(zone,), true).await
}

async fn overview(ctx: &Context) -> Result<Output> {
    let default = default_zone(ctx).await?;
    let active: HashMap<String, HashMap<String, Vec<String>>> = ctx.call(FIREWALLD, PATH, ZONE, "getActiveZones", &(), true).await?;

    let mut zones: Vec<(String, String)> = active
        .iter()
        .map(|(z, info)| {
            let mut bound: Vec<String> = info.get("interfaces").cloned().unwrap_or_default();
            bound.extend(info.get("sources").cloned().unwrap_or_default());
            (z.clone(), bound.join(", "))
        })
        .collect();
    if !active.contains_key(&default) {
        zones.push((default.clone(), String::new()));
    }
    zones.sort_by_key(|(z, _)| (z != &default, z.clone()));

    let mut t = Table::new(&[("zone", "Zone"), ("rule", "Rule"), ("kind", "Type"), ("bound", "Interfaces")]);
    for (zone, bound) in zones {
        let services: Vec<String> = ctx.call(FIREWALLD, PATH, ZONE, "getServices", &(zone.as_str(),), true).await?;
        let ports: Vec<Vec<String>> = ctx.call(FIREWALLD, PATH, ZONE, "getPorts", &(zone.as_str(),), true).await?;
        let label = if zone == default { format!("{bound} (default zone)").trim().to_owned() } else { bound };
        for s in services {
            t.push(vec![zone.clone(), s, "service".into(), label.clone()]);
        }
        for p in ports {
            if let [port, proto, ..] = p.as_slice() {
                t.push(vec![zone.clone(), format!("{port}/{proto}"), "port".into(), label.clone()]);
            }
        }
    }
    Ok(Output::Table(t))
}
