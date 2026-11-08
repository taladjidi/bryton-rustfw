// The app's look, shared with the unit: colour tokens (Assets.xcassets, one Color Set per token, light and dark),
// the numeral face and the card shapes. The values are in app/core/src/theme.rs (TOKENS), which a test keeps equal to
// the colour sets and to the firmware's constants (src/ui/theme.rs LIGHT / OUTDOOR, src/training/ride.rs ZONE_COLORS,
// src/ui/mod.rs ACCENT). Contrast: 4.5:1 for text tokens, 3:1 for the zones (a fill, never small text), against the
// iOS card and background surfaces, light and dark, checked by the same test.

import CoreText
import SwiftUI
import UIKit

enum Tok {
    /// Zone 1 to 7 (the unit's 7-colour palette, Intervals.icu's); outside 1...7 clamps.
    static func zone(_ z: Int) -> Color { Color("zone\(min(max(z, 1), 7))") }
    /// Heart rate zone 1 to 5: the first five of the power zones, as the unit's `hr_color`.
    static func hrZone(_ z: Int) -> Color { zone(min(z, 5)) }
    /// A climb's colour by grade (% x 10): zone 2 to 5 by the bands of src/ui/mod.rs `grade_color`.
    static func grade(_ x10: Int) -> Color { zone(gradeZone(x10)) }
    static func gradeZone(_ x10: Int) -> Int { x10 < 60 ? 2 : x10 < 90 ? 3 : x10 < 120 ? 4 : 5 }
    /// Form (TSB) by src/training/load.rs `form_color`: fresh above 5, neutral to -10, tired to -30, overload below.
    static func form(_ tsb: Double) -> Color { tsb > 5 ? ok : tsb >= -10 ? .secondary : tsb >= -30 ? warn : alarm }
    static let accent = Color("AccentColor")
    static let ok = Color("ok")
    static let warn = Color("warn")
    static let alarm = Color("alarm")
    static let radiusCard: CGFloat = 12
    static let radiusTile: CGFloat = 8

    /// The same colour for UIKit (the map).
    static func ui(_ name: String) -> UIColor { UIColor(named: name) ?? .label }
    static func uiGrade(_ x10: Int) -> UIColor { uiZone(gradeZone(x10)) }
    static func uiZone(_ z: Int) -> UIColor { ui("zone\(min(max(z, 1), 7))") }
}

/// Series colours of the ride charts: none is the alarm red.
extension Tok {
    static let speed = zone(1)
    static let heartRate = zone(5)
    static let power = accent
    static let cadence = zone(2)
}

/// A notice: an icon and text in a token, so colour is never the only cue.
struct Notice: View {
    enum Level { case warn, alarm }
    let text: String
    var level: Level = .warn

    var body: some View {
        Label(text, systemImage: level == .warn ? "exclamationmark.triangle.fill" : "xmark.octagon.fill")
            .foregroundStyle(level == .warn ? Tok.warn : Tok.alarm)
    }
}

/// A tile or card background.
extension View {
    func tile() -> some View {
        padding(8).background(Color(.secondarySystemBackground), in: RoundedRectangle(cornerRadius: Tok.radiusTile))
    }
}

/// Barlow Semi Condensed Bold, the unit's SMOOTH face (tools/fonts, SIL OFL 1.1), for big numbers only; text stays SF.
/// A modifier, not a `Font`, because the tabular figures need a font descriptor with a feature, and a descriptor
/// font does not follow Dynamic Type by itself: `@ScaledMetric` carries the scaling. If the font is missing the
/// system bold face is used with the same feature.
private struct Numeral: ViewModifier {
    @ScaledMetric private var size: CGFloat

    init(_ style: Font.TextStyle) {
        let base: CGFloat =
            switch style {
            case .largeTitle: 36
            case .title: 30
            case .title2: 26
            case .title3: 23
            case .headline, .body: 20
            case .subheadline: 17
            default: 14
            }
        _size = ScaledMetric(wrappedValue: base, relativeTo: style)
    }

    func body(content: Content) -> some View {
        content.font(Font(Self.face(size)))
    }

    /// The face at `size` with tabular figures ('tnum'; the font's default digits differ in width, so live values jitter).
    static func face(_ size: CGFloat) -> UIFont {
        let base = UIFont(name: "BarlowSemiCondensed-Bold", size: size) ?? .systemFont(ofSize: size, weight: .bold)
        let features: [[UIFontDescriptor.FeatureKey: Int]] = [[
            .type: kNumberSpacingType, .selector: kMonospacedNumbersSelector,
        ]]
        return UIFont(descriptor: base.fontDescriptor.addingAttributes([.featureSettings: features]), size: size)
    }
}

extension View {
    /// Big numbers: the unit's face, tabular, scaled with Dynamic Type relative to `style`.
    func numeral(_ style: Font.TextStyle) -> some View { modifier(Numeral(style)) }
}
