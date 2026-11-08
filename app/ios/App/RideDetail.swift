// The ride viewer: summary grid, map, graphs linked to the map, laps, zones and the power curve. The numbers come
// from the Rust core's analyzeRide (the firmware's own NP / IF / TSS / zones / curve code); this file only draws.

import Charts
import CoreLocation
import SwiftUI

/// FTP and max HR for IF / TSS and the zones, until the unit's profile can be read.
struct ProfileSheet: View {
    @AppStorage("ftp") private var ftp = 200
    @AppStorage("maxHR") private var maxHR = 190
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Stepper("FTP: \(ftp) W", value: $ftp, in: 50...600, step: 5)
                Stepper("Max heart rate: \(maxHR) bpm", value: $maxHR, in: 120...230)
                Text("Keep it equal to your FTP on Intervals.icu so the loads match.").font(.footnote).foregroundStyle(.secondary)
            }
            .navigationTitle("Profile")
            .toolbar { Button("Done") { dismiss() } }
        }
        .presentationDetents([.medium])
    }
}

/// One graph's points, x already on the chosen axis.
struct Pt: Identifiable {
    let id: Int
    let x: Double
    let y: Double
}

struct Series: Identifiable {
    let id: String
    let color: Color
    let area: Bool
    let pts: [Pt]
}

/// What the viewer draws, worked out once per ride and axis (not on every drag tick of a chart).
struct Prep {
    var coords: [CLLocationCoordinate2D] = []
    /// The map points with their x, to find the one under the chart's cursor.
    var geo: [(x: Double, c: CLLocationCoordinate2D)] = []
    var series: [Series] = []

    init() {}

    init(_ r: RideView, byTime: Bool) {
        let xf: (TrackSample) -> Double = { byTime ? Double($0.tS) / 60 : $0.distM / 1000 }
        for s in r.samples {
            guard let a = s.lat, let o = s.lon, abs(a) <= 90, abs(o) <= 180 else { continue }
            let c = CLLocationCoordinate2D(latitude: a, longitude: o)
            coords.append(c)
            geo.append((xf(s), c))
        }
        let defs: [(String, Color, Bool, (TrackSample) -> Double?)] = [
            ("Altitude (m)", .gray, true, { $0.altM }),
            ("Speed (km/h)", .blue, false, { $0.speedMs.map { $0 * 3.6 } }),
            ("Heart rate (bpm)", .red, false, { $0.hr.map(Double.init) }),
            ("Power (W)", .orange, false, { $0.power.map(Double.init) }),
            ("Cadence (rpm)", .green, false, { $0.cadence.map(Double.init) }),
        ]
        series = defs.compactMap { d in
            let pts = r.samples.enumerated().compactMap { i, s in d.3(s).map { Pt(id: i, x: xf(s), y: $0) } }
            return pts.count > 1 ? Series(id: d.0, color: d.1, area: d.2, pts: pts) : nil
        }
    }

    /// The element whose x is nearest to `x` (the x are increasing): a binary search.
    static func nearest<T>(_ a: [T], _ x: Double, _ key: (T) -> Double) -> T? {
        var (lo, hi) = (0, a.count)
        while lo < hi {
            let mid = (lo + hi) / 2
            if key(a[mid]) < x { lo = mid + 1 } else { hi = mid }
        }
        let cand = [lo - 1, lo].filter { a.indices.contains($0) }.map { a[$0] }
        return cand.min { abs(key($0) - x) < abs(key($1) - x) }
    }
}

struct RideDetail: View {
    let url: URL
    @AppStorage("ftp") private var ftp = 200
    @AppStorage("maxHR") private var maxHR = 190
    @State private var ride: RideView?
    @State private var prep = Prep()
    @State private var error: String?
    @State private var byTime = false
    @State private var selX: Double?
    @State private var sharing = false
    @State private var profile = false

    /// The start in the phone's time zone: the title and the summary's date are both this one value.
    private var startDate: Date? { ride.map { Date(timeIntervalSince1970: TimeInterval($0.summary.startUnix)) } }

