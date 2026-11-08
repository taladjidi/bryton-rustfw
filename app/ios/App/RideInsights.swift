// The ride page's deeper analysis: climbs, W' balance, aerobic decoupling, DFA alpha1, and a comparison with another
// synced ride. The numbers come from the Rust core (rideInsight: the firmware's climb finder, W' model and DFA code);
// this file caches them per ride and draws. Definitions and sources: docs/app.md, "Deeper analysis".

import Charts
import CoreLocation
import SwiftUI

/// Results kept for the session (the last few rides, least recently used out), one per ride file and profile: worked
/// out once, off the main thread, and two askers of the same key share one run.
actor InsightCache {
    static let shared = InsightCache()
    private static let limit = 8
    private var insights = Lru<RideInsight>()
    private var views = Lru<RideView>()
    private var running: [String: Task<RideInsight?, Never>] = [:]
    private var running2: [String: Task<RideView?, Never>] = [:]

    private func key(_ url: URL, _ extra: String) -> String {
        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
        return "\(url.lastPathComponent) \(size) \(extra)"
    }

    func insight(_ url: URL, weight: UInt16, cp: UInt16, wPrime: UInt16) async -> RideInsight? {
        let k = key(url, "\(weight) \(cp) \(wPrime)")
        if let c = insights.get(k) { return c }
        let t = running[k] ?? Task.detached(priority: .utility) {
            try? rideInsight(bytes: Data(contentsOf: url), weightKg: weight, cp: cp, wprimeJ: wPrime)
        }
        running[k] = t
        let r = await t.value
        running[k] = nil
        if let r { insights.put(k, r, Self.limit) }
        return r
    }

    func view(_ url: URL, ftp: UInt16, maxHR: UInt16) async -> RideView? {
        let k = key(url, "\(ftp) \(maxHR)")
        if let c = views.get(k) { return c }
        let t = running2[k] ?? Task.detached(priority: .utility) {
            try? analyzeRide(bytes: Data(contentsOf: url), ftp: ftp, maxHr: maxHR)
        }
        running2[k] = t
        let r = await t.value
        running2[k] = nil
        if let r { views.put(k, r, Self.limit) }
        return r
    }
}

/// A small least-recently-used map.
struct Lru<V> {
    private var items: [String: V] = [:]
    private var order: [String] = []

    mutating func get(_ k: String) -> V? {
        guard let v = items[k] else { return nil }
        order.removeAll { $0 == k }
        order.append(k)
        return v
    }

    mutating func put(_ k: String, _ v: V, _ limit: Int) {
        items[k] = v
        order.removeAll { $0 == k }
        order.append(k)
        while order.count > limit { items[order.removeFirst()] = nil }
    }
}

/// A climb's part of the altitude line, cut once per ride and axis (not on every pass of a chart's body).
struct ClimbSlice: Identifiable {
    let id: Int
    let color: Color
    let pts: [Pt]
}

extension Color {
    /// 0xRRGGBB from the core.
    init(rgb: UInt32) {
        self.init(red: Double(rgb >> 16 & 255) / 255, green: Double(rgb >> 8 & 255) / 255, blue: Double(rgb & 255) / 255)
    }
}

extension ClimbStat {
    /// The track's stretch of the climb, from the viewer's samples.
    func coords(in samples: [TrackSample]) -> [CLLocationCoordinate2D] {
        samples.compactMap { s in
            guard s.distM >= Double(startM), s.distM <= Double(startM) + Double(lenM), let a = s.lat, let o = s.lon,
                abs(a) <= 90, abs(o) <= 180
            else { return nil }
            return CLLocationCoordinate2D(latitude: a, longitude: o)
        }
    }

    /// The chart x range of the climb on the chosen axis.
    func xRange(byTime: Bool) -> ClosedRange<Double> {
        byTime ? Double(startS) / 60...Double(max(endS, startS)) / 60 : (Double(startM) / 1000)...((Double(startM) + Double(lenM)) / 1000)
    }
}

private func mmss(_ s: UInt32) -> String { String(format: "%d:%02d", s / 60, s % 60) }

