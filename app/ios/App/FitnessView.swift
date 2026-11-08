// The Fitness screen: CTL / ATL / TSB over 90 days, eFTP and the 90-day power curve. With an Intervals.icu key the
// numbers are Intervals.icu's (wellness and power-curves endpoints, see app/core/src/fitness.rs); without one, or when
// it cannot be reached, CTL and ATL are worked out from the loads of the rides this phone holds. The screen says which.

import Charts
import SwiftUI

extension IntervalsClient {
    /// No cache or cookies on disk, as the profile's session.
    private static let getSession = URLSession(configuration: .ephemeral)

    /// A GET of `path` (under /api/v1/athlete/{id}/) with its query, the answer's bytes or why not, in plain words.
    static func get(key: String, athlete: String, path: String, query: [URLQueryItem]) async throws -> Data {
        #if DEBUG
        if CommandLine.arguments.contains("-demo") { return DemoFitness.answer(path) }
        #endif
        let id = athlete.trimmingCharacters(in: .whitespacesAndNewlines)
        let aid = id.isEmpty ? "0" : id
        guard aid.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber) }),
            var c = URLComponents(string: "https://intervals.icu/api/v1/athlete/\(aid)/\(path)")
        else { throw IntervalsFailure(text: "The athlete id should be 0 or like i12345.") }
        c.queryItems = query
        guard let url = c.url else { throw IntervalsFailure(text: "Could not build the request.") }
        var req = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 20)
        let k = key.trimmingCharacters(in: .whitespacesAndNewlines)
        req.setValue("Basic " + Data("API_KEY:\(k)".utf8).base64EncodedString(), forHTTPHeaderField: "Authorization")
        let (data, resp): (Data, URLResponse)
        do {
            (data, resp) = try await getSession.data(for: req)
        } catch {
            throw IntervalsFailure(text: "Could not reach Intervals.icu.")
        }
        switch (resp as? HTTPURLResponse)?.statusCode ?? 0 {
        case 200: return data
        case 401, 403: throw IntervalsFailure(text: "Intervals.icu refused the API key.")
        case let c: throw IntervalsFailure(text: "Intervals.icu answered with an error (\(c)).")
        }
    }
}

struct FitnessView: View {
    @ObservedObject var library: RideLibrary
    @AppStorage("ftp") private var ftp = 200
    @AppStorage("maxHR") private var maxHR = 190
    @State private var days: [FitnessDay] = []
    @State private var eftp: Double?
    @State private var curve: [CurvePoint] = []
    @State private var source = ""
    @State private var note: String?
    @State private var loading = true

