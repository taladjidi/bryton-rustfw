import SwiftUI
import UIKit

@main
struct BrytonApp: App {
    var body: some Scene {
        WindowGroup { ContentView() }
    }
}

struct ContentView: View {
    @StateObject private var link = UnitLink()
    @StateObject private var rides = RideBrowser()
    @StateObject private var library = RideLibrary()
    @StateObject private var intervals = IntervalsSync()
    @StateObject private var loop = IntervalsLoop()
    @State private var profile = false
    @State private var fitness = false
    @StateObject private var store = WorkoutStore()
    @State private var path: [WorkoutRoute] = []
    @State private var editing: EditorTarget?
    @State private var lastSync: Date?

    // the environment objects sit outside the stack: the sheets and the pushed pages are presented from inside it
    var body: some View {
        stack.environmentObject(rides).environmentObject(intervals).environmentObject(loop)
    }

    private var stack: some View {
        NavigationStack(path: $path) {
            List {
                Section {
                    UnitHeader(
                        state: link.state, demo: rides.demo, name: link.name, battery: link.status.batteryPercent,
                        lastSync: lastSync, detail: rides.demo ? nil : message)
                    if noBluetooth && !rides.demo {
                        Button("Demo") { rides.use(DemoTransport(), demo: true) }
                    }
                    #if DEBUG
                    if !rides.debug.isEmpty {
                        Text(rides.debug).font(.caption2.monospaced()).foregroundStyle(.secondary)
                    }
                    #endif
                }
                Section {
                    WeeklyPanel(library: library, missing: missing, openProfile: { profile = true }).equatable()
                }
                IntervalsSection(loop: loop)
                if let m = rides.message ?? library.error {
                    Section { Notice(text: m) }
                }
                Section {
                    ForEach(rides.rides, id: \.name) { r in
                        RideRow(
                            rides: rides, ride: r, brief: library.briefs[r.name],
                            downloaded: rides.downloaded.contains(r.name), uploaded: loop.uploaded[r.name] != nil,
                            active: rides.phase == .downloading(r.name),
                            locked: rides.phase == .listing || (rides.isDownloading && rides.phase != .downloading(r.name))
                        ).equatable()
                    }
                    if rides.rides.isEmpty && rides.phase == .idle {
                        Text("No rides listed").foregroundStyle(.secondary)
                    }
                } header: {
                    HStack {
                        Text("Rides")
                        Spacer()
                        if let line = rides.syncLine { Text(line).textCase(nil) }
                    }
                }
                WorkoutSection(rides: rides)
                PhoneWorkoutSection(store: store) { editing = $0 }
            }
            .onChange(of: rides.downloaded) { _, d in
                library.catchUp(rides.rides, downloaded: d)
                loop.uploadPass()  // a ride just synced goes up
            }
            .onChange(of: intervals.keyVersion) { _, _ in loop.keyChanged(intervals.hasKey) }
            .onChange(of: rides.rides) { _, l in
                library.catchUp(l, downloaded: rides.downloaded)
                lastSync = Date()  // the unit just answered with its list
            }
            .sheet(item: $editing) { WorkoutEditor(target: $0, store: store, rides: rides) }
            .navigationDestination(for: WorkoutRoute.self) { r in
                if let f = rides.workouts.first(where: { $0.key == r.key }) { WorkoutDetail(file: f) { d in
                    var d = d
                    d.name = pretty(d.name)
                    editing = EditorTarget(draft: d, file: nil)
                } }
            }
            .navigationTitle("Bike computer")
            .task {
                loop.attach(rides)
                loop.keyChanged(intervals.hasKey)
                await intervals.sync(rides: rides, force: false)
            }
            .refreshable {
                link.refresh()
                rides.refresh()
                await intervals.sync(rides: rides, force: false)
                loop.kick(force: true)
                try? await Task.sleep(nanoseconds: 800_000_000)
            }
            .navigationDestination(item: $rides.saved) { s in RideDetail(url: s.url) }
            .sheet(isPresented: $profile) { ProfileSheet() }
            .navigationDestination(isPresented: $fitness) { FitnessView(library: library) }
            .toolbar {
                Button { fitness = true } label: { Image(systemName: "chart.line.uptrend.xyaxis") }
                    .accessibilityLabel("Fitness")
                Button { profile = true } label: { Image(systemName: "person.crop.circle") }
                    .accessibilityLabel("Profile")
            }
            #if DEBUG
            .onAppear {
                // launch arguments for the Simulator screenshots: -demo, and -demo-dl to download the first ride
                let args = CommandLine.arguments
                guard args.contains("-demo") else { return }
                rides.use(DemoTransport(), demo: true)
                if args.contains("-demo-workout") {  // open the first workout's page
                    DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
                        if let f = rides.workouts.first { path = [WorkoutRoute(key: f.key)] }
                    }
                }
                if args.contains("-demo-edit") {  // the editor on a copy of the first workout (after a saved one)
                    DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
                        guard let f = rides.workouts.first, let w = decodeWorkout(bytes: f.bytes, ftp: 200, maxHr: 190) else { return }
                        var d = draftFromView(view: w)
                        d.name = pretty(d.name)
                        if args.contains("-demo-saved") { _ = try? store.save(d, replacing: nil) }
                        else { editing = EditorTarget(draft: d, file: nil) }
                    }
                }
                if args.contains("-demo-fitness") {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { fitness = true }
                }
                if args.contains("-demo-dl") {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
                        if let r = rides.rides.first { rides.download(r) }
                    }
                    if args.contains("-demo-open") {  // then the viewer on it (screenshots of the ride page)
                        DispatchQueue.main.asyncAfter(deadline: .now() + 5) {
                            if let r = rides.rides.first { rides.saved = SavedRide(url: RideBrowser.path(r.name)) }
                        }
                    }
                }
            }
            #endif
            .onChange(of: link.state) { _, s in
                if s == .connected && !rides.demo && !rides.hasTransport { rides.use(BleFileTransport()) }
            }
        }
    }

    /// Rides of the last 8 weeks the unit lists that are not on the phone yet.
    private var missing: Int {
        rides.rides.filter { RideBrowser.inWindow($0.name) && library.briefs[$0.name] == nil }.count
    }

    private var noBluetooth: Bool {
        switch link.state {
        case .bluetoothOff, .unauthorized, .unsupported: true
        default: false
        }
    }

    /// What the header says under the state, when the state needs words.
    private var message: String? {
        switch link.state {
        case .bluetoothOff: return "Bluetooth is off. Turn it on in Settings."
        case .unauthorized: return "Bluetooth is not allowed for this app. Allow it in Settings."
        case .unsupported: return "This device has no Bluetooth."
        case .notFound: return "The unit was not found. Wake it, and pair it in Settings if needed."
        case .failed(let why): return why
        case .searching, .connected: return nil
        }
    }
}

