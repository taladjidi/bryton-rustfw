// The ride viewer: summary grid, map, graphs linked to the map, laps, zones and the power curve. The numbers come
// from the Rust core's analyzeRide (the firmware's own NP / IF / TSS / zones / curve code); this file only draws.

import Charts
import CoreLocation
import SwiftUI

/// FTP and max HR for IF / TSS and the zones; filled from Intervals.icu when a key is set, and given to the unit.
struct ProfileSheet: View {
    @AppStorage("ftp") private var ftp = 200
    @AppStorage("maxHR") private var maxHR = 190
    @AppStorage("lthr") private var lthr = 0
    @AppStorage("weight") private var weight = 0
    @AppStorage("wPrime") private var wPrime = 0
    @EnvironmentObject private var rides: RideBrowser
    @EnvironmentObject private var intervals: IntervalsSync
    @Environment(\.dismiss) private var dismiss
    @State private var key = ""
    @State private var athlete = ""
    @State private var opened: (ftp: Int, maxHR: Int)?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Stepper("FTP: \(ftp) W", value: $ftp, in: 50...600, step: 5)
                    Stepper("Max heart rate: \(maxHR) bpm", value: $maxHR, in: 120...230)
                    if lthr > 0 { LabeledContent("Threshold heart rate", value: "\(lthr) bpm") }
                    if weight > 0 { LabeledContent("Weight", value: "\(weight) kg") }
                    if wPrime > 0 { LabeledContent("W'", value: "\(wPrime) J") }
                    if let l = rides.profileLine { Text(l).font(.footnote).foregroundStyle(.secondary) }
                } footer: {
                    Text("Keep the FTP equal to Intervals.icu's so the loads match. A change here is also given to the unit.")
                }
                Section {
                    TextField("Athlete id (0 or i12345)", text: $athlete)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                    SecureField(intervals.hasKey ? "API key stored (type to replace)" : "API key", text: $key)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                    Button("Test") { Task { await intervals.test(key: key, athlete: athlete); key = "" } }
                        .disabled(intervals.busy)
                    Button("Sync from Intervals.icu") { Task { await intervals.sync(rides: rides, force: true) } }
                        .disabled(intervals.busy || !intervals.hasKey)
                    if intervals.hasKey && !intervals.busy { Button("Forget the key", role: .destructive) { intervals.forget() } }
                    if let s = intervals.status { Text(s).font(.footnote).foregroundStyle(.secondary) }
                } header: {
                    Text("Intervals.icu")
                } footer: {
                    Text("The key is in Intervals.icu's Settings, Developer Settings. It is kept in the Keychain. Entering a key turns on ride uploads and sending planned workouts to the unit (the switches below).")
                }
                if intervals.hasKey { IntervalsOptions() }
                Section { NavigationLink("Licences") { LicencesView() } }
            }
            .navigationTitle("Profile")
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
            .onAppear {
                opened = (ftp, maxHR)
                athlete = intervals.athlete
            }
            .onDisappear {
                // a change typed here goes to the unit too (the stepper steps are all in range)
                guard let o = opened, o != (ftp, maxHR) else { return }
                rides.pushProfile(IntervalsProfile(name: nil, ftp: UInt32(clamping: ftp), maxHr: UInt32(clamping: maxHR),
                    lthr: nil, weightKg: nil, wprimeJ: nil), source: "this phone")
            }
        }
        .presentationDetents([.large])
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
    /// Lowest, highest and mean y: the chart's own domain (an area from zero would flatten a 72 m climb) and the
    /// VoiceOver summary.
    let lo: Double
    let hi: Double
    let mean: Double

    init(id: String, color: Color, area: Bool, pts: [Pt]) {
        (self.id, self.color, self.area, self.pts) = (id, color, area, pts)
        let ys = pts.map(\.y)
        lo = ys.min() ?? 0
        hi = ys.max() ?? 0
        mean = ys.isEmpty ? 0 : ys.reduce(0, +) / Double(ys.count)
    }

    /// The y axis: the data's range with 10 % each side (at least one unit, for a flat line).
    var domain: ClosedRange<Double> {
        let pad = max((hi - lo) * 0.1, 1)
        return (lo - pad)...(hi + pad)
    }
}

/// What the viewer draws, worked out once per ride and axis (not on every drag tick of a chart).
struct Prep {
    var coords: [CLLocationCoordinate2D] = []
    /// The map points with their x, to find the one under the chart's cursor.
    var geo: [(x: Double, c: CLLocationCoordinate2D)] = []
    var series: [Series] = []
    /// The climbs' parts of the altitude line (set by `attach`).
    var climbSlices: [ClimbSlice] = []

    init() {}

