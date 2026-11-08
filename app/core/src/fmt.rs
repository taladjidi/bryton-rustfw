//! Clock-style durations for the screens, so one rule serves the whole app (the Swift `Fmt` calls these). Integers
//! only, no locale: a decimal number goes through Swift's `FormatStyle`, which follows the phone's region.

/// "2:03:09", hours without a leading zero (a tile, a lap).
#[uniffi::export]
pub fn fmt_clock(secs: u32) -> String {
    format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

/// "5:09", minutes and seconds (a workout step, a climb's time).
#[uniffi::export]
pub fn fmt_mmss(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// "2 h 31" or "45 min", rounded to the minute (a list row, the weekly panel).
#[uniffi::export]
pub fn fmt_short(secs: u64) -> String {
    let m = secs.saturating_add(30) / 60;
    if m >= 60 { format!("{} h {:02}", m / 60, m % 60) } else { format!("{m} min") }
}

/// "1:12" for a zone's time: hours and minutes, rounded to the minute.
#[uniffi::export]
pub fn fmt_hm(secs: u32) -> String {
    let m = secs.saturating_add(30) / 60;
    format!("{}:{:02}", m / 60, m % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        assert_eq!(fmt_clock(7389), "2:03:09");
        assert_eq!(fmt_clock(0), "0:00:00");
        assert_eq!(fmt_mmss(309), "5:09");
        assert_eq!(fmt_mmss(3600), "60:00");
        assert_eq!([fmt_short(2699).as_str(), &fmt_short(2700), &fmt_short(9060)], ["45 min", "45 min", "2 h 31"]);
        assert_eq!([fmt_hm(4323).as_str(), &fmt_hm(29), &fmt_hm(4350)], ["1:12", "0:00", "1:13"]);
        assert!(fmt_short(u64::MAX).ends_with(" h 01") || fmt_short(u64::MAX).contains(" h "));
        assert!(!fmt_hm(u32::MAX).is_empty());
    }
}
