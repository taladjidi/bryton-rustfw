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

/// What a source gave: the days, eFTP and the curve.
struct FitnessData {
    var days: [FitnessDay] = []
    var eftp: Double?
    var curve: [CurvePoint] = []
}

struct FitnessView: View {
    @ObservedObject var library: RideLibrary
    @EnvironmentObject private var profile: Profile
    private var ftp: Int { profile.ftp }
    private var maxHR: Int { profile.maxHR }
    /// Intervals.icu's answer, fetched once per key and athlete (a change of FTP or a new ride does not ask again).
    @State private var remote: FitnessData?
    @State private var note: String?
    @State private var fetching = true
    /// From this phone's rides, worked out again when they change.
    @State private var local = FitnessData()
    private var shown: FitnessData { remote ?? local }

    private var today: Int64 {
        let now = Date()
        return Int64(((now.timeIntervalSince1970 + Double(TimeZone.current.secondsFromGMT(for: now))) / 86_400).rounded(.down))
    }

    /// Days since 1970 as "YYYY-MM-DD" (Hinnant's civil_from_days, public domain), with no formatter or time zone.
    static func iso(_ day: Int64) -> String {
        let z = day + 719_468
        let era = (z >= 0 ? z : z - 146_096) / 146_097
        let doe = z - era * 146_097
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100)
        let mp = (5 * doy + 2) / 153
        let d = doy - (153 * mp + 2) / 5 + 1
        let m = mp < 10 ? mp + 3 : mp - 9
        return String(format: "%04d-%02d-%02d", yoe + era * 400 + (m <= 2 ? 1 : 0), m, d)
    }

    private func date(_ day: Int64) -> Date { Date(timeIntervalSince1970: Double(day) * 86_400 + 43_200) }

    private var hasKey: Bool {
        #if DEBUG
        if CommandLine.arguments.contains("-demo") { return true }
        #endif
        return Keychain.get("apikey") != nil
    }

    var body: some View {
        let d = shown
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                if let last = d.days.last {
                    let stats = Group {
                        stat("Fitness", Fmt.dec(last.ctl, 0), Tok.accent)
                        stat("Fatigue", Fmt.dec(last.atl, 0), Tok.zone(5))
                        stat("Form", Fmt.signed(last.tsb), Tok.form(last.tsb))
                        if let e = d.eftp { stat("eFTP", "\(Fmt.dec(e, 0)) W", Tok.accent) }
                    }
                    // four across, or a column when the type is large
                    ViewThatFits(in: .horizontal) {
                        HStack { stats }
                        VStack { stats }
                    }
                    if last.day < today - 1 {
                        Notice(text: "The last day is \(Self.iso(last.day)).").font(.footnote)
                    }
                    loadChart(d.days)
                    formChart(d.days)
                } else if fetching && remote == nil {
                    ProgressView().padding(40)
                } else {
                    Text("No rides to work a trend from yet. Download some, or add an Intervals.icu key in the profile.")
                        .foregroundStyle(.secondary)
                }
                if !d.curve.isEmpty { power(d.curve) }
                Text(remote != nil
                    ? "Source: Intervals.icu (wellness)"
                    : "From this phone's rides: 42 and 7 day averages of their loads. The series starts at 0, so it needs about 6 weeks of rides to be right. Intervals.icu's numbers need its key (profile).")
                    .font(.footnote).foregroundStyle(.secondary)
                if let n = note { Notice(text: n).font(.footnote) }
            }
            .padding()
        }
        .navigationTitle("Fitness")
        .navigationBarTitleDisplayMode(.inline)
        .task(id: "\(Keychain.get("apikey") ?? "") \(UserDefaults.standard.string(forKey: IntervalsSync.athleteKey) ?? "")") {
            await fetchRemote()
        }
        .task(id: "\(ftp) \(maxHR) \(library.briefs.count)") { computeLocal() }
    }

    private func stat(_ label: String, _ value: String, _ color: Color) -> some View {
        VStack(alignment: .leading) {
            Text(label).font(.caption).foregroundStyle(.secondary)
            Text(value).numeral(.title2).foregroundStyle(color)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .tile()
    }

    private func loadChart(_ days: [FitnessDay]) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Fitness and fatigue").font(.subheadline.bold())
            Chart(days, id: \.day) { d in
                LineMark(x: .value("Day", date(d.day)), y: .value("Load", d.ctl), series: .value("s", "Fitness (CTL)"))
                    .foregroundStyle(by: .value("Series", "Fitness (CTL)"))
                LineMark(x: .value("Day", date(d.day)), y: .value("Load", d.atl), series: .value("s", "Fatigue (ATL)"))
                    .foregroundStyle(by: .value("Series", "Fatigue (ATL)"))
            }
            .chartForegroundStyleScale(["Fitness (CTL)": Tok.accent, "Fatigue (ATL)": Tok.zone(5)])
            .frame(height: 180)
        }
    }

    private func formChart(_ days: [FitnessDay]) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Form (TSB)").font(.subheadline.bold())
            Chart(days, id: \.day) { d in
                AreaMark(x: .value("Day", date(d.day)), y: .value("TSB", d.tsb)).foregroundStyle(Tok.zone(2).opacity(0.3))
                LineMark(x: .value("Day", date(d.day)), y: .value("TSB", d.tsb)).foregroundStyle(Tok.zone(2))
                RuleMark(y: .value("zero", 0)).foregroundStyle(Color.secondary)
            }
            .frame(height: 120)
        }
    }

    private func power(_ curve: [CurvePoint]) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Power curve, last 90 days").font(.subheadline.bold())
            Chart(curve, id: \.secs) { p in
                BarMark(x: .value("Duration", Fmt.span(p.secs)), y: .value("W", p.watts))
                    .foregroundStyle(Tok.power)
                    .annotation(position: .top) { Text("\(p.watts)").font(.caption2) }
            }
            .frame(height: 150)
        }
    }

    private func fetchRemote() async {
        fetching = true
        defer { fetching = false }
        #if DEBUG
        let demo = CommandLine.arguments.contains("-demo")
        #else
        let demo = false
        #endif
        guard let key = demo ? "demo" : Keychain.get("apikey") else { (remote, note) = (nil, nil); return }
        do {
            let athlete = UserDefaults.standard.string(forKey: IntervalsSync.athleteKey) ?? ""
            let t = today
            let q = [URLQueryItem(name: "oldest", value: Self.iso(t - 89)), URLQueryItem(name: "newest", value: Self.iso(t))]
            let w = try wellnessParse(json: try await IntervalsClient.get(key: key, athlete: athlete, path: "wellness.json", query: q))
            guard !w.days.isEmpty else { throw IntervalsFailure(text: "Intervals.icu has no fitness values for these days.") }
            var r = FitnessData(days: w.days, eftp: w.eftp)
            // the curve is a bonus: its failure leaves the rest
            let cq = [URLQueryItem(name: "curves", value: "90d"), URLQueryItem(name: "type", value: "Ride")]
            r.curve = (try? powerCurveParse(json: try await IntervalsClient.get(key: key, athlete: athlete, path: "power-curves.json", query: cq))) ?? []
            (remote, note) = (r, nil)
        } catch let e as IntervalsFailure {
            (remote, note) = (nil, e.text + " Showing this phone's rides.")
        } catch {
            (remote, note) = (nil, "Intervals.icu answered with something unreadable. Showing this phone's rides.")
        }
    }

    /// From the cached ride loads: the firmware's 42 and 7 day averages.
    private func computeLocal() {
        let tz = TimeZone.current
        let loads: [DayLoad] = library.briefs.compactMap { name, b in
            let off = Int32(clamping: tz.secondsFromGMT(for: Date(timeIntervalSince1970: Double(b.startUnix))))
            let s = rideStat(b: b, name: name, ftp: UInt16(clamping: ftp), maxHr: UInt16(clamping: maxHR), tzOffsetS: off)
            guard let l = s.load else { return nil }
            return DayLoad(day: s.localDay ?? Int64((Double(b.startUnix) + Double(off)) / 86_400), load: l)
        }
        local = FitnessData(days: fitnessFromLoads(loads: loads, today: today))
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
