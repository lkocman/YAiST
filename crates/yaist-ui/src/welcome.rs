//! The welcome screen, shown first: a greeting, starting points, questions to
//! try, the system at a glance and pointers into the openSUSE community. It
//! takes over from openSUSE-welcome. Both clients render this content.

use yaist_core::Distro;

/// Every openSUSE flavour installs its logos here, under the same file names
/// (distribution-logos-openSUSE-*). They are loaded from the system, not
/// shipped with YAiST.
const LOGO_DIR: &str = "/usr/share/pixmaps/distribution-logos";

/// The colour square logo, for the welcome screen.
pub const LOGO_SQUARE: &str = "square-hicolor.svg";
/// The monochrome wordmark (e.g. "◇ Leap"), for the header; tint it.
pub const LOGO_INLINE: &str = "light-inline.svg";

/// A `file://` URI for one of the system's distribution logos, if installed.
pub fn distribution_logo(file: &str) -> Option<String> {
    let path = std::path::Path::new(LOGO_DIR).join(file);
    path.is_file().then(|| format!("file://{}", path.display()))
}

/// Artwork for the welcome screen, as a `file://` URI and whether it carries
/// the openSUSE wordmark itself (then the title shows only the release):
/// `$YAIST_WELCOME_ART`, else the pirate Geeko from openSUSE-welcome
/// (installed in /usr/share/yaist/art, or in the repository next to a
/// development build). It is a separate GPL-3.0-or-later file, loaded at run
/// time and never compiled in. None: the drawn chameleon.
pub fn welcome_art() -> Option<(String, bool)> {
    let uri = |p: &std::path::Path| format!("file://{}", p.display());
    if let Some(custom) = std::env::var_os("YAIST_WELCOME_ART").map(std::path::PathBuf::from).filter(|p| p.is_file()) {
        return Some((uri(&custom), true));
    }
    let dev = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.parent()?.parent()?.join("assets/art/sinking.svg")));
    [Some(std::path::PathBuf::from("/usr/share/yaist/art/sinking.svg")), dev].into_iter().flatten().find(|p| p.is_file()).map(|p| (uri(&p), false))
}

/// Whether the system still gets updates, shown next to the mode tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Support {
    /// A rolling release: no end of life.
    Rolling,
    /// Supported; `soon` when less than 90 days are left.
    Until {
        date: String,
        days_left: i64,
        soon: bool,
    },
    EndOfLife {
        since: String,
    },
    Unknown,
}

pub fn support(d: &Distro, today: i64) -> Support {
    if d.rolling {
        return Support::Rolling;
    }
    match d.end_of_life.as_deref().and_then(|e| yaist_core::date::parse(e).map(|days| (e, days))) {
        Some((e, days)) if days < today => Support::EndOfLife { since: e.to_owned() },
        Some((e, days)) => Support::Until { date: e.to_owned(), days_left: days - today, soon: days - today < 90 },
        None => Support::Unknown,
    }
}

impl Support {
    /// The tag: SUPPORTED or END OF LIFE (none when unknown).
    pub fn label(&self) -> Option<&'static str> {
        match self {
            Support::EndOfLife { .. } => Some("End of life"),
            Support::Unknown => None,
            _ => Some("Supported"),
        }
    }

    /// One sentence for under the tags.
    pub fn sentence(&self) -> String {
        use yaist_core::date::pretty;
        match self {
            Support::Rolling => "Rolling release: no end of life.".into(),
            Support::Until { date, days_left, soon: true } => format!("Support ends {} ({days_left} days left); plan an upgrade.", pretty(date)),
            Support::Until { date, .. } => format!("Supported until {}.", pretty(date)),
            Support::EndOfLife { since } => format!("No security updates since {}. Upgrade to a supported release.", pretty(since)),
            Support::Unknown => String::new(),
        }
    }
}

/// How this system takes changes, shown under the welcome title:
/// (label, explanation for its tooltip).
pub fn mode(d: &Distro) -> (&'static str, &'static str) {
    if d.transactional {
        if d.rolling {
            ("Immutable mode", "Read-only rolling release: updates and software go into a new snapshot, used after a reboot.")
        } else {
            ("Immutable mode", "Read-only system: updates and software go into a new snapshot, used after a reboot.")
        }
    } else if d.rolling {
        ("Standard mode", "Rolling release: always the latest software, updated with zypper dup.")
    } else {
        ("Standard mode", "Regular release: updates and software install directly with zypper.")
    }
}