    var body: some View {
        ScrollViewReader { proxy in
        ScrollView {
            if let r = ride {
                VStack(alignment: .leading, spacing: 20) {
                    SummaryGrid(s: r.summary)
                    if !prep.coords.isEmpty {
                        RideMap(points: prep.coords, selected: selX.flatMap { x in Prep.nearest(prep.geo, x) { $0.x }?.c })
                            .frame(height: 280)
                            .overlay(alignment: .bottomTrailing) {
                                if RideMap.usesIGN(prep.coords) {
                                    Text("© IGN").font(.caption2).padding(.horizontal, 4)
                                        .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 4)).padding(4)
                                }
                            }
                            .clipShape(RoundedRectangle(cornerRadius: 12))
                    }
                    graphs.id("graphs")
                    if !r.summary.laps.isEmpty { LapList(laps: r.summary.laps).id("laps") }
                    if r.summary.powerZoneS.contains(where: { $0 > 0 }) {
                        ZoneBars(title: "Power zones", secs: r.summary.powerZoneS.map(Int.init), color: .orange)
                    }
                    if r.summary.hrZoneS.contains(where: { $0 > 0 }) {
                        ZoneBars(title: "Heart rate zones", secs: r.summary.hrZoneS.map(Int.init), color: .red)
                    }
                    if !r.summary.curve.isEmpty { CurveChart(points: r.summary.curve) }
                }
                .padding()
            } else if let e = error {
                Text(e).foregroundStyle(.secondary).padding()
            } else {
                ProgressView().padding(40)
            }
        }
        #if DEBUG
        .onChange(of: ride != nil) { _, _ in
            // launch argument for the Simulator screenshots: -demo-scroll graphs|laps
            let a = CommandLine.arguments
            if let i = a.firstIndex(of: "-demo-scroll"), i + 1 < a.count {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { proxy.scrollTo(a[i + 1], anchor: .top) }
            }
        }
        #endif
        }
        // the file name only while loading; then the start time, from the same Date as the summary's line
        .navigationTitle(
            startDate?.formatted(date: .abbreviated, time: .shortened)
                ?? RideBrowser.title(url.deletingPathExtension().lastPathComponent)
        )
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            Button { profile = true } label: { Image(systemName: "person.crop.circle") }
            Button { sharing = true } label: { Image(systemName: "square.and.arrow.up") }
        }
        .sheet(isPresented: $sharing) { ShareSheet(url: url) }
        .sheet(isPresented: $profile) { ProfileSheet() }
        .task(id: "\(ftp) \(maxHR)") { await load() }
        .onChange(of: byTime) { _, t in
            selX = nil
            if let r = ride { prep = Prep(r, byTime: t) }
        }
    }

    private func load() async {
        // stored values can be out of range (a hand-edited plist): clamp, never trap
        let (ftp, maxHR, url) = (UInt16(clamping: ftp), UInt16(clamping: maxHR), url)
        let result = await Task.detached { () -> Result<RideView, Error> in
            Result { try analyzeRide(bytes: try Data(contentsOf: url), ftp: ftp, maxHr: maxHR) }
        }.value
        switch result {
        case .success(let r):
            ride = r
            prep = Prep(r, byTime: byTime)
            error = nil
        case .failure(let e): error = "This file cannot be shown: \(e)"
        }
    }

    @ViewBuilder private var graphs: some View {
        VStack(alignment: .leading, spacing: 12) {
            Picker("Axis", selection: $byTime) {
                Text("Distance").tag(false)
                Text("Time").tag(true)
            }
            .pickerStyle(.segmented)
            ForEach(prep.series) { s in MetricChart(series: s, byTime: byTime, selX: $selX) }
        }
    }
}

struct MetricChart: View {
    let series: Series
    let byTime: Bool
    @Binding var selX: Double?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(series.id).font(.subheadline.bold())
                Spacer()
                if let sx = selX, let p = Prep.nearest(series.pts, sx, { $0.x }) {
                    Text(String(format: "%.0f", p.y)).font(.subheadline.monospacedDigit()).foregroundStyle(series.color)
                }
            }
            Chart(series.pts) { p in
                if series.area {
                    AreaMark(x: .value("x", p.x), y: .value("y", p.y)).foregroundStyle(series.color.opacity(0.3))
                }
                LineMark(x: .value("x", p.x), y: .value("y", p.y)).foregroundStyle(series.color)
            }
            .chartXSelection(value: $selX)
            .chartXAxisLabel(byTime ? "min" : "km")
            .chartYScale(domain: .automatic(includesZero: false))
            .chartOverlay { proxy in
                // the cursor line, drawn here so the chart's 2000 marks are not rebuilt on every drag tick
                GeometryReader { g in
                    if let sx = selX, let px = proxy.position(forX: sx), let pf = proxy.plotFrame {
                        let f = g[pf]
                        Rectangle().fill(.secondary).frame(width: 1, height: f.height).position(x: f.minX + px, y: f.midY)
                    }
                }
                .allowsHitTesting(false)
            }
            .frame(height: 110)
        }
    }
}

