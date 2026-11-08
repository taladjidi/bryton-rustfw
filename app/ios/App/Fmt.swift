// The one place that turns a number or a date into text. Decimal numbers and dates follow the phone's region
// (a decimal comma in French); clock-style durations come from the Rust core (app/core/src/fmt.rs, tested there).

import Foundation

enum Fmt {
    /// A decimal number with `digits` places, in the phone's region, no thousands separator.
    static func dec(_ v: Double, _ digits: Int = 1) -> String {
        v.formatted(.number.precision(.fractionLength(digits)).grouping(.never))
    }

    /// "12,4 km" from metres.
    static func km(_ m: Double, _ digits: Int = 1) -> String { "\(dec(m / 1000, digits)) km" }
    /// "28,8 km/h" from m/s.
    static func kmh(_ ms: Double) -> String { "\(dec(ms * 3.6)) km/h" }
    /// "0,77" from a hundredths value.
    static func ifx<T: BinaryInteger>(_ x100: T) -> String { dec(Double(x100) / 100, 2) }
    /// "187 W".
    static func watts<T: BinaryInteger>(_ w: T) -> String { "\(w) W" }
    /// "142 bpm".
    static func bpm<T: BinaryInteger>(_ b: T) -> String { "\(b) bpm" }
    /// "IF 0,77   TSS 58" for a workout.
    static func ifTss<T: BinaryInteger, U: BinaryInteger>(_ x100: T, _ tss: U) -> String { "IF \(ifx(x100))   TSS \(tss)" }
    /// "+3" or "-12" (the form).
    static func signed(_ v: Double) -> String { v.formatted(.number.precision(.fractionLength(0)).sign(strategy: .always()).grouping(.never)) }

    /// "2:03:09" (a tile, a lap).
    static func clock(_ s: UInt32) -> String { fmtClock(secs: s) }
    /// "5:09" (a step, a climb).
    static func mmss(_ s: UInt32) -> String { fmtMmss(secs: s) }
    /// "2 h 31" or "45 min" (a list).
    static func duration(_ s: UInt64) -> String { fmtShort(secs: s) }
    /// "1:12", a zone's time.
    static func hm(_ s: UInt32) -> String { fmtHm(secs: s) }
    /// "45 s" or "5 min", a power curve's duration.
    static func span(_ s: UInt32) -> String { s < 60 ? "\(s) s" : "\(s / 60) min" }

    /// "Sun 8 Nov, 07:00" in the phone's zone and clock; the year only when it is not this one.
    static func when(_ d: Date) -> String {
        let sameYear = Calendar.current.isDate(d, equalTo: Date(), toGranularity: .year)
        let day = sameYear
            ? d.formatted(.dateTime.weekday(.abbreviated).day().month(.abbreviated))
            : d.formatted(.dateTime.day().month(.abbreviated).year())
        return "\(day), \(d.formatted(.dateTime.hour().minute()))"
    }
    static func start(_ unix: Int64) -> String { when(Date(timeIntervalSince1970: Double(unix))) }

    /// "Sun 8 Nov" from `YYYYMMDD...` (a wire date: the unit's workout key, Intervals.icu's day).
    static func dayKey(_ key: String) -> String {
        let n = key.compactMap { $0.wholeNumberValue }
        guard n.count >= 8,
            let d = Calendar.current.date(from: DateComponents(year: n[0] * 1000 + n[1] * 100 + n[2] * 10 + n[3], month: n[4] * 10 + n[5], day: n[6] * 10 + n[7]))
        else { return key }
        return d.formatted(.dateTime.weekday(.abbreviated).day().month(.abbreviated))
    }

    /// "10 Nov", the label of a week by its first day (days since 1970, a plain calendar date).
    static func week(_ day: Int64) -> String {
        // UTC: a plain calendar date, not an instant
        Date(timeIntervalSince1970: Double(day) * 86_400)
            .formatted(Date.FormatStyle(timeZone: TimeZone(identifier: "UTC") ?? .current).day().month(.abbreviated))
    }
}
