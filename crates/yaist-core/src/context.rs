//! What actions get to work with: the detected distribution, the system bus
//! and a way to run privileged helpers.
//!
//! Prototype note: privileged commands go through `pkexec` from the user's
//! process. In the real design they move into the `yaistd` daemon, behind
//! per-action polkit policy (see PLAN.md, "Security model").

use std::path::Path;
use std::process::Stdio;

use anyhow::{anyhow, bail, Context as _, Result};
use serde::Serialize;
use tokio::sync::OnceCell;
use zbus::zvariant::{DynamicDeserialize, DynamicType};

tokio::task_local! {
    /// Commands and D-Bus calls made by the action running in this task.
    static EXECUTED: std::cell::RefCell<Vec<String>>;
}

/// Runs `f`, collecting what it executes (see [`Context::run`] and
/// [`Context::call`]).
pub async fn logged<F: std::future::Future>(f: F) -> (F::Output, Vec<String>) {
    EXECUTED
        .scope(std::cell::RefCell::new(Vec::new()), async {
            let out = f.await;
            (out, EXECUTED.with(|l| l.take()))
        })
        .await
}

fn record(line: String) {
    let _ = EXECUTED.try_with(|l| l.borrow_mut().push(line));
}

/// Quotes `s` for a POSIX shell when it needs it.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c)) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The openSUSE flavour we're running on; actions adapt to it.
#[derive(Clone, Debug, Default)]
pub struct Distro {
    /// `ID` from os-release, e.g. `opensuse-leap`, `opensuse-tumbleweed`.
    pub id: String,
    pub version: String,
    pub pretty: String,
    /// Tumbleweed, Slowroll and the immutable variants roll; updating them
    /// means `zypper dup`, not `zypper up`.
    pub rolling: bool,
    /// Read-only root managed by `transactional-update` (immutable Leap,
    /// Leap Micro, MicroOS, Aeon, Kalpa).
    pub transactional: bool,
    /// `VARIANT_ID`, e.g. `transactional` for immutable Leap.
    pub variant_id: String,
    /// End of support (`YYYY-MM-DD`) from the base product in
    /// /etc/products.d; rolling releases have none.
    pub end_of_life: Option<String>,
}

impl Distro {
    pub fn detect() -> Self {
        // /etc/os-release is usually a symlink to /usr/lib/os-release; read
        // either (os-release(5)).
        // YAIST_OS_RELEASE points at another os-release, to see how YAiST
        // behaves on other flavours (development and tests; plans only).
        // YAIST_END_OF_LIFE overrides the end-of-support date the same way.
        let mut d = if let Ok(path) = std::env::var("YAIST_OS_RELEASE") {
            Self::from_os_release(&std::fs::read_to_string(path).unwrap_or_default(), false)
        } else {
            let text = std::fs::read_to_string("/etc/os-release").or_else(|_| std::fs::read_to_string("/usr/lib/os-release")).unwrap_or_default();
            let mut d = Self::from_os_release(&text, Path::new("/usr/sbin/transactional-update").exists() && root_is_read_only());
            d.end_of_life = end_of_life(&std::fs::read_to_string("/etc/products.d/baseproduct").unwrap_or_default());
            d
        };
        if let Ok(eol) = std::env::var("YAIST_END_OF_LIFE") {
            d.end_of_life = Some(eol).filter(|e| crate::date::parse(e).is_some());
        }
        if d.rolling {
            d.end_of_life = None;
        }
        d
    }

