//! The app's colour tokens, one table, checked against the firmware. The Swift side reads them from the colour sets in
//! `app/ios/App/Assets.xcassets` (`Theme.swift`); the tests below fail when a colour set differs from [`TOKENS`], when
//! a token that must equal the firmware's constant does not, or when a contrast falls under its rule.
//!
//! Sources, all in this repository:
//! - light values: `Theme::LIGHT` (src/ui/theme.rs, the `ink` table: accent 4, alarm 6, ok 8, warn 9, zones 13 to 19),
//!   which holds 4.5:1 for text and 3:1 for lines on white;
//! - dark values: `ACCENT` (src/ui/mod.rs) and `ZONE_COLORS` zones 1 to 4 (src/training/ride.rs), `Theme::OUTDOOR` for
//!   zones 6 and 7 (DARK's own are 3.1 and 2.8 on black), zone 5 lifted from DARK's #BD315A (3.0 on black) by this app;
//!   ok, warn and alarm are iOS's system green, yellow and red (the firmware's neon GREEN / YELLOW / RED are too hard
//!   on a phone);
//! - contrast: WCAG 2.1 (<https://www.w3.org/TR/WCAG21/#dfn-contrast-ratio>), against the iOS surfaces.

use bryton_rustfw::ride::Zones;

/// One colour token: the asset's name, then `RRGGBB` for light and for dark appearance.
pub const TOKENS: [(&str, &str, &str); 11] = [
    ("AccentColor", "B04000", "FFA600"),
    ("zone1", "2F6259", "4A9284"),
    ("zone2", "2F6223", "4A9231"),
    ("zone3", "947D31", "DEBA4A"),
    ("zone4", "8F572A", "D68242"),
    ("zone5", "7C203B", "E0486F"),
    ("zone6", "442D7C", "885AF8"),
    ("zone7", "3C3748", "786E90"),
    ("ok", "007000", "30D158"),
    ("warn", "8A6400", "FFD60A"),
    ("alarm", "D00000", "FF453A"),
];

/// Where heart-rate zones 1 to 4 (bpm) then power zones 1 to 6 (W) end, the unit's standard zones for this FTP and
/// max HR: ten numbers, the firmware's `Zones::upper`. The viewer prints each zone's range from them.
#[uniffi::export]
pub fn zone_upper(ftp: u16, max_hr: u16) -> Vec<u16> {
    Zones::standard(ftp, max_hr).upper().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bryton_rustfw::{
        ride::ZONE_COLORS,
        theme::{Theme, hex},
        ui::ACCENT,
    };

    fn token(name: &str) -> (&'static str, &'static str) {
        TOKENS.iter().find(|t| t.0 == name).map(|t| (t.1, t.2)).unwrap_or(("", ""))
    }

    fn h(s: &str) -> u16 {
        hex(s.as_bytes()).unwrap_or(0xdead)
    }

    fn rgb(s: &str) -> [f64; 3] {
        let n = u32::from_str_radix(s, 16).unwrap_or(0);
        [(n >> 16) & 255, (n >> 8) & 255, n & 255].map(|c| {
            let c = f64::from(c) / 255.0;
            if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        })
    }

    fn lum(s: &str) -> f64 {
        let [r, g, b] = rgb(s);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    fn ratio(a: &str, b: &str) -> f64 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    // the iOS surfaces: card and grouped background, light then dark
    const LIGHT_SURFACES: [&str; 2] = ["FFFFFF", "F2F2F7"];
    const DARK_SURFACES: [&str; 2] = ["1C1C1E", "000000"];

    #[test]
    fn light_tokens_are_the_firmwares_light_theme() {
        let ink = Theme::LIGHT.ink;
        // accent, then zones 1 to 7, ok, warn, alarm: the ink indexes in Theme::LIGHT
        let rows = [("AccentColor", 4), ("ok", 8), ("warn", 9), ("alarm", 6)]
            .into_iter()
            .chain((1..=7).map(|z| (["zone1", "zone2", "zone3", "zone4", "zone5", "zone6", "zone7"][z - 1], 12 + z)));
        for (name, i) in rows {
            assert_eq!(h(token(name).0), ink[i], "{name} light");
        }
    }

    #[test]
    fn dark_tokens_follow_the_firmware() {
        assert_eq!(h(token("AccentColor").1), ACCENT);
        for (z, c) in ZONE_COLORS.iter().enumerate().take(4) {
            assert_eq!(h(token(&format!("zone{}", z + 1)).1), *c, "zone {} dark", z + 1);
        }
        // zones 6 and 7 are OUTDOOR's; zone 5 is lifted, so it differs from DARK's
        let outdoor = Theme::OUTDOOR.ink;
        assert_eq!((h(token("zone6").1), h(token("zone7").1)), (outdoor[18], outdoor[19]));
        assert_ne!(h(token("zone5").1), ZONE_COLORS[4]);
    }

    #[test]
    fn contrast_holds() {
        for (name, l, d) in TOKENS {
            // text colours 4.5:1, zones as graphics 3:1 (zone 3 light is 4.0: a fill, never text)
            let min = if name.starts_with("zone") { 3.0 } else { 4.5 };
            for s in LIGHT_SURFACES {
                assert!(ratio(l, s) >= min, "{name} light {l} on {s}: {:.2}", ratio(l, s));
            }
            for s in DARK_SURFACES {
                assert!(ratio(d, s) >= min, "{name} dark {d} on {s}: {:.2}", ratio(d, s));
            }
        }
    }

    #[test]
    fn colour_sets_equal_the_table() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../ios/App/Assets.xcassets");
        for (name, l, d) in TOKENS {
            let text = std::fs::read_to_string(format!("{root}/{name}.colorset/Contents.json")).unwrap_or_default();
            let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            let hexes: Vec<String> = v["colors"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|c| {
                            let k = &c["color"]["components"];
                            ["red", "green", "blue"]
                                .iter()
                                .map(|ch| k[ch].as_str().unwrap_or("0x??").trim_start_matches("0x").to_uppercase())
                                .collect::<String>()
                        })
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(hexes, [l, d], "{name}.colorset");
        }
    }

    #[test]
    fn zone_edges_are_the_units() {
        // 200 W: power Z1 ends at 111 W (55 % of the FTP, the unit's first edge minus one)
        let u = zone_upper(200, 190);
        assert_eq!(u.len(), 10);
        assert_eq!(u[4], 111);
        assert!(u.windows(2).skip(4).all(|w| w[0] < w[1]) && u[..4].windows(2).all(|w| w[0] < w[1]));
    }
}
