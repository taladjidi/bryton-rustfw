// The ride list and download. All protocol logic is the Rust core's FileClient; this file moves bytes between a
// transport (CoreBluetooth, or the Rust fake unit in Demo mode) and the core, keeps the timers, and saves the file.

import Foundation

/// Where the commands go and the notifications come from.
protocol FileTransport: AnyObject {
    var onData: ((Data) -> Void)? { get set }
    /// Control and Data are usable (again, after a drop).
    var onReady: (() -> Void)? { get set }
    /// The link dropped; the transport reconnects by itself and calls onReady.
    var onLost: (() -> Void)? { get set }
    /// A Control write was taken by the unit (the stack's confirmation): the next one may go.
    var onWriteDone: (() -> Void)? { get set }
    /// A failure the user should read.
    var onError: ((String) -> Void)? { get set }
    /// The debug line (negotiated write length).
    var onDebug: ((String) -> Void)? { get set }
    func start()
    /// The most bytes one Control write may carry (the link's write length).
    var writeLength: Int { get }
    func write(_ d: Data)
}

struct SavedRide: Identifiable, Hashable {
    let url: URL
    var id: URL { url }
}

final class RideBrowser: ObservableObject {
    enum Phase: Equatable { case idle, listing, downloading(String), workouts, routes, writing, profile, failed }

    @Published var rides: [RideEntry] = []
    @Published var phase = Phase.idle
    @Published var progress = Progress(received: 0, total: 0, bytesPerS: 0)
    @Published var message: String?
    @Published var debug = ""
    @Published var downloaded = Set<String>()
    @Published var saved: SavedRide?
    @Published var demo = false
    /// The planned workouts read from the unit, by key (`YYYYMMDDnn`), as the unit holds them: decoded by the core
    /// for the rider's FTP when shown.
    @Published var workouts: [WorkoutFile] = []
    /// How the last send ended: the text of its error, or `sendDone`.
    @Published var sendError: String?
    /// The courses of the unit's `\Courses` folder, as of the last list (`routesListed`: it was read at all); the
    /// words of a failed delete in `routeMessage`.
    @Published var unitRoutes: [UnitRoute] = []
    @Published var routesListed = false
    @Published var routeMessage: String?
    /// Why the unit's courses are not listed (its software has no folder 2 list, or it is riding), or nil.
    @Published var routeListError: String?
    /// Told when the unit took a route, with the name it has there.
    var onRouteSent: ((SentRoute) -> Void)?
    @Published var sendDone = false
    /// The unit's profile after the last sync or push ("Unit profile: FTP 265 W (from Intervals.icu, 8 Nov 14:02)"), or
    /// why the unit did not take it.
    @Published var profileLine: String?

    private let client = FileClient()
    private var transport: FileTransport?
    private var ready = false
    /// The unit's LIST answered at least once (until then `rides` holds only what the phone saved).
    private var unitListed = false
    private var tick: Timer?
    private var busyRetries = 0
    private var wanted: String?
    private var retry: DispatchWorkItem?
    /// Workouts: the LIST_IN under way, the keys still to read and the one being read.
    private var listingWorkouts = false
    private var workoutQueue: [String] = []
    private var workoutKey: String?
    private var workoutFiles: [WorkoutFile] = []
    /// The write under way is a send (not a delete).
    private var sending = false
    /// Routes: the LIST_IN of `\Courses` under way; the write under way is a route's (and what it sends); the last
    /// LIST_IN was `\Courses` (a delete names a key of the last list).
    private var listingRoutes = false
    private var routeWrite = false
    private var routeSending: SentRoute?
    private var routesFresh = false
    /// Calendar changes waiting for an idle link (after the ride sync), and the one under way. They never block the
    /// rider: anything the rider starts puts the one under way back at the front. `onCalendarJob` hears how each ended:
    /// the job, the error in words, the core's verdict (`calendarJobError`), and whether the unit already held the file
    /// (a rider's own copy: never deleted later). A failure drops only its job unless the verdict stops the pass.
    private var calendarJobs: [CalendarJob] = []
    private var calendarJob: CalendarJob?
    private var calendarRetried = false
    var onCalendarJob: ((CalendarJob, String?, JobNext, Bool) -> Void)?
    /// `workouts` is what the unit's folder holds now (a delete needs the keys of a list read just before); `workoutsListed`:
    /// it was read at all on this connection (a send only checks it for the same file).
    private var workoutsFresh = false
    private var workoutsListed = false
    /// The profile to give the unit, kept until the unit took it (or refused for good); `merged` is what was sent.
    /// A push that arrives while an exchange is in flight waits in `queuedProfile` and replaces the pending one after.
    private struct ProfilePush { let iv: IntervalsProfile; let source: String; let at: Date }
    private var pendingProfile: ProfilePush?
    private var queuedProfile: ProfilePush?
    private var merged: ProfileMerge?
    /// The PROFILE_GET / SET exchange is on the link (phase `profile`).
    private var profileInFlight = false
    /// Failed attempts of the pending push (busy or silence), and whether this connection already tried: the next try
    /// is at the next connection, at most `profileMaxTries` in all.
    private var profileTries = 0
    private var profileTried = false
    private static let profileMaxTries = 5
    /// The download under way is the background sync's, not the rider's: anything the rider starts cancels it.
    private(set) var auto = false
    /// The rider cancelled a sync download: no more until they refresh.
    private var syncPaused = false
    /// The unit was busy or its card unreadable during a sync: no more sync until the app restarts or reconnects.
    private var syncOff = false
    /// Rides whose sync download failed: not tried again until the app restarts.
    private var syncFailed = Set<String>()
    /// How far back the sync goes, days (the last 8 weeks; older rides download when opened).
    static let syncDays = 56

