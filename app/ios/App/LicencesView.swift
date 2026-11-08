// Settings > Profile > Licences: the app is GPLv3 like the firmware; the numeral face travels with its SIL OFL text.

import SwiftUI

struct LicencesView: View {
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                Text("This app is free software under the GNU General Public License, version 3 or later, like the firmware of the bike computer.")
                Text("Numerals are set in Barlow Semi Condensed Bold, the face of the unit's smooth font. Copyright 2017 The Barlow Project Authors (https://github.com/jpt/barlow). SIL Open Font License 1.1:")
                Text(Self.ofl).font(.caption.monospaced()).foregroundStyle(.secondary)
            }
            .padding()
        }
        .navigationTitle("Licences")
        .navigationBarTitleDisplayMode(.inline)
    }

    /// OFL.txt from the bundle (tools/fonts/OFL.txt, copied as it is).
    private static let ofl: String =
        Bundle.main.url(forResource: "OFL", withExtension: "txt").flatMap { try? String(contentsOf: $0, encoding: .utf8) }
        ?? "See https://openfontlicense.org/"
}
