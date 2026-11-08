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
    enum Phase: Equatable { case idle, listing, downloading(String), workouts, writing, failed }

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
    @Published var sendDone = false

    private let client = FileClient()
    private var transport: FileTransport?
    private var ready = false
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
        transport = t
        self.demo = demo
        ready = false
        t.onData = { [weak self] d in self?.data(d) }
        t.onReady = { [weak self] in self?.becameReady() }
        t.onWriteDone = { [weak self] in self?.writeDone() }
        t.onLost = { [weak self] in
            guard let self else { return }
            self.ready = false
            self.syncOff = false  // a reconnect tries the sync again
            if case .downloading = self.phase { self.message = "Link lost, resuming..." }
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
        if auto && isDownloading {
            auto = false
            cancel()
        }
    }

    func refresh() {
        yieldAuto()
        syncPaused = false
        guard ready, let t = transport, !isDownloading, phase != .writing else { return }
        message = nil
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
            workoutKey = nil
            phase = .idle
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
        if sendError != nil && phase == .idle { refreshWorkouts() }  // a send ended while the link was down
        if case .downloading = phase { resume() } else if phase == .workouts { if listingWorkouts { refreshWorkouts() } else { resume() } } else if rides.isEmpty { refresh() }
    }

    private func resume() {
        guard let cmd = client.resumeCommand(nowMs: Self.now) else { return }
        transport?.write(cmd)
    }

    /// Nothing else going on: the newest ride of the last 8 weeks that is not on the phone yet.
    private func autoSync() {
        guard ready, phase == .idle, !syncPaused, !syncOff,
            let r = rides.first(where: { Self.inWindow($0.name) && !downloaded.contains($0.name) && !syncFailed.contains($0.name) })
        else { return }
        download(r, auto: true)
    }

    /// "Syncing 3 of 7" while the background sync downloads.
    var syncLine: String? {
        guard auto, isDownloading else { return nil }
        let inWindow = rides.filter { Self.inWindow($0.name) }
        return "Syncing \(inWindow.filter { downloaded.contains($0.name) }.count + 1) of \(inWindow.count)"
    }

    private func silence() {
        autoSync()
        guard ready, client.silent(nowMs: Self.now) else { return }
        if case .downloading = phase {
            message = "No answer, resuming..."
            resume()
        } else if phase == .workouts {
            if listingWorkouts { refreshWorkouts() } else { resume() }
        } else if phase == .listing {
            refresh()
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
        case .listed where listingWorkouts:
            listingWorkouts = false
            workoutQueue = client.filesIn().map(\.name)
            nextWorkout()
        case .listed:
            rides = client.entries().sorted { $0.name > $1.name }
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
                message = "The unit is busy (riding or on USB). Waiting..."
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
        date(name)?.formatted(date: .abbreviated, time: .shortened) ?? name
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
        onDebug?("Demo: fake unit, write length 20 B")
        DispatchQueue.main.async { self.onReady?() }
        timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
            guard let self else { return }
            // take the batch first: a handler may write a new command, which replaces the queue
            let batch = Array(self.queue.prefix(10))
            self.queue.removeFirst(batch.count)
            for f in batch { self.onData?(f) }
        }
    }

    var writeLength: Int { 20 }

    func write(_ d: Data) {
        queue = unit.handle(command: d)
        DispatchQueue.main.async { [weak self] in self?.onWriteDone?() }
    }
}