    /// Parses os-release. Immutable Leap keeps `ID=opensuse-leap` (it follows
    /// Leap releases) but says `VARIANT_ID=transactional` and lists
    /// `opensuse-microos` in `ID_LIKE`; `read_only_root` catches the rest.
    pub fn from_os_release(text: &str, read_only_root: bool) -> Self {
        let field =
            |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).map(|v| v.trim_matches('"').to_owned()).unwrap_or_default();
        let id = field("ID");
        let variant_id = field("VARIANT_ID");
        let id_like = field("ID_LIKE");
        let is = |name: &str| id == name || id_like.split_whitespace().any(|l| l == name);
        // Every Tumbleweed derivative rolls (MicroOS, Aeon, Kalpa, Slowroll…),
        // whatever its own name.
        let rolling = is("opensuse-tumbleweed") || is("opensuse-slowroll");
        // Immutable: said explicitly (immutable Leap, and the expected
        // immutable Tumbleweed), MicroOS-like, a known immutable flavour, or a
        // read-only root on the running system.
        let known = ["opensuse-microos", "opensuse-aeon", "opensuse-kalpa", "opensuse-leap-micro"];
        let transactional = variant_id == "transactional" || is("opensuse-microos") || known.contains(&id.as_str()) || read_only_root;
        let pretty = Some(field("PRETTY_NAME")).filter(|p| !p.is_empty()).unwrap_or_else(|| field("NAME"));
        Self { pretty, version: field("VERSION_ID"), id, rolling, transactional, variant_id, end_of_life: None }
    }

    pub fn is_opensuse(&self) -> bool {
        self.id.starts_with("opensuse") || self.id == "sles" || self.id == "sle-micro"
    }

    /// The path segment OBS uses for this distribution's repositories,
    /// e.g. `openSUSE_Tumbleweed` or `16.0`.
    pub fn obs_repo_name(&self) -> String {
        if self.id.contains("slowroll") || self.pretty.contains("Slowroll") {
            "openSUSE_Slowroll".into()
        } else if self.rolling {
            "openSUSE_Tumbleweed".into()
        } else {
            self.version.clone()
        }
    }
}

/// The product's own `<endoflife>` (the first one; the codestream's follows).
pub fn end_of_life(product_xml: &str) -> Option<String> {
    let start = product_xml.find("<endoflife>")? + "<endoflife>".len();
    let end = product_xml[start..].find("</endoflife>")? + start;
    let date = product_xml[start..end].trim();
    crate::date::parse(date).map(|_| date.to_owned())
}

fn root_is_read_only() -> bool {
    std::fs::read_to_string("/proc/mounts").unwrap_or_default().lines().any(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        f.len() > 3 && f[1] == "/" && f[3].split(',').any(|o| o == "ro")
    })
}

fn effective_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("Uid:"))?;
    line.split_whitespace().nth(2)?.parse().ok()
}

