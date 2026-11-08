// What the Intervals.icu loop shows: a status section on the main screen, the profile sheet's switches, the link on a
// ride's page.

import SwiftUI

/// Last upload, last calendar read, errors in plain words, and the planned workouts with where each stands.
struct IntervalsSection: View {
    @ObservedObject var loop: IntervalsLoop
    @EnvironmentObject private var intervals: IntervalsSync

    var body: some View {
        if intervals.hasKey {
            Section("Intervals.icu") {
                row("Rides", loop.uploading ? "Uploading…" : upload)
                if let e = loop.uploadError { Notice(text: e).font(.footnote) }
                row("Calendar", calendar)
                if let e = loop.calendarError { Notice(text: e).font(.footnote) }
                if let next = nextWorkout(loop.planned, day: { Int($0.day) ?? 0 }) { PlannedRows(list: [next]) }
                if !loop.planned.isEmpty {
                    NavigationLink { AllPlanned(loop: loop) } label: {
                        Text("All planned (\(loop.planned.count))").foregroundStyle(Tok.accent)
                    }
                }
            }
        }
    }

    private var upload: String {
        if !loop.uploadOn { return "upload is off" }
        return loop.lastUpload.map { "last upload \(Fmt.when($0))" } ?? "nothing uploaded yet"
    }

    private var calendar: String {
        if !loop.calendarOn { return "not sent to the unit" }
        guard let t = loop.lastCalendar else { return loop.calendarBusy ? "reading…" : "not read yet" }
        return "read \(Fmt.when(t))" + (loop.calendarRead ? ", \(loop.planned.count) planned" : "")
    }

    private func row(_ k: String, _ v: String) -> some View {
        HStack { Text(k).foregroundStyle(.secondary); Spacer(); Text(v) }.font(.subheadline)
    }
}

/// Planned workouts, one row each: the name, its day, and where it stands on the unit.
struct PlannedRows: View {
    let list: [PlannedItem]

    var body: some View {
        ForEach(list) { p in
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text(pretty(p.name))
                    Spacer()
                    Text(Fmt.dayKey(p.day)).foregroundStyle(.secondary)
                }
                switch p.state {
                case .onUnit: Text("On the unit").font(.caption).foregroundStyle(Tok.ok)
                case .waiting: Text("Waiting for the unit").font(.caption).foregroundStyle(.secondary)
                case .refused(let why): Notice(text: "Not sent: \(why)").font(.caption)
                }
            }
        }
    }
}

/// Every planned workout, pushed from the Intervals.icu section's "All planned" row.
struct AllPlanned: View {
    @ObservedObject var loop: IntervalsLoop

    var body: some View {
        List { PlannedRows(list: loop.planned) }
            .navigationTitle("Planned")
            .navigationBarTitleDisplayMode(.inline)
    }
}

/// The profile sheet's switches and the button for rides from before the upload was turned on.
struct IntervalsOptions: View {
    @EnvironmentObject private var loop: IntervalsLoop
    @EnvironmentObject private var rides: RideBrowser

    var body: some View {
        Section {
            Toggle("Upload rides to Intervals.icu", isOn: Binding(get: { loop.uploadOn }, set: { loop.setUpload($0) }))
            let older = loop.olderCount(rides)
            if older > 0 {
                Button("Upload older rides (\(older))") { loop.uploadPass(older: true) }.disabled(loop.uploading)
            }
            Toggle("Send planned workouts to the unit", isOn: Binding(get: { loop.calendarOn }, set: { loop.setCalendar($0) }))
        } footer: {
            Text("Only rides from after the upload was turned on go up by themselves. Planned workouts of the next 7 days are sent when the unit is connected and idle.")
        }
    }
}

/// "Open on Intervals.icu" on the page of a ride that was uploaded.
struct IntervalsRideLink: View {
    let url: URL
    @EnvironmentObject private var loop: IntervalsLoop

    var body: some View {
        if let id = loop.uploaded[url.deletingPathExtension().lastPathComponent],
            // the activity page of the web app (inferred: the spec has no page URLs); without an id, the list
            let page = URL(string: id.isEmpty ? "https://intervals.icu/activities" : "https://intervals.icu/activities/\(id)")
        {
            Link(destination: page) { Label("Open on Intervals.icu", systemImage: "arrow.up.right.square") }
        }
    }
}