    private static let isoDay: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd"
        return f
    }()

    private func date(_ day: Int64) -> Date { Date(timeIntervalSince1970: Double(day) * 86_400 + 43_200) }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                if let d = days.last {
                    HStack {
                        stat("Fitness", String(format: "%.0f", d.ctl), .blue)
                        stat("Fatigue", String(format: "%.0f", d.atl), .pink)
                        stat("Form", String(format: "%+.0f", d.tsb), d.tsb >= 0 ? .green : .orange)
                        if let e = eftp { stat("eFTP", String(format: "%.0f W", e), .orange) }
                    }
                    loadChart
                    formChart
                } else if loading {
                    ProgressView().padding(40)
                } else {
                    Text("No rides to work a trend from yet. Download some, or add an Intervals.icu key in the profile.")
                        .foregroundStyle(.secondary)
                }
                if !curve.isEmpty { power }
                if !source.isEmpty { Text(source).font(.footnote).foregroundStyle(.secondary) }
                if let n = note { Text(n).font(.footnote).foregroundStyle(.orange) }
            }
            .padding()
        }
        .navigationTitle("Fitness")
        .navigationBarTitleDisplayMode(.inline)
        .task(id: "\(ftp) \(maxHR) \(library.briefs.count)") { await load() }
    }

    private func stat(_ label: String, _ value: String, _ color: Color) -> some View {
        VStack(alignment: .leading) {
            Text(label).font(.caption).foregroundStyle(.secondary)
            Text(value).font(.title2.bold().monospacedDigit()).foregroundStyle(color)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(8)
        .background(Color(.secondarySystemBackground), in: RoundedRectangle(cornerRadius: 8))
    }

    private var loadChart: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Fitness and fatigue").font(.subheadline.bold())
            Chart(days, id: \.day) { d in
                LineMark(x: .value("Day", date(d.day)), y: .value("Load", d.ctl), series: .value("s", "Fitness (CTL)"))
                    .foregroundStyle(by: .value("Series", "Fitness (CTL)"))
                LineMark(x: .value("Day", date(d.day)), y: .value("Load", d.atl), series: .value("s", "Fatigue (ATL)"))
                    .foregroundStyle(by: .value("Series", "Fatigue (ATL)"))
            }
            .chartForegroundStyleScale(["Fitness (CTL)": Color.blue, "Fatigue (ATL)": Color.pink])
            .frame(height: 180)
        }
    }

    private var formChart: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Form (TSB)").font(.subheadline.bold())
            Chart(days, id: \.day) { d in
                AreaMark(x: .value("Day", date(d.day)), y: .value("TSB", d.tsb)).foregroundStyle(.green.opacity(0.3))
                LineMark(x: .value("Day", date(d.day)), y: .value("TSB", d.tsb)).foregroundStyle(.green)
                RuleMark(y: .value("zero", 0)).foregroundStyle(Color.gray)
            }
            .frame(height: 120)
        }
    }

    private var power: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Power curve, last 90 days").font(.subheadline.bold())
            Chart(curve, id: \.secs) { p in
                BarMark(x: .value("Duration", p.secs < 60 ? "\(p.secs) s" : "\(p.secs / 60) min"), y: .value("W", p.watts))
                    .foregroundStyle(.orange)
                    .annotation(position: .top) { Text("\(p.watts)").font(.caption2) }
            }
            .frame(height: 150)
        }
    }

    private func load() async {
        loading = true
        defer { loading = false }
        #if DEBUG
        let demo = CommandLine.arguments.contains("-demo")
        #else
        let demo = false
        #endif
        let now = Date()
        let tz = TimeZone.current
        let today = Int64(((now.timeIntervalSince1970 + Double(tz.secondsFromGMT(for: now))) / 86_400).rounded(.down))
        if let key = demo ? "demo" : Keychain.get("apikey") {
            do {
                let athlete = UserDefaults.standard.string(forKey: IntervalsSync.athleteKey) ?? ""
                let q = [
                    URLQueryItem(name: "oldest", value: Self.isoDay.string(from: date(today - 89))),
                    URLQueryItem(name: "newest", value: Self.isoDay.string(from: date(today))),
                ]
                let w = try wellnessParse(json: try await IntervalsClient.get(key: key, athlete: athlete, path: "wellness.json", query: q))
                guard !w.days.isEmpty else { throw IntervalsFailure(text: "Intervals.icu has no fitness values for these days.") }
                (days, eftp, source, note) = (w.days, w.eftp, "Source: Intervals.icu (wellness)", nil)
                // the curve is a bonus: its failure leaves the rest
                let cq = [URLQueryItem(name: "curves", value: "90d"), URLQueryItem(name: "type", value: "Ride")]
                curve = (try? powerCurveParse(json: try await IntervalsClient.get(key: key, athlete: athlete, path: "power-curves.json", query: cq))) ?? []
                return
            } catch let e as IntervalsFailure {
                note = e.text + " Showing this phone's rides."
            } catch {
                note = "Intervals.icu answered with something unreadable. Showing this phone's rides."
            }
        }
        // from the cached ride loads: the tool's own 42 and 7 day averages
        let loads: [DayLoad] = library.briefs.compactMap { name, b in
            let off = Int32(clamping: tz.secondsFromGMT(for: Date(timeIntervalSince1970: Double(b.startUnix))))
            let s = rideStat(b: b, name: name, ftp: UInt16(clamping: ftp), maxHr: UInt16(clamping: maxHR), tzOffsetS: off)
            guard let l = s.load else { return nil }
            let day = s.localDay ?? Int64((Double(b.startUnix) + Double(off)) / 86_400)
            return DayLoad(day: day, load: l)
        }
        (days, eftp, curve) = (fitnessFromLoads(loads: loads, today: today), nil, [])
        source = "From this phone's rides: 42 and 7 day averages of their loads. Intervals.icu's numbers need its key (profile)."
    }
}

#if DEBUG
/// The Simulator's screenshots: a made-up answer, no network.
enum DemoFitness {
    static func answer(_ path: String) -> Data {
        if path.hasPrefix("power-curves") {
            let j = #"{"list":[{"days":90,"secs":[5,60,300,1200,3600],"values":[820,455,318,262,231]}]}"#
            return Data(j.utf8)
        }
        let cal = Calendar(identifier: .gregorian)
        let f = DateFormatter()
        f.dateFormat = "yyyy-MM-dd"
        var (c, a) = (38.0, 40.0)
        let items = (0..<90).map { i -> String in
            let load = [0.0, 55, 0, 70, 40, 130, 90][(i * 3 + i / 7) % 7] * (i > 60 ? 1.2 : 1.0)
            (c, a) = (c + (load - c) / 42, a + (load - a) / 7)
            let d = cal.date(byAdding: .day, value: i - 89, to: Date()) ?? Date()
            return #"{"id":"\#(f.string(from: d))","ctl":\#(c),"atl":\#(a),"sportInfo":[{"type":"Ride","eftp":251.5}]}"#
        }
        return Data(("[" + items.joined(separator: ",") + "]").utf8)
    }
}
#endif
