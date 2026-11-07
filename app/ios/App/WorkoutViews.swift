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

/// The power zones' colours (Coggan zones 1 to 7, as the unit's ride pages draw them).
private let zoneColors: [Color] = [.gray, .blue, .green, .yellow, .orange, .red, .purple]

func zoneColor(_ z: UInt8) -> Color { z >= 1 && z <= 7 ? zoneColors[Int(z) - 1] : .secondary }

/// The unit keeps names in capitals: "SWEET SPOT 3X10" as "Sweet spot 3x10".
func pretty(_ name: String) -> String { name.prefix(1) + name.dropFirst().lowercased() }

/// "1 h 05" or "45 min".
func duration(_ s: UInt32) -> String {
    let m = (s + 30) / 60
    return m >= 60 ? String(format: "%d h %02d", m / 60, m % 60) : "\(m) min"
}

/// "9 Nov" from `YYYYMMDDnn`.
private func day(_ key: String) -> String {
    let n = key.compactMap { $0.wholeNumberValue }
    guard n.count >= 8 else { return key }
    let c = DateComponents(year: n[0] * 1000 + n[1] * 100 + n[2] * 10 + n[3], month: n[4] * 10 + n[5], day: n[6] * 10 + n[7])
    guard let d = Calendar.current.date(from: c) else { return key }
    return d.formatted(.dateTime.day().month(.abbreviated).weekday(.abbreviated))
}

struct WorkoutSection: View {
    @ObservedObject var rides: RideBrowser
    @AppStorage("ftp") private var ftp = 200
    @AppStorage("maxHR") private var maxHR = 190

    var body: some View {
        Section("Workouts") {
            ForEach(rides.workouts) { f in
                if let w = decodeWorkout(bytes: f.bytes, ftp: UInt16(ftp), maxHr: UInt16(maxHR)) {
                    NavigationLink(value: WorkoutRoute(key: f.key)) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(pretty(w.name))
                            HStack {
                                Text(day(f.key))
                                Spacer()
                                Text(duration(w.totalS))
                                if w.tss > 0 { Text("TSS \(w.tss)") }
                            }
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        }
                    }
                }
            }
            if rides.workouts.isEmpty {
                Text(rides.phase == .workouts ? "Reading workouts..." : "No workouts planned")
                    .foregroundStyle(.secondary)
            }
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
        let w = decodeWorkout(bytes: file.bytes, ftp: UInt16(ftp), maxHr: UInt16(maxHR))
        List {
            if let w {
                Section {
                    StepChart(steps: w.steps, ftp: ftp).frame(height: 180)
                    HStack {
                        Text(duration(w.totalS))
                        Spacer()
                        if w.tss > 0 { Text("IF \(Double(w.intensityX100) / 100, specifier: "%.2f")   TSS \(w.tss)") }
                    }
                    if w.cut { Text("Longer than the unit keeps: the end is cut.").font(.caption).foregroundStyle(.orange) }
                }
                Section {
                    Button { profile = true } label: {
                        HStack { Text("FTP \(ftp) W, max heart rate \(maxHR)"); Spacer(); Image(systemName: "gearshape") }
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
        .chartYAxis { AxisMarks(position: .leading) }
    }
}

struct StepRow: View {
    let index: Int
    let step: StepView

    var body: some View {
        HStack {
            Text("\(index)").foregroundStyle(.secondary).frame(width: 28, alignment: .leading)
            VStack(alignment: .leading, spacing: 2) {
                Text(length)
                Text(target).font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if step.kind == .power { Circle().fill(zoneColor(step.powerZone)).frame(width: 10, height: 10) }
            Text(intensity).font(.caption).foregroundStyle(.secondary)
        }
    }

    private var length: String {
        if step.seconds > 0 { return String(format: "%d:%02d", step.seconds / 60, step.seconds % 60) }
        if step.distanceM > 0 { return String(format: "%.1f km", Double(step.distanceM) / 1000) }
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