/// Returns the total physical system memory in KiB, if detectable.
/// Can be overridden with `YAIST_TOTAL_MEM_KIB` for tests and VM simulation.
pub fn total_memory_kib() -> Option<u64> {
    if let Ok(val) = std::env::var("YAIST_TOTAL_MEM_KIB") {
        if let Ok(kib) = val.parse::<u64>() {
            return Some(kib);
        }
    }
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = info.lines().find(|l| l.starts_with("MemTotal:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

pub struct Context {
    pub distro: Distro,
    bus: OnceCell<zbus::Connection>,
}

impl Context {
    pub fn new() -> Self {
        Self { distro: Distro::detect(), bus: OnceCell::new() }
    }

    pub async fn system_bus(&self) -> Result<&zbus::Connection> {
        self.bus.get_or_try_init(|| async { zbus::Connection::system().await.context("connecting to the system D-Bus") }).await
    }

    async fn proxy(&self, dest: &'static str, path: &str, iface: &'static str) -> Result<zbus::Proxy<'static>> {
        let conn = self.system_bus().await?;
        Ok(zbus::proxy::Builder::<zbus::Proxy<'static>>::new(conn)
            .destination(dest)
            .path(path.to_owned())
            .interface(iface)
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await?)
    }

    /// Calls a D-Bus method. `interactive` lets the service ask polkit to
    /// authenticate the user (the desktop's polkit agent shows the dialog).
    pub async fn call<B, R>(
        &self,
        dest: &'static str,
        path: &str,
        iface: &'static str,
        method: &'static str,
        body: &B,
        interactive: bool,
    ) -> Result<R>
    where
        B: Serialize + DynamicType,
        R: for<'d> DynamicDeserialize<'d>,
    {
        let proxy = self.proxy(dest, path, iface).await?;
        if interactive {
            // Interactive calls are the changing ones; reads stay out of the log.
            record(format!("D-Bus {iface}.{method} {}", serde_json::to_string(body).unwrap_or_default()));
        }
        let flags = if interactive { zbus::proxy::MethodFlags::ALLOW_INTERACTIVE_AUTH } else { zbus::proxy::MethodFlags::empty() };
        let reply = proxy.call_with_flags::<_, B, R>(method, flags, body).await.map_err(dbus_error)?;
        reply.ok_or_else(|| anyhow!("{method}: no reply"))
    }

    pub async fn property<T>(&self, dest: &'static str, path: &str, iface: &'static str, name: &'static str) -> Result<T>
    where
        T: serde::de::DeserializeOwned + zbus::zvariant::Type,
    {
        let proxy = self.proxy(dest, path, iface).await?;
        proxy.get_property::<T>(name).await.map_err(dbus_error)
    }

    /// Runs a command as the user. Output is forced to the C locale so it can
    /// be parsed.
    pub async fn run(&self, argv: &[&str]) -> Result<String> {
        run_command(argv, &[0], true).await
    }

    /// Like [`Context::run`], treating the exit codes in `ok` as success
    /// (zypper reports "nothing found" or "some repos skipped" this way).
    pub async fn run_ok(&self, argv: &[&str], ok: &[i32]) -> Result<String> {
        run_command(argv, ok, true).await
    }

    /// Like [`Context::run_ok`], for output meant for parsing (XML, JSON):
    /// it never goes to the client's log.
    pub async fn read_ok(&self, argv: &[&str], ok: &[i32]) -> Result<String> {
        run_command(argv, ok, false).await
    }

    /// Opens a URL in the user's browser. Unlike other commands this keeps the
    /// environment (display, session bus), so callers must only pass vetted
    /// URLs.
    pub async fn open_in_browser(&self, url: &str) -> Result<()> {
        let status = tokio::process::Command::new("/usr/bin/xdg-open")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .context("running xdg-open")?;
        if !status.success() {
            bail!("xdg-open could not open {url} ({status})");
        }
        Ok(())
    }

    /// Runs a command and returns only its exit code (for tools that answer
    /// with one, like `zypper needs-rebooting`).
    pub async fn exit_code(&self, argv: &[&str]) -> Result<i32> {
        let (prog, rest) = argv.split_first().ok_or_else(|| anyhow!("empty command"))?;
        let status = tokio::process::Command::new(prog)
            .args(rest)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .env("LC_ALL", "C.UTF-8")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status()
            .await
            .with_context(|| format!("running {prog}"))?;
        Ok(status.code().unwrap_or(-1))
    }

    /// Runs a command as root: directly when we already are, otherwise via
    /// pkexec. Arguments are passed as argv, never through a shell.
    pub async fn run_privileged(&self, argv: &[&str]) -> Result<String> {
        self.run_privileged_ok(argv, &[0]).await
    }

    pub async fn run_privileged_ok(&self, argv: &[&str], ok: &[i32]) -> Result<String> {
        if effective_uid() == Some(0) {
            return run_command(argv, ok, true).await;
        }
        let mut full = vec!["/usr/bin/pkexec"];
        full.extend_from_slice(argv);
        run_command(&full, ok, true).await.map_err(|e| {
            let msg = e.to_string();
            if msg.contains("exit status: 126") {
                anyhow!("authentication was cancelled")
            } else if msg.contains("exit status: 127") {
                anyhow!("not authorized (no polkit agent? run yaist as root, or from a desktop session)")
            } else {
                e
            }
        })
    }
    /// Starts a command as root with a pseudo-terminal as its controlling
    /// terminal, for tools that read answers from /dev/tty rather than stdin
    /// (zypper's prompts). Only the tool gets the terminal: `setsid --ctty`
    /// hands it over after pkexec has authenticated, so pkexec never prompts
    /// on it. Output stays on a pipe, apart from what the terminal echoes.
    pub fn spawn_privileged_tty(&self, argv: &[&str]) -> Result<TtyProcess> {
        let (master, slave) = open_pty()?;
        let mut full = Vec::new();
        if effective_uid() != Some(0) {
            full.push("/usr/bin/pkexec");
        }
        full.extend_from_slice(&["/usr/bin/setsid", "--ctty", "--wait"]);
        full.extend_from_slice(argv);
        record(full.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" "));
        let mut child = tokio::process::Command::new(full[0])
            .args(&full[1..])
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .env("LC_ALL", "C.UTF-8")
            .stdin(Stdio::from(slave))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("running {}", argv.first().unwrap_or(&"")))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        Ok(TtyProcess { child, stdout: tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stdout)), tty: master })
    }
}

