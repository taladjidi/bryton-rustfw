// The workout editor: workouts made on the phone, kept in the app's Documents as FIT files the unit reads (the Rust
// core's encodeWorkout), shown beside the unit's own. The core checks every draft against the unit's limits and
// previews it with the firmware's parser, so the chart here is what the unit will show. "Send to the unit" sends the
// encoded file for a day (the file service's WRITE commands).

import SwiftUI
import UniformTypeIdentifiers

/// What the editor opens on: a draft, and the saved file it replaces (nil for a new one).
struct EditorTarget: Identifiable {
    let id = UUID()
    var draft: WorkoutDraft
    var file: String?
}

/// A workout saved on the phone.
struct LocalWorkout: Identifiable {
    let file: String
    let bytes: Data
    var id: String { file }
}

/// The workouts in Documents/Workouts.
final class WorkoutStore: ObservableObject {
    @Published var items: [LocalWorkout] = []

    private static var dir: URL {
        let d = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("Workouts")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    init() { reload() }

    func reload() {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: Self.dir.path)) ?? []
        // newest saved first: the phone section shows the first one
        let dated = names.filter { $0.hasSuffix(".fit") }.compactMap { n -> (LocalWorkout, Date)? in
            let url = Self.dir.appendingPathComponent(n)
            guard let bytes = try? Data(contentsOf: url) else { return nil }
            let saved = (try? url.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate ?? .distantPast
            return (LocalWorkout(file: n, bytes: bytes), saved)
        }
        items = dated.sorted { $0.1 != $1.1 ? $0.1 > $1.1 : $0.0.file < $1.0.file }.map(\.0)
    }

    /// Encodes and writes the draft; throws the core's refusal. A renamed workout leaves its old file.
    @discardableResult
    func save(_ d: WorkoutDraft, replacing old: String?) throws -> String {
        let bytes = try encodeWorkout(draft: d)
        let stem = String(d.name.map { $0.isLetter || $0.isNumber ? $0 : "_" })
        var file = "\(stem.isEmpty ? "Workout" : stem).fit"
        var n = 2
        while file != old && FileManager.default.fileExists(atPath: Self.dir.appendingPathComponent(file).path) {
            file = "\(stem) \(n).fit"
            n += 1
        }
        try bytes.write(to: Self.dir.appendingPathComponent(file), options: .atomic)
        reload()
        if let old, old != file {
            do { try FileManager.default.removeItem(at: Self.dir.appendingPathComponent(old)) } catch {
                throw SaveError.oldFileKept(file: old, why: error.localizedDescription)
            }
            reload()
        }
        return file
    }

    func delete(_ file: String) {
        try? FileManager.default.removeItem(at: Self.dir.appendingPathComponent(file))
        reload()
    }
}

/// A save that wrote the new file but could not remove the renamed workout's old one.
enum SaveError: Error {
    case oldFileKept(file: String, why: String)
}

/// The core's refusal as text.
private func message(_ e: Error) -> String {
    if case DraftError.Invalid(let m) = e { return m }
    if case SaveError.oldFileKept(let f, let why) = e { return "Saved, but the old file \(f) could not be removed: \(why)" }
    return "\(e)"
}

/// The "On the phone" section: the most recently saved workout, the full list, a new one, an import.
struct PhoneWorkoutSection: View {
    @ObservedObject var store: WorkoutStore
    let onEdit: (EditorTarget) -> Void
    @State private var importing = false
    @State private var problem: String?
    #if DEBUG
    private let demoImport = CommandLine.arguments.contains("-demo-import")
    #endif

