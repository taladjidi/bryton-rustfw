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
    func write(_ d: Data)
}

struct SavedRide: Identifiable {
    let url: URL
    var id: URL { url }
}

final class RideBrowser: ObservableObject {
    enum Phase: Equatable { case idle, listing, downloading(String), failed }

    @Published var rides: [RideEntry] = []
    @Published var phase = Phase.idle
    @Published var progress = Progress(received: 0, total: 0, bytesPerS: 0)
    @Published var message: String?
    @Published var debug = ""
    @Published var downloaded = Set<String>()
    @Published var saved: SavedRide?
    @Published var demo = false

    private let client = FileClient()
    private var transport: FileTransport?
    private var ready = false
    private var tick: Timer?
    private var busyRetries = 0
    private var wanted: String?

    static var now: UInt64 { UInt64(ProcessInfo.processInfo.systemUptime * 1000) }
    static var documents: URL { FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0] }

    func use(_ t: FileTransport, demo: Bool = false) {
        transport = t
        self.demo = demo
        ready = false
        t.onData = { [weak self] d in self?.data(d) }
        t.onReady = { [weak self] in self?.becameReady() }
        t.onLost = { [weak self] in
            guard let self else { return }
            self.ready = false
            if case .downloading = self.phase { self.message = "Link lost, resuming..." }
        }
        t.onError = { [weak self] m in self?.message = m }
        t.onDebug = { [weak self] m in self?.debug = m }
        t.start()
        tick?.invalidate()
        tick = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.silence() }
    }

    func refresh() {
        guard ready, let t = transport else { return }
        message = nil
        phase = .listing
        t.write(client.listCommand(nowMs: Self.now))
    }

    func download(_ r: RideEntry) {
        guard ready, let t = transport, let cmd = client.readCommand(name: r.name, nowMs: Self.now) else { return }
        message = nil
        busyRetries = 0
        wanted = r.name
        progress = Progress(received: 0, total: r.size, bytesPerS: 0)
        phase = .downloading(r.name)
        t.write(cmd)
    }

    func cancel() {
        transport?.write(client.cancelCommand())
        phase = .idle
        wanted = nil
    }

    private func becameReady() {
        ready = true
        if case .downloading = phase { resume() } else if rides.isEmpty { refresh() }
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
        } else if phase == .listing {
            refresh()
        }
    }

    private func data(_ d: Data) {
        switch client.onData(value: d, nowMs: Self.now) {
        case .ignored, .entry: break
        case .listed:
            rides = client.entries().sorted { $0.name > $1.name }
            downloaded = Set(rides.map(\.name).filter { FileManager.default.fileExists(atPath: Self.path($0).path) })
            phase = .idle
        case .progress:
            progress = client.progress()
            message = nil
        case .finished:
            progress = client.progress()
            finish()
        case .busy:
            busyRetries += 1
            if busyRetries > 20 {
                fail("The unit is busy (riding or on USB). Try again later.")
            } else {
                message = "The unit is busy (riding or on USB). Waiting..."
                DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in
                    if case .downloading = self?.phase { self?.resume() }
                }
            }
        case .failed(let e):
            fail(Self.text(e))
        }
    }

    private func fail(_ m: String) {
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
            for f in self.queue.prefix(10) { self.onData?(f) }
            self.queue.removeFirst(min(10, self.queue.count))
        }
    }

    func write(_ d: Data) { queue = unit.handle(command: d) }
}