    var hasTransport: Bool { transport != nil }
    var isDownloading: Bool { if case .downloading = phase { true } else { false } }

    static var now: UInt64 { UInt64(ProcessInfo.processInfo.systemUptime * 1000) }
    static var documents: URL { FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0] }

    init() {
        rides = Self.phoneRides()
        downloaded = Set(rides.map(\.name))
    }

    /// The rides saved on the phone (`YYMMDDhhmmss.fit` in Documents), newest first: the list works while the unit is
    /// away, and a ride deleted from the unit stays listed.
    static func phoneRides() -> [RideEntry] {
        let files = (try? FileManager.default.contentsOfDirectory(at: documents, includingPropertiesForKeys: [.fileSizeKey])) ?? []
        return files.compactMap { u -> RideEntry? in
            let n = u.deletingPathExtension().lastPathComponent
            guard u.pathExtension == "fit", n.count == 12, n.allSatisfy({ $0.isASCII && $0.isNumber }),
                let size = (try? u.resourceValues(forKeys: [.fileSizeKey]).fileSize).flatMap({ UInt32(exactly: $0) })
            else { return nil }
            return RideEntry(name: n, size: size)
        }.sorted { $0.name > $1.name }
    }

    func use(_ t: FileTransport, demo: Bool = false) {
        if let old = transport {
            old.onData = nil
            old.onReady = nil
            old.onLost = nil
            old.onWriteDone = nil
            old.onError = nil
            old.onDebug = nil
        }
        tick?.invalidate()
        retry?.cancel()
        workoutsFresh = false
        workoutsListed = false
        transport = t
        self.demo = demo
        ready = false
        t.onData = { [weak self] d in self?.data(d) }
        t.onReady = { [weak self] in self?.becameReady() }
        t.onWriteDone = { [weak self] in self?.writeDone() }
        t.onLost = { [weak self] in
            guard let self else { return }
            self.ready = false
            self.workoutsFresh = false
            self.workoutsListed = false
            self.syncOff = false  // a reconnect tries the sync again
            if case .downloading = self.phase { self.message = "Link lost, resuming…" }
            self.dropProfileExchange()  // the pending profile goes on the next connection
            if self.phase == .routes { (self.listingRoutes, self.phase) = (false, .idle) }
            if self.phase == .writing { self.endWrite("The link dropped. Look at the unit's list before sending again.", isSend: self.sending) }
        }
        t.onError = { [weak self] m in self?.message = m }
        t.onDebug = { [weak self] m in self?.debug = m }
        syncOff = false
        t.start()
        tick?.invalidate()
        tick = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.silence() }
    }

    /// A rider's action takes the link from the background sync.
    private func yieldAuto() {
        if profileInFlight {  // the profile exchange never blocks the rider: it is asked again later
            client.abort()
            dropProfileExchange()
        }
        if let job = calendarJob, phase == .writing {  // the unit drops a write nobody feeds after 5 s
            client.abort()
            calendarJob = nil
            phase = .idle
            calendarJobs.insert(job, at: 0)
        }
        if auto && isDownloading {
            auto = false
            cancel()
        }
        if phase == .routes {  // a short background list: the rider's action goes first
            client.abort()
            (listingRoutes, phase) = (false, .idle)
        }
    }

    func refresh() {
        yieldAuto()
        syncPaused = false
        guard ready, let t = transport, !isDownloading, phase != .writing else { return }
        message = nil
        workoutsFresh = false
        listingWorkouts = false
        workoutKey = nil
        phase = .listing
        t.write(client.listCommand(nowMs: Self.now))
    }

    /// Reads the planned workouts: LIST_IN of the Workout folder, then each file whole (they are a few KB).
    func refreshWorkouts() {
        yieldAuto()
        guard ready, let t = transport, phase == .idle || phase == .workouts else { return }
        message = nil
        (workoutsFresh, routesFresh) = (false, false)
        phase = .workouts
        listingWorkouts = true
        workoutQueue = []
        workoutKey = nil
        workoutFiles = []
        t.write(client.listInCommand(folder: .workout, nowMs: Self.now))
    }

    private func nextWorkout() {
        guard let t = transport else { return }
        guard let key = workoutQueue.first else {
            workouts = workoutFiles.sorted { $0.key < $1.key }
            (workoutsFresh, workoutsListed) = (true, true)
            workoutKey = nil
            phase = .idle
            refreshRoutes()  // the courses follow the workouts
            return
        }
        workoutKey = key
        if let cmd = client.readInCommand(key: key, nowMs: Self.now) {
            t.write(cmd)
        } else {
            workoutQueue.removeFirst()
            nextWorkout()
        }
    }

    /// Sends a workout file to the unit for `day` (`YYYYMMDD`), then lists the unit's workouts again.
    func send(day: String, bytes: Data) {
        sendError = nil
        sendDone = false
        yieldAuto()
        guard ready, phase == .idle || phase == .failed, let t = transport else {
            sendError = "Not connected to the unit, or it is busy with another transfer. Try again in a moment."
            return
        }
        guard let cmd = client.writeCommand(day: day, bytes: bytes, nowMs: Self.now) else {
            sendError = bytes.isEmpty || bytes.count > 8192
                ? "The workout file is empty or larger than the unit keeps." : "That date is not valid."
            return
        }
        message = nil
        sending = true
        phase = .writing
        progress = Progress(received: 0, total: UInt32(bytes.count), bytesPerS: 0)
        t.write(cmd)
    }

    /// Removes a workout of the last list from the unit, then lists again; `file` is what the rider saw, which must
    /// still be what the list holds (a key is a place in the unit's folder).
    func deleteWorkout(file: WorkoutFile) {
        yieldAuto()
        guard ready, phase == .idle, let t = transport else {
            message = "The unit is busy with another transfer."
            return
        }
        guard workouts.contains(where: { $0.key == file.key && $0.bytes == file.bytes }),
            let cmd = client.deleteCommand(key: file.key, nowMs: Self.now)
        else {
            message = "The unit's list changed. Look again."
            refreshWorkouts()
            return
        }
        message = nil
        sending = false
        phase = .writing
        t.write(cmd)
    }

    /// Lists the unit's `\Courses` (LIST_IN of folder 2): names are not in it, only keys, sizes and checks.
    func refreshRoutes() {
        guard ready, phase == .idle, let t = transport else { return }
        phase = .routes
        listingRoutes = true
        routeMessage = nil
        t.write(client.listInCommand(folder: .courses, nowMs: Self.now))
    }

    /// What a rider's route action takes from the background lists (the workouts' reads or the courses' list).
    private func yieldBackground() {
        yieldAuto()
        if phase == .workouts {
            client.abort()
            (listingWorkouts, workoutKey, workoutQueue, workoutsFresh, phase) = (false, nil, [], false, .idle)
        }
    }

    /// Sends a FIT course to the unit as `name` (1 to 20 characters of its alphabet), then lists the courses again.
    /// `source` is the phone route it comes from.
    func sendRoute(name: String, source: String, bytes: Data) {
        sendError = nil
        sendDone = false
        yieldBackground()
        guard ready, phase == .idle || phase == .failed, let t = transport else {
            sendError = "Not connected to the unit, or it is busy with another transfer. Try again in a moment."
            return
        }
        guard let cmd = client.courseWriteCommand(name: name, bytes: bytes, maxWrite: UInt32(t.writeLength), nowMs: Self.now)
        else {
            sendError = "The name is not one the unit takes (1 to 20 letters, digits, spaces, - and _; up to 8 on a slow link), or the file is over 1 MB."
            return
        }
        message = nil
        (sending, routeWrite) = (true, true)
        routeSending = SentRoute(unitName: name, size: UInt32(clamping: bytes.count), route: source)
        phase = .writing
        progress = Progress(received: 0, total: UInt32(clamping: bytes.count), bytesPerS: 0)
        t.write(cmd)
    }

    /// Removes a course of the last list from the unit, then lists again. The unit refuses the one it follows (BUSY).
    func deleteRoute(_ r: UnitRoute) {
        routeMessage = nil
        yieldBackground()
        guard ready, phase == .idle, let t = transport else {
            routeMessage = "The unit is busy with another transfer. Try again in a moment."
            return
        }
        guard routesFresh, unitRoutes.contains(r), let cmd = client.deleteCommand(key: r.key, nowMs: Self.now) else {
            routeMessage = "The unit's list changed. Look again."
            refreshRoutes()
            return
        }
        message = nil
        (sending, routeWrite) = (false, true)
        phase = .writing
        t.write(cmd)
    }

    /// The next WRITE_DATA (or WRITE_END) goes once the stack confirmed the last write.
    private func writeDone() {
        guard phase == .writing, let t = transport else { return }
        if let cmd = client.writeNextCommand(maxWrite: UInt32(t.writeLength), nowMs: Self.now) {
            t.write(cmd)
            progress = client.progress()
        }
    }

    private func endWrite(_ error: String?, isSend: Bool) {
        phase = .idle
        if routeWrite {  // a course sent or deleted: its own outcome, then the courses are listed again
            routeWrite = false
            if isSend {
                sendError = error
                sendDone = error == nil
                if error == nil, let s = routeSending { onRouteSent?(s) }
            } else {
                routeMessage = error
            }
            routeSending = nil
            if error != nil { client.abort() }
            routesFresh = false
            return refreshRoutes()  // not ready (a drop): becameReady lists again
        }
        // the loop's change: its own outcome, not the editor's sendError or the main message (here: done, or the link went)
        if calendarJob != nil { return finishCalendar(error: error, next: error == nil ? .done : .stop) }
        if isSend {
            sendError = error
            sendDone = error == nil
        }
        if error != nil {
            client.abort()  // late answers of the dropped write are ignored
            message = error
        }
        refreshWorkouts()  // not ready (a drop): becameReady lists again
    }

    func download(_ r: RideEntry, auto isAuto: Bool = false) {
        if !isAuto { yieldAuto() }
        guard ready, phase == .idle || phase == .failed, let t = transport, let cmd = client.readCommand(name: r.name, nowMs: Self.now) else { return }
        message = nil
        workoutsFresh = false
        retry?.cancel()
        busyRetries = 0
        (listingWorkouts, workoutKey, workoutQueue) = (false, nil, [])
        wanted = r.name
        auto = isAuto
        progress = Progress(received: 0, total: r.size, bytesPerS: 0)
        phase = .downloading(r.name)
        t.write(cmd)
    }

    func cancel() {
        retry?.cancel()
        transport?.write(client.cancelCommand())
        phase = .idle
        wanted = nil
        if auto { syncPaused = true }
        auto = false
    }

    private func becameReady() {
        ready = true
        profileTried = false  // a new connection tries the pending profile again
        if sendError != nil && phase == .idle { refreshWorkouts() }  // a send ended while the link was down
        if case .downloading = phase { resume() } else if phase == .workouts { if listingWorkouts { refreshWorkouts() } else { resume() } } else if !unitListed { refresh() }
    }

    private func resume() {
        guard let cmd = client.resumeCommand(nowMs: Self.now) else { return }
        transport?.write(cmd)
    }

    /// The phone's copy of a ride fails its FIT CRC and the unit still lists the ride: set aside as `.fit.bad` (kept,
    /// never deleted) so the sync reads it again. False when the copy is whole or the unit cannot give it again.
    @discardableResult
    func reread(_ name: String) -> Bool {
        let url = Self.path(name)
        guard let data = try? Data(contentsOf: url),
              rereadCopy(copyWhole: fitWhole(bytes: data), unitLists: rides.contains(where: { $0.name == name }), linkReady: ready)
        else { return false }
        let bad = url.appendingPathExtension("bad")
        try? FileManager.default.removeItem(at: bad)
        guard (try? FileManager.default.moveItem(at: url, to: bad)) != nil else { return false }
        downloaded.remove(name)
        syncFailed.remove(name)
        autoSync()
        return true
    }

    /// Nothing else going on: the newest ride of the last 8 weeks that is not on the phone yet.
    private func autoSync() {
        guard ready, phase == .idle, !syncPaused, !syncOff, let r = nextSyncRide() else { return }
        download(r, auto: true)
    }

    private func nextSyncRide() -> RideEntry? {
        rides.first { Self.inWindow($0.name) && !downloaded.contains($0.name) && !syncFailed.contains($0.name) }
    }

    /// Replaces the waiting calendar changes (the loop plans them again from its records each time).
    func setCalendarJobs(_ jobs: [CalendarJob]) { calendarJobs = jobs }

    private func unitHolds(_ r: Sent) -> WorkoutFile? {
        workouts.first { $0.key.hasPrefix(r.day) && fileCrc(bytes: $0.bytes) == r.hash }
    }

    static func jobError(_ e: FileError) -> JobError {
        switch e {
        case .notFound: .notFound
        case .mismatch: .mismatch
        case .busy: .busy
        case .noCalendar: .noCalendar
        case .readOnly: .readOnly
        default: .refused
        }
    }

    /// The next calendar change, once the link is idle and the ride sync has nothing left to download. The unit's list is
    /// read first (a delete needs its keys, a send checks the file is not there already) and once more when the queue
    /// empties; a delete after a send reads it again, since the send changed the folder.
    private func autoCalendar() {
        guard ready, phase == .idle, calendarJob == nil, let job = calendarJobs.first, let t = transport else { return }
        if !syncPaused && !syncOff && nextSyncRide() != nil { return }
        guard workoutsFresh || (job.bytes != nil && workoutsListed) else { return refreshWorkouts() }
        calendarJobs.removeFirst()
        calendarJob = job
        if let bytes = job.bytes {
            // the same file on that day already (a rider's own copy, or a send whose record was lost): not written again,
            // and flagged so it is never deleted
            if unitHolds(job.record) != nil {
                calendarJob = nil
                onCalendarJob?(job, nil, .done, true)
                return
            }
            guard let cmd = client.writeCommand(day: job.record.day, bytes: bytes, nowMs: Self.now) else {
                return finishCalendar(error: "That workout has a date or a size the unit cannot take.", next: .drop)
            }
            (sending, phase) = (false, .writing)
            progress = Progress(received: 0, total: UInt32(clamping: bytes.count), bytesPerS: 0)
            t.write(cmd)
        } else {
            // not in the list: already gone, so the job is done
            guard let f = unitHolds(job.record) else {
                calendarJob = nil
                onCalendarJob?(job, nil, .done, false)
                return
            }
            guard let cmd = client.deleteCommand(key: f.key, nowMs: Self.now) else {
                return finishCalendar(error: "The unit's list changed.", next: .drop)
            }
            (sending, phase) = (false, .writing)
            t.write(cmd)
        }
    }

    /// One calendar change ended. `calendarAfter` says what is left of the queue; a retry reads the list again first.
    private func finishCalendar(error: String?, next: JobNext) {
        guard let job = calendarJob else { return }
        calendarJob = nil
        phase = .idle
        if next == .retry {
            client.abort()
            (calendarRetried, workoutsFresh) = (true, false)
            calendarJobs.insert(job, at: 0)
        } else {
            calendarRetried = false
            if next != .done { client.abort() }
            if job.bytes != nil && next == .done { workoutsFresh = false }  // the send changed the folder
            calendarJobs = calendarAfter(queue: calendarJobs, failed: job, next: next)
        }
        onCalendarJob?(job, error, next, false)
        if next == .retry || calendarJobs.isEmpty { refreshWorkouts() }  // once per queue
    }

    /// "Syncing 3 of 7" while the background sync downloads.
    var syncLine: String? {
        guard auto, isDownloading else { return nil }
        let inWindow = rides.filter { Self.inWindow($0.name) }
        return "Syncing \(inWindow.filter { downloaded.contains($0.name) }.count + 1) of \(inWindow.count)"
    }

    /// Gives the unit the profile Intervals.icu (or the profile sheet) set, when nothing else uses the link: PROFILE_GET,
    /// then PROFILE_SET with the unit's other fields kept. The unit answers during a ride; the sync yields to it.
    func pushProfile(_ iv: IntervalsProfile, source: String) {
        let push = ProfilePush(iv: iv, source: source, at: Date())
        if profileInFlight {
            queuedProfile = push
            return
        }
        (pendingProfile, profileTries, profileTried) = (push, 0, false)
        if !ready { profileLine = "Unit profile: waiting for the unit" }
        startProfile()
    }

    private func startProfile() {
        guard pendingProfile != nil, ready, phase == .idle, !profileTried, let t = transport else { return }
        (phase, profileInFlight) = (.profile, true)
        t.write(client.profileGetCommand(nowMs: Self.now))
    }

    /// The exchange is over or given up: the link is free, and a push queued meanwhile becomes the pending one.
    private func endProfileExchange() {
        (phase, profileInFlight, merged) = (.idle, false, nil)
        if let q = queuedProfile {
            (pendingProfile, queuedProfile, profileTries, profileTried) = (q, nil, 0, false)
        }
    }

    /// Preempted or the link dropped: the pending push stays for the next try.
    private func dropProfileExchange() {
        guard profileInFlight else { return }
        if phase == .profile { phase = .idle }
        endProfileExchange()
    }

    private func profileDone(_ p: UnitProfile) {
        guard let pend = pendingProfile else { return endProfileExchange() }
        var line = "Unit profile: FTP \(p.ftp) W (from \(pend.source), \(Fmt.when(pend.at)))"
        if let m = merged, !m.skipped.isEmpty { line += ". Not sent: " + m.skipped.joined(separator: "; ") }
        (profileLine, pendingProfile) = (line, nil)
        endProfileExchange()
    }

    /// The unit refused or went quiet: busy and silence are asked again at the next connection (5 tries in all),
    /// anything else ends the push.
    private func profileFailed(_ e: FileError?) {
        switch e {
        case .busy?, nil:
            profileTries += 1
            profileTried = true
            if profileTries >= Self.profileMaxTries {
                pendingProfile = nil
                profileLine = "Unit profile: the unit stayed busy or silent, giving up until the next sync"
            } else {
                profileLine = "Unit profile: the unit is busy or did not answer, trying again at the next connection"
            }
        case .readOnly?:
            pendingProfile = nil
            profileLine = "Unit profile: read only, this unit's software does not save settings from the phone yet"
        case let e?:
            pendingProfile = nil
            profileLine = "Unit profile: " + Self.text(e)
        }
        endProfileExchange()
    }

    private func silence() {
        startProfile()
        autoSync()
        autoCalendar()
        guard ready, client.silent(nowMs: Self.now) else { return }
        if profileInFlight { client.abort(); profileFailed(nil); return }
        if case .downloading = phase {
            message = "No answer, resuming…"
            resume()
        } else if phase == .workouts {
            if listingWorkouts { refreshWorkouts() } else { resume() }
        } else if phase == .listing {
            refresh()
        } else if phase == .routes {
            client.abort()
            (listingRoutes, phase) = (false, .idle)  // the courses are not worth a wait
        } else if phase == .writing {
            endWrite("The unit did not answer. Look at its list before sending again.", isSend: sending)
        }
    }

    private func data(_ d: Data) {
        let event = client.onData(value: d, nowMs: Self.now)
        // a read the rider or a yield cancelled: what was already on the way must not finish or resume it
        if wanted == nil && workoutKey == nil && phase != .workouts {
            switch event {
            case .progress, .partial, .finished: return
            default: break
            }
        }
        switch event {
        case .ignored, .entry: break
        case .listed where listingRoutes:
            listingRoutes = false
            let list = client.filesIn(), checks = client.filesInChecks()
            unitRoutes = zip(list, checks).map { UnitRoute(key: $0.name, size: $0.size, check: $1) }
            (routesListed, routesFresh, phase, routeListError) = (true, true, .idle, nil)
        case .busy where phase == .routes:
            // a background list refused (riding, USB): the old list stays; the section says why
            (listingRoutes, phase) = (false, .idle)
            routeListError = "The unit is riding or its card is lent to a computer: the list of routes comes later."
        case .failed(let e) where phase == .routes:
            (listingRoutes, phase) = (false, .idle)
            routeListError = e == .badRequest
                ? "This unit's software does not list routes yet." : "The unit could not list its routes: \(Self.text(e))"
        case .listed where listingWorkouts:
            listingWorkouts = false
            workoutQueue = client.filesIn().map(\.name)
            nextWorkout()
        case .listed:
            let unit = client.entries()
            let onUnit = Set(unit.map(\.name))
            rides = (unit + Self.phoneRides().filter { !onUnit.contains($0.name) }).sorted { $0.name > $1.name }
            unitListed = true
            downloaded = Set(rides.map(\.name).filter { FileManager.default.fileExists(atPath: Self.path($0).path) })
            phase = .idle
            refreshWorkouts()
        case .writeReady:
            // one WRITE_DATA at a time, each after the stack confirmed the last write (writeDone); the unit answers
            // a WRITE_DATA only when it refuses one
            if phase == .writing { writeDone() }
        case .written where phase == .writing, .deleted where phase == .writing:
            endWrite(nil, isSend: sending)
        case .written, .deleted:
            break  // a late answer of a write that was given up
        case .profile where profileInFlight:
            guard let pend = pendingProfile, let unit = client.profile() else { return profileFailed(.card) }
            let m = mergeUnitProfile(unit: unit, iv: pend.iv)
            merged = m
            if m.changed { transport?.write(client.profileSetCommand(profile: m.profile, nowMs: Self.now)) } else { profileDone(unit) }
        case .profileSet where profileInFlight:
            if let m = merged { profileDone(m.profile) } else { profileFailed(.card) }
        case .failed(let e) where profileInFlight:
            profileFailed(e)
        case .profile, .profileSet:
            break  // a late answer
        case .failed(let e) where phase == .writing && calendarJob != nil:
            let next = calendarJobError(isSend: calendarJob?.bytes != nil, e: Self.jobError(e), retried: calendarRetried)
            finishCalendar(error: next == .done ? nil : Self.text(e), next: next)
        case .failed(let e) where phase == .writing && routeWrite:
            if !sending && (e == .notFound || e == .mismatch) {
                endWrite("That route is no longer on the unit. The list was refreshed.", isSend: false)
            } else {
                endWrite(Self.routeText(e, deleting: !sending), isSend: sending)
            }
        case .failed(let e) where phase == .writing:
            if !sending && (e == .notFound || e == .mismatch) {
                endWrite("That workout is no longer on the unit. The list was refreshed.", isSend: false)
            } else {
                endWrite(Self.text(e), isSend: sending)
            }
        case .progress:
            progress = client.progress()
            message = nil
        case .finished where workoutKey != nil:
            if let key = workoutKey { workoutFiles.append(WorkoutFile(key: key, bytes: client.file())) }
            workoutQueue.removeFirst()
            nextWorkout()
        case .finished:
            progress = client.progress()
            finish()
        case .busy where auto:
            stopSyncQuietly()
        case .failed(.card) where auto:
            stopSyncQuietly()
        case .busy:
            busyRetries += 1
            if busyRetries > 20 {
                fail("The unit is busy (riding or on USB). Try again later.")
            } else {
                message = "The unit is busy (riding or on USB). Waiting…"
                retry?.cancel()
                let item = DispatchWorkItem { [weak self] in self?.resume() }
                retry = item
                DispatchQueue.main.asyncAfter(deadline: .now() + 3, execute: item)
            }
        case .partial:
            if !isWorkoutPhase { progress = client.progress() }
            resume()  // the answer ended early but brought bytes: ask for the rest
        case .failed(.notFound) where workoutKey != nil:
            // one workout gone since the list: skip it, read the others
            workoutQueue.removeFirst()
            nextWorkout()
        case .failed(let e):
            fail(Self.text(e))
        }
    }

    private var isWorkoutPhase: Bool { phase == .workouts }

    /// A sync download ends without a word: the unit is busy or its card is not readable, so the sync stays off.
    private func stopSyncQuietly() {
        retry?.cancel()
        transport?.write(client.cancelCommand())
        syncOff = true
        (auto, wanted, message, phase) = (false, nil, nil, .idle)
    }

    private func fail(_ m: String) {
        retry?.cancel()
        if auto, let w = wanted { syncFailed.insert(w) }
        auto = false
        wanted = nil
        (listingWorkouts, workoutKey, workoutQueue) = (false, nil, [])
        message = m
        phase = .failed
    }

    private func finish() {
        guard let name = wanted else { return }
        let url = Self.path(name)
        do {
            try client.file().write(to: url, options: .atomic)
            downloaded.insert(name)
            if auto { auto = false } else { saved = SavedRide(url: url) }
            wanted = nil
            message = nil
            phase = .idle
        } catch {
            fail("Could not save the ride: \(error.localizedDescription)")
        }
    }

    static func path(_ name: String) -> URL { documents.appendingPathComponent("\(name).fit") }

    /// A route's send or delete refused, in words.
    static func routeText(_ e: FileError, deleting: Bool) -> String {
        switch e {
        case .full: "The unit's course list is full (64). Delete a route from the unit first."
        case .badRequest: "A route with that name exists on the unit: rename or delete it first."
        case .busy:
            deleting
                ? "The unit is following this route, or is riding, or its card is lent to a computer. Stop following it on the unit, then try again."
                : "The unit is riding or its card is lent to a computer. Try again later."
        case .readOnly: "This unit's software cannot receive routes from the phone yet."
        default: text(e)
        }
    }

    static func text(_ e: FileError) -> String {
        switch e {
        case .notFound: "The ride is not on the unit any more."
        case .badRequest: "The unit refused the request."
        case .card: "The unit could not read its card."
        case .tooLong: "The unit sent more than the list said."
        case .short: "The file arrived short. Try again."
        case .countMismatch: "The transfer was inconsistent. Try again."
        case .readOnly: "This unit's software cannot receive workouts from the phone yet."
        case .full: "That day already holds as many workouts as the unit keeps. Delete one or pick another day."
        case .mismatch: "The unit received other bytes than were sent. Send it again."
        case .busy: "The unit is riding or its card is lent to a computer. Try again later."
        case .noCalendar: "The unit has no workout calendar yet. Plan a workout on the unit or with the Bryton app first."
        }
    }

    /// The start in the unit's local time, from `YYMMDDhhmmss` (taken as the phone's zone).
    static func date(_ name: String) -> Date? {
        let n = name.compactMap { $0.wholeNumberValue }
        guard n.count == 12 else { return nil }
        let v = stride(from: 0, to: 12, by: 2).map { n[$0] * 10 + n[$0 + 1] }
        let c = DateComponents(year: 2000 + v[0], month: v[1], day: v[2], hour: v[3], minute: v[4], second: v[5])
        return Calendar.current.date(from: c)
    }

    /// The ride started within the sync window.
    static func inWindow(_ name: String) -> Bool {
        guard let d = date(name) else { return false }
        return d >= Date().addingTimeInterval(-Double(syncDays) * 86_400)
    }

    /// "1 Nov 2026, 09:00" from `YYMMDDhhmmss`.
    static func title(_ name: String) -> String {
        date(name).map(Fmt.when) ?? name
    }
}

