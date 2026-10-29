//! The colour theme in use: resolved from THEME's name (a built-in, or `\Rustfw\Themes\<NAME>.ini` read from the
//! card), picked in SETTINGS > DISPLAY > THEME, applied with a full redraw. A file read only: this never writes the
//! card (the choice is kept by the settings file, app/settings_file.rs).

use crate::*;
use theme::{Refused, Theme};

/// Theme files listed (the built-ins come before them).
const MAX_FILES: usize = 14;
/// The folder of the theme files, in the volume.
const THEMES_DIR: &str = "Rustfw/Themes";

impl App {
    /// The font and colours every canvas of this app draws with.
    pub(crate) fn look(&self) -> Theme {
        Theme { smooth: self.settings.smooth, ..self.theme }
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

    /// Makes the theme THEME names the one drawn (DARK when its file is missing or refused, with a notice when
    /// `said`), and draws the screen again whole.
    pub(crate) fn apply_theme(&mut self, b: &mut impl Board, said: bool) {
        let mut name = text::Line::<12>::new();
        name.push_str(self.settings.theme_name());
        let t = match Theme::builtin(name.as_str()) {
            Some(t) => Ok(t),
            None if self.emmc.lent() => Err(None),
            None => self.read_theme(b, name.as_str()),
        };
        self.theme = t.unwrap_or(Theme::DARK);
        self.repaint(b);
        if let (Err(why), true) = (t, said) {
            self.notice(why.map_or("THEME NOT FOUND", Refused::text));
        }
    }

    /// THEME's Select: the next of DARK, OUTDOOR and the card's files, applied at once and kept.
    pub(crate) fn next_theme(&mut self, b: &mut impl Board) {
        let (files, n) = self.theme_files(b);
        let name = |i: usize| -> &str {
            match i {
                0 => "DARK",
                1 => "OUTDOOR",
                _ => files
                    .get(i - 2)
                    .and_then(|f| f.split(|&c| c == 0).next())
                    .and_then(|s| core::str::from_utf8(s).ok())
                    .unwrap_or("DARK"),
            }
        };
        let now = (0..2 + n).find(|&i| name(i).eq_ignore_ascii_case(self.settings.theme_name())).unwrap_or(0);
        let mut next = text::Line::<12>::new();
        next.push_str(name((now + 1) % (2 + n)));
        self.settings.set_theme_name(next.as_str());
        self.settings_store.theme_set = true;
        self.save_misc(b);
        self.apply_theme(b, true);
    }
}