/// What a rider checks first: is the unit there, its battery, when the phone last heard from it.
struct UnitHeader: View {
    let state: LinkState
    let demo: Bool
    let name: String
    let battery: UInt8?
    let lastSync: Date?
    let detail: String?
    @Environment(\.dynamicTypeSize) private var type

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            let layout = type.isAccessibilitySize ? AnyLayout(VStackLayout(alignment: .leading, spacing: 8)) : AnyLayout(HStackLayout(spacing: 12))
            layout {
                Image(systemName: "bicycle").font(.title2).foregroundStyle(Tok.accent).accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text(demo ? "Demo unit" : name.isEmpty ? "Bike computer" : name).font(.headline)
                    chip
                }
                Spacer(minLength: 0)
                if let b = battery, state == .connected || demo {
                    Label("\(b) %", systemImage: Self.symbol(b)).font(.subheadline.monospacedDigit())
                        .foregroundStyle(b <= 15 ? Tok.alarm : .secondary)
                        .accessibilityLabel("Battery \(b) percent")
                }
            }
            if let d = detail { Text(d).font(.subheadline).foregroundStyle(.secondary) }
            if let t = lastSync, state == .connected || demo {
                Text("Synced \(t.formatted(.dateTime.hour().minute()))").font(.footnote).foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder private var chip: some View {
        switch (demo, state) {
        case (true, _): Label("Demo", systemImage: "play.circle")
        case (_, .connected): Label("Connected", systemImage: "checkmark.circle.fill").foregroundStyle(Tok.ok)
        case (_, .searching): Label("Searching", systemImage: "dot.radiowaves.left.and.right").foregroundStyle(.secondary)
        case (_, .notFound): Label("Not found", systemImage: "exclamationmark.triangle.fill").foregroundStyle(Tok.warn)
        case (_, .failed): Label("Failed", systemImage: "xmark.octagon.fill").foregroundStyle(Tok.alarm)
        default: Label("Unavailable", systemImage: "exclamationmark.triangle.fill").foregroundStyle(Tok.warn)
        }
    }

    private static func symbol(_ p: UInt8) -> String {
        p >= 88 ? "battery.100" : p >= 63 ? "battery.75" : p >= 38 ? "battery.50" : p >= 13 ? "battery.25" : "battery.0"
    }
}

