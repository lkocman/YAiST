//! Help the user report a problem, when they decide to: a GitHub issue for
//! YAiST's own mistakes, or an openSUSE Bugzilla report for the distribution.
//! Nothing is filed here. The report opens prefilled in the browser; the
//! user checks for an existing one, completes it and submits it there, so
//! reports stay few, personal and checked.
//!
//! The layout follows openQA's prefilled Bugzilla reports (Observation,
//! Reproducible, Expected result, …); product and version follow
//! https://en.opensuse.org/openSUSE:Submitting_bug_reports.

use reqwest::Url;
use yaist_core::{ActionSpec, Context, Distro, Output, Param, Registry, Risk};

pub const ISSUES: &str = "https://github.com/lkocman/YAiST/issues";
const BUGZILLA: &str = "https://bugzilla.opensuse.org";
/// Browsers and servers take URLs of about 8 KB; the rest stays below that.
const MAX_URL: usize = 7000;

pub const REPORT: ActionSpec = ActionSpec {
    // In Community, next to its link to Bugzilla.
    id: "community.report_problem",
    title: "Report a problem",
    description: "Prepare a bug report: a YAiST issue on GitHub (target yaist) when YAiST got something wrong, or an openSUSE Bugzilla report (target opensuse) for a problem in the distribution or a package. It opens prefilled in the browser; the user completes it there, and nothing is sent until they submit it.",
    // Like opening a community link: it only opens pages in the browser.
    risk: Risk::Read,
    params: &[
        Param::choice("target", &["yaist", "opensuse"], "yaist: YAiST did something wrong; opensuse: a package or the distribution is broken").default("yaist"),
        Param::text("title", "One line saying what is wrong, e.g. 'Update fails: %post script of foo exits 1'"),
        Param::text("observation", "What happened: the steps and the exact error output").multiline(),
        Param::text("expected", "What should have happened").optional(),
        Param::choice("reproducible", &["always", "sometimes", "once", "unknown"], "Does it happen again?").default("unknown"),
    ],
};

pub fn register(reg: &mut Registry) {
    reg.action(REPORT, |ctx, args| async move {
        let report = Report {
            target: if args.str("target") == "opensuse" { Target::OpenSuse } else { Target::Yaist },
            title: args.str("title").to_owned(),
            observation: args.str("observation").to_owned(),
            expected: args.opt_str("expected").unwrap_or("").to_owned(),
            reproducible: args.str("reproducible").to_owned(),
            environment: environment(&ctx).await,
        };
        let form = report.url(&ctx.distro);
        // Only the report: Bugzilla suggests similar reports itself as the
        // summary is typed, and a second tab would hide the first.
        ctx.open_in_browser(form.as_str()).await?;
        let place = match report.target {
            Target::Yaist => "a new YAiST issue on GitHub",
            Target::OpenSuse => "a new openSUSE Bugzilla report",
        };
        let text = format!("Opened {place} in your browser, prefilled. Complete it there and submit it.\nReport: {form}");
        Ok(Output::Text(text))
    });
    reg.shell(REPORT.id, |a, d| {
        let page = if a.str("target") == "opensuse" {
            let (product, ..) = bugzilla_product(d);
            link(&format!("{BUGZILLA}/enter_bug.cgi"), &[("product", &product)]).to_string()
        } else {
            format!("{ISSUES}/new")
        };
        vec!["# check for an existing report first, then:".into(), format!("xdg-open '{page}'")]
    });
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Yaist,
    OpenSuse,
}

struct Report {
    target: Target,
    title: String,
    observation: String,
    expected: String,
    reproducible: String,
    /// "- System: …" lines, from our code.
    environment: Vec<String>,
}

impl Report {
    fn body(&self, observation: &str) -> String {
        let reproducible = match self.reproducible.as_str() {
            "always" => "Every time.",
            "sometimes" => "Sometimes.",
            "once" => "Happened once so far.",
            _ => "(please fill in: does it happen again?)",
        };
        let expected = if self.expected.is_empty() { "(please fill in: what should have happened?)" } else { &self.expected };
        format!(
            "## Observation\n\n{observation}\n\n## Reproducible\n\n{reproducible}\n\n## Expected result\n\n{expected}\n\n## Environment\n\n{}\n\n---\nPrepared with YAiST's report assistant, reviewed and submitted by the reporter.\n",
            self.environment.join("\n")
        )
    }

    /// The prefilled new-report page. Long output is cut from the middle of
    /// the observation, which is where the least important lines are.
    fn url(&self, d: &Distro) -> Url {
        let mut observation = self.observation.clone();
        loop {
            let url = self.url_with(d, &observation);
            if url.as_str().len() <= MAX_URL || observation.len() < 200 {
                return url;
            }
            let keep = observation.len() * 3 / 4;
            let head: String = observation.chars().take(keep / 2).collect();
            let tail: String = observation.chars().rev().take(keep / 2).collect::<Vec<_>>().into_iter().rev().collect();
            observation = format!("{head}\n[… shortened to fit the link; attach the full output …]\n{tail}");
        }
    }

