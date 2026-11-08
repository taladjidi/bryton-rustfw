// The phone's cache of ride summaries (a RideBrief per downloaded ride, worked out once by the Rust core), the
// weekly panel built from it, and the text of a ride row. Definitions (Intervals.icu's) are in app/core/src/summary.rs.

import Charts
import SwiftUI

final class RideLibrary: ObservableObject {
    @Published private(set) var briefs: [String: RideBrief] = [:]
    /// The list size each brief was worked out for (a ride that changed size is read again).
    private var sizes: [String: UInt32] = [:]
    private var busy = Set<String>()
    private var unreadable = Set<String>()
    private let file = RideBrowser.documents.appendingPathComponent("ride-summaries.txt")
    private let queue = DispatchQueue(label: "ride-analysis", qos: .utility)

    init() {
        guard let text = try? String(contentsOf: file, encoding: .utf8) else { return }
        for line in text.split(separator: "\n") {
            let p = line.split(separator: " ", maxSplits: 2).map(String.init)
            guard p.count == 3, let size = UInt32(p[1]), let b = briefDecode(text: p[2]) else { continue }
            briefs[p[0]] = b
            sizes[p[0]] = size
        }
    }

    private func save() {
        let text = briefs.keys.sorted().map { "\($0) \(sizes[$0] ?? 0) \(briefEncode(b: briefs[$0]!))" }
        try? (text.joined(separator: "\n") + "\n").write(to: file, atomically: true, encoding: .utf8)
    }

    /// Works out the brief of each downloaded ride that has none, one at a time in the background.
    func catchUp(_ rides: [RideEntry], downloaded: Set<String>) {
        for r in rides where downloaded.contains(r.name) && sizes[r.name] != r.size && !busy.contains(r.name)
            && !unreadable.contains(r.name)
        {
            busy.insert(r.name)
            let url = RideBrowser.path(r.name)
            queue.async {
                let b = (try? Data(contentsOf: url)).flatMap { rideBrief(bytes: $0) }
                DispatchQueue.main.async {
                    self.busy.remove(r.name)
                    if let b {
                        self.briefs[r.name] = b
                        self.sizes[r.name] = r.size
                        self.save()
                    } else {
                        self.unreadable.insert(r.name)
                    }
                }
            }
        }
    }

    func stats(ftp: Int, maxHR: Int) -> [RideStat] {
        briefs.values.map { b in
            let off = TimeZone.current.secondsFromGMT(for: Date(timeIntervalSince1970: Double(b.startUnix)))
            return rideStat(b: b, ftp: UInt16(clamping: ftp), maxHr: UInt16(clamping: maxHR), tzOffsetS: Int32(clamping: off))
        }
    }

    func weeks(ftp: Int, maxHR: Int, count: Int = 8) -> [WeekTotal] {
        weeklyTotals(
            stats: stats(ftp: ftp, maxHR: maxHR), weekStart: 0, nowUnix: Int64(Date().timeIntervalSince1970),
            nowTzOffsetS: Int32(clamping: TimeZone.current.secondsFromGMT()), weeks: UInt32(clamping: count))
    }
}

enum Fmt {
    /// "2 h 31", "45 min".
    static func duration(_ s: UInt64) -> String {
        let m = (s + 30) / 60
        return m >= 60 ? "\(m / 60) h \(String(format: "%02d", m % 60))" : "\(m) min"
    }

    static func km(_ m: Double) -> String { String(format: "%.1f km", m / 1000) }

    /// "Sat 24 Oct, 14:29" in the phone's zone.
    static func start(_ unix: Int64) -> String {
        let f = DateFormatter()
        f.setLocalizedDateFormatFromTemplate("EEE d MMM HH:mm")
        return f.string(from: Date(timeIntervalSince1970: Double(unix)))
    }

    /// "10 Nov", the label of a week by its first day (days since 1970, a plain calendar date).
    static func week(_ day: Int64) -> String {
        let f = DateFormatter()
        f.timeZone = TimeZone(identifier: "UTC")
        f.setLocalizedDateFormatFromTemplate("d MMM")
        return f.string(from: Date(timeIntervalSince1970: Double(day) * 86_400))
    }
}

/// This week's time, distance and load, and the last 8 weeks as bars.
struct WeeklyPanel: View {
    @ObservedObject var library: RideLibrary
    @ObservedObject var rides: RideBrowser
    @AppStorage("ftp") private var ftp = 200
    @AppStorage("maxHR") private var maxHR = 190
    @State private var byLoad = true
    @State private var selected: String?

    var body: some View {
        let weeks = library.weeks(ftp: ftp, maxHR: maxHR)
        let current = weeks.last ?? WeekTotal(firstDay: 0, movingS: 0, distanceM: 0, load: 0, rides: 0)
        let shown = selected.flatMap { s in weeks.first { Fmt.week($0.firstDay) == s } } ?? current
        let missing = rides.rides.filter { RideBrowser.inWindow($0.name) && library.briefs[$0.name] == nil }.count
        VStack(alignment: .leading, spacing: 10) {
            Text(shown.firstDay == current.firstDay ? "This week" : "Week of \(Fmt.week(shown.firstDay))")
                .font(.subheadline).foregroundStyle(.secondary)
            HStack(alignment: .firstTextBaseline) {
                stat(Fmt.duration(shown.movingS), "time")
                Spacer()
                stat(Fmt.km(shown.distanceM), "distance")
                Spacer()
                stat("\(shown.load)", "TSS")
                Spacer()
                stat("\(shown.rides)", shown.rides == 1 ? "ride" : "rides")
            }
            Picker("Bars", selection: $byLoad) {
                Text("Load").tag(true)
                Text("Time").tag(false)
            }
            .pickerStyle(.segmented)
            Chart(weeks, id: \.firstDay) { w in
                BarMark(x: .value("Week", Fmt.week(w.firstDay)), y: .value(byLoad ? "TSS" : "Hours", value(w)))
                    .foregroundStyle(w.firstDay == shown.firstDay ? Color.accentColor : Color.accentColor.opacity(0.4))
            }
            .chartXSelection(value: $selected)
            .chartXAxis {
                AxisMarks { v in
                    if v.index % 2 == 0 { AxisValueLabel() }
                }
            }
            .frame(height: 120)
            if missing > 0 {
                Text("\(missing) ride\(missing == 1 ? "" : "s") of the last 8 weeks not on the phone yet.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 4)
    }

    private func value(_ w: WeekTotal) -> Double { byLoad ? Double(w.load) : Double(w.movingS) / 3600 }

    private func stat(_ v: String, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(v).font(.headline.monospacedDigit())
            Text(label).font(.caption2).foregroundStyle(.secondary)
        }
    }
}
