// Intervals.icu both ways: rides synced from the unit go up, and planned workouts of the calendar go down to the unit.
// The parsing, the selection and the decisions are in app/core/src/calendar.rs, which cites the API pages; this file
// does the network (the key only through the Keychain accessor, never in a message or a log), keeps the sets on disk
// and hands the unit's changes to RideBrowser, which sends them when the link is idle.

import Foundation

// MARK: Network

extension IntervalsClient {
    /// `yyyyMMdd` and `yyyy-MM-dd` in the phone's zone: the calendar's local days.
    static func day(_ d: Date, iso: Bool = false) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = iso ? "yyyy-MM-dd" : "yyyyMMdd"
        return f.string(from: d)
    }

    private static func base(_ athlete: String) throws -> String {
        let a = athlete.trimmingCharacters(in: .whitespacesAndNewlines)
        let id = a.isEmpty ? "0" : a
        guard id.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber) }) else {
            throw IntervalsFailure(text: "The athlete id should be 0 or like i12345.", retry: false)
        }
        return "https://intervals.icu/api/v1/athlete/\(id)"
    }

    private static func call(_ url: URL, key: String, file: (URL, String)? = nil) async throws -> (Data, Int) {
        var req = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 30)
        let k = key.trimmingCharacters(in: .whitespacesAndNewlines)
        req.setValue("Basic " + Data("API_KEY:\(k)".utf8).base64EncodedString(), forHTTPHeaderField: "Authorization")
        if let (_, type) = file {
            req.httpMethod = "POST"
            req.setValue(type, forHTTPHeaderField: "Content-Type")
        }
        do {
            let (data, resp): (Data, URLResponse)
            if let (f, _) = file { (data, resp) = try await session.upload(for: req, fromFile: f) } else { (data, resp) = try await session.data(for: req) }
            return (data, (resp as? HTTPURLResponse)?.statusCode ?? 0)
        } catch {
            throw IntervalsFailure(text: "Could not reach Intervals.icu. Check the connection.")
        }
    }

    private static func refused(_ status: Int) -> IntervalsFailure {
        switch status {
        case 401, 403:
            IntervalsFailure(text: "Intervals.icu refused the API key. Check the key and the athlete id.", auth: true, retry: false)
        case 429: IntervalsFailure(text: "Intervals.icu asked to slow down (429).")
        case 500...599: IntervalsFailure(text: "Intervals.icu had an error (\(status)).")
        default: IntervalsFailure(text: "Intervals.icu answered with an error (\(status)).", retry: false)
        }
    }

    /// The WORKOUT events from `first` to `last`, local days.
    /// https://intervals.icu/api/v1/docs : GET /api/v1/athlete/{id}/events (query oldest, newest, category).
    static func events(key: String, athlete: String, first: Date, last: Date) async throws -> [CalendarEvent] {
        #if DEBUG
        if CommandLine.arguments.contains("-demo") { return demoEvents(first: first, last: last) }
        #endif
        guard var c = URLComponents(string: try base(athlete) + "/events") else { throw IntervalsFailure(text: "Bad athlete id.", retry: false) }
        c.queryItems = [
            URLQueryItem(name: "oldest", value: day(first, iso: true)), URLQueryItem(name: "newest", value: day(last, iso: true)),
            URLQueryItem(name: "category", value: "WORKOUT"), URLQueryItem(name: "limit", value: "500"),
        ]
        guard let url = c.url else { throw IntervalsFailure(text: "Bad athlete id.", retry: false) }
        let (data, status) = try await call(url, key: key)
        guard status == 200 else { throw refused(status) }
        guard let e = calendarParse(json: data, first: day(first), last: day(last)) else {
            throw IntervalsFailure(text: "Intervals.icu answered with something that is not a calendar.")
        }
        return e
    }

    /// A planned workout as ZWO: GET /api/v1/athlete/{id}/events/{eventId}/download.zwo ("Download a planned workout in zwo,
    /// mrc, erg or fit format"; the dot of `{ext}` is inferred).
    static func zwo(key: String, athlete: String, event: Int64) async throws -> Data {
        #if DEBUG
        if CommandLine.arguments.contains("-demo") { return demoZwo(event) }
        #endif
        guard let url = URL(string: try base(athlete) + "/events/\(event)/download.zwo") else { throw IntervalsFailure(text: "Bad athlete id.", retry: false) }
        let (data, status) = try await call(url, key: key)
        guard status == 200 else { throw refused(status) }
        return data
    }

    /// POST /api/v1/athlete/{id}/activities, multipart field `file`: 201 created, 200 already there (de-duped on a hash of
    /// the file). A duplicate counts as uploaded.
    /// The body is built in a temp file off the main thread (a ride is hundreds of KB) and sent from there; nil when the
    /// ride file cannot be read.
    static func upload(key: String, athlete: String, name: String, file: URL) async throws -> Uploaded? {
        #if DEBUG
        if CommandLine.arguments.contains("-demo") {
            try await Task.sleep(nanoseconds: 300_000_000)
            return Uploaded(activityId: "i" + name.suffix(6), duplicate: false)
        }
        #endif
        guard let url = URL(string: try base(athlete) + "/activities") else { throw IntervalsFailure(text: "Bad athlete id.", retry: false) }
        let boundary = "bryton-" + UUID().uuidString
        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("upload-\(UUID().uuidString)")
        let built = await Task.detached(priority: .utility) { () -> Bool in
            guard let bytes = try? Data(contentsOf: file), !bytes.isEmpty else { return false }
            return (try? uploadBody(boundary: boundary, name: name, bytes: bytes).write(to: tmp)) != nil
        }.value
        guard built else { return nil }
        defer { try? FileManager.default.removeItem(at: tmp) }
        let (data, status) = try await call(url, key: key, file: (tmp, "multipart/form-data; boundary=\(boundary)"))
        if let u = uploadOutcome(status: UInt16(clamping: status), json: data) { return u }
        // the file itself is refused (bad request, unreadable, too big): not tried again
        if status == 400 || status == 413 || status == 415 || status == 422 {
            throw IntervalsFailure(text: "Intervals.icu could not read the file (\(status)).", retry: false, file: true)
        }
        throw refused(status)
    }

    #if DEBUG
    /// The Simulator's calendar: made-up workouts on the next days; the last has too many steps for the unit.
    private static func demoEvents(first: Date, last: Date) -> [CalendarEvent] {
        let names = [(9001, "Sweet spot 3x10"), (9002, "Over unders"), (9003, "Endurance 2h"), (9004, "Micro bursts 70")]
        let rows = names.enumerated().map { i, n in
            let d = Calendar.current.date(byAdding: .day, value: [1, 2, 4, 6][i], to: first) ?? first
            return "{\"id\":\(n.0),\"start_date_local\":\"\(day(d, iso: true))T00:00:00\",\"category\":\"WORKOUT\",\"name\":\"\(n.1)\",\"updated\":\"demo\"}"
        }
        return calendarParse(json: Data("[\(rows.joined(separator: ","))]".utf8), first: day(first), last: day(last)) ?? []
    }

    private static func demoZwo(_ id: Int64) -> Data {
        let step = "<SteadyState Duration=\"60\" Power=\"0.6\"/>"
        let body: String
        switch id {
        case 9001: body = "<Warmup Duration=\"600\" PowerLow=\"0.5\" PowerHigh=\"0.75\"/><IntervalsT Repeat=\"3\" OnDuration=\"600\" OnPower=\"0.9\" OffDuration=\"300\" OffPower=\"0.5\"/><Cooldown Duration=\"600\" PowerLow=\"0.6\" PowerHigh=\"0.4\"/>"
        case 9002: body = "<Warmup Duration=\"600\" PowerLow=\"0.5\" PowerHigh=\"0.75\"/><IntervalsT Repeat=\"4\" OnDuration=\"240\" OnPower=\"1.0\" OffDuration=\"240\" OffPower=\"0.88\"/><Cooldown Duration=\"600\" PowerLow=\"0.6\" PowerHigh=\"0.4\"/>"
        case 9003: body = "<SteadyState Duration=\"7200\" Power=\"0.65\"/>"
        default: body = String(repeating: step, count: 70)
        }
        return Data("<workout_file><name>Demo</name><workout>\(body)</workout></workout_file>".utf8)
    }
    #endif
}

