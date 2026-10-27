//! `yaist`: the terminal client (default) and a small CLI over the same
//! actions.
//!
//!   yaist                         start the TUI
//!   yaist run /firewall overview  run a command; changes need --yes, and
//!                                 questions they raise are asked here
//!   yaist ask "open port 8080"    ask the agent once; plans are shown, never applied
//!   yaist help [module]           list commands
//!   yaist tools                   print the LLM tool definitions (JSON)

mod tui;

use std::sync::Arc;

use anyhow::{bail, Result};
use yaist_core::{command, Context, Output, Risk};

fn main() -> Result<()> {
    // Behave like other CLI tools when piped into `head`.
    reset_sigpipe();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => tui::run(),
        Some("run") => cli_run(&args[1..]),
        Some("ask") => cli_ask(&args[1..].join(" ")),
        Some("tui-snapshot") => tui::snapshot(&args[1..], 160, 46),
        Some("help" | "--help" | "-h") => {
            println!("{}", command::help(&yaist_modules::registry(), args.get(1).map(String::as_str)));
            Ok(())
        }
        Some("tools") => {
            let reg = yaist_modules::registry();
            let tools: Vec<_> = reg
                .specs()
                .map(|s| serde_json::json!({ "name": yaist_agent::tool_name(s.id), "risk": s.risk.label(), "description": s.description, "parameters": s.json_schema() }))
                .collect();
            println!("{}", serde_json::to_string_pretty(&tools)?);
            Ok(())
        }
        Some(other) => bail!("unknown command '{other}'; try: yaist, yaist run /<module> <verb> …, yaist help, yaist tools"),
    }
}

fn cli_run(args: &[String]) -> Result<()> {
    let yes = args.iter().any(|a| a == "--yes" || a == "-y");
    let line = args.iter().filter(|a| *a != "--yes" && *a != "-y").cloned().collect::<Vec<_>>().join(" ");
    let line = if line.starts_with('/') { line } else { format!("/{line}") };
    let reg = yaist_modules::registry();
    let (spec, raw) = match command::parse(&reg, &line)? {
        command::Parsed::Help(m) => {
            println!("{}", command::help(&reg, m.as_deref()));
            return Ok(());
        }
        command::Parsed::Action { spec, raw } => (spec, raw),
    };
    let args = spec.validate(&raw)?;
    if spec.risk != Risk::Read && !yes {
        bail!("{} is a {} action: {}\nre-run with --yes to apply", spec.id, spec.risk.label(), command::format(&spec, &args));
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let out = rt.block_on(yaist_core::ask::asking(Arc::new(ask_on_terminal), reg.run(Arc::new(Context::new()), spec.id, args)))?;
    match out {
        Output::Table(t) => {
            let widths: Vec<usize> = (0..t.columns.len())
                .map(|i| t.rows.iter().map(|r| r[i].chars().count()).chain([t.columns[i].title.len()]).max().unwrap_or(0).min(40))
                .collect();
            let fmt =
                |cells: Vec<&str>| cells.iter().zip(&widths).map(|(c, w)| format!("{:<w$}", truncate(c, *w), w = *w)).collect::<Vec<_>>().join("  ");
            println!("{}", fmt(t.columns.iter().map(|c| c.title).collect()));
            for r in &t.rows {
                println!("{}", fmt(r.iter().map(String::as_str).collect()));
            }
        }
        other => println!("{}", other.to_llm_text(usize::MAX)),
    }
    Ok(())
}

/// Answers a running change's questions (a package conflict, a license) on
/// the terminal: choices by number, Enter or end of input for the default.
fn ask_on_terminal(q: yaist_core::ask::Question) -> yaist_core::BoxFuture<Option<String>> {
    Box::pin(async move {
        tokio::task::spawn_blocking(move || {
            eprintln!("\n{}\n{}\n", q.title, q.text);
            for (i, c) in q.choices.iter().enumerate() {
                eprintln!("  {}) {}{}", i + 1, c.label, if c.default { "  [default]" } else { "" });
            }
            loop {
                eprint!("choice: ");
                let mut line = String::new();
                if std::io::stdin().read_line(&mut line).ok()? == 0 {
                    eprintln!();
                    return q.default_choice().map(|c| c.value.clone());
                }
                match line.trim() {
                    "" => return q.default_choice().map(|c| c.value.clone()),
                    n => match n.parse::<usize>().ok().and_then(|n| q.choices.get(n.wrapping_sub(1))) {
                        Some(c) => return Some(c.value.clone()),
                        None => eprintln!("pick 1-{}", q.choices.len()),
                    },
                }
            }
        })
        .await
        .ok()
        .flatten()
    })
}

/// One request through the full agent path, for testing and latency checks.
/// Plans are printed and then cancelled.
fn cli_ask(question: &str) -> Result<()> {
    use yaist_ui::{Cmd, Entry, Session};
    let session = Session::start_headless();
    // Wait for the model check so the first request isn't racing it.
    for _ in 0..300 {
        if session.state().llm != yaist_ui::LlmStatus::Checking {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let start = session.state().conversation.len();
    let t0 = std::time::Instant::now();
    session.send(Cmd::Submit(question.to_owned()));
    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let s = session.state();
        let settled = s.busy.is_none() && s.conversation.len() > start + 1;
        if settled || t0.elapsed().as_secs() > 120 {
            break;
        }
    }
    let s = session.state();
    for e in &s.conversation[start..] {
        match e {
            Entry::User(t) => println!("you    > {t}"),
            Entry::Command(c) => println!("cmd    > {c}"),
            Entry::AgentRan { command, shell, ok } => {
                println!("ran    > {command} ({})", if *ok { "ok" } else { "failed" });
                for c in shell {
                    println!("         $ {c}");
                }
            }
            Entry::Assistant { text, elapsed } => println!("yaist  > {text}  [{:.1}s]", elapsed.as_secs_f32()),
            Entry::Plan(p) => {
                println!("PLAN   > [{}] {}  (not applied)", p.spec.risk.label(), p.command);
                for c in &p.shell {
                    println!("         $ {c}");
                }
            }
            Entry::Result { title, output, .. } => println!("result > {title}: {}", output.headline()),
            Entry::Error(e) => println!("error  > {e}"),
            Entry::Notice(n) => println!("note   > {n}"),
            Entry::Notes(n) => println!("notes  > {}", n.listing()),
        }
    }
    println!("total  > {:.1}s", t0.elapsed().as_secs_f32());
    if let Some(id) = s.pending_plan().map(|p| p.id) {
        drop(s);
        session.send(Cmd::Cancel(id));
    }
    Ok(())
}

fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_owned()
    } else {
        s.chars().take(w.saturating_sub(1)).chain(['…']).collect()
    }
}

fn reset_sigpipe() {
    // SAFETY: restoring the default disposition of SIGPIPE at startup.
    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }
    unsafe {
        signal(13, 0);
    }
}