    var body: some View {
        Section {
            PhoneWorkoutRows(store: store, list: Array(store.items.prefix(1)), onEdit: onEdit)
            if !store.items.isEmpty {
                NavigationLink { AllPhoneWorkouts(store: store, onEdit: onEdit) } label: {
                    ActionRow(title: "All workouts on the phone (\(store.items.count))", icon: "list.bullet")
                }
            }
            Button { onEdit(EditorTarget(draft: .blank, file: nil)) } label: { ActionRow(title: "New workout", icon: "plus") }
            Button { importing = true } label: { ActionRow(title: "Import from Files", icon: "square.and.arrow.down") }
                .fileImporter(isPresented: $importing, allowedContentTypes: [.data]) { r in
                    switch r {
                    case .success(let url):
                        // read and parsed off the main thread
                        Task {
                            let result = await Task.detached { () -> Result<WorkoutDraft, Error> in
                                let ok = url.startAccessingSecurityScopedResource()
                                defer { if ok { url.stopAccessingSecurityScopedResource() } }
                                return Result {
                                    // the core refuses over 1 MB too; this keeps a huge file out of memory
                                    if let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize, size > 1_000_000 {
                                        throw DraftError.Invalid(message: "File too big")
                                    }
                                    return try importWorkout(bytes: Data(contentsOf: url), fallbackName: url.deletingPathExtension().lastPathComponent)
                                }
                            }.value
                            switch result {
                            case .success(let d): onEdit(EditorTarget(draft: d, file: nil))
                            case .failure(let e): problem = message(e)
                            }
                        }
                    case .failure(let e): problem = e.localizedDescription
                    }
                }
            if let problem { Notice(text: problem).font(.caption) }
        } header: {
            #if DEBUG
            SectionTitle("On the phone").onAppear {  // Simulator screenshot: -demo-import opens the Files picker
                if demoImport { DispatchQueue.main.asyncAfter(deadline: .now() + 4) { importing = true } }
            }
            #else
            SectionTitle("On the phone")
            #endif
        } footer: {
            Text("Zwift (.zwo), .erg and .mrc files can be imported. Send a workout to the unit from its editor.")
        }
    }
}

/// The saved workouts as rows: a tap edits a copy, swipe and long press delete the saved file.
struct PhoneWorkoutRows: View {
    @ObservedObject var store: WorkoutStore
    let list: [LocalWorkout]
    let onEdit: (EditorTarget) -> Void
    @EnvironmentObject private var profile: Profile
    private var ftp: Int { profile.ftp }
    private var maxHR: Int { profile.maxHR }

    var body: some View {
        ForEach(list) { f in
            if let w = decodeWorkout(bytes: f.bytes, ftp: UInt16(clamping: ftp), maxHr: UInt16(clamping: maxHR)) {
                Button { onEdit(EditorTarget(draft: draftFromView(view: w).named(pretty(w.name)), file: f.file)) } label: {
                    HStack {
                        Text(pretty(w.name))
                        Spacer()
                        Text(Fmt.duration(UInt64(w.totalS)))
                        if w.tss > 0 { Text("TSS \(w.tss)") }
                    }
                    .font(.body)
                }
                .foregroundStyle(.primary)
                .swipeActions { Button("Delete", role: .destructive) { store.delete(f.file) } }
                .contextMenu { Button("Delete", systemImage: "trash", role: .destructive) { store.delete(f.file) } }
            } else {
                // a saved file the reader refuses stays listed, so it can be deleted
                HStack {
                    Text(f.file)
                    Spacer()
                    Notice(text: "Unreadable")
                }
                .swipeActions { Button("Delete", role: .destructive) { store.delete(f.file) } }
            }
        }
    }
}

/// Every workout saved on the phone, pushed from the "On the phone" section's row.
struct AllPhoneWorkouts: View {
    @ObservedObject var store: WorkoutStore
    let onEdit: (EditorTarget) -> Void

    var body: some View {
        List { PhoneWorkoutRows(store: store, list: store.items, onEdit: onEdit) }
            .navigationTitle("All workouts")
            .navigationBarTitleDisplayMode(.inline)
    }
}

extension WorkoutDraft {
    static var blank: WorkoutDraft { WorkoutDraft(name: "New workout", steps: [.fresh]) }
    func named(_ n: String) -> WorkoutDraft { var d = self; d.name = n; return d }
}

extension DraftStep {
    /// A five minute step at 60 to 70 % of FTP.
    static var fresh: DraftStep {
        DraftStep(repeatBack: 0, repeatTimes: 0, duration: .time, amount: 300, kind: .power, low: 60, high: 70, percent: true, intensity: 0)
    }
    static func repeating(_ back: UInt8) -> DraftStep {
        DraftStep(repeatBack: back, repeatTimes: 3, duration: .lap, amount: 0, kind: .open, low: 0, high: 0, percent: false, intensity: 0)
    }
}

