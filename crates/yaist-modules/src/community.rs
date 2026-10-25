//! The openSUSE community: where to talk, thank people, find events and help,
//! and see how openSUSE is built and tested. The links are a fixed, vetted
//! list; Open only ever launches one of them.

use yaist_core::{ActionSpec, ModuleSpec, Output, Param, Registry, Risk, Table};

#[derive(Clone, Copy, Debug)]
pub struct Link {
    pub group: &'static str,
    pub title: &'static str,
    pub text: &'static str,
    pub url: &'static str,
}

pub const GROUPS: &[&str] = &["Talk", "Events & news", "Help", "Build & test"];

pub const LINKS: &[Link] = &[
    Link { group: "Talk", title: "Chat", text: "Join the chat: Matrix rooms for users and contributors", url: "https://chat.opensuse.org/" },
    Link { group: "Talk", title: "Virtual bar", text: "Drop into the 24×7 openSUSE bar on Jitsi, any time", url: "https://meet.opensuse.org/bar" },
    Link {
        group: "Talk",
        title: "Kudos",
        text: "Recognize others for their work on openSUSE, or for helping you",
        url: "https://kudos.opensuse.org/",
    },
    Link { group: "Talk", title: "Forums", text: "Ask questions and help others", url: "https://forums.opensuse.org/" },
    Link { group: "Events & news", title: "Calendar", text: "Public events, meetings and conferences", url: "https://calendar.opensuse.org/" },
    Link { group: "Events & news", title: "News", text: "Project news and announcements", url: "https://news.opensuse.org/" },
    Link { group: "Help", title: "Documentation", text: "Start-up guide, reference and how-tos", url: "https://doc.opensuse.org/" },
    Link { group: "Help", title: "Release notes", text: "What is new and what to watch out for", url: "https://doc.opensuse.org/release-notes/" },
    Link { group: "Help", title: "Wiki", text: "Community knowledge and support database", url: "https://en.opensuse.org/Portal" },
    Link { group: "Help", title: "Report a bug", text: "Bugzilla, where problems get fixed", url: "https://bugzilla.opensuse.org/" },
    Link {
        group: "Build & test",
        title: "Open Build Service",
        text: "Build and share packages for openSUSE and more",
        url: "https://build.opensuse.org/",
    },
    Link { group: "Build & test", title: "openQA", text: "Automated testing of every openSUSE release", url: "https://openqa.opensuse.org/" },
    Link { group: "Build & test", title: "Software", text: "Find packages, including community builds", url: "https://software.opensuse.org/" },
    Link {
        group: "Build & test",
        title: "osdiff",
        text: "Software versions across openSUSE distributions",
        url: "https://opensuse.github.io/osdiff/",
    },
];

/// Shown on the welcome screen.
pub const FEATURED: &[&str] = &["Chat", "Virtual bar", "Calendar", "Kudos"];

pub fn link(title: &str) -> Option<&'static Link> {
    LINKS.iter().find(|l| l.title == title)
}

fn known_link(s: &str) -> Result<(), String> {
    if LINKS.iter().any(|l| l.url == s) {
        Ok(())
    } else {
        Err("not one of the community links".into())
    }
}

const LIST: ActionSpec = ActionSpec {
    id: "community.links",
    title: "Community",
    description: "List openSUSE community places: chat (Matrix), the 24x7 virtual bar (Jitsi), Kudos to thank people, forums, the events calendar, news, documentation, wiki, bug reports, the Open Build Service, openQA, software search and osdiff.",
    risk: Risk::Read,
    params: &[],
};

const OPEN: ActionSpec = ActionSpec {
    id: "community.visit",
    title: "Visit",
    description: "Open one of the community links in the web browser.",
    risk: Risk::Read,
    params: &[Param::text("link", "The link's URL, as listed").check(known_link)],
};

pub fn register(reg: &mut Registry) {
    reg.module(ModuleSpec {
        id: "community",
        title: "Community",
        tagline: "Chat, events, help, kudos",
        overview: LIST.id,
        row_actions: &[OPEN.id],
        overview_needs_admin: false,
    });
    reg.action(LIST, |_, _| async move { Ok(list()) });
    reg.action(OPEN, |ctx, args| async move {
        ctx.open_in_browser(args.str("link")).await?;
        Ok(Output::Done(format!("Opened {}", args.str("link"))))
    });
    reg.shell(LIST.id, |_, _| vec!["xdg-open https://en.opensuse.org/Portal".into()]);
    reg.shell(OPEN.id, |a, _| vec![format!("xdg-open {}", a.str("link"))]);
}

fn list() -> Output {
    let mut t = Table::new(&[("group", "Group"), ("title", "Place"), ("link", "Link"), ("description", "What for")]);
    for l in LINKS {
        t.push(vec![l.group.into(), l.title.into(), l.url.into(), l.text.into()]);
    }
    Output::Table(t)
}
