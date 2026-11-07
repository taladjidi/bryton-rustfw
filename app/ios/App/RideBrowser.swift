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

    var hasTransport: Bool { transport != nil }
    var isDownloading: Bool { if case .downloading = phase { true } else { false } }

    static var now: UInt64 { UInt64(ProcessInfo.processInfo.systemUptime * 1000) }
    static var documents: URL { FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0] }

    func use(_ t: FileTransport, demo: Bool = false) {
        if let old = transport {
            old.onData = nil
            old.onReady = nil
            old.onLost = nil
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
        t.onLost = { [weak self] in
            guard let self else { return }
            self.ready = false
            if case .downloading = self.phase { self.message = "Link lost, resuming..." }
            if self.phase == .writing { self.endWrite("The link dropped. Look at the unit's list before sending again.") }
        }
        t.onError = { [weak self] m in self?.message = m }
        t.onDebug = { [weak self] m in self?.debug = m }
        t.start()
        tick?.invalidate()
        tick = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.silence() }
    }

    func refresh() {
        guard ready, let t = transport, !isDownloading, phase != .writing else { return }
        message = nil
        listingWorkouts = false
        workoutKey = nil
        phase = .listing
        t.write(client.listCommand(nowMs: Self.now))
    }

    /// Reads the planned workouts: LIST_IN of the Workout folder, then each file whole (they are a few KB).
    func refreshWorkouts() {
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
        guard ready, phase == .idle || phase == .failed, let t = transport,
            let cmd = client.writeCommand(day: day, bytes: bytes, nowMs: Self.now)
        else { return }
        message = nil
        sendError = nil
        sendDone = false
        phase = .writing
        progress = Progress(received: 0, total: UInt32(bytes.count), bytesPerS: 0)
        t.write(cmd)
    }

    /// Removes a workout of the last list from the unit, then lists again.
    func deleteWorkout(key: String) {
        guard ready, phase == .idle, let t = transport, let cmd = client.deleteCommand(key: key, nowMs: Self.now) else { return }
        message = nil
        phase = .writing
        t.write(cmd)
    }

    private func endWrite(_ error: String?) {
        phase = .idle
        sendError = error
        sendDone = error == nil
        if error != nil { message = error } else { refreshWorkouts() }
    }

    func download(_ r: RideEntry) {
        guard ready, phase == .idle || phase == .failed, let t = transport, let cmd = client.readCommand(name: r.name, nowMs: Self.now) else { return }
        message = nil
        retry?.cancel()
        busyRetries = 0
        (listingWorkouts, workoutKey, workoutQueue) = (false, nil, [])
        wanted = r.name
        progress = Progress(received: 0, total: r.size, bytesPerS: 0)
        phase = .downloading(r.name)
        t.write(cmd)
    }

    func cancel() {
        retry?.cancel()
        transport?.write(client.cancelCommand())
        phase = .idle
        wanted = nil
    }

    private func becameReady() {
        ready = true
        if case .downloading = phase { resume() } else if phase == .workouts { if listingWorkouts { refreshWorkouts() } else { resume() } } else if rides.isEmpty { refresh() }
    }

    private func resume() {
        guard let cmd = client.resumeCommand(nowMs: Self.now) else { return }
        transport?.write(cmd)
    }

    private func silence() {
        guard ready, client.silent(nowMs: Self.now) else { return }
        if case .downloading = phase {
            message = "No answer, resuming..."
            resume()
        } else if phase == .workouts {
            if listingWorkouts { refreshWorkouts() } else { resume() }
        } else if phase == .listing {
            refresh()
        } else if phase == .writing {
            endWrite("The unit did not answer. Look at its list before sending again.")
        }
    }

    private func data(_ d: Data) {
        switch client.onData(value: d, nowMs: Self.now) {
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
            guard let t = transport else { break }
            // the unit answers a WRITE_DATA only when it refuses one; the writes go out in order
            while let cmd = client.writeNextCommand(maxWrite: UInt32(t.writeLength), nowMs: Self.now) { t.write(cmd) }
            progress = client.progress()
        case .written, .deleted:
            endWrite(nil)
        case .failed(let e) where phase == .writing:
            endWrite(Self.text(e))
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

    private func fail(_ m: String) {
        retry?.cancel()
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
            saved = SavedRide(url: url)
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
        case .readOnly: "This build of the unit does not write its card."
        case .full: "That day already holds as many workouts as the unit keeps. Delete one or pick another day."
        case .mismatch: "The unit received other bytes than were sent. Send it again."
        case .busy: "The unit is riding or its card is lent to a computer. Try again later."
        case .noCalendar: "The unit has no workout calendar yet. Plan a workout on the unit or with the Bryton app first."
        }
    }

    /// "1 Nov 2026, 09:00" from `YYMMDDhhmmss`.
    static func title(_ name: String) -> String {
        let n = name.compactMap { $0.wholeNumberValue }
        guard n.count == 12 else { return name }
        let v = stride(from: 0, to: 12, by: 2).map { n[$0] * 10 + n[$0 + 1] }
        let c = DateComponents(year: 2000 + v[0], month: v[1], day: v[2], hour: v[3], minute: v[4], second: v[5])
        guard let d = Calendar.current.date(from: c) else { return name }
        return d.formatted(date: .abbreviated, time: .shortened)
    }
}

// MARK: Demo

/// The Rust fake unit, delivering its frames a few at a time like a slow link.
final class DemoTransport: FileTransport {
    var onData: ((Data) -> Void)?
    var onReady: (() -> Void)?
    var onLost: (() -> Void)?
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

    func write(_ d: Data) { queue = unit.handle(command: d) }
}
