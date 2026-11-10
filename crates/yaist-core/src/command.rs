//! The `/command` syntax: `/<module> <verb> [positional…] [name=value…]`,
//! e.g. `/firewall open-port 8080 tcp` or `/users create alice admin=yes`.
//! Every action has one, and panels echo the command for what they did.

use anyhow::{anyhow, bail, Result};
use serde_json::{Map, Value};

use crate::{ActionSpec, Args, Param, ParamKind, Registry};

pub enum Parsed {
    Action { spec: ActionSpec, raw: Map<String, Value> },
    Help(Option<String>),
}

pub fn parse(registry: &Registry, line: &str) -> Result<Parsed> {
    if is_tool_command(line) {
        return parse(registry, &translate(line)?);
    }
    let line = line.trim().strip_prefix('/').ok_or_else(|| anyhow!("commands start with '/'"))?;
    let words = split_words(line)?;
    let mut it = words.into_iter();
    let module = it.next().unwrap_or_default();
    if module.is_empty() || module == "help" || module == "?" {
        return Ok(Parsed::Help(it.next()));
    }
    let Some(verb) = it.next() else {
        if registry.find_module(&module).is_some() {
            return Ok(Parsed::Help(Some(module)));
        }
        bail!("unknown module '{module}' — try /help");
    };
    let id = format!("{module}.{}", verb.replace('-', "_"));
    let spec = *registry.get(&id).ok_or_else(|| anyhow!("unknown command '/{module} {verb}' — try /help {module}"))?;

    let mut raw = Map::new();
    let mut positional = Vec::new();
    for word in it {
        match word.split_once('=') {
            Some((k, v)) if spec.param(k).is_some() => {
                raw.insert(k.to_owned(), Value::String(v.to_owned()));
            }
            _ => positional.push(word),
        }
    }
    let free: Vec<&str> = spec.params.iter().map(|p| p.name).filter(|n| !raw.contains_key(*n)).collect();
    let mut free = free.into_iter();
    for word in positional {
        let p = free.next().ok_or_else(|| anyhow!("too many arguments for /{module} {verb}"))?;
        raw.insert(p.to_owned(), Value::String(word));
    }
    Ok(Parsed::Action { spec, raw })
}