/// A running [`Context::spawn_privileged_tty`] command.
pub struct TtyProcess {
    child: tokio::process::Child,
    pub stdout: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    tty: std::fs::File,
}

impl TtyProcess {
    /// Types a line on the terminal, as a user answering a prompt would.
    pub fn answer(&mut self, line: &str) -> Result<()> {
        use std::io::Write;
        writeln!(self.tty, "{line}").context("answering the prompt")
    }

    /// Waits for the command and returns its exit code.
    pub async fn wait(mut self) -> Result<i32> {
        let status = self.child.wait().await.context("waiting for the command")?;
        match status.code() {
            Some(126) if effective_uid() != Some(0) => bail!("authentication was cancelled"),
            Some(127) if effective_uid() != Some(0) => bail!("not authorized (no polkit agent? run yaist as root, or from a desktop session)"),
            Some(code) => Ok(code),
            None => bail!("the command was killed ({status})"),
        }
    }
}

/// A new pseudo-terminal: (master, slave), both close-on-exec.
fn open_pty() -> Result<(std::fs::File, std::fs::File)> {
    use std::os::fd::FromRawFd;
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: openpty only writes the two descriptors; name, termios and
    // window size are optional and passed as null.
    let r = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
    if r != 0 {
        return Err(std::io::Error::last_os_error()).context("opening a pseudo-terminal");
    }
    for fd in [master, slave] {
        // SAFETY: fd is a descriptor openpty just returned.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    // SAFETY: both descriptors are new and owned by nobody else.
    Ok(unsafe { (std::fs::File::from_raw_fd(master), std::fs::File::from_raw_fd(slave)) })
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

async fn run_command(argv: &[&str], ok: &[i32], show: bool) -> Result<String> {
    let (prog, rest) = argv.split_first().ok_or_else(|| anyhow!("empty command"))?;
    record(argv.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" "));
    let mut cmd = tokio::process::Command::new(prog);
    cmd.args(rest)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let (status, stdout, stderr) = if show && crate::log::active() {
        streamed(cmd).await
    } else {
        cmd.output().await.map(|o| (o.status, String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned()))
    }
    .with_context(|| format!("running {prog}"))?;
    if !status.code().is_some_and(|c| ok.contains(&c)) {
        let detail = if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() };
        bail!("{prog} failed ({status}): {}", last_lines(detail, 6));
    }
    Ok(stdout)
}

/// Runs `cmd`, passing each line of its output to the client's log as it
/// comes, and collects stdout and stderr as `output()` would.
async fn streamed(mut cmd: tokio::process::Command) -> std::io::Result<(std::process::ExitStatus, String, String)> {
    use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
    async fn collect(pipe: Option<impl AsyncRead + Unpin>) -> String {
        // Bytes, not lines(): one invalid UTF-8 line must not stop the
        // reading and leave the command blocked on a full pipe.
        let mut all = String::new();
        let Some(pipe) = pipe else { return all };
        let mut reader = BufReader::new(pipe);
        let mut buf = Vec::new();
        while reader.read_until(b'\n', &mut buf).await.is_ok_and(|n| n > 0) {
            let l = String::from_utf8_lossy(&buf);
            let l = l.trim_end_matches(['\n', '\r']);
            crate::log::line(l);
            all.push_str(l);
            all.push('\n');
            buf.clear();
        }
        all
    }
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let (stdout, stderr) = tokio::join!(collect(child.stdout.take()), collect(child.stderr.take()));
    Ok((child.wait().await?, stdout, stderr))
}

