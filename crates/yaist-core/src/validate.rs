//! Validators for parameters that end up in commands or D-Bus calls.

type R = Result<(), String>;

fn chars_ok(s: &str, extra: &str) -> bool {
    s.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

pub fn username(s: &str) -> R {
    let first_ok = s.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_');
    if !first_ok || s.len() > 32 || !s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
        return Err("use lowercase letters, digits, '_' or '-', starting with a letter (max 32)".into());
    }
    Ok(())
}

/// GECOS field: no colons or newlines.
pub fn full_name(s: &str) -> R {
    if s.contains([':', '\n', ',']) || s.len() > 128 {
        return Err("must not contain ':', ',' or newlines".into());
    }
    Ok(())
}

/// A port or range: `8080` or `60000-61000`.
pub fn port(s: &str) -> R {
    let ok = |p: &str| p.parse::<u16>().is_ok_and(|n| n > 0);
    let valid = match s.split_once('-') {
        Some((a, b)) => ok(a) && ok(b),
        None => ok(s),
    };
    if valid {
        Ok(())
    } else {
        Err("expected a port 1-65535 or a range like 60000-61000".into())
    }
}

/// A firewall rule as shown in the panel: `8080/tcp` or a service name.
pub fn firewall_rule(s: &str) -> R {
    match s.split_once('/') {
        Some((p, proto)) => {
            port(p)?;
            if !["tcp", "udp", "sctp", "dccp"].contains(&proto) {
                return Err("protocol must be tcp, udp, sctp or dccp".into());
            }
            Ok(())
        }
        None => name(s),
    }
}

/// firewalld zone/service names, SELinux booleans, repository aliases.
pub fn name(s: &str) -> R {
    if s.is_empty() || s.len() > 128 || s.starts_with('-') || !chars_ok(s, "_.+-") {
        return Err("use letters, digits, '_', '.', '+' or '-'".into());
    }
    Ok(())
}

/// Repository aliases, space separated (may be empty).
pub fn names(s: &str) -> R {
    s.split_whitespace().try_for_each(name)
}

/// Package names, space separated.
pub fn packages(s: &str) -> R {
    if s.split_whitespace().next().is_none() {
        return Err("no package given".into());
    }
    for p in s.split_whitespace() {
        if p.starts_with('-') || !chars_ok(p, "_.+-") {
            return Err(format!("'{p}' is not a package name"));
        }
    }
    Ok(())
}

pub fn search_term(s: &str) -> R {
    if s.starts_with('-') || s.len() > 100 || !chars_ok(s, "_.+-* ") {
        return Err("use letters, digits and _ . + - *".into());
    }
    Ok(())
}

pub fn repo_url(s: &str) -> R {
    let ok_scheme = ["https://", "http://", "obs://"].iter().any(|p| s.starts_with(p));
    if !ok_scheme || s.chars().any(char::is_whitespace) {
        return Err("expected an https:// (or obs://) URL".into());
    }
    Ok(())
}

/// OBS project, e.g. `home:alice` or `devel:languages:rust`.
pub fn obs_project(s: &str) -> R {
    if s.is_empty() || s.starts_with(':') || s.contains("::") || !chars_ok(s, ":_.+-") {
        return Err("expected an OBS project like home:user or devel:tools".into());
    }
    Ok(())
}

pub fn hostname(s: &str) -> R {
    let label_ok = |l: &str| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && chars_ok(l, "-");
    if s.len() > 253 || !s.split('.').all(label_ok) {
        return Err("letters, digits and '-', dot-separated labels".into());
    }
    Ok(())
}

pub fn timezone(s: &str) -> R {
    if s.contains("..") || s.starts_with('/') || !chars_ok(s, "/_+-") {
        return Err("expected a zone like Europe/Prague".into());
    }
    if !std::path::Path::new("/usr/share/zoneinfo").join(s).is_file() {
        return Err(format!("unknown time zone '{s}'"));
    }
    Ok(())
}