// MARK: The loop

/// One planned workout of the calendar and where it stands.
struct PlannedItem: Identifiable, Equatable {
    enum State: Equatable { case waiting, onUnit, refused(String) }
    let id: Int64
    let day: String  // YYYYMMDD
    let name: String
    var state: State
}

private struct SentRec: Codable {
    let e: Int64
    let d: String
    let u: String
    let h: UInt32
    var p: Bool?  // a file the rider already had (older stores lack it)
}

@MainActor
final class IntervalsLoop: ObservableObject {
    /// Ride name to Intervals.icu's activity id (empty when the answer gave none).
    @Published private(set) var uploaded: [String: String]
    @Published private(set) var planned: [PlannedItem] = []
    /// CRCs of the unit files this app sent: the unit's list marks them "from Intervals.icu".
    @Published private(set) var sentHashes = Set<UInt32>()
    @Published private(set) var lastUpload: Date?
    @Published private(set) var lastCalendar: Date?
    @Published private(set) var uploadError: String?
    @Published private(set) var calendarError: String?
    @Published private(set) var uploading = false
    @Published private(set) var calendarBusy = false
    /// The calendar was read in this session (`planned` is filled).
    @Published private(set) var calendarRead = false
    @Published private(set) var uploadOn: Bool
    @Published private(set) var calendarOn: Bool

