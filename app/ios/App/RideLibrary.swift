// The phone's cache of ride summaries (a RideBrief per downloaded ride, worked out once by the Rust core), the
// weekly panel built from it, and the text of a ride row. Definitions are in app/core/src/summary.rs.

import Charts
import SwiftUI

final class RideLibrary: ObservableObject {
    /// Written as the cache's first line: a change of the definitions (moving time, load) drops older entries.
    static let definitions = 2

    @Published private(set) var briefs: [String: RideBrief] = [:]
    /// The cache could not be written.
    @Published private(set) var error: String?
    /// The list size each brief was worked out for (a ride that changed size is read again).
    private var sizes: [String: UInt32] = [:]
    private var busy = Set<String>()
    /// Rides the brief could not be read from, by name: the copy they failed on (its size and date) and how many times.
    /// A copy that failed twice is reported to `onUnreadable` (RideBrowser.reread sets a damaged one aside) and not
    /// tried again; another copy of the same name (other size or date) is.
    private var unreadable: [String: (copy: String, fails: Int)] = [:]
    var onUnreadable: ((String) -> Void)?
    private var saveTask: Task<Void, Never>?
    private var pendingSince: Date?
    private let file = RideBrowser.documents.appendingPathComponent("ride-summaries.txt")
    private let queue = DispatchQueue(label: "ride-analysis", qos: .utility)
    /// The weeks last worked out, for the inputs they were worked out for.
    private var memo: (key: String, weeks: [WeekTotal])?

    init() {
        let size = (try? file.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
        if size > 1_000_000 {
            // a big cache is read off the main thread
            queue.async {
                let (b, s) = Self.read(self.file)
                DispatchQueue.main.async {
                    // what was analysed meanwhile wins
                    self.briefs.merge(b) { mine, _ in mine }
                    self.sizes.merge(s) { mine, _ in mine }
                }
            }
        } else {
            (briefs, sizes) = Self.read(file)
        }
    }

    private static func read(_ file: URL) -> ([String: RideBrief], [String: UInt32]) {
        var (briefs, sizes) = ([String: RideBrief](), [String: UInt32]())
        guard let text = try? String(contentsOf: file, encoding: .utf8) else { return (briefs, sizes) }
        var lines = text.split(separator: "\n")
        guard !lines.isEmpty, lines.removeFirst() == "definitions \(definitions)" else { return (briefs, sizes) }
        for line in lines {
            let p = line.split(separator: " ", maxSplits: 2).map(String.init)
            guard p.count == 3, let size = UInt32(p[1]), let b = briefDecode(text: p[2]) else { continue }
            briefs[p[0]] = b
            sizes[p[0]] = size
        }
        return (briefs, sizes)
    }

    /// One write for a burst of analysed rides (a catch-up of many would rewrite the whole cache for each).
    private func scheduleSave() {
        // at least every 10 s during a long catch-up
        if pendingSince == nil { pendingSince = Date() }
        if let t = pendingSince, Date().timeIntervalSince(t) >= 10 { return saveNow() }
        saveTask?.cancel()
        saveTask = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            if !Task.isCancelled { self?.saveNow() }
        }
    }

    /// Writes a pending cache now (the app goes to the background and may not come back).
    func saveNow() {
        guard pendingSince != nil else { return }
        saveTask?.cancel()
        pendingSince = nil
        save()
    }

    private func save() {
        let rows = briefs.keys.sorted().compactMap { k in briefs[k].map { "\(k) \(sizes[k] ?? 0) \(briefEncode(b: $0))" } }
        let text = (["definitions \(Self.definitions)"] + rows).joined(separator: "\n") + "\n"
        do {
            try text.write(to: file, atomically: true, encoding: .utf8)
            error = nil
        } catch {
            self.error = "Could not save the ride summaries: \(error.localizedDescription)"
        }
    }

    /// Works out the brief of each downloaded ride that has none, one at a time in the background.
    func catchUp(_ rides: [RideEntry], downloaded: Set<String>) {
        for r in rides where downloaded.contains(r.name) && sizes[r.name] != r.size && !busy.contains(r.name)
            && !(unreadable[r.name].map { $0.fails >= 2 && $0.copy == Self.copy(r.name) } ?? false)
        {
            busy.insert(r.name)
            let url = RideBrowser.path(r.name)
            let copy = Self.copy(r.name)  // the copy the analysis reads, not whatever is there when it ends
            queue.async {
                let b = (try? Data(contentsOf: url)).flatMap { rideBrief(bytes: $0) }
                DispatchQueue.main.async {
                    self.busy.remove(r.name)
                    if let b {
                        self.briefs[r.name] = b
                        self.sizes[r.name] = r.size
                        self.scheduleSave()
                    } else {
                        let fails = (self.unreadable[r.name]?.copy == copy ? self.unreadable[r.name]?.fails ?? 0 : 0) + 1
                        self.unreadable[r.name] = (copy, fails)
                        if fails == 2 { self.onUnreadable?(r.name) }
                    }
                }
            }
        }
    }

    /// Which copy of a ride is on the phone: its size and date.
    private static func copy(_ name: String) -> String {
        let v = try? RideBrowser.path(name).resourceValues(forKeys: [.fileSizeKey, .contentModificationDateKey])
        return "\(v?.fileSize ?? -1) \(v?.contentModificationDate?.timeIntervalSince1970 ?? 0)"
    }

