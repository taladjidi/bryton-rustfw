//! The colour theme in use: resolved from THEME's name (a built-in, or `\Rustfw\Themes\<NAME>.ini` read from the
//! card), picked in SETTINGS > DISPLAY > THEME, applied with a full redraw. AUTO's day and night themes are resolved
//! into RAM between rides ([`App::load_day_night`]), so the light's switch never reads the card. A file read only:
//! this never writes the card (the choice is kept by the settings file, app/settings_file.rs).

use crate::*;
use theme::{Refused, Theme};

/// Theme files listed (the built-ins come before them).
const MAX_FILES: usize = 14;
/// The folder of the theme files, in the volume.
const THEMES_DIR: &str = "Rustfw/Themes";

impl App {
    /// Rows a menu shows at the theme's row pitch ([`ui::theme::MENU_ROW`]); the hit test of a tap uses the same pitch.
    pub(crate) fn menu_rows(&self) -> usize {
        ui::menu_rows(self.theme.menu_row)
    }

    /// The font and colours every canvas of this app draws with.
    pub(crate) fn look(&self) -> Theme {
        Theme { face: self.settings.face, ..self.theme }
    }

    /// The names of the `.ini` files of the themes folder (up to [`MAX_FILES`], 12 characters of letters, digits, `-`
    /// and `_`; not the built-ins' names), without the extension.
    #[inline(never)]
    pub(crate) fn theme_files(&mut self, b: &mut impl Board) -> ([[u8; 12]; MAX_FILES], usize) {
        let mut out = [[0u8; 12]; MAX_FILES];
        let mut n = 0;
        if self.emmc.lent() {
            return (out, 0);
        }
        let mut d = emmc::Volume(&mut self.emmc, b);
        let Ok(mut fat) = fat::Fat::mount(&mut d) else { return (out, 0) };
        let Ok(dir) = fat.open(&mut d, THEMES_DIR) else { return (out, 0) };
        let Ok(mut cur) = fat.cursor(dir) else { return (out, 0) };
        for _ in 0..256 {
            let Ok(Some(e)) = fat.list(&mut d, &mut cur) else { break };
            let name = e.name();
            let Some((stem, ext)) = name.len().checked_sub(4).and_then(|i| name.split_at_checked(i)) else { continue };
            let ok = ext.eq_ignore_ascii_case(b".ini")
                && !e.file.dir
                && !stem.is_empty()
                && stem.len() <= 12
                && stem.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
            let builtin = core::str::from_utf8(stem).is_ok_and(|s| Theme::builtin(s).is_some());
            if let (true, false, Some(slot)) = (ok, builtin, out.get_mut(n)) {
                slot.iter_mut().zip(stem).for_each(|(o, &c)| *o = c);
                n += 1;
            }
        }
        (out, n)
    }

    /// Theme file `name`, parsed and checked. Err(None): not on the card (or unreadable).
    #[inline(never)] // its 4 KB buffer in its own frame
    fn read_theme(&mut self, b: &mut impl Board, name: &str) -> Result<Theme, Option<Refused>> {
        let mut path = text::Line::<32>::new();
        path.push_str(THEMES_DIR);
        path.push_str("/");
        path.push_str(name);
        path.push_str(".ini");
        let mut buf = [0u8; 4097];
        let mut d = emmc::Volume(&mut self.emmc, b);
        let mut fat = fat::Fat::mount(&mut d).map_err(|_| None)?;
        let f = fat.open(&mut d, path.as_str()).ok().filter(|f| !f.dir).ok_or(None)?;
        let n = fat.read_at(&mut d, &f, 0, &mut buf).map_err(|_| None)?;
        theme::parse(buf.get(..n).unwrap_or(&[])).map_err(Some)
    }

    /// The whole screen drawn again, in the colours now in use (the background first: the strip's gaps and the
    /// margins of the page are never drawn otherwise).
    pub(crate) fn repaint(&mut self, b: &mut impl Board) {
        let look = self.look();
        Panel(b, (0, ui::H), look).fill(0, 0, ui::W, ui::H, ui::BLACK);
        self.font_changed(b);
    }

    /// The theme `name` names: a built-in, or a file read from the card. A file that is missing or refused gives
    /// `fallback` and the notice's text. A card read: between rides only ([`App::load_day_night`]).
    fn resolve(&mut self, b: &mut impl Board, name: &[u8; 12], fallback: Theme) -> (Theme, Option<&'static str>) {
        let n = Settings::name_of(name);
        let t = match Theme::builtin(n) {
            Some(t) => Ok(t),
            None if self.emmc.lent() => Err(None),
            None => self.read_theme(b, n),
        };
        match t {
            Ok(t) => (t, None),
            Err(why) => (fallback, Some(why.map_or("THEME NOT FOUND", Refused::text))),
        }
    }