    private weak var rides: RideBrowser?
    private var records: [Sent]
    private var rejected: Set<String>
    /// Rides refused or read again from the unit this launch: a rejected ride gets one more try a launch (a damaged
    /// copy read again, or a ride Intervals.icu already holds, which it answers as a duplicate: then marked uploaded).
    private var tried = Set<String>()
    private var events: [CalendarEvent] = []
    /// Events the unit cannot hold, by id: the stamp they had and why (not downloaded again until they change).
    private var refusedAt: [Int64: (stamp: String, why: String)] = [:]
    private var failures = 0
    private var uploadNotBefore = Date.distantPast
    private var calendarRetryAt: Date?
    /// The key was refused: no pass runs until a key is entered again or an upload switch is turned on.
    private var keyRefused = false
    /// The unit cannot take calendar writes (no calendar folder, read only): not asked again before this.
    private var hardUntil: Date?
    private var timer: Timer?
    private var retryTask: Task<Void, Never>?
    private let d = UserDefaults.standard

    init() {
        uploaded = (d.dictionary(forKey: "ivUploaded") as? [String: String]) ?? [:]
        rejected = Set(d.stringArray(forKey: "ivRejected") ?? [])
        uploadOn = d.bool(forKey: "ivUpload")
        calendarOn = d.bool(forKey: "ivCalendar")
        lastUpload = d.object(forKey: "ivLastUpload") as? Date
        lastCalendar = d.object(forKey: "ivLastCalendar") as? Date
        let recs = (d.data(forKey: "ivSent").flatMap { try? JSONDecoder().decode([SentRec].self, from: $0) }) ?? []
        records = recs.map { Sent(eventId: $0.e, day: $0.d, updated: $0.u, hash: $0.h, pre: $0.p ?? false) }
        sentHashes = Set(records.map(\.hash))
    }

    static func key() -> String? {
        #if DEBUG
        if CommandLine.arguments.contains("-demo") { return "demo" }
        #endif
        return Keychain.get("apikey")
    }

    private var athlete: String { d.string(forKey: IntervalsSync.athleteKey) ?? "" }

    private static var isDemo: Bool {
        #if DEBUG
        CommandLine.arguments.contains("-demo")
        #else
        false
        #endif
    }