    func weeks(ftp: Int, maxHR: Int, count: Int = 8) -> [WeekTotal] {
        // worked out again only when the briefs, the profile or the day change
        let now = Date()
        let off = TimeZone.current.secondsFromGMT(for: now)
        let key = "\(briefs.count) \(sizes.values.reduce(0, &+)) \(ftp) \(maxHR) \(off) \(Int(now.timeIntervalSince1970) / 3600)"
        if let m = memo, m.key == key { return m.weeks }
        let stats = briefs.map { name, b in
            rideStat(
                b: b, name: name, ftp: UInt16(clamping: ftp), maxHr: UInt16(clamping: maxHR),
                tzOffsetS: Int32(clamping: TimeZone.current.secondsFromGMT(for: Date(timeIntervalSince1970: Double(b.startUnix)))))
        }
        let w = weeklyTotals(
            stats: stats, weekStart: 0, nowUnix: Int64(now.timeIntervalSince1970), nowTzOffsetS: Int32(clamping: off),
            weeks: UInt32(clamping: count))
        memo = (key, w)
        return w
    }
}

/// This week's time, distance and load, and the last 8 weeks as bars.
struct WeeklyPanel: View, Equatable {
    @ObservedObject var library: RideLibrary
    /// Rides of the last 8 weeks the unit lists that are not on the phone yet.
    let missing: Int
    let openProfile: () -> Void
    @EnvironmentObject private var profile: Profile
    @State private var byLoad = true
    @State private var selected: String?
    @Environment(\.dynamicTypeSize) private var type
    @ScaledMetric private var chartHeight: CGFloat = 120

    static func == (a: WeeklyPanel, b: WeeklyPanel) -> Bool { a.missing == b.missing && a.library === b.library }

    var body: some View {
        let weeks = library.weeks(ftp: profile.ftp, maxHR: profile.maxHR)
        let current = weeks.last ?? WeekTotal(firstDay: 0, movingS: 0, distanceM: 0, load: 0, rides: 0, hasHrLoad: false)
        let shown = selected.flatMap { s in weeks.first { Fmt.week($0.firstDay) == s } } ?? current
        let unit = shown.hasHrLoad ? "Load*" : "TSS"
        VStack(alignment: .leading, spacing: 10) {
            CardHeader(title: shown.firstDay == current.firstDay ? "This week" : "Week of \(Fmt.week(shown.firstDay))") {
                Picker("Bars", selection: $byLoad) {
                    Text("Load").tag(true)
                    Text("Time").tag(false)
                }
            }
            // four columns, or two by two when the type is large (a column would wrap its label mid-word)
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .firstTextBaseline) { stats(shown, unit) }
                Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 8) {
                    GridRow { stat(Fmt.duration(shown.movingS), "time"); stat(Fmt.distance(shown.distanceM), "distance") }
                    GridRow { stat("\(shown.load)", unit); stat("\(shown.rides)", shown.rides == 1 ? "ride" : "rides") }
                }
            }
            if weeks.allSatisfy({ $0.rides == 0 }) {
                Text("No rides in the last 8 weeks.").font(.subheadline).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: chartHeight / 2, alignment: .center)
            } else {
            Chart(weeks, id: \.firstDay) { w in
                BarMark(x: .value("Week", Fmt.week(w.firstDay)), y: .value(byLoad ? "TSS" : "Hours", value(w)))
                    .foregroundStyle(w.firstDay == shown.firstDay ? Tok.accent : Tok.accent.opacity(0.6))
            }
            .chartXSelection(value: $selected)
            .chartXAxis {
                AxisMarks { v in
                    // every second week; every fourth when the type is large and the labels would collide
                    if v.index % (type.isAccessibilitySize ? 4 : 2) == 0 { AxisValueLabel() }
                }
            }
            .frame(height: chartHeight)
            .accessibilityLabel("Weekly \(byLoad ? "load" : "time"), last 8 weeks")
            .accessibilityValue(weeks.map { "\(Fmt.week($0.firstDay)): \(byLoad ? "\($0.load)" : Fmt.duration($0.movingS))" }.joined(separator: ", "))
            }
            if weeks.contains(where: \.hasHrLoad) {
                Text("* Load includes rides worked out from heart rate (no power), which are not TSS.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            if !profile.ftpSet {
                Button(action: openProfile) {
                    HStack {
                        Text("FTP not set: loads use 200 W. Open the profile.").multilineTextAlignment(.leading)
                        Spacer(minLength: 8)
                        Image(systemName: "chevron.right").font(.footnote.bold()).foregroundStyle(.tertiary)
                    }
                    .frame(minHeight: 44)
                }
                .font(.subheadline)
            }
            if missing > 0 {
                Text("\(missing) ride\(missing == 1 ? "" : "s") of the last 8 weeks not on the phone yet.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 4)
    }

    private func value(_ w: WeekTotal) -> Double { byLoad ? Double(w.load) : Double(w.movingS) / 3600 }

    @ViewBuilder private func stats(_ shown: WeekTotal, _ unit: String) -> some View {
        stat(Fmt.duration(shown.movingS), "time")
        Spacer()
        stat(Fmt.distance(shown.distanceM), "distance")
        Spacer()
        stat("\(shown.load)", unit)
        Spacer()
        stat("\(shown.rides)", shown.rides == 1 ? "ride" : "rides")
    }

    private func stat(_ v: String, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(v).numeral(.title3)
            Text(label).font(.caption).foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .combine)
    }
}