    /// AUTO's day and night themes resolved into RAM, between rides: when the names change, and when AUTO is picked. A
    /// file missing or refused gives its side's built-in default (OUTDOOR by day, DARK by night), with a notice.
    pub(crate) fn load_day_night(&mut self, b: &mut impl Board) {
        let s = self.settings;
        let (day, day_why) = self.resolve(b, &s.day_theme, Theme::OUTDOOR);
        let (night, night_why) = self.resolve(b, &s.night_theme, Theme::DARK);
        self.auto_themes = [day, night];
        if let Some(why) = day_why.or(night_why) {
            self.notice(why);
        }
    }

    /// The theme AUTO draws now: the day one or the night one, from RAM.
    fn auto_now(&self) -> Theme {
        self.auto_themes[usize::from(self.light.day_night.night)]
    }

    /// AUTO's theme drawn when it is not the one on the screen (a whole redraw); nothing otherwise.
    pub(crate) fn show_auto(&mut self, b: &mut impl Board) {
        let t = self.auto_now();
        if t != self.theme {
            self.theme = t;
            self.repaint(b);
        }
    }

    /// Makes the theme THEME names the one drawn (DARK when its file is missing or refused, with a notice when
    /// `said`), and draws the screen again whole. With AUTO on, the day or the night one, from RAM.
    pub(crate) fn apply_theme(&mut self, b: &mut impl Board, said: bool) {
        if self.settings.auto_theme {
            self.theme = self.auto_now();
            self.repaint(b);
            return;
        }
        let name = self.settings.theme;
        let (t, why) = self.resolve(b, &name, Theme::DARK);
        self.theme = t;
        if said
            && let Some(f) = t.font
            && f != self.settings.face
        {
            self.settings.face = f; // the file's `font` key: FONT when the theme is picked, not on an AUTO switch
            self.save_misc(b);
        }
        self.repaint(b);
        if let (Some(why), true) = (why, said) {
            self.notice(why);
        }
    }

    /// AUTO (THEME): one light reading, `dt_ms` after the last. The switch is decided and drawn from RAM (no card read).
    pub(crate) fn follow_light(&mut self, b: &mut impl Board, lux10: u32, dt_ms: u32) {
        if self.settings.auto_theme && self.light.day_night.update(lux10, dt_ms).is_some() {
            self.show_auto(b);
        }
    }

    /// THEME's, DAY THEME's or NIGHT THEME's Select: the next name of the list, applied at once and kept. THEME's list
    /// starts with AUTO; the others are DARK, OUTDOOR, LIGHT and the card's files. A DAY or NIGHT pick redraws only
    /// when AUTO is on and the theme in use changes.
    pub(crate) fn next_theme(&mut self, b: &mut impl Board, s: menu::Setting) {
        let (files, n) = self.theme_files(b);
        let auto_first = s == menu::Setting::Theme;
        let off = usize::from(auto_first);
        let name = |i: usize| -> &str {
            match i {
                0 => "DARK",
                1 => "OUTDOOR",
                2 => "LIGHT",
                _ => files
                    .get(i - 3)
                    .and_then(|f| f.split(|&c| c == 0).next())
                    .and_then(|s| core::str::from_utf8(s).ok())
                    .unwrap_or("DARK"),
            }
        };
        let cur = match s {
            menu::Setting::DayTheme => self.settings.day_theme,
            menu::Setting::NightTheme => self.settings.night_theme,
            _ => self.settings.theme,
        };
        let now = if auto_first && self.settings.auto_theme {
            0
        } else {
            off + (0..3 + n).find(|&i| name(i).eq_ignore_ascii_case(Settings::name_of(&cur))).unwrap_or(0)
        };
        let next = (now + 1) % (off + 3 + n);
        if auto_first && next == 0 {
            self.settings.auto_theme = true;
            self.light.day_night = Default::default(); // the first reading decides at once
        } else {
            let picked = Settings::name_bytes(name(next - off));
            match s {
                menu::Setting::DayTheme => self.settings.day_theme = picked,
                menu::Setting::NightTheme => self.settings.night_theme = picked,
                _ => {
                    self.settings.theme = picked;
                    self.settings.auto_theme = false;
                }
            }
        }
        if auto_first {
            self.settings_store.theme_set = true;
        } else {
            self.settings_store.names_set = true;
        }
        self.save_misc(b);
        if auto_first {
            if self.settings.auto_theme {
                self.load_day_night(b);
            }
            self.apply_theme(b, true);
        } else {
            self.load_day_night(b);
            if self.settings.auto_theme {
                self.show_auto(b);
            }
        }
    }
}