/// The command a user would type to run `spec` with `args`.
pub fn format(spec: &ActionSpec, args: &Args) -> String {
    let mut out = format!("/{} {}", spec.module(), spec.verb().replace('_', "-"));
    let mut positional = true;
    for p in spec.params {
        let Some(v) = args.0.get(p.name) else {
            positional = false;
            continue;
        };
        let text = match v {
            Value::String(s) if p.secret => {
                let _ = s;
                "••••••".to_owned()
            }
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if !p.required && p.default == Some(text.as_str()) {
            positional = false;
            continue;
        }
        positional &= p.required;
        out.push(' ');
        if !positional {
            out.push_str(p.name);
            out.push('=');
        }
        out.push_str(&quote(&text));
    }
    out
}

/// Tools whose familiar commands YAiST understands, so users can type what
/// they know (or are learning) and still get a plan card.
const TOOLS: &[&str] = &[
    "zypper",
    "systemctl",
    "snapper",
    "firewall-cmd",
    "hostnamectl",
    "timedatectl",
    "setsebool",
    "getsebool",
    "sestatus",
    "setenforce",
    "useradd",
    "userdel",
];

/// `zypper in fish`, `sudo systemctl restart sshd`, `/snapper list`…
pub fn is_tool_command(line: &str) -> bool {
    let mut words = line.trim().trim_start_matches('/').split_whitespace();
    let first = match words.next() {
        Some("sudo") => words.next(),
        w => w,
    };
    first.is_some_and(|w| TOOLS.contains(&w))
}

/// Translates a familiar tool command into the matching `/command`.
pub fn translate(line: &str) -> Result<String> {
    let words = split_words(line.trim().trim_start_matches('/'))?;
    let mut w: Vec<&str> = words.iter().map(String::as_str).collect();
    if w.first() == Some(&"sudo") {
        w.remove(0);
    }
    let (tool, rest) = w.split_first().ok_or_else(|| anyhow!("empty command"))?;
    // Options with a value, as `--opt value` or `--opt=value`.
    let opt =
        |names: &[&str]| -> Option<String> {
            rest.iter().enumerate().find_map(|(i, a)| {
                names.iter().find_map(|n| {
                    if *a == *n {
                        rest.get(i + 1).map(|v| v.to_string())
                    } else {
                        a.strip_prefix(&format!("{n}=")).map(str::to_owned)
                    }
                })
            })
        };
    let has = |names: &[&str]| rest.iter().any(|a| names.contains(a));
    // Options that take the next word as their value, per tool.
    let valued: &[&str] = match *tool {
        "zypper" => &["--releasever", "-r", "--repo"],
        "snapper" => &["-d", "--description", "-c", "--config"],
        "useradd" => &["-c", "--comment", "-G", "--groups", "-s", "--shell"],
        _ => &[],
    };
    let mut args: Vec<&str> = Vec::new();
    let mut skip = false;
    for a in rest {
        if std::mem::take(&mut skip) {
            continue;
        }
        if a.starts_with('-') {
            skip = valued.contains(a);
            continue;
        }
        args.push(a);
    }
    let (cmd, args) = args.split_first().map(|(c, a)| (*c, a.to_vec())).unwrap_or(("", Vec::new()));
    let need = |n: usize, what: &str| -> Result<()> {
        if args.len() < n {
            bail!("`{tool} {cmd}` needs {what}");
        }
        Ok(())
    };
    let q = |s: &str| quote(s);
    let joined = || quote(&args.join(" "));
    let unknown = || anyhow!("YAiST doesn't know `{tool} {cmd}` yet; try /help");
    Ok(match *tool {
        "zypper" => match cmd {
            "in" | "install" => {
                need(1, "a package")?;
                format!("/packages install {}", joined())
            }
            "rm" | "remove" => {
                need(1, "a package")?;
                format!("/packages remove {}", joined())
            }
            "se" | "search" => {
                need(1, "a search term")?;
                format!("/packages search {}", q(args[0]))
            }
            "if" | "info" => {
                need(1, "a package")?;
                format!("/packages info {}", q(args[0]))
            }
            "up" | "update" if args.is_empty() => "/packages update-system".into(),
            "up" | "update" => format!("/packages update {}", joined()),
            "lu" | "list-updates" => "/packages list-updates".into(),
            "ref" | "refresh" => "/packages refresh".into(),
            "dup" | "dist-upgrade" => match opt(&["--releasever"]) {
                Some(v) => format!("/upgrade migrate leap:{}", v),
                None => "/upgrade migrate dup".into(),
            },
            "lr" | "repos" => "/repos list".into(),
            "ar" | "addrepo" => {
                need(2, "a URL and an alias")?;
                format!("/repos add {} {}", q(args[0]), q(args[1]))
            }
            "rr" | "removerepo" => {
                need(1, "a repository alias")?;
                format!("/repos remove {}", q(args[0]))
            }
            "mr" | "modifyrepo" if has(&["-e", "--enable"]) => format!("/repos enable {}", q(args.first().copied().unwrap_or(""))),
            "mr" | "modifyrepo" if has(&["-d", "--disable"]) => format!("/repos disable {}", q(args.first().copied().unwrap_or(""))),
            _ => return Err(unknown()),
        },
        "systemctl" => match cmd {
            "" | "list-units" if has(&["--failed"]) || rest.contains(&"--state=failed") => "/services list state=failed".into(),
            "" | "list-units" => "/services list state=running".into(),
            "--failed" => "/services list state=failed".into(),
            "status" | "start" | "stop" | "restart" => {
                need(1, "a service")?;
                format!("/services {cmd} {}", q(args[0]))
            }
            "enable" | "disable" => {
                need(1, "a service")?;
                format!("/services {cmd} {} now={}", q(args[0]), if has(&["--now"]) { "yes" } else { "no" })
            }
            _ => return Err(unknown()),
        },
        "snapper" => match cmd {
            "" | "list" | "ls" => "/snapshots list".into(),
            "create" => format!("/snapshots create {}", q(&opt(&["-d", "--description"]).unwrap_or_else(|| "manual snapshot".into()))),
            "delete" | "remove" | "rm" => {
                need(1, "a snapshot number")?;
                format!("/snapshots delete {}", q(args[0]))
            }
            "rollback" => {
                need(1, "a snapshot number")?;
                format!("/snapshots rollback {}", q(args[0]))
            }
            "status" => {
                need(1, "a range like 42..0")?;
                format!("/snapshots changes {}", q(args[0].split("..").next().unwrap_or("")))
            }
            _ => return Err(unknown()),
        },
        "firewall-cmd" => {
            let zone = opt(&["--zone"]).map(|z| format!(" zone={}", q(&z))).unwrap_or_default();
            if let Some(p) = opt(&["--add-port"]) {
                let (port, proto) = p.split_once('/').unwrap_or((&p, "tcp"));
                format!("/firewall open-port {} {}{zone}", q(port), q(proto))
            } else if let Some(svc) = opt(&["--add-service"]) {
                format!("/firewall allow-service {}{zone}", q(&svc))
            } else if let Some(rule) = opt(&["--remove-port", "--remove-service"]) {
                format!("/firewall remove {}{zone}", q(&rule))
            } else if has(&["--get-services"]) {
                "/firewall available-services".into()
            } else {
                "/firewall overview".into()
            }
        }
        "hostnamectl" => match cmd {
            "set-hostname" | "hostname" if !args.is_empty() => format!("/system set-hostname {}", q(args[0])),
            _ => "/system overview".into(),
        },
        "timedatectl" => match cmd {
            "set-timezone" => {
                need(1, "a time zone")?;
                format!("/system set-timezone {}", q(args[0]))
            }
            "set-ntp" => {
                need(1, "true or false")?;
                format!("/system set-ntp {}", q(args[0]))
            }
            _ => "/system overview".into(),
        },
        "setsebool" => {
            if args.len() < 2 && !cmd.is_empty() && args.is_empty() {
                bail!("`setsebool` needs a boolean and on/off");
            }
            let value = args.first().copied().unwrap_or("");
            format!("/selinux set-boolean {} {} persistent={}", q(cmd), q(value), if has(&["-P"]) { "yes" } else { "no" })
        }
        "getsebool" if !cmd.is_empty() => format!("/selinux booleans filter={}", q(cmd)),
        "getsebool" => "/selinux booleans".into(),
        "sestatus" => "/selinux status".into(),
        "setenforce" => {
            let mode = match cmd.to_ascii_lowercase().as_str() {
                "1" | "enforcing" => "enforcing",
                "0" | "permissive" => "permissive",
                _ => bail!("`setenforce` takes 1/Enforcing or 0/Permissive"),
            };
            format!("/selinux set-mode {mode}")
        }
        "useradd" => {
            if cmd.is_empty() {
                bail!("`useradd` needs a user name");
            }
            let admin = opt(&["-G", "--groups"]).is_some_and(|g| g.split(',').any(|x| x == "wheel"));
            let name = opt(&["-c", "--comment"]).map(|n| format!(" full_name={}", q(&n))).unwrap_or_default();
            format!("/users create {}{name} admin={}", q(cmd), if admin { "yes" } else { "no" })
        }
        "userdel" => {
            if cmd.is_empty() {
                bail!("`userdel` needs a user name");
            }
            format!("/users delete {} remove_home={}", q(cmd), if has(&["-r", "--remove"]) { "yes" } else { "no" })
        }
        _ => return Err(unknown()),
    })
}

/// The words that can finish the last word of a command line, for Tab.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Completion {
    /// Byte offset in the line where the word being completed starts.
    pub start: usize,
    /// Whole words that can stand there, each starting with the partial
    /// word already typed: names sorted, parameters in the action's order.
    pub candidates: Vec<String>,
}