/// A Flatpak application ID, reverse DNS: `com.valvesoftware.Steam`.
pub fn flatpak_app(s: &str) -> R {
    let parts: Vec<&str> = s.split('.').collect();
    let part_ok = |p: &&str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && !p.starts_with('-');
    if parts.len() < 3 || s.len() > 255 || !parts.iter().all(part_ok) {
        return Err("expected an application ID like org.videolan.VLC".into());
    }
    Ok(())
}

/// A CUPS device URI: `ipp://printer.local/ipp/print`, `usb://HP/...`,
/// `hp:/usb/...`, `socket://10.0.0.5`.
pub fn printer_uri(s: &str) -> R {
    let schemes = ["ipp://", "ipps://", "http://", "https://", "socket://", "lpd://", "usb://", "hp:/", "dnssd://", "ippusb://"];
    if !schemes.iter().any(|p| s.starts_with(p)) || s.chars().any(|c| c.is_whitespace() || c.is_control()) || s.len() > 1024 {
        return Err("expected a printer URI such as ipp://printer.local/ipp/print".into());
    }
    Ok(())
}

/// What hp-setup accepts: an IP address, a hostname or a USB serial number.
pub fn hp_address(s: &str) -> R {
    if s.starts_with('-') || s.is_empty() || s.len() > 253 || !chars_ok(s, ".-:_") {
        return Err("expected an IP address, a hostname or the printer's serial number".into());
    }
    Ok(())
}

/// A Leap release such as `16.1`.
pub fn release_version(s: &str) -> R {
    match s.split_once('.') {
        Some((a, b)) if !a.is_empty() && !b.is_empty() && a.len() <= 2 && b.len() <= 2 && (a.to_owned() + b).chars().all(|c| c.is_ascii_digit()) => {
            Ok(())
        }
        _ => Err("expected a release like 16.1".into()),
    }
}

/// `16.1` > `16.0` > `15.6`, compared numerically.
pub fn version_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> (u32, u32) {
        let (a, b) = v.split_once('.').unwrap_or((v, "0"));
        (a.parse().unwrap_or(0), b.parse().unwrap_or(0))
    };
    parse(candidate) > parse(current)
}

/// A snapper snapshot number (0 is the running system and can't be used).
pub fn snapshot_number(s: &str) -> R {
    match s.parse::<u32>() {
        Ok(n) if n > 0 => Ok(()),
        _ => Err("expected a snapshot number (1 or higher)".into()),
    }
}

/// A one-line description, e.g. for a snapshot.
pub fn description(s: &str) -> R {
    if s.is_empty() || s.len() > 200 || s.chars().any(char::is_control) || s.starts_with('-') {
        return Err("one line, up to 200 characters".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_option_injection() {
        assert!(username("-oops").is_err());
        assert!(packages("vim --force").is_err());
        assert!(name("-x").is_err());
    }

    #[test]
    fn accepts_common_values() {
        assert!(username("alice").is_ok());
        assert!(firewall_rule("8080/tcp").is_ok());
        assert!(firewall_rule("ssh").is_ok());
        assert!(port("60000-61000").is_ok());
        assert!(obs_project("devel:languages:rust").is_ok());
        assert!(release_version("16.1").is_ok());
        assert!(flatpak_app("com.valvesoftware.Steam").is_ok());
        assert!(flatpak_app("--user").is_err());
        assert!(flatpak_app("org.foo").is_err());
        assert!(snapshot_number("42").is_ok());
    }

    #[test]
    fn release_versions() {
        assert!(release_version("16").is_err());
        assert!(release_version("16.1; rm").is_err());
        assert!(release_version("../16.0").is_err());
        assert!(snapshot_number("0").is_err());
        assert!(snapshot_number("-1").is_err());
        assert!(version_newer("16.0", "15.6"));
        assert!(version_newer("16.10", "16.9"));
        assert!(!version_newer("16.0", "16.0"));
    }
}