/// What a starting point does when chosen.
#[derive(Clone, Copy, Debug)]
pub enum Start {
    /// Run (reads) or propose (changes) an action; a form opens if it needs input.
    Action(&'static str),
    /// Open a module's panel.
    Module(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub struct StartItem {
    pub title: &'static str,
    pub text: &'static str,
    pub start: Start,
}

pub fn start_items(d: &Distro) -> Vec<StartItem> {
    let update = if d.transactional {
        StartItem {
            title: "Install updates",
            text: "Updates go into a new snapshot; reboot to use them, or roll back.",
            start: Start::Action("packages.update_system"),
        }
    } else if d.rolling {
        StartItem {
            title: "Update Tumbleweed",
            text: "Bring the system up to the latest snapshot with zypper dup.",
            start: Start::Action("packages.update_system"),
        }
    } else {
        StartItem {
            title: "Install updates",
            text: "Get the latest maintenance and security updates.",
            start: Start::Action("packages.update_system"),
        }
    };
    let mut items = vec![
        update,
        StartItem {
            title: "Take a snapshot",
            text: "Save the system state with snapper, so you can roll back.",
            start: Start::Action("snapshots.create"),
        },
        StartItem {
            title: "Find software",
            text: "Search the openSUSE repositories and install with one click.",
            start: Start::Action("packages.search"),
        },
        StartItem { title: "Add a user", text: "Create an account for someone else on this computer.", start: Start::Action("users.create") },
        StartItem { title: "Gaming & media", text: "Steam, graphics drivers and codecs, set up for this system.", start: Start::Module("media") },
    ];
    items.push(if d.id == "opensuse-leap" {
        StartItem { title: "New Leap release?", text: "See whether a newer openSUSE Leap is available.", start: Start::Module("upgrade") }
    } else {
        StartItem { title: "Community software", text: "Add repositories from the Open Build Service.", start: Start::Module("repos") }
    });
    items
}

/// Requests to try in the conversation.
pub fn suggestions(d: &Distro) -> Vec<&'static str> {
    let mut s = vec!["Is my system up to date?", "Set up Steam", "Which services failed?", "Install VLC", "Open port 8080 for my dev server"];
    s.push(if d.id == "opensuse-leap" { "Upgrade to the next Leap" } else { "Take a snapshot before I update" });
    s.push("zypper in fish");
    s
}

/// Requests for the conversation's greeting, about the module that is open:
/// the welcome screen (or a module without its own) gets the mixed ones.
pub fn prompts(module: &str, d: &Distro) -> Vec<&'static str> {
    let leap = d.id == "opensuse-leap";
    match module {
        "system" => vec!["What time zone am I in?", "Change the hostname", "Keep the clock in sync", "Reboot the computer"],
        "packages" => vec!["Is my system up to date?", "Install VLC", "Find a PDF editor", "Which Flatpak apps do I have?"],
        "repos" => vec!["Which repositories do I have?", "Add Flathub", "Enable the non-OSS repository", "Add the games repository from OBS"],
        "snapshots" => vec!["Take a snapshot before I update", "What changed in the last snapshot?", "Roll back to yesterday's snapshot", "Which snapshots do I have?"],
        "upgrade" if leap => vec!["Is there a newer Leap?", "Upgrade to the next Leap", "Switch to Tumbleweed", "Move to SUSE Linux Enterprise"],
        "upgrade" => vec!["Where can I upgrade to?", "Update the system"],
        "services" => vec!["Which services failed?", "Is sshd running?", "Restart the printing service", "Which services start at boot?"],
        "firewall" => vec!["Open port 8080 for my dev server", "Allow SSH through the firewall", "Which ports are open?", "What else can I allow?"],
        "users" => vec!["Who has an account here?", "Who can use sudo?", "Let my nephew tom log in, without sudo", "Which groups am I in?"],
        "selinux" => vec!["Is SELinux enforcing?", "Show recent SELinux denials", "Allow the web server to make network connections", "Which SELinux booleans are on?"],
        "printers" => vec!["Find my printer", "Print a test page", "Set up my HP printer", "Cancel all print jobs"],
        "media" => vec!["Set up Steam", "Set up the graphics driver", "Play all video formats", "What's set up for gaming?"],
        "community" => vec!["Where can I chat with other users?", "Any openSUSE events coming up?", "Where do I get help?", "Report a problem"],
        _ => suggestions(d),
    }
}

pub use yaist_modules::community::{link, Link, FEATURED, GROUPS, LINKS};

/// The chameleon for terminals: our own drawing, not the Geeko logo.
pub const ASCII_CHAMELEON: &[&str] = &[
    r"            __.---.__          ",
    r"        _.-'  .  .   '-._      ",
    r"      .'   .   .   .  ( @ )\   ",
    r"     /  .   .   .   .  '-'  >  ",
    r"     \_   .   .   .   _.---'   ",
    r"    (@ '-.__.-.__.-.-'         ",
    r"     '-'  /_/     /_/          ",
    r"   =====================~~     ",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_module_has_its_own_prompts() {
        let leap = Distro { id: "opensuse-leap".into(), ..Default::default() };
        let tw = Distro { id: "opensuse-tumbleweed".into(), rolling: true, ..Default::default() };
        for m in yaist_modules::registry().modules.iter().filter(|m| m.id != "welcome") {
            for d in [&leap, &tw] {
                let p = prompts(m.id, d);
                assert!(!p.is_empty() && p != suggestions(d), "{} on {}", m.id, d.id);
            }
        }
    }
}