private struct EditItem: Identifiable {
    let id = UUID()
    var step: DraftStep
}

private let intensityNames = ["Active", "Rest", "Warm-up", "Cool-down", "Recovery", "Interval", "Other"]

/// Name, chart and steps of one workout; Save writes it to the phone.
struct WorkoutEditor: View {
    let target: EditorTarget
    @ObservedObject var store: WorkoutStore
    @ObservedObject var rides: RideBrowser
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var profile: Profile
    private var ftp: Int { profile.ftp }
    private var maxHR: Int { profile.maxHR }
    @State private var name: String
    @State private var items: [EditItem]
    @State private var saveError: String?
    @State private var sending = false

    init(target: EditorTarget, store: WorkoutStore, rides: RideBrowser) {
        self.target = target
        self.store = store
        self.rides = rides
        _name = State(initialValue: target.draft.name)
        _items = State(initialValue: target.draft.steps.map { EditItem(step: $0) })
    }

    private var draft: WorkoutDraft { WorkoutDraft(name: name, steps: items.map(\.step)) }

    private var preview: Result<WorkoutView, Error> {
        Result { try draftPreview(draft: draft, ftp: UInt16(clamping: ftp), maxHr: UInt16(clamping: maxHR)) }
    }

    var body: some View {
        let shown = preview
        NavigationStack {
            List {
                Section {
                    TextField("Name", text: $name)
                    switch shown {
                    case .success(let w):
                        StepChart(steps: w.steps, ftp: ftp, height: 150)
                        ZoneLegend(ftp: ftp)
                        HStack {
                            Text(Fmt.duration(UInt64(w.totalS))).numeral(.subheadline)
                            Spacer()
                            if w.tss > 0 { Text(Fmt.ifTss(w.intensityX100, w.tss)).numeral(.subheadline) }
                        }
                    case .failure(let e):
                        Notice(text: message(e))
                    }
                } footer: {
                    Text("The chart is the unit's own reading of this file, for FTP \(ftp) W.")
                }
                Section {
                    ForEach($items) { $it in
                        NavigationLink { StepForm(step: $it.step) } label: {
                            StepSummary(step: it.step, index: items.firstIndex { $0.id == it.id } ?? 0)
                        }
                            .swipeActions(edge: .leading) {
                                Button("Duplicate") { duplicate(it.id) }.tint(Tok.zone(1))
                            }
                    }
                    .onMove { items.move(fromOffsets: $0, toOffset: $1) }
                    .onDelete { items.remove(atOffsets: $0) }
                    Button { items.append(EditItem(step: .fresh)) } label: { Label("Add a step", systemImage: "plus") }
                    Button { items.append(EditItem(step: .repeating(UInt8(min(items.count, 2))))) } label: {
                        Label("Add a repeat", systemImage: "repeat")
                    }
                    .disabled(items.isEmpty)
                } header: {
                    HStack { Text("Steps"); Spacer(); EditButton().textCase(nil) }
                }
                Section {
                    Button("Send to the unit") { sending = true }
                        .disabled(!rides.hasTransport || { if case .failure = shown { true } else { false } }())
                } footer: {
                    Text("Save keeps the workout on the phone; Send puts a copy on the unit for a day you pick.")
                }
                if let saveError { Section { Notice(text: saveError, level: .alarm) } }
            }
            .onChange(of: draft) { saveError = nil }
            .sheet(isPresented: $sending) { SendSheet(rides: rides, draft: draft) }
            #if DEBUG
            .onAppear {  // Simulator screenshots: the sheet cannot open while the editor's own sheet is still coming up
                if CommandLine.arguments.contains("-demo-send") {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { sending = true }
                }
            }
            #endif
            .navigationTitle(target.draft.name.isEmpty ? "New workout" : target.draft.name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") { save() }.disabled({ if case .failure = shown { true } else { false } }())
                }
            }
        }
    }

    private func duplicate(_ id: UUID) {
        if let i = items.firstIndex(where: { $0.id == id }) { items.insert(EditItem(step: items[i].step), at: i + 1) }
    }

    private func save() {
        saveError = nil
        do {
            try store.save(draft, replacing: target.file)
            dismiss()
        } catch { saveError = message(error) }
    }
}

