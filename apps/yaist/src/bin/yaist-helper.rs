//! yaist-helper: the privileged half of YAiST, started through pkexec.
//!
//! It runs one fixed operation per invocation and re-validates every argument
//! itself, since whoever starts it is not trusted. Multi-step operations (a
//! Leap upgrade) run under one authentication instead of one per step.
//! This is the seed of the planned `yaistd` daemon.
//!
//!   yaist-helper snapper-list
//!   yaist-helper snapper-create <description>
//!   yaist-helper snapper-status <number>
//!   yaist-helper snapper-delete <number>
//!   yaist-helper snapper-rollback <number>
//!   yaist-helper migrate-leap leap|leap-micro <version> [repo-alias-to-disable…]
//!   yaist-helper nvidia-setup leap|tumbleweed
//!   yaist-helper codecs-packman
//!   yaist-helper migrate-rolling tumbleweed|slowroll [repo-alias-to-disable…]
//!   yaist-helper migrate-sle <email> <regcode-file> [repo-alias-to-disable…]

use std::path::Path;
use std::process::{Command, ExitCode};

use yaist_core::validate;

const TMP_REPO: &str = "yaist-migration-tmp";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yaist-helper: {e}");
            ExitCode::FAILURE
        }
    }
}

type R = Result<(), String>;

fn run(args: &[String]) -> R {
    let arg = |i: usize| args.get(i).map(String::as_str).ok_or_else(|| format!("missing argument {i}"));
    let number = |i: usize| -> Result<&str, String> {
        let n = arg(i)?;
        validate::snapshot_number(n)?;
        Ok(n)
    };
    match args.first().map(String::as_str) {
        Some("snapper-list") => snapper(&["--jsonout", "list", "--disable-used-space"]),
        Some("snapper-create") => {
            let d = arg(1)?;
            validate::description(d)?;
            snapper(&["create", "--description", d, "--cleanup-algorithm", "number", "--print-number", "--userdata", "important=yes"])
        }
        Some("snapper-status") => snapper(&["status", &format!("{}..0", number(1)?)]),
        Some("snapper-delete") => snapper(&["delete", number(1)?]),
        Some("snapper-rollback") => {
            let n = number(1)?;
            if Path::new("/usr/sbin/transactional-update").exists() && root_read_only() {
                exec("/usr/sbin/transactional-update", &["--non-interactive", "rollback", n])
            } else {
                snapper(&["rollback", n])
            }
        }
        Some("migrate-leap") => migrate_leap(arg(1)?, arg(2)?, &args[3..]),
        Some("nvidia-setup") => nvidia_setup(arg(1)?),
        Some("codecs-packman") => codecs_packman(),
        Some("migrate-rolling") => migrate_rolling(arg(1)?, &args[2..]),
        Some("migrate-sle") => migrate_sle(arg(1)?, arg(2)?, &args[3..]),
        _ => Err("usage: yaist-helper snapper-list|snapper-create|snapper-status|snapper-delete|snapper-rollback|migrate-leap|migrate-rolling|migrate-sle|nvidia-setup|codecs-packman …".into()),
    }
}

fn exec(prog: &str, args: &[&str]) -> R {
    println!("==> {prog} {}", args.join(" "));
    let status = Command::new(prog)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LC_ALL", "C.UTF-8")
        .status()
        .map_err(|e| format!("running {prog}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{prog} {} failed ({status})", args.first().unwrap_or(&"")))
    }
}

fn snapper(args: &[&str]) -> R {
    let mut full = vec!["-c", "root"];
    full.extend_from_slice(args);
    exec("/usr/bin/snapper", &full)
}

fn zypper(args: &[&str]) -> R {
    let mut full = vec!["--non-interactive"];
    full.extend_from_slice(args);
    exec("/usr/bin/zypper", &full)
}

fn root_read_only() -> bool {
    std::fs::read_to_string("/proc/mounts").unwrap_or_default().lines().any(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        f.len() > 3 && f[1] == "/" && f[3].split(',').any(|o| o == "ro")
    })
}

fn os_release(key: &str) -> String {
    std::fs::read_to_string("/etc/os-release")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
        .unwrap_or("")
        .trim_matches('"')
        .to_owned()
}

/// Leap 16 needs x86-64-v2.
fn x86_64_v2() -> bool {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let flags: Vec<&str> = cpuinfo.lines().find(|l| l.starts_with("flags")).map(|l| l.split_whitespace().collect()).unwrap_or_default();
    ["cx16", "sse4_1", "sse4_2", "popcnt"].iter().all(|f| flags.contains(f))
}

