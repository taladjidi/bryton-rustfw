// The planned workouts of the unit, read only: a section of the main list and a detail page with the step chart.
// The file is decoded by the Rust core (the firmware's own workout parser); this file only draws it.

import Charts
import SwiftUI

/// A workout file as the unit holds it.
struct WorkoutFile: Identifiable {
    let key: String  // YYYYMMDD and the file's number on that day
    let bytes: Data
    var id: String { key }
}

/// The navigation value of a workout page (its own type, beside the ride viewer's String destinations).
struct WorkoutRoute: Hashable { let key: String }

/// A power zone's colour: the shared tokens, as the unit's workout page draws them.
func zoneColor(_ z: UInt8) -> Color { z >= 1 && z <= 7 ? Tok.zone(Int(z)) : .secondary }

/// The unit keeps names in capitals: "SWEET SPOT 3X10" as "Sweet spot 3x10".
func pretty(_ name: String) -> String { name.prefix(1) + name.dropFirst().lowercased() }

/// The next of a list by its YYYYMMDD day (today or later, the soonest), else the latest.
func nextWorkout<T>(_ items: [T], day: (T) -> Int) -> T? {
    let c = Calendar.current.dateComponents([.year, .month, .day], from: .now)
    let today = (c.year ?? 0) * 10000 + (c.month ?? 0) * 100 + (c.day ?? 0)
    return items.filter { day($0) >= today }.min { day($0) < day($1) } ?? items.max { day($0) < day($1) }
}

/// The home screen's "On the unit": the next planned workout, then the row that pushes all of them.
struct WorkoutSection: View {
    @ObservedObject var rides: RideBrowser

    private var next: [WorkoutFile] { nextWorkout(rides.workouts, day: { Int($0.key.prefix(8)) ?? 0 }).map { [$0] } ?? [] }

    var body: some View {
        Section("On the unit") {
            WorkoutRows(rides: rides, list: next)
            if !rides.workouts.isEmpty {
                NavigationLink { AllWorkoutsList(rides: rides) } label: {
                    Text("All workouts on the unit (\(rides.workouts.count))").foregroundStyle(Tok.accent)
                }
            }
            if rides.workouts.isEmpty {
                Text(rides.phase == .workouts ? "Reading workouts…" : "No workouts planned")
                    .foregroundStyle(.secondary)
            }
        }
    }
}

/// The unit's workouts as rows: a tap opens the page, swipe and long press ask to delete (a copy on the phone stays).
struct WorkoutRows: View {
    @ObservedObject var rides: RideBrowser
    let list: [WorkoutFile]
    @EnvironmentObject private var profile: Profile
    private var ftp: Int { profile.ftp }
    private var maxHR: Int { profile.maxHR }
    @State private var deleting: WorkoutFile?
    @EnvironmentObject private var loop: IntervalsLoop

    var body: some View {
        ForEach(list) { f in
            if let w = decodeWorkout(bytes: f.bytes, ftp: UInt16(clamping: ftp), maxHr: UInt16(clamping: maxHR)) {
                NavigationLink(value: WorkoutRoute(key: f.key)) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(pretty(w.name))
                        if loop.sentHashes.contains(fileCrc(bytes: f.bytes)) {
                            Text("from Intervals.icu").font(.caption).foregroundStyle(Tok.ok)
                        }
                        HStack {
                            Text(Fmt.dayKey(f.key))
                            Spacer()
                            Text(Fmt.duration(UInt64(w.totalS)))
                            if w.tss > 0 { Text("TSS \(w.tss)") }
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    }
                }
                // only when nothing else uses the link: the same guard as a download
                .swipeActions { Button("Delete", role: .destructive) { deleting = f }.disabled(rides.phase != .idle) }
                .contextMenu { Button("Delete from the unit", systemImage: "trash", role: .destructive) { deleting = f }.disabled(rides.phase != .idle) }
            }
        }
        #if DEBUG
        .onAppear {  // Simulator screenshot: -demo-delete asks to delete the first workout
            if CommandLine.arguments.contains("-demo-delete") {
                DispatchQueue.main.asyncAfter(deadline: .now() + 4) { deleting = list.first }
            }
        }
        #endif
        .confirmationDialog("Delete this workout from the unit?", isPresented: Binding(
            get: { deleting != nil }, set: { if !$0 { deleting = nil } }), titleVisibility: .visible
        ) {
            Button("Delete from the unit", role: .destructive) {
                if let f = deleting { rides.deleteWorkout(file: f) }
                deleting = nil
            }
        } message: {
            Text("A copy kept on the phone stays.")
        }
    }
}