struct SummaryGrid: View {
    let s: RideSummary

    var body: some View {
        let cells: [(String, String)?] = [
            ("Distance", String(format: "%.1f km", s.distanceM / 1000)),
            ("Moving", Self.hms(s.movingS)),
            ("Elapsed", Self.hms(s.elapsedS)),
            s.avgSpeedMs.map { ("Avg speed", String(format: "%.1f km/h", $0 * 3.6)) },
            s.maxSpeedMs.map { ("Max speed", String(format: "%.1f km/h", $0 * 3.6)) },
            s.ascentM.map { ("Ascent", "\($0) m") },
            s.descentM.map { ("Descent", "\($0) m") },
            s.avgHr.map { ("Avg HR", "\($0) bpm") },
            s.maxHr.map { ("Max HR", "\($0) bpm") },
            s.avgPower.map { ("Avg power", "\($0) W") },
            s.maxPower.map { ("Max power", "\($0) W") },
            s.np.map { ("NP", "\($0) W") },
            s.ifX100.map { ("IF", String(format: "%.2f", Double($0) / 100)) },
            s.tss.map { ("TSS", "\($0)") },
            s.avgCadence.map { ("Avg cadence", "\($0) rpm") },
            s.kcal.map { ("Energy", "\($0) kcal") },
        ]
        VStack(alignment: .leading, spacing: 4) {
            Text(Date(timeIntervalSince1970: TimeInterval(s.startUnix)).formatted(date: .complete, time: .shortened))
                .font(.subheadline).foregroundStyle(.secondary)
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 100), spacing: 8)], spacing: 8) {
                ForEach(cells.compactMap { $0 }, id: \.0) { c in
                    VStack(alignment: .leading) {
                        Text(c.0).font(.caption).foregroundStyle(.secondary)
                        Text(c.1).font(.headline.monospacedDigit()).lineLimit(1).minimumScaleFactor(0.7)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(8)
                    .background(Color(.secondarySystemBackground), in: RoundedRectangle(cornerRadius: 8))
                }
            }
        }
    }

    static func hms(_ s: UInt32) -> String { String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) }
}

struct LapList: View {
    let laps: [RideLap]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Laps").font(.subheadline.bold())
            ForEach(laps.indices, id: \.self) { i in
                let l = laps[i]
                HStack {
                    Text("\(i + 1)").frame(width: 24, alignment: .leading)
                    Text(SummaryGrid.hms(l.timeS))
                    Spacer()
                    Text(String(format: "%.2f km", l.distM / 1000))
                    if let v = l.avgSpeedMs { Text(String(format: "%.1f km/h", v * 3.6)) }
                    if let p = l.avgPower { Text("\(p) W") }
                    if let h = l.avgHr { Text("\(h) bpm") }
                }
                .font(.caption.monospacedDigit())
            }
        }
    }
}

struct ZoneBars: View {
    let title: String
    let secs: [Int]
    let color: Color

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.subheadline.bold())
            Chart {
                ForEach(secs.indices, id: \.self) { i in
                    BarMark(x: .value("Minutes", Double(secs[i]) / 60), y: .value("Zone", "Z\(i + 1)"))
                        .foregroundStyle(color.opacity(0.35 + 0.65 * Double(i + 1) / Double(secs.count)))
                }
            }
            .chartXAxisLabel("min")
            .frame(height: CGFloat(secs.count) * 24 + 30)
        }
    }
}

struct CurveChart: View {
    let points: [CurvePoint]

    private func name(_ s: UInt32) -> String { s < 60 ? "\(s) s" : "\(s / 60) min" }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Power curve").font(.subheadline.bold())
            Chart(points, id: \.secs) { p in
                BarMark(x: .value("Duration", name(p.secs)), y: .value("W", p.watts))
                    .foregroundStyle(.orange)
                    .annotation(position: .top) { Text("\(p.watts)").font(.caption2) }
            }
            .frame(height: 140)
        }
    }
}
