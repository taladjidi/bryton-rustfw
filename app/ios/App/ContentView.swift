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
    @State private var profile = false
    @StateObject private var store = WorkoutStore()
    @State private var path: [WorkoutRoute] = []
    @State private var editing: EditorTarget?

    var body: some View {
        NavigationStack(path: $path) {
            List {
                Section { WeeklyPanel(library: library, rides: rides) }
                Section {
                    Text(message).foregroundStyle(.secondary)
                    if link.state == .connected {
                        row("Name", link.name)
                        row("Maker", link.status.maker ?? "unknown")
                        row("Battery", link.status.batteryPercent.map { "\($0) %" } ?? "unknown")
                    }
                    if noBluetooth && !rides.demo {
                        Button("Demo") { rides.use(DemoTransport(), demo: true) }
                    }
                    if !rides.debug.isEmpty {
                        Text(rides.debug).font(.caption2.monospaced()).foregroundStyle(.secondary)
                    }
                }
                if let m = rides.message {
                    Section { Text(m).foregroundStyle(.orange) }
                }
                Section {
                    ForEach(rides.rides, id: \.name) { r in RideRow(rides: rides, library: library, ride: r) }
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
            .onChange(of: rides.downloaded) { _, d in library.catchUp(rides.rides, downloaded: d) }
            .onChange(of: rides.rides) { _, l in library.catchUp(l, downloaded: rides.downloaded) }
            .sheet(item: $editing) { WorkoutEditor(target: $0, store: store, rides: rides) }
            .navigationDestination(for: WorkoutRoute.self) { r in
                if let f = rides.workouts.first(where: { $0.key == r.key }) { WorkoutDetail(file: f) { d in
                    var d = d
                    d.name = pretty(d.name)
                    editing = EditorTarget(draft: d, file: nil)
                } }
            }
            .navigationTitle("Bike computer")
            .refreshable {
                link.refresh()
                rides.refresh()
                try? await Task.sleep(nanoseconds: 800_000_000)
            }
            .navigationDestination(item: $rides.saved) { s in RideDetail(url: s.url) }
            .sheet(isPresented: $profile) { ProfileSheet() }
            .toolbar {
                Button { profile = true } label: { Image(systemName: "gearshape") }
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
                if args.contains("-demo-dl") {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
                        if let r = rides.rides.first { rides.download(r) }
                    }
                }
            }
            #endif
            .onChange(of: link.state) { _, s in
                if s == .connected && !rides.demo && !rides.hasTransport { rides.use(BleFileTransport()) }
            }
        }
    }

    private var noBluetooth: Bool {
        switch link.state {
        case .bluetoothOff, .unauthorized, .unsupported: true
        default: false
        }
    }

    private func row(_ k: String, _ v: String) -> some View {
        HStack { Text(k).foregroundStyle(.secondary); Spacer(); Text(v) }
    }

    private var message: String {
        if rides.demo { return "Demo mode: a fake unit with made-up rides." }
        switch link.state {
        case .bluetoothOff: return "Bluetooth is off. Turn it on in Settings."
        case .unauthorized: return "Bluetooth is not allowed for this app. Allow it in Settings."
        case .unsupported: return "This device has no Bluetooth."
        case .searching: return "Searching for the unit..."
        case .notFound: return "The unit was not found. Wake it, and pair it in Settings if needed."
        case .connected: return "Connected"
        case .failed(let why): return why
        }
    }
}

struct RideRow: View {
    @ObservedObject var rides: RideBrowser
    @ObservedObject var library: RideLibrary
    let ride: RideEntry
    @AppStorage("ftp") private var ftp = 200
    @AppStorage("maxHR") private var maxHR = 190

    var body: some View {
        let active = rides.phase == .downloading(ride.name)
        Button {
            if active { return }
            if rides.downloaded.contains(ride.name) {
                rides.saved = SavedRide(url: RideBrowser.path(ride.name))  // opens the viewer
            } else {
                rides.download(ride)
            }
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                let brief = library.briefs[ride.name]
                Text(brief.map { Fmt.start($0.startUnix) } ?? RideBrowser.title(ride.name))
                Text(subtitle(brief)).font(.subheadline).foregroundStyle(.secondary)
                if active {
                    ProgressView(value: Double(rides.progress.received), total: Double(max(rides.progress.total, 1)))
                    HStack {
                        Text("\(rides.progress.received / 1024) of \(rides.progress.total / 1024) KB")
                        Spacer()
                        Text(String(format: "%.1f KB/s", Double(rides.progress.bytesPerS) / 1024))
                        Button("Cancel") { rides.cancel() }.buttonStyle(.borderless)
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
            }
        }
        .foregroundStyle(.primary)
        .disabled(rides.phase == .listing || (rides.isDownloading && !active))
    }
}

extension RideRow {
    /// "2 h 31 · 68.4 km · TSS 112", or why there is no such line yet.
    func subtitle(_ b: RideBrief?) -> String {
        guard let b else { return rides.downloaded.contains(ride.name) ? "Reading the ride..." : "not on the phone yet" }
        var parts = [Fmt.duration(UInt64(b.movingS)), Fmt.km(b.distanceM)]
        let s = rideStat(b: b, ftp: UInt16(clamping: ftp), maxHr: UInt16(clamping: maxHR), tzOffsetS: 0)
        if let l = s.load { parts.append("TSS \(l)") }
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