/// zypper, or zypper inside the next snapshot on transactional systems.
fn zypper_step(tx: bool, args: &[&str]) -> R {
    if tx {
        let mut full = vec!["--non-interactive", "--continue", "run", "/usr/bin/zypper", "--non-interactive"];
        full.extend_from_slice(args);
        exec("/usr/sbin/transactional-update", &full)
    } else {
        zypper(args)
    }
}

/// NVIDIA drivers the openSUSE way: the NVIDIA repository definitions, then
/// install-new-recommends, which picks the driver matching the detected card.
fn nvidia_setup(flavour: &str) -> R {
    let repos = match flavour {
        "leap" => "openSUSE-repos-Leap-NVIDIA",
        "tumbleweed" => "openSUSE-repos-Tumbleweed-NVIDIA",
        _ => return Err(format!("unknown flavour {flavour}")),
    };
    let tx = transactional();
    zypper_step(tx, &["install", repos])?;
    zypper_step(tx, &["refresh-services"])?;
    zypper_step(tx, &["install-new-recommends"])?;
    println!("==> NVIDIA drivers installed; reboot to load them");
    Ok(())
}

fn disable_repos(tx: bool, aliases: &[String]) -> R {
    for alias in aliases {
        validate::name(alias)?;
        if zypper_step(tx, &["modifyrepo", "--disable", "--", alias]).is_err() {
            println!("    (repository {alias} not found, skipped)");
        }
    }
    Ok(())
}

fn is_slowroll() -> bool {
    os_release("NAME").contains("Slowroll") || os_release("ID").contains("slowroll")
}

/// Leap to Tumbleweed or Slowroll, or between the two, the way
/// opensuse-migration-tool does: openSUSE-repos for the target from its own
/// oss repository, then a full dup (allowing downgrades for Slowroll, which
/// lags Tumbleweed).
fn migrate_rolling(target: &str, disable: &[String]) -> R {
    let id = os_release("ID");
    let from_leap = id == "opensuse-leap";
    let from_tw = id == "opensuse-tumbleweed" && !is_slowroll();
    let from_slowroll = is_slowroll();
    let arch = std::env::consts::ARCH;
    let (url, repos_pkg, downgrade) = match target {
        "tumbleweed" if from_leap || from_slowroll => {
            let url = match arch {
                "x86_64" => "https://download.opensuse.org/tumbleweed/repo/oss/".to_owned(),
                "aarch64" => "https://download.opensuse.org/ports/aarch64/tumbleweed/repo/oss/".to_owned(),
                _ => return Err(format!("migrating to Tumbleweed is not supported on {arch} yet")),
            };
            (url, "openSUSE-repos-Tumbleweed", from_slowroll)
        }
        "slowroll" if from_leap || from_tw => {
            if arch != "x86_64" {
                return Err(format!("Slowroll is only built for x86_64, not {arch}"));
            }
            ("https://download.opensuse.org/slowroll/repo/oss/".to_owned(), "openSUSE-repos-Slowroll", true)
        }
        _ => return Err(format!("migrating from {} to {target} is not supported", os_release("NAME"))),
    };
    let tx = transactional();
    snapshot_before(tx, &format!("before migrating to {target}"))?;
    disable_repos(tx, disable)?;
    zypper_step(tx, &["addrepo", "--refresh", "--", &url, TMP_REPO])?;
    let mut install = vec!["install", "--repo", TMP_REPO, "--force-resolution", "--from", TMP_REPO];
    if downgrade {
        install.push("--allow-downgrade");
    }
    install.push(repos_pkg);
    let installed = zypper_step(tx, &install);
    let _ = zypper_step(tx, &["removerepo", "--", TMP_REPO]);
    installed?;
    zypper_step(tx, &["refresh-services"])?;
    let mut flags = vec!["--force-resolution", "--allow-vendor-change"];
    if downgrade {
        flags.push("--allow-downgrade");
    }
    for args in dup_steps(&[], &flags) {
        zypper_step(tx, &args)?;
    }
    // Tumbleweed and Slowroll both use openSUSE-release (Slowroll its own build).
    ensure_release(tx, &[], "openSUSE-release", downgrade, "opensuse-tumbleweed")?;
    println!("==> migrated to {target}; reboot to finish");
    Ok(())
}