    /// Cuts the climbs out of the altitude line, once, when the insight or the axis changes.
    mutating func attach(_ climbs: [ClimbStat], byTime: Bool) {
        guard let alt = series.first(where: { $0.id.hasPrefix("Altitude") }) else { climbSlices = []; return }
        climbSlices = climbs.indices.map { i in
            let r = climbs[i].xRange(byTime: byTime)
            return ClimbSlice(id: i, color: Tok.grade(Int(climbs[i].avgGradeX10)), pts: alt.pts.filter { r.contains($0.x) })
        }
    }

    init(_ r: RideView, byTime: Bool) {
        let xf: (TrackSample) -> Double = { byTime ? Double($0.tS) / 60 : $0.distM / 1000 }
        for s in r.samples {
            guard let a = s.lat, let o = s.lon, abs(a) <= 90, abs(o) <= 180 else { continue }
            let c = CLLocationCoordinate2D(latitude: a, longitude: o)
            coords.append(c)
            geo.append((xf(s), c))
        }
        let defs: [(String, Color, Bool, (TrackSample) -> Double?)] = [
            ("Altitude (m)", .secondary, true, { $0.altM }),
            ("Speed (km/h)", Tok.speed, false, { $0.speedMs.map { $0 * 3.6 } }),
            ("Heart rate (bpm)", Tok.heartRate, false, { $0.hr.map(Double.init) }),
            ("Power (W)", Tok.power, false, { $0.power.map(Double.init) }),
            ("Cadence (rpm)", Tok.cadence, false, { $0.cadence.map(Double.init) }),
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
    @AppStorage("weight") private var weight = 0
    @AppStorage("wPrime") private var wPrime = 0
    @State private var insight: RideInsight?
    @State private var ride: RideView?
    @State private var prep = Prep()
    @State private var error: String?
    @State private var byTime = false
    @State private var selX: Double?
    @State private var sharing = false
    @State private var profile = false
    @ScaledMetric private var mapHeight: CGFloat = 280

    /// The start in the phone's time zone: the title and the summary's date are both this one value.
    private var startDate: Date? { ride.map { Date(timeIntervalSince1970: TimeInterval($0.summary.startUnix)) } }

    var body: some View {
        ScrollViewReader { proxy in
        ScrollView {
            if let r = ride {
                VStack(alignment: .leading, spacing: 20) {
                    SummaryGrid(s: r.summary)
                    IntervalsRideLink(url: url)
                    if !prep.coords.isEmpty {
                        RideMap(
                            points: prep.coords, selected: selX.flatMap { x in Prep.nearest(prep.geo, x) { $0.x }?.c },
                            climbs: (insight?.climbs ?? []).map {
                                ClimbLine.Spec(coords: $0.coords(in: r.samples), color: Tok.uiGrade(Int($0.avgGradeX10)))
                            })
                            .frame(height: mapHeight)
                            .accessibilityElement()
                            .accessibilityLabel("Route map, \(Fmt.km(r.summary.distanceM))")
                            .overlay(alignment: .bottomTrailing) {
                                if RideMap.usesIGN(prep.coords) {
                                    Text("© IGN").font(.caption2).padding(.horizontal, 4)
                                        .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 4)).padding(4)
                                }
                            }
                            .clipShape(RoundedRectangle(cornerRadius: 12))
                    }
                    graphs.id("graphs")
                    if let i = insight { insightSections(r, i) }
                    if !r.summary.laps.isEmpty { LapList(laps: r.summary.laps).id("laps") }
                    let edges = zoneUpper(ftp: UInt16(clamping: ftp), maxHr: UInt16(clamping: maxHR))
                    if r.summary.powerZoneS.contains(where: { $0 > 0 }) {
                        ZoneBars(
                            title: "Power zones", secs: r.summary.powerZoneS, upper: Array(edges.dropFirst(4)), unit: "W",
                            color: Tok.zone)
                    }
                    if r.summary.hrZoneS.contains(where: { $0 > 0 }) {
                        ZoneBars(
                            title: "Heart rate zones", secs: r.summary.hrZoneS, upper: Array(edges.prefix(4)), unit: "bpm",
                            color: Tok.hrZone)
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
        .navigationTitle(startDate.map(Fmt.when) ?? RideBrowser.title(url.deletingPathExtension().lastPathComponent))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            Button { profile = true } label: { Image(systemName: "person.crop.circle") }
                .accessibilityLabel("Profile")
            Button { sharing = true } label: { Image(systemName: "square.and.arrow.up") }
                .accessibilityLabel("Share ride")
        }
        .sheet(isPresented: $sharing) { ShareSheet(url: url) }
        .sheet(isPresented: $profile) { ProfileSheet() }
        .task(id: "\(ftp) \(maxHR)") { await load() }
        // off the main thread and cached for the session; CP is the FTP, as the unit takes it
        .task(id: "\(ftp) \(weight) \(wPrime)") {
            insight = await InsightCache.shared.insight(
                url, weight: UInt16(clamping: weight), cp: UInt16(clamping: ftp), wPrime: UInt16(clamping: wPrime))
        }
        .onChange(of: byTime) { _, t in
            selX = nil
            if let r = ride {
                prep = Prep(r, byTime: t)
                prep.attach(insight?.climbs ?? [], byTime: t)
            }
        }
        .onChange(of: insight) { _, i in prep.attach(i?.climbs ?? [], byTime: byTime) }
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
            prep.attach(insight?.climbs ?? [], byTime: byTime)
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
            ForEach(prep.series) { s in
                MetricChart(series: s, byTime: byTime, selX: $selX, slices: s.id.hasPrefix("Altitude") ? prep.climbSlices : [])
            }
        }
    }

    @ViewBuilder private func insightSections(_ r: RideView, _ i: RideInsight) -> some View {
        if !i.climbs.isEmpty { ClimbsSection(climbs: i.climbs).id("climbs") }
        if !i.wbal.isEmpty { WbalChart(points: i.wbal, minJ: i.wbalMinJ).id("wbal") }
        if let d = i.decoupling { DecouplingRow(d: d).id("decoupling") }
        if i.dfa.count > 1 { DfaChart(points: i.dfa, vt1: i.vt1, vt2: i.vt2).id("dfa") }
        CompareSection(
            url: url, view: r, climbs: i.climbs, weight: UInt16(clamping: weight), cp: UInt16(clamping: ftp),
            wPrime: UInt16(clamping: wPrime)
        ).id("compare")
    }
}

struct MetricChart: View {
    let series: Series
    let byTime: Bool
    @Binding var selX: Double?
    /// The climbs, drawn over the altitude line in their grade colour.
    var slices: [ClimbSlice] = []
    @ScaledMetric private var height: CGFloat = 110

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(series.id).font(.subheadline.bold())
                Spacer()
                if let sx = selX, let p = Prep.nearest(series.pts, sx, { $0.x }) {
                    Text(Fmt.dec(p.y, 0)).numeral(.subheadline).foregroundStyle(series.color)
                }
            }
            Chart {
                ForEach(series.pts) { p in
                    if series.area {
                        // from the series' own floor: a zero baseline would stretch the axis down to 0 m
                        AreaMark(x: .value("x", p.x), yStart: .value("floor", series.domain.lowerBound), yEnd: .value("y", p.y))
                            .foregroundStyle(series.color.opacity(0.3))
                    }
                    LineMark(x: .value("x", p.x), y: .value("y", p.y)).foregroundStyle(series.color)
                }
                ForEach(slices) { c in
                    ForEach(c.pts) { p in
                        LineMark(x: .value("x", p.x), y: .value("y", p.y), series: .value("climb", c.id))
                            .foregroundStyle(c.color).lineStyle(StrokeStyle(lineWidth: 4))
                    }
                }
            }
            .chartXSelection(value: $selX)
            .chartXAxisLabel(byTime ? "min" : "km")
            .chartYScale(domain: series.domain)
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
            .frame(height: height)
            .accessibilityLabel(series.id)
            .accessibilityValue(
                "lowest \(Fmt.dec(series.lo, 0)), highest \(Fmt.dec(series.hi, 0)), average \(Fmt.dec(series.mean, 0))")
        }
    }
}

