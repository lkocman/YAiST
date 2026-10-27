//! What's new in the release an upgrade goes to, for this system: the
//! release notes, the entries in them that concern its hardware, desktop
//! and packages, and links to read the rest.
//!
//! Our code picks the entries; a model may summarise them, never choose
//! them. Leap's own notes come first: online (doc.opensuse.org, kept in
//! ~/.cache/yaist/release-notes) or installed (release-notes-openSUSE-Leap-
//! <version>, which has them before they are online). Leap's notes are an
//! overlay on the notes of the SUSE Linux Enterprise Server release it is
//! built on, so without them, SLES's stand in. Either way, for Leap the
//! entries only enterprise customers care about are left out. `YAIST_RELEASE_NOTES=file.html` reads a local copy
//! instead, for testing before the notes are out.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use yaist_core::{validate, ActionSpec, Context, Output, Param, Registry, Risk};

/// Entries offered at most: what fits a glance and a small model's prompt.
const MAX_ENTRIES: usize = 10;
/// Characters of each entry the model gets.
const EXCERPT: usize = 450;
/// A cached page younger than this is not fetched again.
const FRESH: Duration = Duration::from_secs(24 * 3600);

pub const WHATS_NEW: ActionSpec = ActionSpec {
    id: "upgrade.release_notes",
    title: "What's new",
    description: "The release notes of a Leap release (version, e.g. 16.1): the entries that concern this system's hardware, desktop and installed packages, and links to the full notes. Use it when the user asks what is new or what changes in a release.",
    risk: Risk::Read,
    params: &[Param::text("version", "Leap release, e.g. 16.1").check(validate::release_version)],
};

pub fn register(reg: &mut Registry) {
    reg.action(WHATS_NEW, |ctx, args| async move {
        let notes = load(&ctx, &Target::Leap(args.str("version").to_owned())).await;
        Ok(Output::Text(notes.listing()))
    });
    reg.shell(WHATS_NEW.id, |a, _| vec![format!("xdg-open {}", Doc::Leap(a.str("version").to_owned()).url())]);
}

/// Where an upgrade goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A Leap release, e.g. "16.1".
    Leap(String),
    /// A SUSE Linux Enterprise Server release, e.g. "16.0".
    Sles(String),
}

impl Target {
    /// The release an `upgrade.migrate` target names, if it has notes:
    /// `leap:16.1` does; Tumbleweed and Slowroll publish none per snapshot.
    pub fn of_upgrade(target: &str) -> Option<Self> {
        target.strip_prefix("leap:").filter(|v| validate::release_version(v).is_ok()).map(|v| Target::Leap(v.to_owned()))
    }

    pub fn name(&self) -> String {
        match self {
            Target::Leap(v) => format!("Leap {v}"),
            Target::Sles(v) => format!("SUSE Linux Enterprise Server {v}"),
        }
    }
}

/// One set of release notes.
#[derive(Clone, Debug, PartialEq)]
enum Doc {
    Leap(String),
    Sles(String),
}

impl Doc {
    fn url(&self) -> String {
        match self {
            Doc::Leap(v) => {
                format!("https://doc.opensuse.org/release-notes/x86_64/openSUSE/Leap/{v}/html/release-notes-leap-{}/index.html", v.replace('.', ""))
            }
            Doc::Sles(v) => format!("https://documentation.suse.com/releasenotes/sles/html/releasenotes_sles_{v}/index.html"),
        }
    }

    fn name(&self) -> String {
        match self {
            Doc::Leap(v) => format!("Leap {v} release notes"),
            Doc::Sles(v) => format!("SUSE Linux Enterprise Server {v} release notes"),
        }
    }

    fn cache_name(&self) -> String {
        match self {
            Doc::Leap(v) => format!("leap-{v}.html"),
            Doc::Sles(v) => format!("sles-{v}.html"),
        }
    }