/// The SLES product description shipped with YAiST (opensuse-migration-tool's
/// SLES.prod). As in that tool it is used for every Leap version: the SCC
/// registration and the dup bring the proper product.
fn sles_product() -> Option<std::path::PathBuf> {
    let name = "SLES.prod";
    let dev = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.parent()?.parent()?.join("assets/migration").join(name)));
    [Some(Path::new("/usr/share/yaist/migration").join(name)), dev].into_iter().flatten().find(|p| p.is_file())
}

/// Leap to SUSE Linux Enterprise Server, the way opensuse-migration-tool does
/// (a community tool, not supported by SUSE; the resulting SLES is): SLES os-release and product, registration
/// with SCC (and PackageHub), dup, SLE branding. The registration code comes
/// in a file that only the user can read, so it never shows on a command line
/// here; it is deleted at once.
fn migrate_sle(email: &str, regcode_file: &str, disable: &[String]) -> R {
    let email_ok = email.split_once('@').is_some_and(|(u, d)| !u.is_empty() && d.contains('.'))
        && email.chars().all(|c| c.is_ascii_alphanumeric() || "@._%+-".contains(c))
        && !email.starts_with('-');
    if !email_ok {
        return Err("invalid email address".into());
    }
    let regcode = std::fs::read_to_string(regcode_file).map_err(|e| format!("reading the registration code: {e}"))?;
    let _ = std::fs::remove_file(regcode_file);
    let regcode = regcode.trim().to_owned();
    if regcode.len() < 4 || regcode.len() > 64 || !regcode.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("invalid registration code".into());
    }
    if os_release("ID") != "opensuse-leap" {
        return Err("this system is not openSUSE Leap".into());
    }
    if transactional() {
        return Err("migrating immutable systems to SLE is not supported".into());
    }
    let version = os_release("VERSION_ID");
    let product = sles_product().ok_or("SLES.prod not found (should be in /usr/share/yaist/migration)")?;
    let (major, minor) = version.split_once('.').ok_or("unexpected VERSION_ID")?;
    let arch = std::env::consts::ARCH;

    snapshot_before(false, "before migrating to SUSE Linux Enterprise")?;
    disable_repos(false, disable)?;
    zypper(&["install", "suseconnect-ng", "snapper", "grub2-snapper-plugin"])?;
    println!("==> backing up /etc/os-release as /etc/os-release.backup");
    std::fs::copy("/etc/os-release", "/etc/os-release.backup").map_err(|e| e.to_string())?;
    let _ = exec("/usr/bin/rpm", &["-e", "--nodeps", "openSUSE-release"]);
    let _ = exec("/usr/bin/rpm", &["-e", "--nodeps", "openSUSE-repos"]);
    let os_release = format!(
        "NAME=\"SLES\"\nVERSION=\"{major}-SP{minor}\"\nVERSION_ID=\"{version}\"\nPRETTY_NAME=\"SUSE Linux Enterprise Server {major} SP{minor}\"\nID=\"sles\"\nID_LIKE=\"suse\"\nANSI_COLOR=\"0;32\"\nCPE_NAME=\"cpe:/o:suse:sles:{major}:sp{minor}\"\nDOCUMENTATION_URL=\"https://documentation.suse.com/\"\n"
    );
    // /etc/os-release may be a symlink to /usr/lib/os-release; replace the link.
    let _ = std::fs::remove_file("/etc/os-release");
    std::fs::write("/etc/os-release", os_release).map_err(|e| e.to_string())?;
    std::fs::copy(&product, "/etc/products.d/SLES.prod").map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file("/etc/products.d/baseproduct");
    std::os::unix::fs::symlink("/etc/products.d/SLES.prod", "/etc/products.d/baseproduct").map_err(|e| e.to_string())?;
    println!("==> registering with SUSE Customer Center as {email}");
    let status = Command::new("/usr/bin/suseconnect")
        .args(["-e", email, "-r", &regcode])
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .status()
        .map_err(|e| format!("running suseconnect: {e}"))?;
    if !status.success() {
        return Err(format!("registration failed ({status}); /etc/os-release.backup holds the original"));
    }
    exec("/usr/bin/suseconnect", &["-p", &format!("PackageHub/{version}/{arch}")])?;
    for args in dup_steps(&[], &["--force-resolution", "--allow-vendor-change"]) {
        zypper(&args)?;
    }
    let _ = exec(
        "/usr/bin/rpm",
        &[
            "-e",
            "--nodeps",
            "branding-openSUSE",
            "grub2-branding-openSUSE",
            "wallpaper-branding-openSUSE",
            "plymouth-branding-openSUSE",
            "systemd-presets-branding-openSUSE",
            "systemd-presets-branding-MicroOS",
        ],
    );
    let _ = zypper(&["remove", "opensuse-welcome"]);
    // Branding for the SLE major version, as the tool does with -15; cosmetic,
    // so never fatal.
    let branding = |m: &str| {
        zypper(&[
            "install",
            &format!("branding-SLE-{m}"),
            "grub2-branding-SLE",
            &format!("wallpaper-branding-SLE-{m}"),
            "plymouth-branding-SLE",
            "systemd-presets-branding-SLE",
        ])
    };
    if branding(major).is_err() && major != "15" && branding("15").is_err() {
        println!("    (SLE branding could not be installed; the system works without it)");
    }
    println!("==> migrated to SUSE Linux Enterprise Server {major} SP{minor}; reboot to finish");
    Ok(())
}