struct ClimbsSection: View {
    let climbs: [ClimbStat]

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Climbs").font(.subheadline.bold())
            ForEach(climbs.indices, id: \.self) { i in
                let c = climbs[i]
                HStack(alignment: .top, spacing: 8) {
                    RoundedRectangle(cornerRadius: 2).fill(Color(rgb: c.color)).frame(width: 6)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("\(i + 1)  \(String(format: "%.1f km", Double(c.lenM) / 1000)) at \(String(format: "%.1f", Double(c.avgGradeX10) / 10)) %, +\(c.gainM) m")
                            .font(.subheadline.bold())
                        Text(detail(c)).font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                    }
                    Spacer()
                }
            }
        }
    }

    private func detail(_ c: ClimbStat) -> String {
        var t = ["max \(String(format: "%.1f", Double(c.maxGradeX10) / 10)) %", mmss(c.timeS)]
        if let v = c.vam { t.append("VAM \(v) m/h") }
        if let p = c.avgPower { t.append("\(p) W") }
        if let w = c.wPerKgX100 { t.append(String(format: "%.1f W/kg", Double(w) / 100)) }
        return t.joined(separator: "  ")
    }
}

struct WbalChart: View {
    let points: [WbalPoint]
    let minJ: Int32?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text("W' balance (kJ)").font(.subheadline.bold())
                Spacer()
                if let m = minJ { Text(String(format: "lowest %.1f kJ", Double(m) / 1000)).font(.caption).foregroundStyle(.secondary) }
            }
            Chart(points.indices, id: \.self) { i in
                let p = points[i]
                AreaMark(x: .value("min", Double(p.tS) / 60), y: .value("kJ", Double(p.wbalJ) / 1000))
                    .foregroundStyle(.purple.opacity(0.3))
                LineMark(x: .value("min", Double(p.tS) / 60), y: .value("kJ", Double(p.wbalJ) / 1000))
                    .foregroundStyle(.purple)
            }
            .chartXAxisLabel("min")
            .frame(height: 110)
        }
    }
}

struct DecouplingRow: View {
    let d: Decoupling

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Aerobic decoupling (Pa:HR)").font(.subheadline.bold())
            HStack {
                // under 5 % is the usual mark of a well-built aerobic base (Friel); inferred, not a diagnosis
                Text(String(format: "%.1f %%", d.percent)).font(.title3.bold().monospacedDigit())
                    .foregroundStyle(d.percent < 5 ? .green : .orange)
                Text(String(format: "%.2f to %.2f W/bpm, first half to second", d.efFirst, d.efSecond))
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }
}

struct DfaChart: View {
    let points: [DfaPoint]
    let vt1: Double
    let vt2: Double

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("DFA alpha1").font(.subheadline.bold())
            Chart {
                ForEach(points.indices, id: \.self) { i in
                    LineMark(x: .value("min", Double(points[i].tS) / 60), y: .value("a1", points[i].a1)).foregroundStyle(.teal)
                }
                RuleMark(y: .value("VT1", vt1)).lineStyle(StrokeStyle(lineWidth: 1, dash: [4]))
                    .foregroundStyle(.green).annotation(position: .top, alignment: .trailing) {
                        Text(String(format: "%.2f aerobic threshold", vt1)).font(.caption2)
                    }
                RuleMark(y: .value("VT2", vt2)).lineStyle(StrokeStyle(lineWidth: 1, dash: [4]))
                    .foregroundStyle(.red).annotation(position: .top, alignment: .trailing) {
                        Text(String(format: "%.2f anaerobic threshold", vt2)).font(.caption2)
                    }
            }
            .chartXAxisLabel("min")
            .chartYScale(domain: 0...max(1.5, ((points.map(\.a1).max() ?? 0) * 4).rounded(.up) / 4))
            .frame(height: 130)
        }
    }
}

/// "Compare with...": another synced ride's power curve over this one's, and the climbs the two share.
struct CompareSection: View {
    let url: URL
    let view: RideView
    let climbs: [ClimbStat]
    let weight: UInt16
    let cp: UInt16
    let wPrime: UInt16
    @AppStorage("ftp") private var ftp = 200
    @AppStorage("maxHR") private var maxHR = 190
    @State private var other: (name: String, view: RideView, climbs: [ClimbStat])?
    @State private var loading = false
    @State private var picking: Task<Void, Never>?