    /// The installed notes for this release: release-notes-openSUSE-Leap-16.1
    /// and the like (from github.com/SUSE/release-notes), else the older
    /// single release-notes-openSUSE if it is for this release (the one in
    /// Leap 16.0 carried Tumbleweed's notes from 2018).
    fn package(&self) -> Option<(String, String)> {
        let Doc::Leap(v) = self else { return None };
        let dir = "/usr/share/doc/release-notes/openSUSE";
        [format!("{dir}/Leap-{v}/RELEASE-NOTES.en.html"), format!("{dir}/RELEASE-NOTES.en.html")].into_iter().find_map(|path| {
            let html = std::fs::read_to_string(&path).ok()?;
            html.contains(&format!("Leap {v}")).then_some((html, path))
        })
    }
}

/// What's new for this system, ready to show.
#[derive(Clone, Debug, PartialEq)]
pub struct Notes {
    /// "Leap 16.1".
    pub release: String,
    /// Which notes the entries come from, e.g. "Leap 16.1 release notes".
    pub source: String,
    /// Something to know about the source (another product's notes, or
    /// none could be read).
    pub caveat: Option<String>,
    /// The full notes online.
    pub online: String,
    /// The full notes on this machine, when there is a copy.
    pub local: Option<String>,
    pub entries: Vec<NoteEntry>,
    /// Entries in the whole notes.
    pub total: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoteEntry {
    pub title: String,
    /// Why it concerns this system, e.g. "you have an NVIDIA graphics card".
    pub why: String,
    /// The entry itself, online or in the copy on this machine.
    pub link: String,
    /// Its first lines, for a summary.
    pub excerpt: String,
}

impl Notes {
    /// Plain text: the entries with their links, for the model and /commands.
    pub fn listing(&self) -> String {
        let mut out = format!("What's new in {} for this system, from the {}.", self.release, self.source);
        if let Some(c) = &self.caveat {
            out.push_str(&format!(" {c}"));
        }
        for e in &self.entries {
            out.push_str(&format!("\n\n- {} ({})\n  {}\n  {}", e.title, e.why, e.excerpt, e.link));
        }
        out.push_str(&format!("\n\nFull notes: {}", self.online));
        if let Some(l) = &self.local {
            out.push_str(&format!(" (on this machine: {l})"));
        }
        out
    }
}

/// Loads the notes for `target` and picks the entries for this system.
/// Without notes, the result has no entries and says where to read them.
pub async fn load(ctx: &Context, target: &Target) -> Notes {
    let docs = match target {
        Target::Leap(v) => vec![Doc::Leap(v.clone()), Doc::Sles(v.clone())],
        Target::Sles(v) => vec![Doc::Sles(v.clone())],
    };
    let facts = Facts::gather(ctx).await;
    let release = target.name();
    let override_path = std::env::var("YAIST_RELEASE_NOTES").ok().filter(|p| !p.is_empty());
    for (i, doc) in docs.iter().enumerate() {
        let found = match (&override_path, i) {
            (Some(p), 0) => std::fs::read_to_string(p).ok().map(|html| Page { html, local: Some(p.clone()), from_server: false }),
            _ => page(doc).await,
        };
        let Some(Page { html, local, from_server }) = found else { continue };
        let sections = parse(&html);
        if sections.is_empty() {
            continue;
        }
        let other_product = matches!((target, doc), (Target::Leap(_), Doc::Sles(_)));
        let caveat = other_product.then(|| {
            format!("{release}'s own notes aren't published yet; these are from {}, which {release} is built on.", doc.name().trim_end_matches(" release notes"))
        });
        // Notes that came only from this machine may not be online yet:
        // entries link into the local copy, "online" to the list of notes.
        let (base, online) = match (&local, from_server) {
            (Some(path), false) => (format!("file://{path}"), ONLINE_INDEX.to_owned()),
            _ => (doc.url(), doc.url()),
        };
        return Notes {
            // Leap's notes are SLES's with a Leap overlay: SLES's
            // enterprise-only entries are in them too.
            entries: relevant(&sections, &facts, matches!(target, Target::Leap(_)), &base),
            total: sections.len(),
            release,
            source: doc.name(),
            caveat,
            online,
            local,
        };
    }
    let first = &docs[0];
    Notes {
        caveat: Some(format!("The release notes couldn't be read (no network, or not published yet). They will be at {}.", first.url())),
        release,
        source: first.name(),
        online: first.url(),
        local: None,
        entries: Vec::new(),
        total: 0,
    }
}

/// Where all release notes are listed online.
const ONLINE_INDEX: &str = "https://doc.opensuse.org/release-notes/";

/// A release-notes page as read.
struct Page {
    html: String,
    /// The copy on this machine: the cache, the package or a test file.
    local: Option<String>,
    /// It is (or was) on the server, so links to it work.
    from_server: bool,
}

/// A fresh cached copy, else the server (kept in the cache), else the
/// installed package, else an old cached copy.
async fn page(doc: &Doc) -> Option<Page> {
    let cached = cache_dir().map(|d| d.join(doc.cache_name()));
    let read = |p: &PathBuf| std::fs::read_to_string(p).ok().map(|html| Page { html, local: Some(p.display().to_string()), from_server: true });
    if let Some(p) = &cached {
        let fresh = std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < FRESH);
        if fresh {
            if let Some(found) = read(p) {
                return Some(found);
            }
        }
    }
    if let Some(html) = fetch(&doc.url()).await {
        let local = cached.as_ref().and_then(|p| {
            std::fs::create_dir_all(p.parent()?).ok()?;
            std::fs::write(p, &html).ok()?;
            Some(p.display().to_string())
        });
        return Some(Page { html, local, from_server: true });
    }
    if let Some((html, path)) = doc.package() {
        return Some(Page { html, local: Some(path), from_server: false });
    }
    cached.as_ref().and_then(read)
}