// MARK: Demo

/// The Rust fake unit, delivering its frames a few at a time like a slow link.
final class DemoTransport: FileTransport {
    var onData: ((Data) -> Void)?
    var onReady: (() -> Void)?
    var onLost: (() -> Void)?
    var onWriteDone: (() -> Void)?
    var onError: ((String) -> Void)?
    var onDebug: ((String) -> Void)?
    private let unit = DemoUnit()
    private var queue: [Data] = []
    private var timer: Timer?

    func start() {
        onDebug?("Demo: fake unit, write length \(writeLength) B")
        DispatchQueue.main.async { self.onReady?() }
        timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
            guard let self else { return }
            // take the batch first: a handler may write a new command, which replaces the queue
            let batch = Array(self.queue.prefix(10))
            self.queue.removeFirst(batch.count)
            for f in batch { self.onData?(f) }
        }
    }

    /// The default MTU's 20 bytes; `-demo-mtu 244` is what the owner's phone negotiated (docs/app.md), where a course
    /// name of 20 characters fits.
    var writeLength: Int {
        let a = CommandLine.arguments
        return a.firstIndex(of: "-demo-mtu").flatMap { a[safe: $0 + 1] }.flatMap { Int($0) } ?? 20
    }

    func write(_ d: Data) {
        queue = unit.handle(command: d)
        DispatchQueue.main.async { [weak self] in self?.onWriteDone?() }
    }
}
