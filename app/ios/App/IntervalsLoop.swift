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

    private static func call(_ url: URL, key: String, post: (Data, String)? = nil) async throws -> (Data, Int) {
        var req = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 30)
        let k = key.trimmingCharacters(in: .whitespacesAndNewlines)
        req.setValue("Basic " + Data("API_KEY:\(k)".utf8).base64EncodedString(), forHTTPHeaderField: "Authorization")
        if let (body, type) = post {
            req.httpMethod = "POST"
            req.setValue(type, forHTTPHeaderField: "Content-Type")
            req.httpBody = body
        }
        do {
            let (data, resp) = try await session.data(for: req)
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
            URLQueryItem(name: "category", value: "WORKOUT"),
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
    static func upload(key: String, athlete: String, name: String, bytes: Data) async throws -> Uploaded {
        #if DEBUG
        if CommandLine.arguments.contains("-demo") {
            try await Task.sleep(nanoseconds: 300_000_000)
            return Uploaded(activityId: "i" + name.suffix(6), duplicate: false)
        }
        #endif
        guard let url = URL(string: try base(athlete) + "/activities") else { throw IntervalsFailure(text: "Bad athlete id.", retry: false) }
        let boundary = "bryton-" + UUID().uuidString
        let safe = String(name.filter { $0.isASCII && ($0.isLetter || $0.isNumber) })
        var body = Data("--\(boundary)\r\nContent-Disposition: form-data; name=\"file\"; filename=\"\(safe).fit\"\r\nContent-Type: application/octet-stream\r\n\r\n".utf8)
        body.append(bytes)
        body.append(Data("\r\n--\(boundary)--\r\n".utf8))
        let (data, status) = try await call(url, key: key, post: (body, "multipart/form-data; boundary=\(boundary)"))
        if let u = uploadOutcome(status: UInt16(clamping: status), json: data) { return u }
        // the file itself is refused (bad request, unreadable, too big): not tried again
        if status == 400 || status == 413 || status == 415 || status == 422 {
            throw IntervalsFailure(text: "Intervals.icu could not read the file (\(status)).", retry: false)
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
    @Published private(set) var uploadOn: Bool
    @Published private(set) var calendarOn: Bool

    private weak var rides: RideBrowser?
    private var records: [Sent]
    private var rejected: Set<String>
    private var events: [CalendarEvent] = []
    /// Events the unit cannot hold, by id: the stamp they had and why (not downloaded again until they change).
    private var refusedAt: [Int64: (stamp: String, why: String)] = [:]
    private var failures = 0
    private var uploadNotBefore = Date.distantPast
    private var calendarRetryAt: Date?
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
        records = recs.map { Sent(eventId: $0.e, day: $0.d, updated: $0.u, hash: $0.h) }
        sentHashes = Set(records.map(\.hash))
    }

    static func key() -> String? {
        #if DEBUG
        if CommandLine.arguments.contains("-demo") { return "demo" }
        #endif
        return Keychain.get("apikey")
    }

    private var athlete: String { d.string(forKey: IntervalsSync.athleteKey) ?? "" }

    /// Wires the loop to the unit's link and starts its slow clock (every 10 minutes: pending uploads, the hourly calendar).
    func attach(_ rides: RideBrowser) {
        self.rides = rides
        rides.onCalendarJob = { [weak self] job, err in Task { @MainActor in self?.jobDone(job, err) } }
        guard timer == nil else { return }
        timer = Timer.scheduledTimer(withTimeInterval: 600, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.kick() }
        }
    }

    /// The key appeared or went: the first key turns both features on, and only rides from now on go up.
    func keyChanged(_ has: Bool) {
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
        rides.downloaded.filter { uploaded[$0] == nil && !rejected.contains($0) && (all || eligible($0)) }.sorted(by: >)
    }

    /// How many rides "Upload older rides" would send.
    func olderCount(_ rides: RideBrowser) -> Int {
        candidates(rides, all: true).count - candidates(rides, all: false).count
    }

    func uploadPass(older: Bool = false) {
        guard !uploading, let rides, uploadOn || older, let key = Self.key() else { return }
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
            guard let bytes = try? Data(contentsOf: RideBrowser.path(name)), !bytes.isEmpty else { continue }
            do {
                let r = try await IntervalsClient.upload(key: key, athlete: athlete, name: name, bytes: bytes)
                uploaded[name] = r.activityId
                d.set(uploaded, forKey: "ivUploaded")
                (lastUpload, failures, uploadError) = (Date(), 0, nil)
                d.set(lastUpload, forKey: "ivLastUpload")
            } catch let e as IntervalsFailure {
                if e.retry || e.auth {
                    // 2, 4, 8 ... minutes, an hour at most
                    failures += 1
                    let wait = min(3600, 120 * pow(2, Double(min(failures - 1, 10))))
                    uploadNotBefore = Date().addingTimeInterval(wait)
                    uploadError = e.text + " Will try again later."
                    scheduleKick(after: wait)
                    return
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
        guard !calendarBusy, calendarOn, let rides, let key = Self.key() else { return }
        let now = Date()
        let due = calendarRetryAt.map { now >= $0 } ?? false
        if !force && !due && !events.isEmpty && now.timeIntervalSince(d.object(forKey: "ivCalTry") as? Date ?? .distantPast) < 3600 { return }
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
            return failCalendar(e.text)
        } catch {
            return failCalendar("Could not read the calendar.")
        }
        self.events = events
        let todayKey = IntervalsClient.day(today)
        var converted = [Sent](), files = [Int64: PlannedFile](), network = false
        for ev in calendarToFetch(sent: records, events: events) where refusedAt[ev.id]?.stamp != ev.updated {
            do {
                let zwo = try await IntervalsClient.zwo(key: key, athlete: athlete, event: ev.id)
                do {
                    let f = try planWorkout(zwo: zwo, fallbackName: ev.name)
                    files[ev.id] = f
                    converted.append(Sent(eventId: ev.id, day: ev.day, updated: ev.updated, hash: f.hash))
                } catch {
                    refusedAt[ev.id] = (ev.updated, Self.words(error))
                }
            } catch {
                network = true  // the rest would fail the same way; what was read is still used
                break
            }
        }
        let s = calendarSettle(
            sent: records, events: events.map(\.id), converted: converted, refused: refusedAt.filter { id, r in
                events.contains { $0.id == id && $0.updated == r.stamp }
            }.map(\.key), today: todayKey)
        for r in s.refresh { records.removeAll { $0.eventId == r.eventId }; records.append(r) }
        saveRecords()
        rides.setCalendarJobs(
            s.delete.map { CalendarJob(record: $0, bytes: nil) }
                + s.send.compactMap { id in
                    converted.first { $0.eventId == id }.flatMap { r in files[id].map { CalendarJob(record: r, bytes: $0.bytes) } }
                })
        lastCalendar = Date()
        d.set(lastCalendar, forKey: "ivLastCalendar")
        calendarError = network ? "Could not download every workout. Trying again soon." : nil
        calendarRetryAt = network ? Date().addingTimeInterval(900) : nil
        showPlanned()
    }

    private func failCalendar(_ text: String) {
        calendarError = text
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
        let recs = records.map { SentRec(e: $0.eventId, d: $0.day, u: $0.updated, h: $0.hash) }
        d.set(try? JSONEncoder().encode(recs), forKey: "ivSent")
    }

    /// RideBrowser finished (or gave up) one change of the unit's calendar.
    private func jobDone(_ job: CalendarJob, _ err: String?) {
        if let err {
            calendarError = "Planned workouts: \(err)"
            calendarRetryAt = Date().addingTimeInterval(900)
            scheduleKick(after: 900)
        } else if job.bytes == nil {
            records.removeAll { $0.eventId == job.record.eventId && $0.day == job.record.day && $0.hash == job.record.hash }
        } else {
            records.removeAll { $0.eventId == job.record.eventId }
            records.append(job.record)
            calendarError = nil
        }
        saveRecords()
        showPlanned()
    }
}