/// A step in the list: its length, its target, its intensity.
private struct StepSummary: View {
    let step: DraftStep
    let index: Int

    var body: some View {
        if step.repeatBack > 0 {
            VStack(alignment: .leading, spacing: 2) {
                Label("Repeat the last \(step.repeatBack) \(step.repeatBack == 1 ? "step" : "steps"), \(step.repeatTimes) times", systemImage: "repeat")
                if Int(step.repeatBack) > index {
                    Notice(text: "Reaches past the start").font(.caption)
                }
            }
        } else {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(length(step)).numeral(.subheadline)
                    Text(targetText(step)).font(.subheadline).foregroundStyle(.secondary)
                }
                Spacer()
                Text(intensityNames[min(Int(step.intensity), 6)]).font(.subheadline).foregroundStyle(.secondary)
            }
        }
    }
}

private func length(_ s: DraftStep) -> String {
    switch s.duration {
    case .time: Fmt.mmss(UInt32(clamping: s.amount))
    case .distance: Fmt.distance(Double(s.amount))
    case .lap: "Until Lap"
    }
}

private func targetText(_ s: DraftStep) -> String {
    let range = s.low == s.high ? "\(s.low)" : "\(s.low) to \(s.high)"
    switch s.kind {
    case .open: return "No target"
    case .power: return s.percent ? "\(range) % FTP" : "\(range) W"
    case .heartRate: return s.percent ? "\(range) % max heart rate" : "\(range) bpm"
    case .cadence: return "\(range) rpm"
    }
}

private func int<T: FixedWidthInteger>(_ b: Binding<T>) -> Binding<Int> {
    Binding(get: { Int(b.wrappedValue) }, set: { b.wrappedValue = T(clamping: $0) })
}

/// One step's settings.
private struct StepForm: View {
    @Binding var step: DraftStep

    var body: some View {
        Form {
            if step.repeatBack > 0 {
                Section {
                    Stepper("The \(step.repeatBack) items above", value: int($step.repeatBack), in: 1...63)
                    Stepper("\(step.repeatTimes) times in all", value: int($step.repeatTimes), in: 2...1000)
                } footer: {
                    Text("A repeat can reach back over another repeat.")
                }
            } else {
                Section("Length") {
                    Picker("Ends", selection: $step.duration) {
                        Text("Time").tag(DurationKind.time)
                        Text("Distance").tag(DurationKind.distance)
                        Text("Lap key").tag(DurationKind.lap)
                    }
                    .pickerStyle(.segmented)
                    .onChange(of: step.duration) { _, d in
                        step.amount = d == .time ? 300 : d == .distance ? 5000 : 0
                    }
                    switch step.duration {
                    case .time:
                        Stepper("\(step.amount / 60) min", value: Binding(
                            get: { Int(step.amount / 60) }, set: { step.amount = UInt32($0) * 60 + step.amount % 60 }), in: 0...1440)
                        Stepper("\(step.amount % 60) s", value: Binding(
                            get: { Int(step.amount % 60) }, set: { step.amount = step.amount / 60 * 60 + UInt32($0) }), in: 0...55, step: 5)
                    case .distance:
                        Stepper(Fmt.distance(Double(step.amount)), value: int($step.amount), in: 100...1_000_000, step: 100)
                    case .lap:
                        Text("The step lasts until you press Lap.").foregroundStyle(.secondary)
                    }
                }
                Section("Target") {
                    Picker("On", selection: $step.kind) {
                        Text("None").tag(TargetKind.open)
                        Text("Power").tag(TargetKind.power)
                        Text("Heart rate").tag(TargetKind.heartRate)
                        Text("Cadence").tag(TargetKind.cadence)
                    }
                    .onChange(of: step.kind) { _, k in defaults(k, percent: k == .power || k == .heartRate) }
                    if step.kind == .power || step.kind == .heartRate {
                        Picker("In", selection: $step.percent) {
                            Text(step.kind == .power ? "% of FTP" : "% of max").tag(true)
                            Text(step.kind == .power ? "Watts" : "bpm").tag(false)
                        }
                        .pickerStyle(.segmented)
                        .onChange(of: step.percent) { _, p in defaults(step.kind, percent: p) }
                    }
                    if step.kind != .open {
                        Stepper("From \(step.low)", value: Binding(get: { Int(step.low) }, set: { v in
                            step.low = UInt16(clamping: v)
                            step.high = max(step.high, step.low)
                        }), in: 0...Int(limit), step: unitStep)
                        Stepper("To \(step.high)", value: Binding(get: { Int(step.high) }, set: { v in
                            step.high = UInt16(clamping: v)
                            step.low = min(step.low, step.high)
                        }), in: 1...Int(limit), step: unitStep)
                    }
                }
                Section("Intensity") {
                    Picker("Intensity", selection: $step.intensity) {
                        ForEach(0..<7, id: \.self) { Text(intensityNames[$0]).tag(UInt8($0)) }
                    }
                }
            }
        }
        .navigationTitle(step.repeatBack > 0 ? "Repeat" : "Step")
        .navigationBarTitleDisplayMode(.inline)
    }

