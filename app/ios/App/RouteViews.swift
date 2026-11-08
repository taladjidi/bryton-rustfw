// Routes: the home list's sections (on the phone, on the unit), the route page (map, elevation with its climbs, the
// numbers) and the sheet that sends one to the unit. The GPX is read and the FIT course written by the Rust core
// (route.rs, with the firmware's GPX reader and climb finder); this file only draws and moves it.

import Charts
import MapKit
import SwiftUI

/// "Routes" on the phone: the imported ones, and the import.
struct RoutesSection: View {
    @ObservedObject var store: RouteStore
    @ObservedObject var rides: RideBrowser
    @State private var importing = false

    var body: some View {
        Section {
            ForEach(store.items) { r in
                NavigationLink(value: RouteRoute(name: r.name)) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(r.name)
                        HStack {
                            Text(Fmt.distance(Double(r.view.distanceM))).numeral(.subheadline)
                            if r.view.hasAlt { Text("+\(Fmt.height(r.view.ascentM))") }
                            Spacer()
                            if store.onUnit(r, rides.unitRoutes) {
                                Label("On the unit", systemImage: "checkmark.circle").foregroundStyle(Tok.ok)
                            }
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    }
                }
                .swipeActions { Button("Delete", role: .destructive) { store.delete(r) } }
                .contextMenu { Button("Delete from the phone", systemImage: "trash", role: .destructive) { store.delete(r) } }
            }
            Button { importing = true } label: { Label("Import a GPX file", systemImage: "square.and.arrow.down") }
                .fileImporter(isPresented: $importing, allowedContentTypes: [.data]) { r in
                    if case .success(let url) = r { store.importGpx(url) }  // stays on the list: the row appears
                }
            if store.importing { HStack { ProgressView(); Text("Reading the file…").foregroundStyle(.secondary) } }
            if let p = store.problem { Notice(text: p, level: .alarm) }
        } header: {
            Text("Routes")
        } footer: {
            if store.items.isEmpty {
                Text("Export a GPX from a planner (Komoot, RideWithGPS, BRouter...), or open one in this app from Files.")
            }
        }
    }
}

/// "Routes on the unit": the courses of its `\Courses` folder, deleted from here.
struct UnitRoutesSection: View {
    @ObservedObject var store: RouteStore
    @ObservedObject var rides: RideBrowser
    @State private var deleting: UnitRoute?
    @State private var renaming: UnitRoute?
    @State private var newName = ""

    var body: some View {
        Section("Routes on the unit") {
            ForEach(rides.unitRoutes) { u in
                let l = store.label(u)
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(l?.name ?? "Route \(u.key)")
                        if l == nil {
                            Text("Not sent from this phone").font(.caption).foregroundStyle(.secondary)
                        } else if l?.own == true {
                            Text("Named on this phone").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    Spacer()
                    Text("\(max(1, u.size / 1024)) KB").font(.caption).foregroundStyle(.secondary)
                }
                .swipeActions {
                    Button("Delete", role: .destructive) { deleting = u }
                    if store.unitName(u) == nil { Button("Rename") { renaming = u; newName = l?.name ?? "" }.tint(.blue) }
                }
                .contextMenu {
                    if store.unitName(u) == nil { Button("Name on this phone", systemImage: "pencil") { renaming = u; newName = l?.name ?? "" } }
                    Button("Delete from the unit", systemImage: "trash", role: .destructive) { deleting = u }
                }
            }
            if rides.unitRoutes.isEmpty {
                Text(rides.routeListError ?? (rides.routesListed ? "No routes on the unit" : "Reading routes…")).foregroundStyle(.secondary)
            } else if let e = rides.routeListError {
                Notice(text: e)
            }
            if let m = rides.routeMessage { Notice(text: m, level: .alarm) }
        }
        #if DEBUG
        .onAppear {  // Simulator screenshot: -demo-route-delete asks to delete the first course
            if CommandLine.arguments.contains("-demo-route-delete") {
                DispatchQueue.main.asyncAfter(deadline: .now() + 4) { deleting = rides.unitRoutes.first }
            }
        }
        #endif
        .alert("Name this route", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Name", text: $newName)
            Button("Save") { if let u = renaming { store.rename(u, to: newName) }; renaming = nil }
            Button("Cancel", role: .cancel) { renaming = nil }
        } message: {
            Text("Only on this phone: the unit lists the file under its own name. 1 to 20 letters, digits, spaces, - and _.")
        }
        .confirmationDialog(
            "Delete this route from the unit?",
            isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }), titleVisibility: .visible
        ) {
            Button("Delete from the unit", role: .destructive) {
                if let u = deleting { rides.deleteRoute(u) }
                deleting = nil
            }
        } message: {
            Text("The copy on the phone stays. The unit refuses the route it is following.")
        }
    }
}