/// A ride's row. Equatable, with the transfer's progress in its own view, so a download's ticks redraw only that bar.
struct RideRow: View, Equatable {
    let rides: RideBrowser
    let ride: RideEntry
    let brief: RideBrief?
    let downloaded: Bool
    let uploaded: Bool
    let active: Bool
    let locked: Bool
    @AppStorage("ftp") private var ftp = 200
    @AppStorage("maxHR") private var maxHR = 190

    init(rides: RideBrowser, ride: RideEntry, brief: RideBrief?, downloaded: Bool, uploaded: Bool, active: Bool, locked: Bool) {
        (self.rides, self.ride, self.brief, self.downloaded, self.uploaded, self.active, self.locked) =
            (rides, ride, brief, downloaded, uploaded, active, locked)
    }

    static func == (a: RideRow, b: RideRow) -> Bool {
        a.ride == b.ride && a.brief == b.brief && a.downloaded == b.downloaded && a.uploaded == b.uploaded && a.active == b.active
            && a.locked == b.locked
    }

    var body: some View {
        Button {
            if active { return }
            if downloaded {
                rides.saved = SavedRide(url: RideBrowser.path(ride.name))  // opens the viewer, link or not
            } else if !locked {
                rides.download(ride)
            }
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                Text(brief.map { Fmt.start($0.startUnix) } ?? RideBrowser.title(ride.name))
                Text(subtitle(brief)).font(.subheadline).foregroundStyle(.secondary)
                if uploaded {
                    Label("Intervals.icu", systemImage: "checkmark.circle").font(.caption).foregroundStyle(Tok.ok)
                }
                if active { RideProgress(rides: rides) }
            }
        }
        .foregroundStyle(.primary)
    }
}

struct RideProgress: View {
    @ObservedObject var rides: RideBrowser

    var body: some View {
        ProgressView(value: Double(rides.progress.received), total: Double(max(rides.progress.total, 1)))
        HStack {
            Text("\(rides.progress.received / 1024) of \(rides.progress.total / 1024) KB")
            Spacer()
            Text("\(Fmt.dec(Double(rides.progress.bytesPerS) / 1024)) KB/s")
            Button { rides.cancel() } label: { Label("Cancel", systemImage: "xmark.circle") }
                .buttonStyle(.borderless).frame(minHeight: 44)
        }
        .font(.caption)
        .foregroundStyle(.secondary)
    }
}

extension RideRow {
    /// "2 h 31 · 68.4 km · TSS 112" ("HR load 80" without power), or why there is no such line yet.
    func subtitle(_ b: RideBrief?) -> String {
        guard let b else {
            return downloaded ? "Reading the ride…" : locked ? "Waiting for the sync" : "Not on the phone yet"
        }
        var parts = [Fmt.duration(UInt64(b.movingS)), Fmt.km(b.distanceM)]
        let s = rideStat(b: b, name: ride.name, ftp: UInt16(clamping: ftp), maxHr: UInt16(clamping: maxHR), tzOffsetS: 0)
        if let l = s.load { parts.append("\(s.loadFromHr ? "HR load" : "TSS") \(l)") }
        return parts.joined(separator: " · ")
    }
}

struct ShareSheet: UIViewControllerRepresentable {
    let url: URL
    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: [url], applicationActivities: nil)
    }
    func updateUIViewController(_ c: UIActivityViewController, context: Context) {}
}
