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
    @Environment(\.scenePhase) private var scenePhase
    @State private var profile = false
    @State private var fitness = false
    @State private var allRides = false
    @StateObject private var store = WorkoutStore()
    @State private var path = NavigationPath()
    @StateObject private var routes = RouteStore()
    @State private var editing: EditorTarget?
    @State private var lastSync: Date?
    @StateObject private var profileValues = Profile()
    @State private var scrollTo: String?

    // the environment objects sit outside the stack: the sheets and the pushed pages are presented from inside it
    var body: some View {
        stack.environmentObject(rides).environmentObject(intervals).environmentObject(loop).environmentObject(profileValues)
    }

    private var stack: some View {
        NavigationStack(path: $path) {
            ScrollViewReader { proxy in
            List {
                Section {
                    UnitHeader(
                        state: link.state, demo: rides.demo, name: link.name, battery: link.status.batteryPercent,
                        lastSync: lastSync, detail: rides.demo ? nil : message)
                    if noBluetooth && !rides.demo {
                        Button("Demo") { rides.use(DemoTransport(), demo: true) }
                    }
                    #if DEBUG
                    if !rides.debug.isEmpty && !rides.demo {
                        Text(rides.debug).font(.caption2.monospaced()).foregroundStyle(.secondary)
                    }
                    #endif
                }
                Section {
                    WeeklyPanel(library: library, missing: missing, openProfile: { profile = true }).equatable()
                }
                IntervalsSection(loop: loop).id("intervals")
                if let m = rides.message ?? library.error {
                    Section { Notice(text: m) }
                }
                ridesSection
                WorkoutSection(rides: rides).id("unit")
                PhoneWorkoutSection(store: store) { editing = $0 }.id("phone")
                RoutesSection(store: routes, rides: rides).id("routes")
                UnitRoutesSection(store: routes, rides: rides).id("unitroutes")
            }
            .onChange(of: scrollTo) { _, id in
                if let id { withAnimation { proxy.scrollTo(id, anchor: .top) } }
            }
            }
            .onChange(of: rides.downloaded) { _, d in
                library.catchUp(rides.rides, downloaded: d)
                loop.uploadPass()  // a ride just synced goes up
            }
            .onChange(of: intervals.keyVersion) { _, _ in loop.keyChanged(intervals.hasKey) }
            .onChange(of: scenePhase) { _, p in if p == .active { loop.foreground() } else if p == .background { library.saveNow() } }
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
            .navigationDestination(for: RouteRoute.self) { r in
                if let p = routes.items.first(where: { $0.name == r.name }) { RoutePage(route: p, rides: rides, store: routes) }
            }
            .onOpenURL { url in  // "Open in Bryton" from Files or the share sheet
                guard url.pathExtension.lowercased() == "gpx" else { return }
                routes.importGpx(url) { path.append(RouteRoute(name: $0.name)) }
            }
            .navigationTitle("Rides")
            .navigationBarTitleDisplayMode(.inline)
            .task {
                loop.attach(rides)
                rides.onRouteSent = { routes.remember($0) }
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
            .navigationDestination(isPresented: $allRides) { AllRidesList(rides: rides, library: library, loop: loop) }
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
                        if let f = rides.workouts.first { path.append(WorkoutRoute(key: f.key)) }
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
                // a sheet or a scroll position of the home list, for the screenshots (docs/app.md, "Screenshots")
                let later: (Double, @escaping () -> Void) -> Void = { s, f in DispatchQueue.main.asyncAfter(deadline: .now() + s, execute: f) }
                if args.contains("-demo-profile") { later(2) { profile = true } }
                if args.contains("-demo-scroll-phone") { later(3) { scrollTo = "phone" } }
                if args.contains("-demo-routes") {  // the demo GPX imported, its page opened with -demo-route-open
                    if let r = try? routes.add(gpx: Data(demoRouteGpx()), fallback: "Eiffel loop"), args.contains("-demo-route-open") {
                        later(3) { path.append(RouteRoute(name: r.name)) }
                    }
                    if args.contains("-demo-route-sent") {  // sent without the sheet, once the demo unit is ready
                        later(2.5) { if let r = routes.items.first { rides.sendRoute(name: r.name, source: r.name, bytes: r.view.fit) } }
                    }
                    if args.contains("-demo-scroll-routes") { later(3) { scrollTo = "routes" } }
                    if args.contains("-demo-scroll-unitroutes") { later(3) { scrollTo = "unitroutes" } }
                }
                if args.contains("-demo-scroll-intervals") { later(3) { scrollTo = "intervals" } }
                if args.contains("-demo-scroll-rides") { later(3) { scrollTo = "rides" } }
                if args.contains("-demo-all-rides") { later(3) { allRides = true } }  // the full list, pushed
                if args.contains("-demo-scroll-unit") { later(3) { scrollTo = "unit" } }
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

    @ViewBuilder private var ridesSection: some View {
        Section {
            RideRows(rides: rides, library: library, loop: loop, list: Array(rides.rides.prefix(1)))  // the latest only
            if rides.rides.isEmpty && rides.phase == .idle {
                EmptyRides(state: link.state, demo: rides.demo) { link.refresh(); rides.refresh() }
            }
            if !rides.rides.isEmpty {
                Button { allRides = true } label: {
                    HStack {
                        Text("All rides (\(rides.rides.count))")
                        Spacer()
                        Image(systemName: "chevron.right").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                    }
                }
                .foregroundStyle(Tok.accent)
            }
        } header: {
            HStack {
                Text("Rides")
                Spacer()
                if let line = rides.syncLine { Text(line).textCase(nil) }
            }
        }
        .id("rides")
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
        case (_, .searching):
            HStack(spacing: 6) { ProgressView().controlSize(.small); Text("Searching") }.foregroundStyle(.secondary)
        case (_, .notFound): Label("Not found", systemImage: "exclamationmark.triangle.fill").foregroundStyle(Tok.warn)
        case (_, .failed): Label("Failed", systemImage: "xmark.octagon.fill").foregroundStyle(Tok.alarm)
        default: Label("Unavailable", systemImage: "exclamationmark.triangle.fill").foregroundStyle(Tok.warn)
        }
    }

    private static func symbol(_ p: UInt8) -> String {
        p >= 88 ? "battery.100" : p >= 63 ? "battery.75" : p >= 38 ? "battery.50" : p >= 13 ? "battery.25" : "battery.0"
    }
}

/// The rides of a list, one row each: the home screen's latest ride, or every ride on the full list.
struct RideRows: View {
    @ObservedObject var rides: RideBrowser
    @ObservedObject var library: RideLibrary
    @ObservedObject var loop: IntervalsLoop
    let list: [RideEntry]

    var body: some View {
        ForEach(list, id: \.name) { r in
            RideRow(
                rides: rides, ride: r, brief: library.briefs[r.name],
                downloaded: rides.downloaded.contains(r.name), uploaded: loop.uploaded[r.name] != nil,
                active: rides.phase == .downloading(r.name),
                locked: rides.phase == .listing || (rides.isDownloading && rides.phase != .downloading(r.name))
            ).equatable()
        }
    }
}

/// The full ride list, pushed from the home screen's "All rides" row.
struct AllRidesList: View {
    @ObservedObject var rides: RideBrowser
    @ObservedObject var library: RideLibrary
    @ObservedObject var loop: IntervalsLoop

    var body: some View {
        List { RideRows(rides: rides, library: library, loop: loop, list: rides.rides) }
            .navigationTitle("All rides")
            .navigationBarTitleDisplayMode(.inline)
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
    @EnvironmentObject private var profile: Profile
    @ScaledMetric(relativeTo: .title3) private var glyph: CGFloat = 28

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
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .top, spacing: 12) {
                    Image(systemName: "bicycle").font(.title3).foregroundStyle(Tok.accent).frame(width: glyph).accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(brief.map { Fmt.start($0.startUnix) } ?? RideBrowser.title(ride.name))
                        Text(subtitle(brief)).font(.subheadline).foregroundStyle(.secondary)
                        if uploaded {
                            Label("Intervals.icu", systemImage: "checkmark.circle").font(.caption).foregroundStyle(Tok.ok)
                        }
                    }
                    Spacer(minLength: 4)
                    if let b = brief { Text(Fmt.distance(b.distanceM)).numeral(.title3).fixedSize() }
                }
                // the row's whole width: beside the glyph the counts and Cancel wrapped a letter a line
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
    /// "2 h 31 · TSS 112" ("Load 80 (HR)" without power; the distance is the row's big number), or why there is no
    /// such line yet.
    func subtitle(_ b: RideBrief?) -> String {
        guard let b else {
            return downloaded ? "Reading the ride…" : locked ? "Waiting for the sync" : "Not on the phone yet"
        }
        var parts = [Fmt.duration(UInt64(b.movingS))]
        let s = rideStat(b: b, name: ride.name, ftp: UInt16(clamping: profile.ftp), maxHr: UInt16(clamping: profile.maxHR), tzOffsetS: 0)
        if let l = s.load { parts.append(s.loadFromHr ? "Load \(l) (HR)" : "TSS \(l)") }
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

/// No ride listed: why, and what to do (Retry, the pairing step).
struct EmptyRides: View {
    let state: LinkState
    let demo: Bool
    let retry: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.headline)
            Text(help).font(.subheadline).foregroundStyle(.secondary)
            if state != .searching || demo {
                Button(action: retry) { Label("Try again", systemImage: "arrow.clockwise") }
                    .buttonStyle(.bordered).frame(minHeight: 44)
            }
        }
        .padding(.vertical, 4)
    }

    private var title: String {
        switch state {
        case .connected: "No rides on the unit"
        case .searching: "Looking for your unit"
        case .notFound, .failed: "Unit not found"
        default: "Bluetooth is not available"
        }
    }

    private var help: String {
        switch state {
        case .connected: "A ride appears here once the unit has saved one. Pull down to ask again."
        case .searching: "Wake the unit and keep it close."
        case .notFound, .failed: "Wake the unit. The first time, pair it in Settings, Bluetooth, then try again."
        default: "Allow Bluetooth for this app in Settings, or use the Demo."
        }
    }
}