    fn url_with(&self, d: &Distro, observation: &str) -> Url {
        let body = self.body(observation);
        match self.target {
            Target::Yaist => link(&format!("{ISSUES}/new"), &[("title", &self.title), ("body", &body)]),
            Target::OpenSuse => {
                let (product, version, component) = bugzilla_product(d);
                // The prefix tells triage where the report comes from.
                let title = format!("[YAiST] {}", self.title);
                let mut params = vec![("product", product), ("short_desc", title), ("comment", body), ("cf_foundby", "Community User".into())];
                if let Some(v) = version {
                    params.push(("version", v));
                }
                if let Some(c) = component {
                    params.push(("component", c));
                }
                let params: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
                link(&format!("{BUGZILLA}/enter_bug.cgi"), &params)
            }
        }
    }
}

/// `base?k=v&…` encoded the way openQA's prefilled Bugzilla links are:
/// every byte outside A-Z a-z 0-9 - _ . ~ as %XX, so spaces are %20, not
/// '+'. Bugzilla's bot check (Anubis) passes the URL on in a redirect and
/// fails on form-style encoding.
fn link(base: &str, params: &[(&str, &str)]) -> Url {
    let enc = |s: &str| {
        s.bytes()
            .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect::<String>()
    };
    let query = params.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&");
    Url::parse(&format!("{base}?{query}")).expect("static URL")
}

/// Bugzilla product, version and component for this system, as the wiki
/// says, corrected where it is out of date: Leap under "openSUSE
/// Distribution" with its version (triage moves packages inherited from
/// SLE); Tumbleweed and MicroOS (component MicroOS) under "openSUSE
/// Tumbleweed", version Current, and Slowroll there as version Slowroll;
/// Aeon, Kalpa and Leap Micro under their own products.
fn bugzilla_product(d: &Distro) -> (String, Option<String>, Option<String>) {
    let is = |id: &str| d.id == id;
    if is("opensuse-leap") {
        ("openSUSE Distribution".into(), Some(format!("Leap {}", d.version)), None)
    } else if is("opensuse-leap-micro") {
        ("openSUSE Leap Micro".into(), Some(d.version.clone()), None)
    } else if is("opensuse-aeon") {
        ("openSUSE Aeon".into(), Some("Current".into()), None)
    } else if is("opensuse-kalpa") {
        ("openSUSE Kalpa".into(), Some("Current".into()), None)
    } else if d.id.contains("slowroll") || d.pretty.contains("Slowroll") {
        ("openSUSE Tumbleweed".into(), Some("Slowroll".into()), None)
    } else if is("opensuse-microos") {
        ("openSUSE Tumbleweed".into(), Some("Current".into()), Some("MicroOS".into()))
    } else {
        ("openSUSE Tumbleweed".into(), Some("Current".into()), None)
    }
}

/// Facts for the Environment section, gathered by our code.
async fn environment(ctx: &Context) -> Vec<String> {
    let d = &ctx.distro;
    let mut system = if d.pretty.is_empty() { d.id.clone() } else { d.pretty.clone() };
    if let Some(phase) = crate::upgrade::prerelease_phase(d).await {
        system.push_str(&format!(" ({phase})"));
    }
    let mode = if d.transactional { "immutable" } else { "standard" };
    // The version of what is running: the package's when installed, else a
    // development build (an installed package says nothing about it).
    let packaged = std::env::current_exe().is_ok_and(|e| e.starts_with("/usr/"));
    let rpm = if packaged { ctx.run(&["/usr/bin/rpm", "-qf", "--qf", "%{VERSION}-%{RELEASE}", "/usr/bin/yaist"]).await.ok() } else { None };
    let yaist =
        rpm.filter(|v| !v.is_empty() && !v.contains("not owned")).unwrap_or_else(|| format!("{} (development build)", env!("CARGO_PKG_VERSION")));
    vec![format!("- System: {system}, {}, {mode} mode", std::env::consts::ARCH), format!("- YAiST: {yaist}")]
}

/// The likely place for a failure, from its error text, and why: a
/// package's own script or a dependency problem in the repositories is the
/// distribution's; a name that doesn't exist is no one's ("none"); anything
/// else is first YAiST's to look at.
pub fn suggest(error: &str) -> (&'static str, &'static str) {
    let e = error.to_lowercase();
    if not_found(&e) {
        ("none", "the name doesn't exist, perhaps a typo")
    } else if ["package script failed", "scriptlet", "%post", "%pre"].iter().any(|s| e.contains(s)) {
        ("opensuse", "a package's script failed")
    } else if ["unresolved package conflict", "nothing provides", "conflicts with"].iter().any(|s| e.contains(s)) {
        ("opensuse", "a dependency problem in the repositories")
    } else {
        ("yaist", "no sign of a package problem")
    }
}