    /// The sets belong to one athlete id: another id (another account) starts them again. Files already sent to the unit
    /// stay there. Shortcut: the same id with another key is not noticed.
    private func checkAccount() {
        let a = athlete.trimmingCharacters(in: .whitespacesAndNewlines)
        let id = a.isEmpty ? "0" : a
        let old = d.string(forKey: "ivAccount")
        guard old != id else { return }
        d.set(id, forKey: "ivAccount")
        guard old != nil else { return }
        (uploaded, rejected, records, refusedAt, events, planned) = ([:], [], [], [:], [], [])
        for k in ["ivUploaded", "ivRejected", "ivSent", "ivCalTry"] { d.removeObject(forKey: k) }
        d.set(Date().timeIntervalSince1970, forKey: "ivUploadSince")
        saveRecords()
    }

    /// Wires the loop to the unit's link and starts its slow clock (every 10 minutes: pending uploads, the hourly calendar).
    func attach(_ rides: RideBrowser) {
        self.rides = rides
        rides.onCalendarJob = { [weak self] job, err, next, present in
            Task { @MainActor in self?.jobDone(job, err, next, present) }
        }
        guard timer == nil else { return }
        timer = Timer.scheduledTimer(withTimeInterval: 600, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.kick() }
        }
    }

    /// The key appeared or went: the first key turns both features on, and only rides from now on go up.
    func keyChanged(_ has: Bool) {
        keyRefused = false
        guard has else { return }
        if d.object(forKey: "ivUpload") == nil { setUpload(true) }
        if d.object(forKey: "ivCalendar") == nil { setCalendar(true) }
        if d.object(forKey: "ivUploadSince") == nil { d.set(Date().timeIntervalSince1970, forKey: "ivUploadSince") }
        kick()
    }

    func setUpload(_ on: Bool) {
        uploadOn = on
        d.set(on, forKey: "ivUpload")
        if on {
            keyRefused = false
            d.set(Date().timeIntervalSince1970, forKey: "ivUploadSince")  // rides before this moment are not sent
            kick()
        }
    }

    func setCalendar(_ on: Bool) {
        calendarOn = on
        d.set(on, forKey: "ivCalendar")
        if on { calendarPass(force: true) } else { rides?.setCalendarJobs([]) }
    }

    func kick(force: Bool = false) {
        checkAccount()
        uploadPass()
        calendarPass(force: force)
    }

    private func scheduleKick(after s: TimeInterval) {
        retryTask?.cancel()
        retryTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(max(s, 1) * 1_000_000_000))
            if !Task.isCancelled { self?.kick() }
        }
    }

    // MARK: Rides up

    private func eligible(_ name: String) -> Bool {
        #if DEBUG
        if CommandLine.arguments.contains("-demo") { return true }
        #endif
        guard let date = RideBrowser.date(name) else { return false }
        return date.timeIntervalSince1970 >= d.double(forKey: "ivUploadSince")
    }

    /// Downloaded rides not up yet, newest first; `all` also takes the ones from before the toggle was turned on.
    private func candidates(_ rides: RideBrowser, all: Bool) -> [String] {
        rides.downloaded.filter { uploaded[$0] == nil && !(rejected.contains($0) && tried.contains($0)) && (all || eligible($0)) }.sorted(by: >)
    }

    /// How many rides "Upload older rides" would send.
    func olderCount(_ rides: RideBrowser) -> Int {
        candidates(rides, all: true).count - candidates(rides, all: false).count
    }

    func uploadPass(older: Bool = false) {
        checkAccount()
        guard !uploading, !keyRefused, let rides, uploadOn || older, let key = Self.key() else { return }
        if !older && Date() < uploadNotBefore { return }
        let names = candidates(rides, all: older)
        guard !names.isEmpty else { return }
        uploading = true
        Task {
            await upload(names, key: key)
            uploading = false
        }
    }

    private func upload(_ names: [String], key: String) async {
        for name in names {
            // one after the other; a ride the phone cannot read is skipped, not retried in a loop
            do {
                guard let r = try await IntervalsClient.upload(key: key, athlete: athlete, name: name, file: RideBrowser.path(name))
                else { continue }
                uploaded[name] = r.activityId
                d.set(uploaded, forKey: "ivUploaded")
                if rejected.remove(name) != nil { d.set(Array(rejected), forKey: "ivRejected") }
                (lastUpload, failures, uploadError) = (Date(), 0, nil)
                d.set(lastUpload, forKey: "ivLastUpload")
            } catch let e as IntervalsFailure {
                if e.auth {
                    keyRefused = true
                    uploadError = "Intervals.icu refused the key. Enter it again in the profile."
                    return
                }
                if e.retry {
                    // 2, 4, 8 ... minutes, an hour at most
                    failures += 1
                    let wait = min(3600, 120 * pow(2, Double(min(failures - 1, 10))))
                    uploadNotBefore = Date().addingTimeInterval(wait)
                    uploadError = e.text + " Will try again later."
                    scheduleKick(after: wait)
                    return
                }
                let first = tried.insert(name).inserted
                if e.file, first, let rides = self.rides {
                    // the phone's copy may be damaged in transit (2026-11-10): read it again before giving up
                    rides.forget(name)
                    continue
                }
                rejected.insert(name)
                d.set(Array(rejected), forKey: "ivRejected")
                uploadError = "Intervals.icu did not take the ride of \(RideBrowser.title(name)). \(e.text)"
            } catch {
                uploadError = "Upload stopped."
                return
            }
        }
    }

    // MARK: Calendar down

    func calendarPass(force: Bool) {
        checkAccount()
        guard !calendarBusy, !keyRefused, calendarOn, let rides, let key = Self.key() else { return }
        let now = Date()
        if !force, let h = hardUntil, now < h { return }
        let due = calendarRetryAt.map { now >= $0 } ?? false
        // at most hourly (an empty calendar counts too); the Simulator's demo always reads
        let tried = d.object(forKey: "ivCalTry") as? Date ?? .distantPast
        if !force && !due && !Self.isDemo && now.timeIntervalSince(tried) < 3600 { return }
        calendarBusy = true
        d.set(now, forKey: "ivCalTry")
        Task {
            await readCalendar(rides, key: key, today: now)
            calendarBusy = false
        }
    }

    private func readCalendar(_ rides: RideBrowser, key: String, today: Date) async {
        let last = Calendar.current.date(byAdding: .day, value: 7, to: today) ?? today
        let events: [CalendarEvent]
        do {
            events = try await IntervalsClient.events(key: key, athlete: athlete, first: today, last: last)
        } catch let e as IntervalsFailure {
            return failCalendar(e.text, auth: e.auth)
        } catch {
            return failCalendar("Could not read the calendar.")
        }
        self.events = events
        calendarRead = true
        let todayKey = IntervalsClient.day(today)
        var converted = [Planned](), network = false
        for ev in calendarToFetch(sent: records, events: events) where refusedAt[ev.id]?.stamp != ev.updated {
            do {
                let zwo = try await IntervalsClient.zwo(key: key, athlete: athlete, event: ev.id)
                do {
                    let f = try planWorkout(zwo: zwo, fallbackName: ev.name)
                    let r = Sent(eventId: ev.id, day: ev.day, updated: ev.updated, hash: f.hash, pre: false)
                    converted.append(Planned(record: r, bytes: f.bytes))
                } catch {
                    refusedAt[ev.id] = (ev.updated, Self.words(error))
                }
            } catch let e as IntervalsFailure where e.auth {
                keyRefused = true
                calendarError = "Intervals.icu refused the key. Enter it again in the profile."
                return
            } catch {
                network = true  // the rest would fail the same way; what was read is still used
                break
            }
        }
        var s = calendarSettle(
            sent: records, events: events.map(\.id), converted: converted, refused: refusedAt.filter { id, r in
                events.contains { $0.id == id && $0.updated == r.stamp }
            }.map(\.key), today: todayKey)
        // a workout that left the calendar is deleted from the unit only when a second read agrees, and never on an empty
        // or failed answer (shortcut: a calendar emptied on purpose is not followed, the files stay)
        if s.jobs.contains(where: \.gone) {
            let again = try? await IntervalsClient.events(key: key, athlete: athlete, first: today, last: last)
            s.jobs = calendarKeepGone(jobs: s.jobs, first: events.map(\.id), again: again?.map(\.id), records: UInt32(records.count))
        }
        let same = { (a: Sent, b: Sent) in a.eventId == b.eventId && a.day == b.day && a.hash == b.hash }
        for f in s.forget { records.removeAll { same($0, f) } }
        for r in s.refresh { records.removeAll { same($0, r) }; records.append(r) }
        saveRecords()
        rides.setCalendarJobs(s.jobs)
        lastCalendar = Date()
        d.set(lastCalendar, forKey: "ivLastCalendar")
        calendarError = network ? "Could not download every workout. Trying again soon." : nil
        calendarRetryAt = network ? Date().addingTimeInterval(900) : nil
        showPlanned()
    }

    private func failCalendar(_ text: String, auth: Bool = false) {
        calendarError = text
        if auth {
            keyRefused = true
            calendarError = "Intervals.icu refused the key. Enter it again in the profile."
            return
        }
        calendarRetryAt = Date().addingTimeInterval(900)
        scheduleKick(after: 900)
    }

    private static func words(_ e: Error) -> String {
        if case DraftError.Invalid(let m) = e { return m }
        return "The unit cannot hold this workout."
    }

    private func showPlanned() {
        planned = events.map { e in
            let state: PlannedItem.State
            if records.contains(where: { $0.eventId == e.id && $0.day == e.day && $0.updated == e.updated }) {
                state = .onUnit
            } else if let r = refusedAt[e.id], r.stamp == e.updated {
                state = .refused(r.why)
            } else {
                state = .waiting
            }
            return PlannedItem(id: e.id, day: e.day, name: e.name, state: state)
        }
    }

    private func saveRecords() {
        // a past day is not followed any more
        let today = IntervalsClient.day(Date())
        records.removeAll { $0.day < today }
        sentHashes = Set(records.map(\.hash))
        let recs = records.map { SentRec(e: $0.eventId, d: $0.day, u: $0.updated, h: $0.hash, p: $0.pre) }
        d.set(try? JSONEncoder().encode(recs), forKey: "ivSent")
    }

    /// RideBrowser finished (or gave up) one change of the unit's calendar. A record leaves the set only for an exact
    /// match (event, day, file), once its delete is done; a send never removes the old file's record.
    private func jobDone(_ job: CalendarJob, _ err: String?, _ next: JobNext, _ present: Bool) {
        let r = job.record
        let exact = { (x: Sent) in x.eventId == r.eventId && x.day == r.day && x.hash == r.hash }
        switch next {
        case .done:
            records.removeAll(where: exact)
            if job.bytes != nil {
                records.append(Sent(eventId: r.eventId, day: r.day, updated: r.updated, hash: r.hash, pre: present || r.pre))
                calendarError = nil
            }
        case .retry:
            return
        case .drop:
            // only this job: a workout the unit would not take is shown with the reason, stamped so it is not asked again
            if job.bytes != nil { refusedAt[r.eventId] = (r.updated, err ?? "The unit did not take it.") }
            calendarError = "Planned workouts: \(err ?? "a change was not made")"
        case .stop:
            calendarError = "Planned workouts: \(err ?? "stopped")"
            calendarRetryAt = Date().addingTimeInterval(900)
            scheduleKick(after: 900)
        case .stopHard:
            calendarError = "Planned workouts: \(err ?? "the unit cannot take them")"
            hardUntil = Date().addingTimeInterval(3600)
        }
        saveRecords()
        showPlanned()
    }
}
