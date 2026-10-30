//! The GUI's text size: a zoom of the whole window, for screen sharing and
//! for eyes that want it bigger. The header buttons, Ctrl+Plus/Minus/0,
//! `/zoom` and the agent all set it; it is kept across starts.

use std::path::PathBuf;

pub const NORMAL: f32 = 1.0;
const MIN: f32 = 0.5;
const MAX: f32 = 3.0;
const STEP: f32 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Change {
    Bigger,
    Smaller,
    Reset,
    /// A factor, 1.0 being the normal size.
    Set(f32),
}

impl Change {
    /// `in`, `out`, `reset`, `150`, `150%` or `1.5`; also the plain words
    /// a model or a user would use ("bigger", "normal").
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim().to_lowercase();
        match s.as_str() {
            "in" | "+" | "bigger" | "larger" | "increase" => Ok(Self::Bigger),
            "out" | "-" | "smaller" | "decrease" => Ok(Self::Smaller),
            "" | "reset" | "normal" | "default" => Ok(Self::Reset),
            _ => {
                let n: f32 = s.trim_end_matches('%').trim().parse().map_err(|_| format!("expected in, out, reset or a size like 150%, not {s:?}"))?;
                // 150 and 150% are percent; 1.5 is a factor.
                let factor = if n > 5.0 || s.ends_with('%') { n / 100.0 } else { n };
                if !(MIN..=MAX).contains(&factor) {
                    return Err(format!("the text size goes from {} to {}", percent(MIN), percent(MAX)));
                }
                Ok(Self::Set(factor))
            }
        }
    }

    /// The new size, starting from `current`.
    pub fn apply(self, current: f32) -> f32 {
        let z = match self {
            Self::Bigger => current + STEP,
            Self::Smaller => current - STEP,
            Self::Reset => NORMAL,
            Self::Set(z) => z,
        };
        ((z * 20.0).round() / 20.0).clamp(MIN, MAX)
    }
}

/// "130%".
pub fn percent(z: f32) -> String {
    format!("{:.0}%", z * 100.0)
}

/// The size from the last run; `YAIST_TEXT_SIZE` (e.g. 150%) overrides it,
/// for screenshots.
pub fn load() -> f32 {
    let saved = || std::fs::read_to_string(path()?).ok();
    match std::env::var("YAIST_TEXT_SIZE").ok().or_else(saved).map(|s| Change::parse(&s)) {
        Some(Ok(c)) => c.apply(NORMAL),
        _ => NORMAL,
    }
}

pub fn save(z: f32) {
    let Some(path) = path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, percent(z));
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("yaist/text-size"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_from_words_and_numbers() {
        assert_eq!(Change::parse("in"), Ok(Change::Bigger));
        assert_eq!(Change::parse(" Smaller "), Ok(Change::Smaller));
        assert_eq!(Change::parse(""), Ok(Change::Reset));
        assert_eq!(Change::parse("150%"), Ok(Change::Set(1.5)));
        assert_eq!(Change::parse("150"), Ok(Change::Set(1.5)));
        assert_eq!(Change::parse("1.25"), Ok(Change::Set(1.25)));
        assert!(Change::parse("1000%").is_err());
        assert!(Change::parse("huge").is_err());
    }

    #[test]
    fn steps_stay_in_range_and_round() {
        assert_eq!(Change::Bigger.apply(1.0), 1.1);
        assert_eq!(Change::Smaller.apply(1.13), 1.05);
        assert_eq!(Change::Bigger.apply(3.0), 3.0);
        assert_eq!(Change::Smaller.apply(0.5), 0.5);
        assert_eq!(Change::Reset.apply(2.0), 1.0);
        assert_eq!(percent(1.3), "130%");
    }
}