struct SummaryGrid: View {
    let s: RideSummary
    /// A tile's least width grows with the type, so a label never breaks mid-word and a value is not clipped.
    @ScaledMetric(relativeTo: .headline) private var tileMin: CGFloat = 100

    var body: some View {
        let cells: [(String, String)?] = [
            ("Distance", Fmt.km(s.distanceM)),
            ("Moving", Fmt.clock(s.movingS)),
            ("Elapsed", Fmt.clock(s.elapsedS)),
            s.avgSpeedMs.map { ("Avg speed", Fmt.kmh($0)) },
            s.maxSpeedMs.map { ("Max speed", Fmt.kmh($0)) },
            s.ascentM.map { ("Ascent", "\($0) m") },
            s.descentM.map { ("Descent", "\($0) m") },
            s.avgHr.map { ("Avg HR", Fmt.bpm($0)) },
            s.maxHr.map { ("Max HR", Fmt.bpm($0)) },
            s.avgPower.map { ("Avg power", Fmt.watts($0)) },
            s.maxPower.map { ("Max power", Fmt.watts($0)) },
            s.np.map { ("NP", Fmt.watts($0)) },
            s.ifX100.map { ("IF", Fmt.ifx($0)) },
            s.tss.map { ("TSS", "\($0)") },
            s.avgCadence.map { ("Avg cadence", "\($0) rpm") },
            s.kcal.map { ("Energy", "\($0) kcal") },
        ]
        LazyVGrid(columns: [GridItem(.adaptive(minimum: tileMin), spacing: 8)], spacing: 8) {
            ForEach(cells.compactMap { $0 }, id: \.0) { c in
                VStack(alignment: .leading) {
                    Text(c.0).font(.caption).foregroundStyle(.secondary)
                    Text(c.1).numeral(.title3).lineLimit(1).minimumScaleFactor(0.7)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .tile()
                .accessibilityElement(children: .combine)
            }
        }
    }
}

struct LapList: View {
    let laps: [RideLap]

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Laps").font(.subheadline.bold())
            ForEach(laps.indices, id: \.self) { i in LapRow(index: i + 1, lap: laps[i]) }
        }
    }
}

