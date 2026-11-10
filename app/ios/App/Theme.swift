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
    /// A slate of its own (zone 7 is pinned to the firmware's constant by a test): 3:1 or more on the card, light and dark.
    static let speed = Color(UIColor { $0.userInterfaceStyle == .dark
        ? UIColor(red: 0.58, green: 0.54, blue: 0.68, alpha: 1) : UIColor(red: 0.35, green: 0.40, blue: 0.52, alpha: 1) })
    static let heartRate = zone(5)
    static let power = accent
    static let cadence = zone(2)
    /// Fatigue (ATL) beside fitness (CTL, the accent): a purple, not zone 5's maroon, which sits next to the orange.
    static let fatigue = zone(6)
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
    /// A chart or panel as a card: the grouped list's row colour on the page's grouped background, flat, no shadow.
    func card() -> some View {
        padding().background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: Tok.radiusCard))
    }

    func tile() -> some View {
        padding(8).background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: Tok.radiusTile))
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

/// A home section's title: one style for every section (the default header grey is about 3.5:1 on the grouped background).
struct SectionTitle: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View { Text(text).font(.headline).foregroundColor(Color(.label)).textCase(nil) }
}

/// A list row that acts (an "All ..." row, New, Import): the icon column is the same on every one, so the text lines up.
struct ActionRow: View {
    let title: String
    let icon: String

    var body: some View { Label(title, systemImage: icon).foregroundStyle(Tok.accent) }
}

/// A card's title with its one control (the segmented picker) at the trailing edge; stacked when the type is large.
struct CardHeader<Control: View>: View {
    let title: String
    @ViewBuilder let control: Control

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack {
                Text(title).font(.subheadline.bold()).lineLimit(1)
                Spacer(minLength: 8)
                control.pickerStyle(.segmented).frame(maxWidth: 190)
            }
            VStack(alignment: .leading, spacing: 8) {
                Text(title).font(.subheadline.bold())
                control.pickerStyle(.segmented)
            }
        }
    }
}

/// A portrait bike computer drawn on a 12 x 12 pixel grid: the body with its corners cut, a one-cell screen frame with a
/// small bar chart inside, a side button and a chin. One colour (the foreground style), for the unit card and the empty
/// states; rides keep the `bicycle` symbol.
struct UnitGlyph: View {
    @ScaledMetric(relativeTo: .title2) private var side: CGFloat = 30

    var body: some View {
        Canvas { ctx, size in
            let u = size.width / 12
            var path = Path()
            func cell(_ x: Int, _ y: Int) { path.addRect(CGRect(x: CGFloat(x) * u, y: CGFloat(y) * u, width: u, height: u)) }
            for y in 0...11 {
                for x in 2...9 {
                    let corner = (x == 2 || x == 9) && (y == 0 || y == 11)
                    if !corner && (x == 2 || x == 9 || y == 0 || y == 11) { cell(x, y) }
                }
            }
            for x in 3...8 { for y in [2, 7] { cell(x, y) } }  // the screen frame
            for y in 3...6 { for x in [3, 8] { cell(x, y) } }
            for (x, h) in [(4, 1), (5, 3), (6, 2), (7, 4)] { for k in 0..<h { cell(x, 6 - k) } }  // bars
            cell(10, 3); cell(10, 4)  // side button
            cell(5, 9); cell(6, 9)  // chin
            ctx.fill(path, with: .foreground)
        }
        .frame(width: side, height: side)
        .accessibilityHidden(true)
    }
}

/// Axis marks at round values: the multiples of the smallest 1, 2 or 5 x 10^n step that leave at most 5 marks inside
/// `lo...hi` (at least 2: the step halves otherwise). Swift Charts' own `desiredCount` is only a hint and gave six
/// labels on a speed axis; marks computed from the domain's ends gave 265 / 205 / 145 / 85, which no chart app shows.
/// 2.5 steps are left out: the labels are whole numbers.
func niceMarks(_ lo: Double, _ hi: Double) -> [Double] {
    guard hi > lo, abs(lo) < 1e9, abs(hi) < 1e9 else { return [lo] }
    func marks(_ step: Double) -> [Double] {
        let first = (lo / step).rounded(.up)
        let last = (hi / step).rounded(.down)
        return last >= first ? (Int(first)...Int(last)).map { Double($0) * step } : []
    }
    var pow10 = pow(10.0, floor(log10(hi - lo)) - 1)
    var found: [Double] = []
    outer: while pow10 <= hi - lo {
        for m in [1.0, 2.0, 5.0] {
            found = marks(m * pow10)
            if found.count <= 5 { break outer }
        }
        pow10 *= 10
    }
    if found.count < 2 { found = marks(max(pow10 / 2, .ulpOfOne)) }
    return found
}