/// Every workout on the unit, pushed from the home screen's "All workouts on the unit" row.
struct AllWorkoutsList: View {
    @ObservedObject var rides: RideBrowser

    var body: some View {
        List { WorkoutRows(rides: rides, list: rides.workouts) }
            .navigationTitle("All workouts")
            .navigationBarTitleDisplayMode(.inline)
    }
}

struct WorkoutDetail: View {
    let file: WorkoutFile
    var onEdit: (WorkoutDraft) -> Void = { _ in }
    @EnvironmentObject private var profile: Profile
    private var ftp: Int { profile.ftp }
    private var maxHR: Int { profile.maxHR }
    @State private var showProfile = false

    var body: some View {
        let w = decodeWorkout(bytes: file.bytes, ftp: UInt16(clamping: ftp), maxHr: UInt16(clamping: maxHR))
        List {
            if let w {
                Section {
                    StepChart(steps: w.steps, ftp: ftp)
                    ZoneLegend(ftp: ftp)
                    HStack {
                        Text(Fmt.duration(UInt64(w.totalS)))
                        Spacer()
                        if w.tss > 0 { Text(Fmt.ifTss(w.intensityX100, w.tss)) }
                    }
                    if w.cut { Notice(text: "Longer than the unit keeps: the end is cut.").font(.caption) }
                    Button { showProfile = true } label: {
                        HStack {
                            Label("FTP \(ftp) W, max heart rate \(maxHR) bpm", systemImage: "person.crop.circle")
                            Spacer()
                            Image(systemName: "chevron.right").font(.footnote.bold()).foregroundStyle(.tertiary)
                        }
                        .font(.subheadline).frame(minHeight: 44)
                    }
                } footer: {
                    Text("Targets follow your profile. IF and TSS are estimates from power steps with a time; they are left out when other steps are mixed in.")
                }
                Section("Steps") {
                    ForEach(Array(w.steps.enumerated()), id: \.offset) { i, s in StepRow(index: i + 1, step: s) }
                }
            } else {
                Text("This file is not a workout.")
            }
        }
        .navigationTitle(w.map { pretty($0.name) } ?? "Workout")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if let w {
                Button { onEdit(draftFromView(view: w)) } label: { Label("Edit a copy", systemImage: "square.and.pencil") }
            }
        }
        .sheet(isPresented: $showProfile) { ProfileSheet() }
    }
}

/// Zone chips under a step chart: the colour of each zone with its watt range, and the grey of a step with no power target.
struct ZoneLegend: View {
    let ftp: Int
    @ScaledMetric(relativeTo: .caption) private var dot: CGFloat = 10

    var body: some View {
        let up = zoneUpper(ftp: UInt16(clamping: ftp), maxHr: 190).dropFirst(4)
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 110), alignment: .leading)], alignment: .leading, spacing: 4) {
            ForEach(1...7, id: \.self) { z in
                HStack(spacing: 4) {
                    Circle().fill(Tok.zone(z)).frame(width: dot, height: dot)
                    Text("Z\(z) \(range(z, Array(up)))")
                }
            }
            HStack(spacing: 4) {
                Circle().fill(Color.secondary.opacity(0.5)).frame(width: dot, height: dot)
                Text("No power target")
            }
        }
        .font(.caption2).foregroundStyle(.secondary)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Zone colours, Z1 to Z7, and grey for a step with no power target")
    }

    /// "0 to 111", "152 to 205", "over 300".
    private func range(_ z: Int, _ upper: [UInt16]) -> String {
        let lo = z == 1 ? 0 : Int(upper[safe: z - 2] ?? 0) + 1
        guard let hi = upper[safe: z - 1] else { return "over \(lo - 1) W" }
        return "\(lo) to \(hi) W"
    }
}

