//! Calendar dates as days since 1970-01-01, enough for end-of-life checks
//! without a date library.

/// Days since 1970-01-01 for a proleptic Gregorian date (H. Hinnant's
/// days_from_civil).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `YYYY-MM-DD` as days since 1970-01-01.
pub fn parse(s: &str) -> Option<i64> {
    let mut it = s.trim().splitn(3, '-');
    let y = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.parse().ok()?;
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then(|| days_from_civil(y, m, d))
}

/// Today (UTC) as days since 1970-01-01.
pub fn today() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64 / 86_400).unwrap_or(0)
}

/// Now in local time as `2026-10-26_10-12-07`, for file names.
pub fn local_stamp() -> String {
    // SAFETY: time() with a null pointer only returns; localtime_r writes
    // into the tm we own.
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return now.to_string();
    }
    format!("{:04}-{:02}-{:02}_{:02}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// `2026-12-31` -> `31 December 2026`.
pub fn pretty(s: &str) -> String {
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let parts: Vec<&str> = s.trim().splitn(3, '-').collect();
    match parts.as_slice() {
        [y, m, d] => match (m.parse::<usize>(), d.parse::<u32>()) {
            (Ok(m @ 1..=12), Ok(d)) => format!("{d} {} {y}", MONTHS[m - 1]),
            _ => s.to_owned(),
        },
        _ => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates() {
        assert_eq!(super::days_from_civil(1970, 1, 1), 0);
        assert_eq!(super::parse("2026-12-31").unwrap() - super::parse("2026-10-23").unwrap(), 69);
        assert_eq!(super::parse("2024-03-01").unwrap() - super::parse("2024-02-28").unwrap(), 2);
        assert!(super::parse("2026-13-01").is_none());
        assert_eq!(super::pretty("2026-12-31"), "31 December 2026");
    }
}
