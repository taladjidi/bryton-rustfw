// What the Intervals.icu loop shows: a status section on the main screen, the profile sheet's switches, the link on a
// ride's page.

import SwiftUI

private let stamp: DateFormatter = {
    let f = DateFormatter()
    f.dateFormat = "d MMM HH:mm"
    return f
}()

/// "9 Nov" from `YYYYMMDD`.
private func dayLabel(_ day: String) -> String {
    let n = day.compactMap { $0.wholeNumberValue }
    guard n.count == 8,
        let d = Calendar.current.date(from: DateComponents(year: n[0] * 1000 + n[1] * 100 + n[2] * 10 + n[3], month: n[4] * 10 + n[5], day: n[6] * 10 + n[7]))
    else { return day }
    return d.formatted(.dateTime.weekday(.abbreviated).day().month(.abbreviated))
}

/// Last upload, last calendar read, errors in plain words, and the planned workouts with where each stands.
struct IntervalsSection: View {
    @ObservedObject var loop: IntervalsLoop
    @EnvironmentObject private var intervals: IntervalsSync

    var body: some View {
        if intervals.hasKey {
            Section("Intervals.icu") {
                row("Rides", loop.uploading ? "Uploading..." : upload)
                if let e = loop.uploadError { Text(e).font(.footnote).foregroundStyle(.orange) }
                row("Calendar", calendar)
                if let e = loop.calendarError { Text(e).font(.footnote).foregroundStyle(.orange) }
                ForEach(loop.planned) { p in
                    VStack(alignment: .leading, spacing: 2) {
                        HStack {
                            Text(pretty(p.name))
                            Spacer()
                            Text(dayLabel(p.day)).foregroundStyle(.secondary)
                        }
                        switch p.state {
                        case .onUnit: Text("On the unit").font(.caption).foregroundStyle(.green)
                        case .waiting: Text("Waiting for the unit").font(.caption).foregroundStyle(.secondary)
                        case .refused(let why): Text("Not sent: \(why)").font(.caption).foregroundStyle(.orange)
                        }
                    }
                }
            }
        }
    }

    private var upload: String {
        if !loop.uploadOn { return "upload is off" }
        return loop.lastUpload.map { "last upload \(stamp.string(from: $0))" } ?? "nothing uploaded yet"
    }

    private var calendar: String {
        if !loop.calendarOn { return "not sent to the unit" }
        guard let t = loop.lastCalendar else { return loop.calendarBusy ? "reading..." : "not read yet" }
        let n = loop.planned.count
        return "read \(stamp.string(from: t)), \(n) planned"
    }

    private func row(_ k: String, _ v: String) -> some View {
        HStack { Text(k).foregroundStyle(.secondary); Spacer(); Text(v) }.font(.subheadline)
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