pub fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn dbus_error(e: zbus::Error) -> anyhow::Error {
    match &e {
        zbus::Error::MethodError(name, detail, _) => {
            let name = name.as_str();
            let detail = detail.clone().unwrap_or_default();
            if name.ends_with("InteractiveAuthorizationRequired") || name.ends_with("AccessDenied") || name.ends_with("NotAuthorized") {
                anyhow!("not authorized: {detail}")
            } else {
                anyhow!("{detail} ({name})")
            }
        }
        _ => anyhow!(e),
    }
}

#[cfg(test)]
mod tests {
    use super::Distro;

    fn load(name: &str) -> Distro {
        let path = format!("{}/../../tests/os-release/{name}", env!("CARGO_MANIFEST_DIR"));
        Distro::from_os_release(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")), false)
    }

    #[test]
    fn flavours_from_real_os_release_files() {
        // (file, name shown, rolling, immutable, OBS repository)
        let cases = [
            ("leap-16.0", "openSUSE Leap 16.0", false, false, "16.0"),
            ("leap-16.1-immutable", "openSUSE Leap 16.1", false, true, "16.1"),
            ("tumbleweed", "openSUSE Tumbleweed", true, false, "openSUSE_Tumbleweed"),
            ("microos", "openSUSE MicroOS", true, true, "openSUSE_Tumbleweed"),
            ("aeon", "openSUSE Aeon", true, true, "openSUSE_Tumbleweed"),
            ("kalpa", "openSUSE Kalpa", true, true, "openSUSE_Tumbleweed"),
            ("tumbleweed-immutable-future", "openSUSE Tumbleweed", true, true, "openSUSE_Tumbleweed"),
        ];
        for (file, name, rolling, immutable, obs) in cases {
            let d = load(file);
            assert_eq!(d.pretty, name, "{file}");
            assert_eq!(d.rolling, rolling, "{file}: rolling");
            assert_eq!(d.transactional, immutable, "{file}: immutable");
            assert_eq!(d.obs_repo_name(), obs, "{file}");
        }
    }

    #[test]
    fn read_only_root_means_immutable() {
        let d = Distro::from_os_release("ID=\"opensuse-tumbleweed\"\n", true);
        assert!(d.rolling && d.transactional);
    }

    #[tokio::test]
    async fn output_reaches_the_log_as_it_comes() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = seen.clone();
        let logger: crate::log::Logger = std::sync::Arc::new(move |l| sink.lock().unwrap().push(l));
        let script = "printf 'one\\n'; printf 'Retrieving 10%%\\rRetrieving done\\n'; printf '\\377bad\\n'; echo oops >&2; printf 'last\\n'";
        let out = crate::log::logging(logger, super::run_command(&["/bin/sh", "-c", script], &[0], true)).await.unwrap();
        assert_eq!(out, "one\nRetrieving 10%\rRetrieving done\n\u{fffd}bad\nlast\n");
        let mut seen = seen.lock().unwrap().clone();
        seen.sort();
        assert_eq!(seen, ["Retrieving done", "last", "one", "oops", "\u{fffd}bad"]);
    }

    #[tokio::test]
    async fn failures_still_say_why_when_streamed() {
        let logger: crate::log::Logger = std::sync::Arc::new(|_| ());
        let e = crate::log::logging(logger, super::run_command(&["/bin/sh", "-c", "echo broken >&2; exit 3"], &[0], true)).await.unwrap_err();
        assert!(e.to_string().contains("broken"), "{e}");
    }
}