async fn fetch(url: &str) -> Option<String> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let html = resp.text().await.ok()?;
    // A page without headings is an error page, not notes.
    html.contains("class=\"title\"").then_some(html)
}

fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("yaist/release-notes"))
}

/// One section of the notes.
#[derive(Clone, Debug, PartialEq)]
struct Section {
    level: u8,
    title: String,
    /// The anchor, for links.
    id: String,
    /// The section's own text, without its subsections.
    text: String,
    /// The titles of the sections it is in, outermost first.
    parents: Vec<String>,
}

/// The sections of a release-notes page. Both doc.opensuse.org and
/// documentation.suse.com (DAPS output) give each heading `class="title"`
/// and a permalink to its anchor; that is all we rely on.
fn parse(html: &str) -> Vec<Section> {
    // Headings and where their text starts and ends.
    let mut heads = Vec::new();
    let mut at = 0;
    while let Some(off) = html[at..].find("<h") {
        let start = at + off;
        at = start + 2;
        let Some(level) = html[at..].chars().next().and_then(|c| c.to_digit(10)).filter(|l| (1..=6).contains(l)) else { continue };
        let Some(tag_end) = html[start..].find('>').map(|e| start + e + 1) else { break };
        if !html[start..tag_end].contains("class=\"title\"") {
            continue;
        }
        let close = format!("</h{level}>");
        let Some(end) = html[tag_end..].find(&close).map(|e| tag_end + e) else { break };
        let inner = &html[tag_end..end];
        let id = inner.split("href=\"").skip(1).find_map(|h| h.split('"').next()?.split_once('#').map(|(_, id)| id.to_owned()));
        if let Some(id) = id.filter(|i| !i.is_empty()) {
            let title = text(inner);
            let title = title.trim_end_matches('#').trim();
            // "2.3.1 NVIDIA …": the number goes.
            let title = match title.split_once(' ') {
                Some((n, rest)) if n.chars().all(|c| c.is_ascii_digit() || c == '.') => rest.trim(),
                _ => title,
            };
            heads.push((level as u8, title.to_owned(), id, start, end + close.len()));
        }
        at = end;
    }
    // The page's end: navigation and scripts after the last section.
    let last = heads.last().map_or(0, |h| h.4);
    let stop = ["<footer", "</main>", "<script"].iter().filter_map(|m| html[last..].find(m).map(|i| last + i)).min().unwrap_or(html.len());
    let mut out: Vec<Section> = Vec::new();
    let mut stack: Vec<(u8, String)> = Vec::new();
    for (i, (level, title, id, _, body_start)) in heads.iter().enumerate() {
        let body_end = heads.get(i + 1).map_or(stop, |h| h.3).max(*body_start);
        while stack.last().is_some_and(|(l, _)| l >= level) {
            stack.pop();
        }
        out.push(Section {
            level: *level,
            title: title.clone(),
            id: id.clone(),
            text: text(&html[*body_start..body_end]),
            parents: stack.iter().map(|(_, t)| t.clone()).collect(),
        });
        stack.push((*level, title.clone()));
    }
    // The document's own title is not an entry.
    out.retain(|s| s.level > 1);
    out
}