/// A command the client handles itself rather than the registry (`/clear`,
/// `/zoom`): its name with the slash, and the words its argument can be.
pub type ClientCommand = (&'static str, &'static [&'static str]);

/// Completes the last word of `line` (the text before the cursor): after
/// `/` the commands and modules, then a module's verbs, then an action's
/// choices, yes/no values and `name=` parameters. Plain-language requests
/// complete to nothing.
pub fn complete(registry: &Registry, line: &str, client: &[ClientCommand]) -> Completion {
    let start = line.char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(i, c)| i + c.len_utf8());
    let word = &line[start..];
    let before: Vec<&str> = line[..start].split_whitespace().collect();
    let client_args = |cmd: &str| client.iter().find(|(name, _)| name.strip_prefix('/') == Some(cmd)).map(|(_, args)| *args);
    // Names sort; an action's parameters keep its order, as /help shows them.
    let (mut words, sorted): (Vec<String>, bool) = match before.split_first() {
        None if word.starts_with('/') => {
            let mut w = vec!["/help".to_owned()];
            w.extend(registry.modules.iter().map(|m| format!("/{}", m.id)));
            w.extend(client.iter().map(|(name, _)| (*name).to_owned()));
            (w, true)
        }
        None => (Vec::new(), false),
        Some((first, rest)) => match (first.strip_prefix('/'), rest) {
            (Some("help" | "?"), []) => (registry.modules.iter().map(|m| m.id.to_owned()).collect(), true),
            (Some(cmd), []) if client_args(cmd).is_some() => (client_args(cmd).unwrap_or_default().iter().map(|a| (*a).to_owned()).collect(), false),
            (Some(m), []) => (registry.module_actions(m).iter().map(|s| s.verb().replace('_', "-")).collect(), true),
            (Some(m), [verb, typed @ ..]) => match registry.get(&format!("{m}.{}", verb.replace('-', "_"))) {
                Some(spec) => (param_words(spec, typed, word), false),
                None => (Vec::new(), false),
            },
            (None, _) => (Vec::new(), false),
        },
    };
    if sorted {
        words.sort();
    }
    let mut candidates: Vec<String> = Vec::new();
    for w in words {
        if w.starts_with(word) && !candidates.contains(&w) {
            candidates.push(w);
        }
    }
    Completion { start, candidates }
}

/// The values a parameter can take, when there is a fixed set.
fn values(p: &Param) -> &'static [&'static str] {
    match p.kind {
        ParamKind::Choice(options) => options,
        ParamKind::Bool => &["yes", "no"],
        ParamKind::Text | ParamKind::Integer => &[],
    }
}

