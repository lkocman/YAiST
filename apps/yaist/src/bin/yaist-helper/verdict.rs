//! Whether an upgrade happened, judged from the system rather than from exit
//! codes: a package script can fail, or a step after the install, while the
//! system is fully upgraded; and a failure before the install changed nothing.
//!
//! The helper says so in its output, which the client reads:
//!   ==> warning: <something to look at; the upgrade is done>
//!   ==> problem: <why the upgrade is not complete>
//!   ==> verdict: unchanged | incomplete | migrated | migrated-with-warnings

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use super::{os_release, R};

/// More packages than this still to change after the install means it
/// stopped part-way; fewer is a newer build in a kept repository, or one the
/// release left behind, and `zypper dup` after the reboot handles it.
const PENDING_INCOMPLETE: usize = 20;

/// What the upgrade should have produced, set when its install step starts:
/// from then on the system may have changed.
pub struct Check {
    pub tx: bool,
    /// Global zypper options of the upgrade, e.g. `--releasever 16.1`.
    pub global: Vec<String>,
    pub flags: Vec<String>,
    /// os-release ID and VERSION_ID afterwards (none: not checked).
    pub id: Option<String>,
    pub version: Option<String>,
}

static CHECK: OnceLock<Check> = OnceLock::new();
static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Something to look at, though the upgrade went through.
pub fn warn(text: &str) {
    println!("==> warning: {text}");
    WARNINGS.lock().unwrap_or_else(|e| e.into_inner()).push(text.to_owned());
}

/// The install step of an upgrade: zypper's output passes through, failed
/// package scripts become warnings, and zypper's "done, but" exit codes
/// (100-107) count as done. Anything else stops the upgrade; [`judge`] then
/// looks at how far it got.
pub fn install(check: Check, prog: &str, args: &[&str]) -> R {
    let _ = CHECK.set(check);
    println!("==> {prog} {}", args.join(" "));
    let mut child = Command::new(prog)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LC_ALL", "C.UTF-8")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("running {prog}: {e}"))?;
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let scripts = std::thread::scope(|s| {
        let o = s.spawn(|| pass_through(out, false));
        let e = s.spawn(|| pass_through(err, true));
        [o.join().unwrap_or_default(), e.join().unwrap_or_default()].concat()
    });
    let status = child.wait().map_err(|e| format!("running {prog}: {e}"))?;
    for line in &scripts {
        warn(line);
    }
    match status.code() {
        Some(0) => Ok(()),
        Some(107) => {
            if scripts.is_empty() {
                warn("a package script failed (zypper exit code 107)");
            }
            Ok(())
        }
        Some(106) => {
            warn("some repositories could not be refreshed (zypper exit code 106)");
            Ok(())
        }
        Some(c) if (100..=105).contains(&c) => Ok(()),
        _ => Err(format!("the install stopped ({status})")),
    }
}

/// Copies a pipe to our own output line by line, keeping the lines that say
/// a package script failed.
fn pass_through(pipe: Option<impl std::io::Read>, stderr: bool) -> Vec<String> {
    use std::io::BufRead;
    let mut failed = Vec::new();
    let Some(pipe) = pipe else { return failed };
    for line in std::io::BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
        let line = String::from_utf8_lossy(&line);
        let line = line.trim_end();
        if stderr {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
        if let Some(i) = line.find('%').filter(|_| line.contains("scriptlet failed")) {
            failed.push(line[i..].to_owned());
        }
    }
    failed
}

/// The verdict on a whole upgrade, given how its steps ended: an error
/// before the install step means nothing changed; from the install on, the
/// system decides. Errors stand only when the upgrade is incomplete.
pub fn judge(result: R) -> R {
    let Some(check) = CHECK.get() else {
        if result.is_err() {
            println!("==> verdict: unchanged");
        }
        return result;
    };
    let mut problems = Vec::new();
    if let Err(e) = result {
        if check.tx {
            // The new snapshot can't be looked at from here.
            problems.push(e);
        } else {
            warn(&e);
        }
    }
    if !check.tx {
        problems.extend(inspect(check));
    }
    if !problems.is_empty() {
        for p in &problems {
            println!("==> problem: {p}");
        }
        println!("==> verdict: incomplete");
        return Err(format!("the upgrade is incomplete: {}", problems.join("; ")));
    }
    let warned = !WARNINGS.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
    println!("==> verdict: {}", if warned { "migrated-with-warnings" } else { "migrated" });
    Ok(())
}