/// Text without tags, entities decoded, whitespace collapsed.
fn text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = decode(&out);
    let words: Vec<&str> = out.split_whitespace().collect();
    // Spaces left by tags before punctuation: "Leap 16.0 ." → "Leap 16.0."
    words.join(" ").replace(" .", ".").replace(" ,", ",").replace(" )", ")").replace("( ", "(")
}

fn decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest[..rest.len().min(10)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let c = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            _ => name
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// What the entries are matched against.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub arch: String,
    /// "nvidia", "amd", "intel".
    pub gpus: Vec<&'static str>,
    /// Desktop names as in $XDG_CURRENT_DESKTOP, lowercase ("kde", "gnome").
    pub desktops: Vec<String>,
    /// Installed package names.
    pub packages: HashSet<String>,
}

impl Facts {
    /// From this machine, without root: /sys, the environment, rpm.
    pub async fn gather(ctx: &Context) -> Self {
        let mut gpus = Vec::new();
        if let Ok(cards) = std::fs::read_dir("/sys/class/drm") {
            for card in cards.flatten() {
                let vendor = std::fs::read_to_string(card.path().join("device/vendor")).unwrap_or_default();
                let gpu = match vendor.trim() {
                    "0x10de" => "nvidia",
                    "0x1002" => "amd",
                    "0x8086" => "intel",
                    _ => continue,
                };
                if !gpus.contains(&gpu) {
                    gpus.push(gpu);
                }
            }
        }
        let desktops = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').map(|d| d.trim().to_lowercase()).filter(|d| !d.is_empty()).collect();
        let packages = ctx.run(&["/usr/bin/rpm", "-qa", "--qf", "%{NAME}\\n"]).await.unwrap_or_default().lines().map(str::to_owned).collect();
        Facts { arch: std::env::consts::ARCH.to_owned(), gpus, desktops, packages }
    }
}

/// Words in titles that are package names too, but say nothing about one.
const NOT_PACKAGES: &[&str] = &[
    "about", "added", "and", "available", "boot", "change", "changes", "data", "default", "error", "file", "files", "for", "group", "image", "images",
    "info", "filesystem", "format", "install", "installation", "legacy", "less", "minimal", "library", "login", "make", "messages", "mode", "network", "new", "not", "notes", "now", "package",
    "packages", "release", "removal", "screen", "server", "support", "system", "systems", "the", "time", "tool", "tools", "update", "updated", "updates",
    "user", "users", "using", "version", "which", "with",
];

/// Names followed by a version in titles that are not packages.
const PRODUCTS: &[&str] = &["leap", "sles", "sle", "opensuse", "suse", "tumbleweed", "slowroll", "linux", "kernel", "python", "gcc", "perl", "sp"];

/// The package a title gives a version of ("dovecot 2.4 configuration
/// upgrade"), when it is not installed: the entry is not for this system.
fn about_missing_package(title: &str, facts: &Facts) -> bool {
    let words: Vec<&str> = title.split(|c: char| c.is_whitespace() || "(),:;".contains(c)).filter(|w| !w.is_empty()).collect();
    words.windows(2).any(|w| {
        let version = w[1].starts_with(|c: char| c.is_ascii_digit()) && w[1].chars().all(|c| c.is_ascii_digit() || c == '.');
        version && !NOT_PACKAGES.contains(&w[0]) && !PRODUCTS.contains(&w[0]) && !facts.packages.contains(w[0])
    })
}