/// A step's length on the chart, s: a step that ends on a distance or the Lap key is drawn five minutes long.
private func shown(_ s: StepView) -> Double { s.seconds > 0 ? Double(s.seconds) : 300 }

/// One bar of the chart: minutes from the start, watts, and its colour.
private struct Bar: Identifiable {
    let id: Int
    let from: Double
    let to: Double
    let watts: Double
    let color: Color
}

struct StepChart: View {
    let steps: [StepView]
    let ftp: Int
    @ScaledMetric var height: CGFloat = 180

    /// Power steps at their target's middle in watts; the others a quarter of the FTP, grey.
    private var bars: [Bar] {
        var at = 0.0
        var out: [Bar] = []
        for (i, s) in steps.enumerated() {
            let power = s.kind == .power
            let watts: Double = power ? Double(Int(s.wattsLow) + Int(s.wattsHigh)) / 2 : Double(ftp) / 4
            let color: Color = power ? zoneColor(s.powerZone) : Color.secondary.opacity(0.5)
            out.append(Bar(id: i, from: at / 60, to: (at + shown(s)) / 60, watts: watts, color: color))
            at += shown(s)
        }
        return out
    }

    var body: some View {
        Chart {
            ForEach(bars) { b in
                RectangleMark(
                    xStart: .value("Start", b.from),
                    xEnd: .value("End", b.to),
                    yStart: .value("Zero", 0.0),
                    yEnd: .value("Watts", b.watts)
                )
                .foregroundStyle(b.color)
            }
            RuleMark(y: .value("FTP", ftp))
                .lineStyle(StrokeStyle(lineWidth: 1, dash: [4, 3]))
                .foregroundStyle(.secondary)
        }
        .chartXAxisLabel("min")
        .chartYAxis { AxisMarks(position: .trailing) }
        .frame(height: height)
        .accessibilityLabel("Workout steps")
        .accessibilityValue("\(steps.count) steps, \(Fmt.mmss(UInt32(clamping: Int(steps.reduce(0.0) { $0 + shown($1) })))) in all")
    }
}

struct StepRow: View {
    let index: Int
    let step: StepView
    @ScaledMetric(relativeTo: .caption) private var dot: CGFloat = 10

    var body: some View {
        HStack {
            Text("\(index)").numeral(.subheadline).foregroundStyle(.secondary).frame(minWidth: 28, alignment: .leading)
            VStack(alignment: .leading, spacing: 2) {
                Text(length)
                Text(target).font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if step.kind == .power { Circle().fill(zoneColor(step.powerZone)).frame(width: dot, height: dot) }
            Text(intensity).font(.caption).foregroundStyle(.secondary)
        }
    }

    private var length: String {
        if step.seconds > 0 { return Fmt.mmss(step.seconds) }
        if step.distanceM > 0 { return Fmt.distance(Double(step.distanceM)) }
        return "Until Lap"
    }

    private var target: String {
        let range = step.low == step.high ? "\(step.low)" : "\(step.low) to \(step.high)"
        switch step.kind {
        case .open: return "No target"
        case .power:
            let w = step.wattsLow == step.wattsHigh ? "\(step.wattsLow) W" : "\(step.wattsLow) to \(step.wattsHigh) W"
            if step.zone > 0 { return "Zone \(step.zone), \(w)" }
            return step.percent ? "\(range) % FTP, \(w)" : w
        case .heartRate: return step.percent ? "\(range) % max heart rate" : "\(range) bpm"
        case .cadence: return "\(range) rpm"
        }
    }

    private var intensity: String {
        ["", "Rest", "Warm-up", "Cool-down", "Recovery", "Interval", ""][min(Int(step.intensity), 6)]
    }
}