/// What the running system says about the upgrade: problems make it
/// incomplete, smaller things are warnings.
fn inspect(check: &Check) -> Vec<String> {
    let mut problems = Vec::new();
    if let Some(id) = &check.id {
        let now = os_release("ID");
        if &now != id {
            problems.push(format!("os-release says {now}, not {id}: the release package was not replaced"));
        }
    }
    if let Some(v) = &check.version {
        let now = os_release("VERSION_ID");
        if &now != v {
            problems.push(format!("os-release says version {now}, not {v}"));
        }
    }
    match pending(check) {
        Some(0) => {}
        Some(n) if n > PENDING_INCOMPLETE => problems.push(format!("{n} packages are still to be upgraded")),
        Some(n) => warn(&format!("{n} packages still differ from the new release; zypper dup after the reboot brings them along")),
        None => warn("could not check whether every package was upgraded"),
    }
    if !deps_satisfied() {
        warn("some installed packages have unmet dependencies (zypper verify)");
    }
    if let Some(kernel) = kernel_without_initrd() {
        problems.push(format!("kernel {kernel} has no initrd: don't reboot before this is fixed or rolled back"));
    }
    problems
}

/// How many packages a dry-run of the same dup would still change (from the
/// cache: no network).
fn pending(check: &Check) -> Option<usize> {
    let mut args: Vec<&str> = check.global.iter().map(String::as_str).collect();
    args.extend(["--non-interactive", "--no-refresh", "dup", "--dry-run"]);
    args.extend(check.flags.iter().map(String::as_str));
    println!("==> /usr/bin/zypper {}", args.join(" "));
    let out = Command::new("/usr/bin/zypper").args(&args).env_clear().env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin").env("LC_ALL", "C.UTF-8").output().ok()?;
    pending_from(&String::from_utf8_lossy(&out.stdout))
}

/// Packages to change, from zypper's summary ("175 packages to upgrade, 4 to
/// downgrade, 20 new, 1 to remove.") or "Nothing to do.".
fn pending_from(out: &str) -> Option<usize> {
    if out.lines().any(|l| l.trim() == "Nothing to do.") {
        return Some(0);
    }
    let summary = out.lines().map(str::trim).find(|l| {
        l.starts_with(|c: char| c.is_ascii_digit()) && [" to upgrade", " to downgrade", " new", " to remove", " to reinstall"].iter().any(|k| l.contains(k))
    })?;
    // Vendor and architecture changes are among the upgrades already.
    Some(
        summary
            .trim_end_matches('.')
            .split(',')
            .filter(|part| !part.contains("vendor") && !part.contains("arch"))
            .filter_map(|part| part.split_whitespace().next()?.parse::<usize>().ok())
            .sum(),
    )
}

fn deps_satisfied() -> bool {
    Command::new("/usr/bin/zypper")
        .args(["--non-interactive", "--no-refresh", "verify", "--dry-run"])
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LC_ALL", "C.UTF-8")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("Dependencies of all installed packages are satisfied"))
}

/// The newest installed kernel, when no initrd for it exists: in /boot (GRUB)
/// or on the ESP (systemd-boot and GRUB with BLS: <ESP>/<entry>/<version>/).
fn kernel_without_initrd() -> Option<String> {
    let newest = std::fs::read_dir("/usr/lib/modules")
        .ok()?
        .flatten()
        .filter_map(|e| Some((e.path().join("vmlinuz").metadata().ok()?.modified().ok()?, e.file_name().to_string_lossy().into_owned())))
        .max()?
        .1;
    if Path::new(&format!("/boot/initrd-{newest}")).exists() {
        return None;
    }
    let on_esp = ["/boot/efi", "/efi", "/boot"].iter().filter_map(|esp| std::fs::read_dir(esp).ok()).flatten().flatten().any(|entry| {
        std::fs::read_dir(entry.path().join(&newest))
            .into_iter()
            .flatten()
            .flatten()
            .any(|f| f.file_name().to_string_lossy().starts_with("initrd"))
    });
    (!on_esp).then_some(newest)
}

#[cfg(test)]
mod tests {
    use super::pending_from;

    #[test]
    fn packages_still_to_change() {
        assert_eq!(pending_from("Loading repository data...\nNothing to do.\n"), Some(0));
        assert_eq!(pending_from("...\n175 packages to upgrade, 4 to downgrade, 20 new, 1 to remove.\nOverall download size: 1 GiB."), Some(200));
        assert_eq!(pending_from("1 package to upgrade, 3  to change vendor.\n"), Some(1));
        assert_eq!(pending_from("Problem: nothing provides foo\n"), None);
    }
}
