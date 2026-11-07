import SwiftUI

@main
struct BrytonApp: App {
    var body: some Scene {
        WindowGroup { ContentView() }
    }
}

struct ContentView: View {
    @StateObject private var link = UnitLink()

    var body: some View {
        VStack(spacing: 16) {
            Text("Bike computer").font(.largeTitle).bold()
            Text(message).foregroundStyle(.secondary).multilineTextAlignment(.center)
            if link.state == .connected {
                VStack(alignment: .leading, spacing: 8) {
                    row("Name", link.name)
                    row("Maker", link.status.maker ?? "unknown")
                    row("Battery", link.status.batteryPercent.map { "\($0) %" } ?? "unknown")
                }
            }
            Button("Refresh") { link.refresh() }.buttonStyle(.borderedProminent)
        }
        .padding()
    }

    private func row(_ k: String, _ v: String) -> some View {
        HStack { Text(k).foregroundStyle(.secondary); Spacer(); Text(v) }.frame(maxWidth: 280)
    }

    private var message: String {
        switch link.state {
        case .bluetoothOff: "Bluetooth is off. Turn it on in Settings."
        case .unauthorized: "Bluetooth is not allowed for this app. Allow it in Settings."
        case .unsupported: "This device has no Bluetooth."
        case .searching: "Searching for the unit..."
        case .notFound: "The unit was not found. Wake it, and pair it in Settings if needed."
        case .connected: "Connected"
        case .failed(let why): why
        }
    }
}