/// Entries only enterprise customers need, skipped for Leap: in the SLES
/// notes standing in for Leap's, and in the SLE chapters of Leap's own.
const ENTERPRISE: &[&str] = &[
    "sap",
    "rmt",
    "repository mirroring tool",
    "support statement",
    "general support",
    "specific contracts",
    "agpl",
    "technology preview",
    "technology previews",
    "real-time",
    "kabi",
    "bci",
    "customer center",
    "containerdisk",
    "gpu operator",
    "kubernetes",
    "quarterly updates",
    // SLES's own upgrade paths; Leap's is the one being run.
    "supported upgrade",
    "lifecycle",
    "support and lifecycle",
];

/// Architecture-specific chapters: (arch, words naming it).
const ARCHES: &[(&str, &[&str])] =
    &[("s390x", &["s390x", "ibm z"]), ("powerpc64", &["ppc64le", "power-specific"]), ("aarch64", &["aarch64", "arm64", "arm-specific"])];

/// `phrase` as whole words in `text` (both lowercase).
fn has(text: &str, phrase: &str) -> bool {
    let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
    text.match_indices(phrase).any(|(i, _)| boundary(text[..i].chars().next_back()) && boundary(text[i + phrase.len()..].chars().next()))
}

/// The entries that concern this system, the most important first: about
/// upgrading, then hardware, desktop and installed packages.
fn relevant(sections: &[Section], facts: &Facts, for_leap: bool, page: &str) -> Vec<NoteEntry> {
    let desktop_names: &[(&str, &[&str], &str)] = &[
        ("kde", &["kde", "plasma"], "KDE Plasma"),
        ("gnome", &["gnome"], "GNOME"),
        ("xfce", &["xfce"], "Xfce"),
        ("lxqt", &["lxqt"], "LXQt"),
        ("mate", &["mate"], "MATE"),
        ("cinnamon", &["cinnamon"], "Cinnamon"),
    ];
    let gpu_names: &[(&str, &[&str], &str)] = &[
        ("nvidia", &["nvidia"], "an NVIDIA graphics card"),
        ("amd", &["amd gpu", "amdgpu", "radeon"], "an AMD graphics card"),
        ("intel", &["intel gpu", "intel graphics", "i915"], "Intel graphics"),
    ];
    let mut picked: Vec<(u8, NoteEntry)> = Vec::new();
    for s in sections {
        // Too short to say anything, or only pointers to other entries
        // ("Section 7.41, “…”").
        if s.text.len() < 40 || s.text.starts_with("Section ") {
            continue;
        }
        let title = s.title.to_lowercase();
        let parents = s.parents.join(" / ").to_lowercase();
        let in_title = |w: &str| has(&title, w) || has(&parents, w);
        let other_arch = ARCHES.iter().any(|(arch, words)| *arch != facts.arch && words.iter().any(|w| in_title(w)));
        if other_arch || (for_leap && ENTERPRISE.iter().any(|w| in_title(w))) || about_missing_package(&title, facts) {
            continue;
        }
        let own_arch = ARCHES.iter().any(|(arch, words)| *arch == facts.arch && words.iter().any(|w| in_title(w)));
        let why: Option<(u8, String)> = if ["upgrade", "upgrading", "migration", "migrating"].iter().any(|w| in_title(w)) {
            Some((0, "about upgrading".into()))
        } else if own_arch {
            Some((1, format!("for {} machines", facts.arch)))
        } else if let Some((_, _, what)) =
            gpu_names.iter().find(|(gpu, words, _)| facts.gpus.contains(gpu) && words.iter().any(|w| has(&title, w)))
        {
            Some((1, format!("you have {what}")))
        } else if let Some((_, _, what)) = desktop_names.iter().find(|(d, words, _)| facts.desktops.iter().any(|x| x == d) && words.iter().any(|w| has(&title, w)))
        {
            Some((2, format!("you use {what}")))
        } else {
            title
                .split(|c: char| c.is_whitespace() || "(),:;\"'’“”/".contains(c))
                .map(|w| w.trim_end_matches('.'))
                .find(|w| w.len() >= 3 && !NOT_PACKAGES.contains(w) && facts.packages.contains(*w))
                .map(|p| (3, format!("you have {p} installed")))
        };
        let Some((rank, why)) = why else { continue };
        let page = page.split('#').next().unwrap_or(page);
        picked.push((rank, NoteEntry { title: s.title.clone(), why, link: format!("{page}#{}", s.id), excerpt: excerpt(&s.text) }));
    }
    picked.sort_by_key(|(rank, _)| *rank);
    picked.into_iter().map(|(_, e)| e).take(MAX_ENTRIES).collect()
}