/// A climb's part of the track, for the map and the chart.
private struct Slice: Identifiable {
    let id: Int
    let color: Color
    let pts: [(x: Double, y: Double)]
    let coords: [CLLocationCoordinate2D]
    let grade: Int
}

struct RoutePage: View {
    let route: PhoneRoute
    @ObservedObject var rides: RideBrowser
    @ObservedObject var store: RouteStore
    @State private var sending = false
    @State private var selX: Double?
    @ScaledMetric private var chartHeight: CGFloat = 150
    @ScaledMetric private var mapHeight: CGFloat = 260

    private var v: RouteView { route.view }

    private var coords: [CLLocationCoordinate2D] {
        v.points.map { CLLocationCoordinate2D(latitude: $0.lat, longitude: $0.lon) }
    }

    /// Each climb's stretch: points inside it, with the chart's units (distance, height).
    private var slices: [Slice] {
        v.climbs.enumerated().map { i, c in
            let inside = v.points.filter { $0.distM >= c.startM && $0.distM <= c.startM + c.lenM }
            return Slice(
                id: i, color: Tok.grade(Int(c.gradeX10)),
                pts: inside.compactMap { p in p.altM.map { (Fmt.toDist(Double(p.distM)), Fmt.toHeight($0)) } },
                coords: inside.map { CLLocationCoordinate2D(latitude: $0.lat, longitude: $0.lon) }, grade: Int(c.gradeX10))
        }
    }

    /// The point nearest the chart's cursor.
    private var cursor: RoutePoint? {
        guard let x = selX else { return nil }
        return v.points.min { abs(Fmt.toDist(Double($0.distM)) - x) < abs(Fmt.toDist(Double($1.distM)) - x) }
    }

    var body: some View {
        let slices = slices
        List {
            Section {
                RideMap(
                    points: coords,
                    selected: cursor.map { CLLocationCoordinate2D(latitude: $0.lat, longitude: $0.lon) },
                    climbs: slices.map { ClimbLine.Spec(coords: $0.coords, color: Tok.uiGrade($0.grade)) }
                )
                .frame(height: mapHeight)
                .listRowInsets(EdgeInsets())
                .accessibilityLabel("Map of the route")
                numbers
            }
            if v.hasAlt {
                Section("Elevation") { profile(slices) }
            } else {
                Section { Notice(text: "This file has no elevation: no profile and no climbs.") }
            }
            if !v.climbs.isEmpty {
                Section("Climbs") {
                    ForEach(v.climbs.indices, id: \.self) { i in
                        let c = v.climbs[i]
                        HStack(alignment: .top, spacing: 8) {
                            RoundedRectangle(cornerRadius: 2).fill(Tok.grade(Int(c.gradeX10))).frame(width: 6)
                            VStack(alignment: .leading, spacing: 2) {
                                Text("\(i + 1)  \(Fmt.distance(Double(c.lenM))) at \(Fmt.dec(Double(c.gradeX10) / 10)) %, +\(Fmt.height(c.gainM))")
                                    .font(.subheadline.bold())
                                Text("from \(Fmt.distance(Double(c.startM)))").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                            }
                        }
                        .accessibilityElement(children: .combine)
                    }
                }
            }
            Section {
                Button { sending = true } label: { Label("Send to the unit", systemImage: "arrow.up.circle") }
                    .frame(minHeight: 44)
                if v.thinned { Notice(text: "The file had more points than the unit keeps, so the route was thinned.").font(.caption) }
                ForEach(v.notes, id: \.self) { Notice(text: $0).font(.caption) }
            } footer: {
                Text("The unit follows it from COURSES, with the climbs and \(v.cueCount) turns or waypoints of the file.")
            }
        }
        .navigationTitle(route.name)
        .navigationBarTitleDisplayMode(.inline)
        .sheet(isPresented: $sending) { SendRouteSheet(route: route, rides: rides, store: store) }
        #if DEBUG
        .onAppear {  // Simulator: `-demo-route-send` opens the send sheet
            if CommandLine.arguments.contains("-demo-route-send") {
                DispatchQueue.main.asyncAfter(deadline: .now() + 1) { sending = true }
            }
        }
        #endif
    }

    private var numbers: some View {
        Grid(horizontalSpacing: 16, verticalSpacing: 4) {
            GridRow {
                stat("Distance", Fmt.distance(Double(v.distanceM)))
                if v.hasAlt {
                    stat("Ascent", Fmt.height(v.ascentM))
                    stat("Descent", Fmt.height(v.descentM))
                }
                stat("Climbs", "\(v.climbs.count)")
            }
        }
        .accessibilityElement(children: .combine)
    }