/// One lap: a line when it fits, else a two-line card (the numbers never wrap digit by digit).
struct LapRow: View {
    let index: Int
    let lap: RideLap

    var body: some View {
        let rest = [
            Fmt.km(lap.distM, 2), lap.avgSpeedMs.map(Fmt.kmh), lap.avgPower.map { Fmt.watts($0) }, lap.avgHr.map { Fmt.bpm($0) },
        ].compactMap { $0 }
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 12) {
                head
                Spacer(minLength: 4)
                ForEach(rest, id: \.self) { Text($0).fixedSize() }
            }
            VStack(alignment: .leading, spacing: 2) {
                head
                Text(rest.joined(separator: " · ")).foregroundStyle(.secondary)
            }
        }
        .font(.caption.monospacedDigit())
        .accessibilityElement(children: .combine)
    }

    private var head: some View {
        HStack(spacing: 12) {
            Text("\(index)").numeral(.subheadline).frame(minWidth: 24, alignment: .leading)
            Text(Fmt.clock(lap.timeS)).numeral(.subheadline).fixedSize()
        }
    }
}

/// Time in each zone: the zone's colour for the bar, "Z2 152 to 205 W" on the left, "1:12  41 %" on the right.
struct ZoneBars: View {
    let title: String
    let secs: [UInt32]
    /// Where each zone but the last ends (W or bpm): the next starts one over.
    let upper: [UInt16]
    let unit: String
    let color: (Int) -> Color
    @ScaledMetric(relativeTo: .caption) private var barHeight: CGFloat = 10

    var body: some View {
        let total = max(secs.reduce(0, +), 1)
        let longest = max(secs.max() ?? 1, 1)
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.subheadline.bold())
            ForEach(secs.indices, id: \.self) { i in
                VStack(alignment: .leading, spacing: 2) {
                    HStack(alignment: .firstTextBaseline) {
                        Text("Z\(i + 1)").font(.caption.bold())
                        Text(range(i)).font(.caption).foregroundStyle(.secondary)
                    }
                    HStack(spacing: 8) {
                        GeometryReader { g in
                            Capsule().fill(color(i + 1)).frame(width: max(g.size.width * Double(secs[i]) / Double(longest), secs[i] > 0 ? 4 : 0))
                        }
                        .frame(height: barHeight)
                        Text("\(Fmt.hm(secs[i]))  \(secs[i] * 100 / total) %").font(.caption.monospacedDigit()).fixedSize()
                    }
                }
                .accessibilityElement(children: .combine)
            }
        }
    }

    /// "0 to 111 W", "152 to 205 W", "over 300 W".
    private func range(_ i: Int) -> String {
        let lo = i == 0 ? 0 : Int(upper[safe: i - 1] ?? 0) + 1
        guard let hi = upper[safe: i] else { return "over \(lo - 1) \(unit)" }
        return "\(lo) to \(hi) \(unit)"
    }
}

struct CurveChart: View {
    let points: [CurvePoint]
    @ScaledMetric private var height: CGFloat = 140

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Power curve").font(.subheadline.bold())
            Chart(points, id: \.secs) { p in
                BarMark(x: .value("Duration", Fmt.span(p.secs)), y: .value("W", p.watts))
                    .foregroundStyle(Tok.power)
                    .annotation(position: .top) { Text("\(p.watts)").font(.caption2) }
            }
            .frame(height: height)
            .accessibilityLabel("Power curve")
            .accessibilityValue(points.map { "\(Fmt.span($0.secs)) \($0.watts) W" }.joined(separator: ", "))
        }
    }
}