fn excerpt(text: &str) -> String {
    if text.len() <= EXCERPT {
        return text.to_owned();
    }
    let cut = text[..EXCERPT].rfind(' ').unwrap_or(EXCERPT);
    format!("{}…", &text[..cut])
}

/// The prompt for a summary of `notes`: our selection, the model's words.
pub fn summary_prompt(notes: &Notes) -> (String, String) {
    let mut system = format!(
        "You tell a user what is new in {release} while the upgrade to it runs. Below are the release-notes entries that concern their system, picked by YAiST, each with why it concerns them. Summarise them in plain language: at most 6 short bullet points, the most important first, each saying what changes for them or what they should do. A plain version update gets a few words, not advice. Use only the entries given and invent nothing. No links, no headings, no introduction.",
        release = notes.release
    );
    if notes.caveat.is_some() && notes.release.starts_with("Leap") {
        system.push_str(&format!(
            " The entries come from the SUSE Linux Enterprise Server notes, but the user runs {release}: call it {release}, and leave out what only concerns other products (SLES, SLES for SAP, SLE Micro, SLE HA) or upgrade paths from them.",
            release = notes.release
        ));
    }
    let mut user = String::new();
    if let Some(c) = &notes.caveat {
        user.push_str(&format!("Note: {c}\n\n"));
    }
    for e in &notes.entries {
        user.push_str(&format!("### {} ({})\n{}\n\n", e.title, e.why, e.excerpt));
    }
    (system, user)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The two markups, cut down: doc.opensuse.org (Leap) and
    // documentation.suse.com (SLES).
    const LEAP: &str = r##"<html><head><title>openSUSE Leap Release Notes</title></head><body>
<div class="article"><div class="titlepage"><h1 class="title">openSUSE Leap Release Notes <a class="permalink" href="index.html#id-1">#</a></h1></div>
<div class="sect1" id="id-general"><div class="titlepage"><div><div><h2 class="title"><span class="number">2 </span><span class="name">General</span> <a title="Permalink" class="permalink" href="index.html#id-general">#</a></h2></div></div></div>
<p>Changes in this release of openSUSE Leap 16.0 that concern everyone using it.</p>
<div class="sect2" id="id-migration"><div class="titlepage"><h3 class="title"><span class="number">2.2 </span><span class="name">Migration from Leap 15.6</span> <a class="permalink" href="index.html#id-migration">#</a></h3></div>
<p>Use <code>opensuse-migration-tool</code> to move from Leap 15.6 &amp; keep your data.</p></div>
<div class="sect2" id="id-nvidia"><div class="titlepage"><h3 class="title"><span class="number">2.5 </span><span class="name">Automated NVIDIA Driver and Repository Setup</span> <a class="permalink" href="index.html#id-nvidia">#</a></h3></div>
<p>The NVIDIA repository is added and the open driver installed when a supported card is found.</p></div>
<div class="sect2" id="id-pipewire"><div class="titlepage"><h3 class="title"><span class="number">2.11 </span><span class="name">PipeWire replaces PulseAudio</span> <a class="permalink" href="index.html#id-pipewire">#</a></h3></div>
<div class="admonition"><h6>Warning</h6><p>Users upgrading from previous releases should be automatically migrated from PulseAudio.</p></div></div>
<div class="sect2" id="id-hexchat"><div class="titlepage"><h3 class="title"><span class="number">2.12 </span><span class="name">Hexchat drop</span> <a class="permalink" href="index.html#id-hexchat">#</a></h3></div>
<p>Hexchat is no longer part of the distribution; use another IRC client instead of it.</p></div></div>
<div class="sect1" id="id-s390"><div class="titlepage"><h2 class="title"><span class="number">3.12 </span><span class="name">IBM Z-specific changes (s390x)</span> <a class="permalink" href="index.html#id-s390">#</a></h2></div>
<p>Hardware support for pulseaudio on the mainframe, which nobody needs on a laptop.</p></div>
</div><footer>openSUSE footer with pipewire mentioned</footer><script>var x = "<h2 class=\"title\">";</script></body></html>"##;

    const SLES: &str = r##"<html><body><main><section class="article" id="id-1"><div class="titlepage"><h1 class="title"><span class="title-name">SUSE Linux Enterprise Server Release Notes</span> <a class="permalink" href="index.html#id-1" title="Permalink">#</a></h1>
<div class="productname">SUSE Linux Enterprise Server</span> 16.1</div></div>
<section class="sect1" data-id-title="Support and lifecycle" id="v161-support"><div class="titlepage"><div class="title-container"><h2 class="title"><span class="title-number-name"><span class="title-number">3 </span><span class="title-name">Support and lifecycle</span></span> <a class="permalink" href="index.html#v161-support" title="Permalink">#</a></h2></div></div>
<p>SUSE Linux Enterprise Server comes with long-term support for subscribers.</p>
<section class="sect2" id="jsc-PED-1"><div class="titlepage"><h3 class="title"><span class="title-number">3.4 </span><span class="title-name">SMB over QUIC support in Samba (technology preview)</span> <a class="permalink" href="index.html#jsc-PED-1">#</a></h3></div>
<p>Samba can now serve SMB over QUIC as a technology preview for testing.</p></section></section>
<section class="sect1" id="v161-changes"><div class="titlepage"><h2 class="title"><span class="title-number">7 </span><span class="title-name">Changes affecting all architectures</span> <a class="permalink" href="index.html#v161-changes">#</a></h2></div>
<section class="sect2" id="jsc-PED-2"><div class="titlepage"><h3 class="title"><span class="title-number">7.33 </span><span class="title-name">PHP updated to version 8.5</span> <a class="permalink" href="index.html#jsc-PED-2">#</a></h3></div>
<p>PHP is now version 8.5; check your applications for deprecated functions before upgrading them.</p></section>
<section class="sect2" id="jsc-PED-3"><div class="titlepage"><h3 class="title"><span class="title-number">7.25 </span><span class="title-name">SAP patterns updated</span> <a class="permalink" href="index.html#jsc-PED-3">#</a></h3></div>
<p>The SAP patterns now pull in php for the web dispatcher and a lot more.</p></section></section>
</section></main><footer>SUSE</footer></body></html>"##;

    fn facts() -> Facts {
        Facts {
            arch: "x86_64".into(),
            gpus: vec!["nvidia"],
            desktops: vec!["kde".into()],
            packages: ["pipewire", "pulseaudio", "php8", "php", "samba", "update", "hexchat-not"].into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn leap_sections() {
        let s = parse(LEAP);
        let titles: Vec<&str> = s.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["General", "Migration from Leap 15.6", "Automated NVIDIA Driver and Repository Setup", "PipeWire replaces PulseAudio", "Hexchat drop", "IBM Z-specific changes (s390x)"]);
        assert_eq!(s[1].id, "id-migration");
        assert_eq!(s[1].parents, ["openSUSE Leap Release Notes", "General"]);
        assert_eq!(s[1].text, "Use opensuse-migration-tool to move from Leap 15.6 & keep your data.");
        // The admonition's heading is not a section; the footer is not text.
        assert!(s[3].text.contains("Warning Users upgrading"), "{}", s[3].text);
        assert!(!s.last().unwrap().text.contains("footer"));
    }

    #[test]
    fn sles_sections() {
        let s = parse(SLES);
        let titles: Vec<&str> = s.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Support and lifecycle", "SMB over QUIC support in Samba (technology preview)", "Changes affecting all architectures", "PHP updated to version 8.5", "SAP patterns updated"]);
        assert_eq!(s[3].id, "jsc-PED-2");
    }

    #[test]
    fn entries_for_this_system() {
        let page = "https://example.org/notes/index.html";
        let e = relevant(&parse(LEAP), &facts(), false, page);
        let got: Vec<(&str, &str)> = e.iter().map(|e| (e.title.as_str(), e.why.as_str())).collect();
        assert_eq!(
            got,
            [
                ("Migration from Leap 15.6", "about upgrading"),
                ("Automated NVIDIA Driver and Repository Setup", "you have an NVIDIA graphics card"),
                ("PipeWire replaces PulseAudio", "you have pipewire installed"),
            ]
        );
        assert_eq!(e[0].link, "https://example.org/notes/index.html#id-migration");
        // Without the NVIDIA card, that entry goes.
        let mut f = facts();
        f.gpus = vec!["intel"];
        assert!(relevant(&parse(LEAP), &f, false, page).iter().all(|e| !e.title.contains("NVIDIA")));
        // On a mainframe, its chapter counts.
        f.arch = "s390x".into();
        assert!(relevant(&parse(LEAP), &f, false, page).iter().any(|e| e.title.contains("IBM Z")));
    }

    #[test]
    fn leap_skips_enterprise_entries() {
        let e = relevant(&parse(SLES), &facts(), true, "https://example.org/");
        let got: Vec<&str> = e.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(got, ["PHP updated to version 8.5"]);
        // As SLES's own notes, Samba's preview counts.
        assert!(relevant(&parse(SLES), &facts(), false, "https://example.org/").iter().any(|e| e.title.contains("Samba")));
    }

    #[test]
    fn versions_of_missing_packages() {
        let f = facts();
        assert!(about_missing_package("dovecot 2.4 configuration upgrade", &f));
        assert!(!about_missing_package("php 8.5 configuration upgrade", &f));
        assert!(!about_missing_package("migration from leap 15.6", &f));
        assert!(!about_missing_package("php updated to version 8.5", &f));
    }

    #[test]
    fn whole_words_only() {
        assert!(has("sap patterns updated", "sap"));
        assert!(!has("entries disappear", "sap"));
        assert!(has("ibm z-specific changes (s390x)", "ibm z"));
        assert!(!has("powerful tools", "power"));
    }

    #[test]
    fn urls_and_targets() {
        assert_eq!(Doc::Leap("16.0".into()).url(), "https://doc.opensuse.org/release-notes/x86_64/openSUSE/Leap/16.0/html/release-notes-leap-160/index.html");
        assert_eq!(Doc::Sles("16.1".into()).url(), "https://documentation.suse.com/releasenotes/sles/html/releasenotes_sles_16.1/index.html");
        assert_eq!(Target::of_upgrade("leap:16.1"), Some(Target::Leap("16.1".into())));
        assert_eq!(Target::of_upgrade("tumbleweed"), None);
        assert_eq!(decode("a &amp; b &#8217; &#x2014; &bogus; &"), "a & b ’ — &bogus; &");
    }

    #[test]
    fn excerpts_end_on_a_word() {
        let long = "word ".repeat(200);
        let e = excerpt(&long);
        assert!(e.len() <= EXCERPT + 4 && e.ends_with("word…"), "{e}");
    }
}

/// `YAIST_RELEASE_NOTES=page.html cargo test -p yaist-modules real_page -- --ignored --nocapture`
/// shows how a real page is read.
#[cfg(test)]
mod real {
    #[test]
    #[ignore]
    fn real_page() {
        let html = std::fs::read_to_string(std::env::var("YAIST_RELEASE_NOTES").expect("YAIST_RELEASE_NOTES")).unwrap();
        for s in super::parse(&html) {
            println!("{}{} #{} ({} chars) [{}]", "  ".repeat(s.level as usize - 2), s.title, s.id, s.text.len(), s.parents.join(" / "));
        }
    }
}