    private static func candidates(excluding url: URL) -> [String] {
        let dir = url.deletingLastPathComponent()
        let names = (try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []
        return names.filter { $0.hasSuffix(".fit") && $0 != url.lastPathComponent }.map { String($0.dropLast(4)) }
            .sorted(by: >)
    }

    var body: some View {
        let names = Self.candidates(excluding: url)
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Compare with...").font(.subheadline.bold())
                Spacer()
                if !names.isEmpty {
                    Menu(other.map { RideBrowser.title($0.name) } ?? "Pick a ride") {
                        ForEach(names, id: \.self) { n in Button(RideBrowser.title(n)) { pick(n, dir: url.deletingLastPathComponent()) } }
                    }
                }
            }
            if names.isEmpty { Text("Download another ride to compare.").font(.caption).foregroundStyle(.secondary) }
            if loading { ProgressView() }
            if let o = other {
                curve(o)
                shared(o)
            }
        }
    }

    private func pick(_ name: String, dir: URL) {
        loading = true
        picking?.cancel()  // two quick picks: only the last one may set the result
        let (f, m) = (UInt16(clamping: ftp), UInt16(clamping: maxHR))
        picking = Task {
            let u = dir.appendingPathComponent("\(name).fit")
            async let v = InsightCache.shared.view(u, ftp: f, maxHR: m)
            async let i = InsightCache.shared.insight(u, weight: weight, cp: cp, wPrime: wPrime)
            let (rv, ri) = (await v, await i)
            guard !Task.isCancelled else { return }
            if let rv { other = (name, rv, ri?.climbs ?? []) }
            loading = false
        }
    }

    private func label(_ s: UInt32) -> String { s < 60 ? "\(s) s" : "\(s / 60) min" }

    @ViewBuilder private func curve(_ o: (name: String, view: RideView, climbs: [ClimbStat])) -> some View {
        let rows = [("This ride", view.summary.curve), (RideBrowser.title(o.name), o.view.summary.curve)]
        // every duration either ride held, in order
        let secs = Set(rows.flatMap { $0.1.map(\.secs) }).sorted()
        Chart {
            ForEach(rows.indices, id: \.self) { r in
                ForEach(rows[r].1, id: \.secs) { p in
                    BarMark(x: .value("Duration", label(p.secs)), y: .value("W", p.watts))
                        .foregroundStyle(by: .value("Ride", rows[r].0))
                        .position(by: .value("Ride", rows[r].0))
                }
            }
        }
        .chartXScale(domain: secs.map(label))
        .chartForegroundStyleScale([rows[0].0: Color.orange, rows[1].0: Color.blue])
        .frame(height: 160)
    }

    @ViewBuilder private func shared(_ o: (name: String, view: RideView, climbs: [ClimbStat])) -> some View {
        let pairs = matchClimbs(first: climbs, second: o.climbs)
        if !pairs.isEmpty {
            Text("Same climbs").font(.caption.bold()).foregroundStyle(.secondary)
            ForEach(pairs.indices, id: \.self) { k in
                if let a = climbs[safe: Int(pairs[k].first)], let b = o.climbs[safe: Int(pairs[k].second)] {
                    let d = Int(a.timeS) - Int(b.timeS)
                    HStack {
                        Text("\(Int(pairs[k].first) + 1)  \(String(format: "%.1f km", Double(a.lenM) / 1000))")
                        Spacer()
                        Text("\(mmss(a.timeS)) vs \(mmss(b.timeS))")
                        Text(d == 0 ? "same" : "\(d < 0 ? "-" : "+")\(mmss(UInt32(abs(d))))")
                            .foregroundStyle(d <= 0 ? .green : .orange)
                    }
                    .font(.caption.monospacedDigit())
                }
            }
        } else if !climbs.isEmpty {
            Text("The two rides share no climb.").font(.caption).foregroundStyle(.secondary)
        }
    }
}

extension Array {
    subscript(safe i: Int) -> Element? { indices.contains(i) ? self[i] : nil }
}