/// What can follow the arguments already `typed` for `spec`, the way
/// [`parse`] reads them: `name=value`, or the next free parameter in order.
fn param_words(spec: &ActionSpec, typed: &[&str], word: &str) -> Vec<String> {
    if let Some((name, _)) = word.split_once('=') {
        return spec.param(name).map(|p| values(p).iter().map(|v| format!("{name}={v}")).collect()).unwrap_or_default();
    }
    let named: Vec<&str> = typed.iter().filter_map(|w| w.split_once('=').map(|(k, _)| k)).filter(|k| spec.param(k).is_some()).collect();
    let positional = typed.len() - named.len();
    let free: Vec<&Param> = spec.params.iter().filter(|p| !named.contains(&p.name)).skip(positional).collect();
    let mut out: Vec<String> = free.first().map(|p| values(p).iter().map(|v| (*v).to_owned()).collect()).unwrap_or_default();
    out.extend(free.iter().map(|p| format!("{}=", p.name)));
    out
}

fn quote(s: &str) -> String {
    if !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'') {
        return s.to_owned();
    }
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn split_words(line: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => cur.extend(chars.next()),
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            (None, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        bail!("unterminated quote");
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

/// `/help` text: all modules, or one module's commands.
pub fn help(registry: &Registry, module: Option<&str>) -> String {
    match module.and_then(|m| registry.find_module(m)) {
        Some(m) => {
            let mut out = format!("{} — {}\n", m.title, m.tagline);
            for s in registry.module_actions(m.id) {
                let params: Vec<String> =
                    s.params.iter().map(|p| if p.required { format!("<{}>", p.name) } else { format!("[{}=…]", p.name) }).collect();
                out.push_str(&format!("\n/{} {} {}\n    {}", m.id, s.verb().replace('_', "-"), params.join(" "), s.description));
            }
            out
        }
        None => {
            let mut out = String::from("Type a request in plain language, or a command:\n");
            for m in &registry.modules {
                out.push_str(&format!("\n/help {:<12} {}", m.id, m.tagline));
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{validate, ActionSpec, Output, Param, Risk};

    const OPEN: ActionSpec = ActionSpec {
        id: "firewall.open_port",
        title: "Open port",
        description: "test",
        risk: Risk::Change,
        params: &[
            Param::text("port", "port").check(validate::port),
            Param::choice("protocol", &["tcp", "udp"], "proto").default("tcp"),
            Param::text("zone", "zone").optional(),
        ],
    };

    fn registry() -> Registry {
        let mut reg = Registry::default();
        reg.action(OPEN, |_, _| async { Ok(Output::Done(String::new())) });
        reg
    }

    fn parse_args(line: &str) -> Args {
        match parse(&registry(), line).expect("parses") {
            Parsed::Action { spec, raw } => spec.validate(&raw).expect("valid"),
            Parsed::Help(_) => panic!("expected an action"),
        }
    }

    #[test]
    fn positional_and_named_arguments() {
        let a = parse_args("/firewall open-port 8080 udp");
        assert_eq!((a.str("port"), a.str("protocol")), ("8080", "udp"));
        let a = parse_args("/firewall open_port zone=public 8080");
        assert_eq!((a.str("port"), a.str("protocol"), a.str("zone")), ("8080", "tcp", "public"));
    }

    #[test]
    fn format_round_trips() {
        // Optional parameters are always spelled out, so the formatted command
        // is canonical however it was typed.
        for (typed, canonical) in [
            ("/firewall open-port 8080", "/firewall open-port 8080"),
            ("/firewall open-port 8080 udp", "/firewall open-port 8080 protocol=udp"),
            ("/firewall open-port zone=\"my zone\" 8080", "/firewall open-port 8080 zone=\"my zone\""),
        ] {
            let args = parse_args(typed);
            assert_eq!(format(&OPEN, &args), canonical);
            assert_eq!(parse_args(canonical), args);
        }
    }

    #[test]
    fn rejects_bad_input() {
        let reg = registry();
        assert!(parse(&reg, "/firewall nope").is_err());
        assert!(parse(&reg, "/firewall open-port \"unterminated").is_err());
        assert!(parse(&reg, "/firewall open-port 1 tcp zone extra").is_err());
        let Ok(Parsed::Action { spec, raw }) = parse(&reg, "/firewall open-port 99999") else { panic!() };
        assert!(spec.validate(&raw).is_err());
        let Ok(Parsed::Action { spec, raw }) = parse(&reg, "/firewall open-port 80 icmp") else { panic!() };
        assert!(spec.validate(&raw).is_err());
    }

    #[test]
    fn unknown_parameters_are_refused() {
        let mut raw = serde_json::Map::new();
        raw.insert("port".into(), "80".into());
        raw.insert("command".into(), "rm -rf /".into());
        assert!(OPEN.validate(&raw).is_err());
    }

    #[test]
    fn tool_commands_translate() {
        let cases = [
            ("zypper in fish", "/packages install fish"),
            ("sudo zypper install fish htop", "/packages install \"fish htop\""),
            ("/zypper se vim", "/packages search vim"),
            ("zypper --releasever 16.1 dup", "/upgrade migrate leap:16.1"),
            ("sudo zypper -n --releasever=16.1 dup --allow-vendor-change", "/upgrade migrate leap:16.1"),
            ("zypper up", "/packages update-system"),
            ("zypper ar https://example.org/repo games", "/repos add https://example.org/repo games"),
            ("zypper mr -d games", "/repos disable games"),
            ("systemctl restart sshd", "/services restart sshd"),
            ("systemctl enable --now sshd", "/services enable sshd now=yes"),
            ("systemctl --failed", "/services list state=failed"),
            ("snapper create -d 'before tests'", "/snapshots create \"before tests\""),
            ("snapper status 42..0", "/snapshots changes 42"),
            ("firewall-cmd --zone=public --add-port=8080/tcp --permanent", "/firewall open-port 8080 tcp zone=public"),
            ("firewall-cmd --add-service=https", "/firewall allow-service https"),
            ("setsebool -P httpd_can_network_connect on", "/selinux set-boolean httpd_can_network_connect on persistent=yes"),
            ("setenforce 0", "/selinux set-mode permissive"),
            ("useradd -m -c 'Tom S' -G wheel tom", "/users create tom full_name=\"Tom S\" admin=yes"),
            ("userdel -r tom", "/users delete tom remove_home=yes"),
        ];
        for (typed, expected) in cases {
            assert!(is_tool_command(typed), "{typed}");
            assert_eq!(translate(typed).unwrap(), expected, "{typed}");
        }
        assert!(!is_tool_command("install fish please"));
        assert!(!is_tool_command("/packages install fish"));
        assert!(translate("zypper frobnicate").is_err());
    }

    #[test]
    fn completes_words() {
        let mut reg = registry();
        reg.module(crate::ModuleSpec {
            id: "firewall",
            title: "Firewall",
            tagline: "test",
            overview: "firewall.open_port",
            row_actions: &[],
            overview_needs_admin: false,
        });
        let client: &[ClientCommand] = &[("/demo", &["update-done", "upgrade-running"]), ("/clear", &[])];
        let words = |line: &str| complete(&reg, line, client).candidates;
        assert_eq!(words("/"), ["/clear", "/demo", "/firewall", "/help"]);
        assert_eq!(words("/de"), ["/demo"]);
        assert_eq!(words("/demo u"), ["update-done", "upgrade-running"]);
        assert_eq!(words("/demo upg"), ["upgrade-running"]);
        assert_eq!(words("/help f"), ["firewall"]);
        assert_eq!(words("/firewall "), ["open-port"]);
        // The port is free text; the protocol is next, with its choices.
        assert_eq!(words("/firewall open-port "), ["port=", "protocol=", "zone="]);
        assert_eq!(words("/firewall open-port 80 "), ["tcp", "udp", "protocol=", "zone="]);
        assert_eq!(words("/firewall open-port port=80 "), ["tcp", "udp", "protocol=", "zone="]);
        assert_eq!(words("/firewall open-port 80 protocol="), ["protocol=tcp", "protocol=udp"]);
        assert_eq!(words("/firewall open-port 80 udp z"), ["zone="]);
        assert!(words("/clear ").is_empty());
        assert!(words("/nope ").is_empty());
        assert!(words("install fish").is_empty());
        assert_eq!(complete(&reg, "/demo upd", client).start, 6);
    }

    #[test]
    fn help_lists_modules() {
        assert!(matches!(parse(&registry(), "/help").unwrap(), Parsed::Help(None)));
    }
}