    private var limit: UInt16 {
        switch (step.kind, step.percent) {
        case (.power, true): 400
        case (.power, false): 2000
        case (.heartRate, true): 100
        case (.heartRate, false): 250
        default: 250
        }
    }

    private var unitStep: Int { step.kind == .power ? (step.percent ? 1 : 5) : 1 }

    private func defaults(_ k: TargetKind, percent: Bool) {
        (step.low, step.high) = switch (k, percent) {
        case (.power, true): (60, 70)
        case (.power, false): (150, 200)
        case (.heartRate, true): (60, 70)
        case (.heartRate, false): (120, 140)
        case (.cadence, _): (85, 95)
        default: (0, 0)
        }
        step.percent = percent && (k == .power || k == .heartRate)
    }
}

/// Picks a day and sends the draft's encoded file to the unit, with progress and the unit's refusal as text.
private struct SendSheet: View {
    @ObservedObject var rides: RideBrowser
    let draft: WorkoutDraft
    @Environment(\.dismiss) private var dismiss
    @State private var day = Date()
    @State private var started = false
    @State private var encodeError: String?

    private var busy: Bool { rides.phase != .idle && rides.phase != .failed }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    DatePicker("Day", selection: $day, displayedComponents: .date)
                } footer: {
                    Text("The unit lists it on that day. Sending the same workout twice for a day keeps one.")
                }
                Section {
                    Button("Send \(pretty(draft.name))") { send() }.disabled(busy)
                    if rides.phase == .writing {
                        ProgressView(value: Double(rides.progress.received), total: Double(max(rides.progress.total, 1)))
                    }
                    if started, rides.sendDone { Label("Sent", systemImage: "checkmark.circle").foregroundStyle(Tok.ok) }
                    if let e = encodeError ?? (started ? rides.sendError : nil) { Notice(text: e, level: .alarm) }
                }
            }
            .navigationTitle("Send to the unit")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button(rides.sendDone ? "Done" : "Close") { dismiss() } } }
            .interactiveDismissDisabled(rides.phase == .writing)
            #if DEBUG
            .task {  // Simulator: `-demo-send-go` presses Send once the demo unit is ready
                guard CommandLine.arguments.contains("-demo-send-go") else { return }
                try? await Task.sleep(for: .seconds(2))
                send()
            }
            #endif
        }
    }

    private func send() {
        let c = Calendar.current.dateComponents([.year, .month, .day], from: day)
        let text = String(format: "%04d%02d%02d", c.year ?? 0, c.month ?? 0, c.day ?? 0)
        do {
            let bytes = try encodeWorkout(draft: draft)
            encodeError = nil
            started = true
            rides.send(day: text, bytes: bytes)
        } catch { encodeError = message(error) }
    }
}
