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

struct WorkoutSection: View {
    @ObservedObject var rides: RideBrowser
    @AppStorage("ftp") private var ftp = 200
    @AppStorage("maxHR") private var maxHR = 190
    @State private var deleting: WorkoutFile?
    @EnvironmentObject private var loop: IntervalsLoop

    var body: some View {
        Section("Workouts") {
            ForEach(rides.workouts) { f in
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
                    .swipeActions {
                        // only when nothing else uses the link: the same guard as a download
                        Button("Delete", role: .destructive) { deleting = f }.disabled(rides.phase != .idle)
                    }
                }
            }
            if rides.workouts.isEmpty {
                Text(rides.phase == .workouts ? "Reading workouts…" : "No workouts planned")
                    .foregroundStyle(.secondary)
            }
        }
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

struct WorkoutDetail: View {
    let file: WorkoutFile
    var onEdit: (WorkoutDraft) -> Void = { _ in }
    @AppStorage("ftp") private var ftp = 200
    @AppStorage("maxHR") private var maxHR = 190
    @State private var profile = false

    var body: some View {
        let w = decodeWorkout(bytes: file.bytes, ftp: UInt16(clamping: ftp), maxHr: UInt16(clamping: maxHR))
        List {
            if let w {
                Section {
                    StepChart(steps: w.steps, ftp: ftp)
                    HStack {
                        Text(Fmt.duration(UInt64(w.totalS)))
                        Spacer()
                        if w.tss > 0 { Text(Fmt.ifTss(w.intensityX100, w.tss)) }
                    }
                    if w.cut { Notice(text: "Longer than the unit keeps: the end is cut.").font(.caption) }
                }
                Section {
                    Button { profile = true } label: {
                        HStack { Text("FTP \(ftp) W, max heart rate \(maxHR)"); Spacer(); Image(systemName: "person.crop.circle") }
                    }
                } footer: {
                    Text("Targets follow your profile. IF and TSS are estimates from power steps with a time; they are left out when other steps are mixed in.")
                }
                Section {
                    Button { onEdit(draftFromView(view: w)) } label: { Label("Edit a copy", systemImage: "square.and.pencil") }
                } footer: {
                    Text("The copy stays on the phone; the workout on the unit is not changed.")
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
        .sheet(isPresented: $profile) { ProfileSheet() }
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
    @ScaledMetric private var height: CGFloat = 180

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
        if step.distanceM > 0 { return Fmt.km(Double(step.distanceM)) }
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