/// Full multimedia codecs from Packman, via opi (adds Packman and switches the
/// multimedia packages to its builds). Third-party; not for immutable systems.
fn codecs_packman() -> R {
    if transactional() {
        return Err("Packman codecs are not supported on immutable systems; use Flatpak apps, which bring their own codecs".into());
    }
    zypper(&["install", "opi"])?;
    exec("/usr/bin/opi", &["-n", "codecs"])
}

/// An important snapshot to roll back to, and its number in the log for the
/// client's rollback button. Transactional systems keep the running snapshot
/// as it is anyway; without snapper's root config there is nothing to take.
fn snapshot_before(tx: bool, description: &str) -> R {
    if tx {
        return Ok(());
    }
    if !Path::new("/etc/snapper/configs/root").exists() {
        println!("    (no snapper configuration for /: no snapshot to roll back to)");
        return Ok(());
    }
    let args = ["-c", "root", "create", "--description", description, "--cleanup-algorithm", "number", "--print-number", "--userdata", "important=yes"];
    println!("==> /usr/bin/snapper {}", args.join(" "));
    let out = Command::new("/usr/bin/snapper")
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LC_ALL", "C.UTF-8")
        .output()
        .map_err(|e| format!("running snapper: {e}"))?;
    let number = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !out.status.success() || number.parse::<u32>().is_err() {
        return Err(format!("taking a snapshot failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    // yaist_ui::snapshot_before reads this line.
    println!("==> snapshot {number} taken before the upgrade");
    Ok(())
}

/// A distribution upgrade as two zypper runs: first everything is
/// downloaded, while nothing has changed yet, so a network or mirror problem
/// stops it cleanly; then the install from the package cache, without
/// refreshing, as the upgrade restarts the network on the way (DNS gone for
/// seconds) and running programs meet replaced libraries.
fn dup_steps<'a>(global: &[&'a str], flags: &[&'a str]) -> [Vec<&'a str>; 2] {
    let run = |extra: &[&'a str], verb: &[&'a str]| -> Vec<&'a str> { [global, extra, verb, flags].concat() };
    [run(&[], &["dup", "--download-only"]), run(&["--no-refresh"], &["dup"])]
}

/// After the dup, make sure the target's release package (it defines which
/// distribution this is: os-release, products.d) is the one installed, and
/// check os-release agrees. --force-resolution replaces a conflicting one,
/// e.g. Leap-release by openSUSE-release when moving to Tumbleweed.
fn ensure_release(tx: bool, global: &[&str], pkg: &str, downgrade: bool, expect_id: &str) -> R {
    // The dup has just refreshed the metadata. Refreshing again fails when the
    // upgrade restarted the network a moment ago (DNS gone for a few seconds).
    let mut args: Vec<&str> = global.to_vec();
    args.extend(["--no-refresh", "install", "--force-resolution"]);
    if downgrade {
        args.push("--allow-downgrade");
    }
    args.push(pkg);
    zypper_step(tx, &args)?;
    if tx {
        // The new snapshot isn't running yet; its os-release can't be read here.
        return Ok(());
    }
    let id = os_release("ID");
    if id != expect_id {
        return Err(format!("after the upgrade os-release says ID={id}, expected {expect_id}; check that {pkg} is installed"));
    }
    Ok(())
}

/// Is this a transactional (immutable) system? Immutable Leap says so in
/// os-release; a read-only root catches the rest.
fn transactional() -> bool {
    os_release("VARIANT_ID") == "transactional"
        || os_release("ID_LIKE").split_whitespace().any(|l| l == "opensuse-microos")
        || ["opensuse-microos", "opensuse-aeon", "opensuse-kalpa", "opensuse-leap-micro"].contains(&os_release("ID").as_str())
        || (Path::new("/usr/sbin/transactional-update").exists() && root_read_only())
}

/// Upgrades Leap (or Leap Micro) to a newer release, the way
/// opensuse-migration-tool does: the repository definitions come from the
/// *target* release's openSUSE-repos package, then
/// `zypper --releasever <target> dup`. On transactional systems every step runs
/// inside one chain of snapshots (`transactional-update --continue run`), which
/// becomes active after a reboot.
fn migrate_leap(flavour: &str, target: &str, disable: &[String]) -> R {
    validate::release_version(target)?;
    let (id, repos_pkg) = match flavour {
        "leap" => ("opensuse-leap", "openSUSE-repos-Leap"),
        "leap-micro" => ("opensuse-leap-micro", "openSUSE-repos-LeapMicro"),
        _ => return Err(format!("unknown Leap flavour {flavour}")),
    };
    if os_release("ID") != id {
        return Err(format!("this system is not {id}"));
    }
    let current = os_release("VERSION_ID");
    if !validate::version_newer(target, &current) {
        return Err(format!("{target} is not newer than the installed {current}"));
    }
    let major: u32 = target.split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0);
    if flavour == "leap" && std::env::consts::ARCH == "x86_64" && major >= 16 && !x86_64_v2() {
        return Err(format!("Leap {target} needs an x86-64-v2 CPU; see https://en.opensuse.org/openSUSE:X86-64-Architecture-Levels"));
    }
    let url = match flavour {
        "leap" => format!("https://download.opensuse.org/distribution/leap/{target}/repo/oss"),
        _ => format!(
            "https://download.opensuse.org/distribution/leap-micro/{target}/product/repo/openSUSE-Leap-Micro-{target}-{}/",
            std::env::consts::ARCH
        ),
    };

    let tx = transactional();
    snapshot_before(tx, &format!("before upgrading to {target}"))?;
    let step = |args: &[&str]| -> R {
        if tx {
            let mut full = vec!["--non-interactive", "--continue", "run", "/usr/bin/zypper", "--non-interactive"];
            full.extend_from_slice(args);
            exec("/usr/sbin/transactional-update", &full)
        } else {
            zypper(args)
        }
    };

    // Third-party repositories pinned to the old release break `zypper dup`.
    for alias in disable {
        validate::name(alias)?;
        if step(&["modifyrepo", "--disable", "--", alias]).is_err() {
            println!("    (repository {alias} not found, skipped)");
        }
    }
    step(&["install", "--force-resolution", repos_pkg])?;
    step(&["refresh-services"])?;
    step(&["addrepo", "--refresh", "--", &url, TMP_REPO])?;
    let installed = step(&["install", "--repo", TMP_REPO, "--force-resolution", "--from", TMP_REPO, repos_pkg]);
    let _ = step(&["removerepo", "--", TMP_REPO]);
    installed?;
    step(&["refresh-services"])?;
    // --releasever is a global option: it goes before the command.
    for args in dup_steps(&["--releasever", target], &["--force-resolution", "--allow-vendor-change"]) {
        step(&args)?;
    }
    if flavour == "leap" {
        // Leap 16 is Leap-release; Leap 15 was openSUSE-release.
        let pkg = if major >= 16 { "Leap-release" } else { "openSUSE-release" };
        ensure_release(tx, &["--releasever", target], pkg, false, "opensuse-leap")?;
    }
    println!("==> upgraded to {target}; reboot to finish");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn upgrade_downloads_first_then_installs_from_the_cache() {
        let [download, install] = super::dup_steps(&["--releasever", "16.1"], &["--force-resolution", "--allow-vendor-change"]);
        assert_eq!(download, ["--releasever", "16.1", "dup", "--download-only", "--force-resolution", "--allow-vendor-change"]);
        assert_eq!(install, ["--releasever", "16.1", "--no-refresh", "dup", "--force-resolution", "--allow-vendor-change"]);
    }
}
