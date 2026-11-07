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
            }
            .navigationTitle("Profile")
            .toolbar { Button("Done") { dismiss() } }
        }
        .presentationDetents([.medium])
    }
}

struct RideDetail: View {
    let url: URL
    @AppStorage("ftp") private var ftp = 200
    @AppStorage("maxHR") private var maxHR = 190
    @State private var ride: RideView?
    @State private var error: String?
    @State private var byTime = false
    @State private var selX: Double?
    @State private var sharing = false
    @State private var profile = false

    var body: some View {
        ScrollViewReader { proxy in
        ScrollView {
            if let r = ride {
                VStack(alignment: .leading, spacing: 20) {
                    SummaryGrid(s: r.summary)
                    if !coords(r).isEmpty {
                        RideMap(points: coords(r), selected: selectedCoord(r))
                            .frame(height: 280)
                            .clipShape(RoundedRectangle(cornerRadius: 12))
                    }
                    graphs(r).id("graphs")
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
        .onChange(of: ride != nil) { _, _ in
            // launch argument for the Simulator screenshots: -demo-scroll graphs|laps
            let a = CommandLine.arguments
            if let i = a.firstIndex(of: "-demo-scroll"), i + 1 < a.count {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { proxy.scrollTo(a[i + 1], anchor: .top) }
            }
        }
        }
        .navigationTitle(RideBrowser.title(url.deletingPathExtension().lastPathComponent))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            Button { profile = true } label: { Image(systemName: "person.crop.circle") }
            Button { sharing = true } label: { Image(systemName: "square.and.arrow.up") }
        }
        .sheet(isPresented: $sharing) { ShareSheet(url: url) }
        .sheet(isPresented: $profile) { ProfileSheet() }
        .task(id: "\(ftp) \(maxHR)") { await load() }
    }

    private func load() async {
        let (ftp, maxHR, url) = (UInt16(ftp), UInt16(maxHR), url)
        let result = await Task.detached { () -> Result<RideView, Error> in
            Result { try analyzeRide(bytes: try Data(contentsOf: url), ftp: ftp, maxHr: maxHR) }
        }.value
        switch result {
        case .success(let r): ride = r; error = nil
        case .failure(let e): error = "This file cannot be shown: \(e)"
        }
    }

    private func x(_ s: TrackSample) -> Double { byTime ? Double(s.tS) / 60 : s.distM / 1000 }

    private func coords(_ r: RideView) -> [CLLocationCoordinate2D] {
        r.samples.compactMap { s in
            guard let a = s.lat, let o = s.lon, abs(a) <= 90, abs(o) <= 180 else { return nil }
            return CLLocationCoordinate2D(latitude: a, longitude: o)
        }
    }

    private func selectedCoord(_ r: RideView) -> CLLocationCoordinate2D? {
        guard let sx = selX else { return nil }
        let with = r.samples.filter { $0.lat != nil && $0.lon != nil }
        guard let s = with.min(by: { abs(x($0) - sx) < abs(x($1) - sx) }), let a = s.lat, let o = s.lon else {
            return nil
        }
        return CLLocationCoordinate2D(latitude: a, longitude: o)
    }

    @ViewBuilder private func graphs(_ r: RideView) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Picker("Axis", selection: $byTime) {
                Text("Distance").tag(false)
                Text("Time").tag(true)
            }
            .pickerStyle(.segmented)
            metric(r, "Altitude (m)", .gray, area: true) { $0.altM }
            metric(r, "Speed (km/h)", .blue) { $0.speedMs.map { $0 * 3.6 } }
            metric(r, "Heart rate (bpm)", .red) { $0.hr.map(Double.init) }
            metric(r, "Power (W)", .orange) { $0.power.map(Double.init) }
            metric(r, "Cadence (rpm)", .green) { $0.cadence.map(Double.init) }
        }
    }

    @ViewBuilder private func metric(
        _ r: RideView, _ title: String, _ color: Color, area: Bool = false, _ v: @escaping (TrackSample) -> Double?
    ) -> some View {
        let pts = r.samples.compactMap { s in v(s).map { (x(s), $0) } }
        if pts.count > 1 {
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text(title).font(.subheadline.bold())
                    Spacer()
                    if let sx = selX, let p = pts.min(by: { abs($0.0 - sx) < abs($1.0 - sx) }) {
                        Text(String(format: "%.0f", p.1)).font(.subheadline.monospacedDigit()).foregroundStyle(color)
                    }
                }
                Chart {
                    ForEach(pts.indices, id: \.self) { i in
                        if area {
                            AreaMark(x: .value("x", pts[i].0), y: .value("y", pts[i].1)).foregroundStyle(color.opacity(0.3))
                        }
                        LineMark(x: .value("x", pts[i].0), y: .value("y", pts[i].1)).foregroundStyle(color)
                    }
                    if let sx = selX { RuleMark(x: .value("x", sx)).foregroundStyle(.secondary) }
                }
                .chartXSelection(value: $selX)
                .chartXAxisLabel(byTime ? "min" : "km")
                .chartYScale(domain: .automatic(includesZero: false))
                .frame(height: 110)
            }
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