    private func stat(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(value).numeral(.title3).lineLimit(1).minimumScaleFactor(0.6)
            Text(label).font(.caption).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func profile(_ slices: [Slice]) -> some View {
        let line = v.points.compactMap { p in p.altM.map { (x: Fmt.toDist(Double(p.distM)), y: Fmt.toHeight($0)) } }
        let lo = (line.map(\.y).min() ?? 0).rounded(.down), hi = (line.map(\.y).max() ?? 1).rounded(.up)
        return VStack(alignment: .leading, spacing: 2) {
            if let c = cursor, let a = c.altM {
                Text("\(Fmt.distance(Double(c.distM))) · \(Fmt.height(Int(a.rounded())))").numeral(.subheadline).foregroundStyle(Tok.accent)
            }
            Chart {
                ForEach(Array(line.enumerated()), id: \.offset) { _, p in
                    AreaMark(x: .value("x", p.x), yStart: .value("floor", lo), yEnd: .value("y", p.y)).foregroundStyle(Tok.accent.opacity(0.25))
                    LineMark(x: .value("x", p.x), y: .value("y", p.y)).foregroundStyle(Tok.accent)
                }
                ForEach(slices) { c in
                    ForEach(Array(c.pts.enumerated()), id: \.offset) { _, p in
                        LineMark(x: .value("x", p.x), y: .value("y", p.y), series: .value("climb", c.id))
                            .foregroundStyle(c.color).lineStyle(StrokeStyle(lineWidth: 4))
                    }
                }
            }
            .chartXSelection(value: $selX)
            .chartXAxisLabel(Fmt.distUnit, position: .bottom, alignment: .trailing)
            .chartYScale(domain: lo...max(hi, lo + 1))
            .frame(height: chartHeight)
            .accessibilityLabel("Elevation")
            .accessibilityValue("lowest \(Fmt.dec(lo, 0)), highest \(Fmt.dec(hi, 0)), \(v.climbs.count) climbs")
        }
    }
}

/// Names the course and sends it, with progress and the unit's refusal in words.
struct SendRouteSheet: View {
    let route: PhoneRoute
    @ObservedObject var rides: RideBrowser
    @ObservedObject var store: RouteStore
    @Environment(\.dismiss) private var dismiss
    @State private var name: String
    @State private var started = false

    init(route: PhoneRoute, rides: RideBrowser, store: RouteStore) {
        (self.route, self.rides, self.store) = (route, rides, store)
        _name = State(initialValue: route.name)
    }

    private var valid: Bool { routeName(text: name) == name }
    private var busy: Bool { rides.phase == .writing }
    /// The name of a course on the unit this one would meet.
    private var clash: (name: String, same: Bool)? { valid ? store.clash(name, route: route, in: rides.unitRoutes) : nil }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Name on the unit", text: $name)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                        .submitLabel(.done)
                } footer: {
                    footer
                }
                Section {
                    Button("Send \(name)") { send() }.disabled(busy || !valid || (clash.map { !$0.same } ?? false))
                    if rides.phase == .writing {
                        ProgressView(value: Double(rides.progress.received), total: Double(max(rides.progress.total, 1)))
                        Text("\(rides.progress.received / 1024) of \(rides.progress.total / 1024) KB").font(.caption).foregroundStyle(.secondary)
                    }
                    if started, rides.sendDone { Label("Sent", systemImage: "checkmark.circle").foregroundStyle(Tok.ok) }
                    if started, let e = rides.sendError { Notice(text: e, level: .alarm) }
                }
            }
            .navigationTitle("Send to the unit")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button(rides.sendDone ? "Done" : "Close") { dismiss() } } }
            .interactiveDismissDisabled(rides.phase == .writing)
            #if DEBUG
            .task {  // Simulator: `-demo-route-send-go` presses Send once the demo unit is ready
                guard CommandLine.arguments.contains("-demo-route-send-go") else { return }
                try? await Task.sleep(for: .seconds(2))
                send()
            }
            #endif
        }
    }

    @ViewBuilder private var footer: some View {
        if !valid {
            Text("1 to 20 letters, digits, spaces, - and _. Accents are not kept.").foregroundStyle(Tok.alarm)
        } else if let c = clash {
            VStack(alignment: .leading, spacing: 6) {
                if c.same {
                    Text("This route is already on the unit as \(c.name). Sending it again changes nothing.")
                } else {
                    Text("A route is already on the unit as \(c.name): rename this one or delete that one first.")
                        .foregroundStyle(Tok.alarm)
                }
            }
        } else {
            Text("The unit lists it as \(name.uppercased()) in COURSES. Sending the same route twice keeps one.")
        }
    }

    private func send() {
        started = true
        rides.sendRoute(name: name, source: route.name, bytes: route.view.fit)
    }
}