/// A package, app or file of that name doesn't exist: usually a typo.
fn not_found(e: &str) -> bool {
    // Not a bare "not found": "File … not found on medium" is a mirror problem.
    [
        "nothing matches",
        "not found in package names",
        "no provider of",
        "package not found",
        "no matching",
        "no installed refs found",
        "not found in the user installation",
        "not found in the system installation",
    ]
    .iter()
    .any(|s| e.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distro(id: &str, version: &str, pretty: &str) -> Distro {
        Distro { id: id.into(), version: version.into(), pretty: pretty.into(), ..Default::default() }
    }

    fn report(target: Target, observation: &str) -> Report {
        Report {
            target,
            title: "Update fails: %post of foo".into(),
            observation: observation.into(),
            expected: "The update installs".into(),
            reproducible: "always".into(),
            environment: vec!["- System: openSUSE Leap 16.1 (RC), x86_64, standard mode".into(), "- YAiST: 0.1.0".into()],
        }
    }

    fn param(url: &Url, key: &str) -> Option<String> {
        url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
    }

    #[test]
    fn bugzilla_products_follow_the_wiki() {
        assert_eq!(bugzilla_product(&distro("opensuse-leap", "16.1", "")), ("openSUSE Distribution".into(), Some("Leap 16.1".into()), None));
        assert_eq!(bugzilla_product(&distro("opensuse-tumbleweed", "20261025", "")).1.as_deref(), Some("Current"));
        assert_eq!(bugzilla_product(&distro("opensuse-slowroll", "", "openSUSE Slowroll")).1.as_deref(), Some("Slowroll"));
        assert_eq!(bugzilla_product(&distro("opensuse-microos", "", "")).2.as_deref(), Some("MicroOS"));
        assert_eq!(bugzilla_product(&distro("opensuse-aeon", "", "")).0, "openSUSE Aeon");
        assert_eq!(bugzilla_product(&distro("opensuse-kalpa", "", "")), ("openSUSE Kalpa".into(), Some("Current".into()), None));
        assert_eq!(bugzilla_product(&distro("opensuse-leap-micro", "6.2", "")), ("openSUSE Leap Micro".into(), Some("6.2".into()), None));
    }

    #[test]
    fn bugzilla_url_carries_the_report() {
        let url = report(Target::OpenSuse, "zypper update failed:\nfoo: %post scriptlet failed").url(&distro("opensuse-leap", "16.1", ""));
        assert!(url.as_str().starts_with("https://bugzilla.opensuse.org/enter_bug.cgi?"));
        assert_eq!(param(&url, "product").as_deref(), Some("openSUSE Distribution"));
        assert_eq!(param(&url, "version").as_deref(), Some("Leap 16.1"));
        assert_eq!(param(&url, "cf_foundby").as_deref(), Some("Community User"));
        assert_eq!(param(&url, "short_desc").as_deref(), Some("[YAiST] Update fails: %post of foo"));
        let comment = param(&url, "comment").unwrap();
        assert!(comment.starts_with("## Observation\n\nzypper update failed:\nfoo: %post scriptlet failed\n\n## Reproducible\n\nEvery time."));
        assert!(comment.contains("## Environment\n\n- System: openSUSE Leap 16.1 (RC), x86_64, standard mode\n- YAiST: 0.1.0"));
    }

    #[test]
    fn github_url() {
        let r = report(Target::Yaist, "YAiST could not read zypper's output");
        let d = distro("opensuse-leap", "16.0", "");
        let url = r.url(&d);
        assert!(url.as_str().starts_with("https://github.com/lkocman/YAiST/issues/new?"));
        assert_eq!(param(&url, "title").as_deref(), Some("Update fails: %post of foo"));
    }

    #[test]
    fn links_are_encoded_like_openqa() {
        let url = report(Target::OpenSuse, "a b").url(&distro("opensuse-leap", "16.0", ""));
        assert!(url.as_str().contains("product=openSUSE%20Distribution"), "{url}");
        assert!(url.as_str().contains("short_desc=%5BYAiST%5D%20Update%20fails%3A%20%25post%20of%20foo"), "{url}");
        assert!(!url.as_str().contains('+'), "{url}");
    }

    #[test]
    fn long_output_is_shortened_to_fit_the_link() {
        let log = (0..2000).map(|i| format!("line {i}: something happened")).collect::<Vec<_>>().join("\n");
        let url = report(Target::Yaist, &log).url(&distro("opensuse-leap", "16.0", ""));
        assert!(url.as_str().len() <= MAX_URL, "{}", url.as_str().len());
        let body = param(&url, "body").unwrap();
        assert!(body.contains("line 0:") && body.contains("line 1999:") && body.contains("shortened to fit the link"));
    }

    #[test]
    fn target_suggestion() {
        assert_eq!(suggest("bar-2: %post scriptlet failed"), ("opensuse", "a package's script failed"));
        assert_eq!(suggest("unresolved package conflict, no packages were changed: …").0, "opensuse");
        assert_eq!(suggest("could not read zypper's output: unexpected end").0, "yaist");
        assert_eq!(suggest("error: Nothing matches org.example.DoesNotExist in remote flathub").0, "none");
        assert_eq!(suggest("error: No installed refs found for ‘org.example.NotInstalled’").0, "none");
        assert_eq!(suggest("error: Remote \"x\" not found in the user installation").0, "none");
        assert_eq!(suggest("File './hello-2.12.3-1.4.x86_64.rpm' not found on medium 'dir:/tmp/repo'").0, "yaist");
    }
}
