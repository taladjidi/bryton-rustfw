//! Groups of the app's fields, one struct per concern, so `App` (lib.rs) stays a list of parts rather than of
//! flags. Each has its own `new` and keeps the small pure logic that belongs to it alone, testable without an `App`
//! (the same way as `Phone` and `Ftms`). Fields are `pub(crate)`: the other `impl App` blocks read them as
//! `self.notes.ms`.

use crate::text::Line;
use crate::*;

/// The notices and the banner over the screen (app/screens.rs).
pub(crate) struct Notices {
    /// The latest notices, newest first (empty lines: none); the overview shows the first [`ui::NOTICES`].
    pub(crate) list: [Line<40>; 2],
    /// How long the banner (the latest notice, over the screen) has been up; None: none. Nothing under it is
    /// redrawn meanwhile; when it goes, the screen is drawn again.
    pub(crate) ms: Option<u32>,
    /// What the banner says (two lines at most).
    pub(crate) text: Line<40>,
    /// The banner's kind (an alarm white on red).
    pub(crate) kind: Banner,
    /// The one waiting for the banner to go.
    pub(crate) next: Option<(Line<40>, Banner)>,
    /// The banner is drawn yet.
    pub(crate) drawn: bool,
}

impl Notices {
    /// No notice, no banner.
    pub(crate) fn new() -> Self {
        Notices { list: Default::default(), ms: None, text: Line::new(), kind: Banner::Info, next: None, drawn: false }
    }

    /// `line` is the newest notice.
    pub(crate) fn push(&mut self, line: Line<40>) {
        self.list.rotate_right(1);
        self.list[0] = line;
    }

    /// Shows `text` as a banner of `kind`. One up of a higher kind stays and this one waits; one up of a lower kind
    /// waits for this one; one of the same kind is replaced (2026-10-25 review: a turn said again after the next
    /// one, LAP twice shown 8 s). One waits at most (the higher kind kept, else the newer) and shows when the
    /// banner up goes ([`Notices::show_waiting`]): an alarm over a workout step, the step after it. A cue is good
    /// for its moment only: one waiting, or one up pushed off, goes when another cue is said.
    pub(crate) fn say(&mut self, text: &str, kind: Banner) {
        let mut l = Line::<40>::new();
        l.push_str(text);
        if kind.is_cue() && self.next.is_some_and(|n| n.1.is_cue()) {
            self.next = None;
        }
        let up = self.ms.is_some().then_some((self.text, self.kind));
        let (show, wait) = match up {
            Some(up) if up.1 > kind => (up, Some((l, kind))),
            Some(up) if up.1 < kind && !(up.1.is_cue() && kind.is_cue()) => ((l, kind), Some(up)),
            _ => ((l, kind), None),
        };
        if let Some(w) = wait
            && self.next.is_none_or(|n| n.1 <= w.1)
        {
            self.next = Some(w);
        }
        if up != Some(show) {
            (self.text, self.kind) = show;
            (self.ms, self.drawn) = (Some(0), false);
        }
    }

    /// The banner is gone: the one that waited, if any, is up next.
    pub(crate) fn show_waiting(&mut self) {
        if let Some((text, kind)) = self.next.take() {
            (self.text, self.kind) = (text, kind);
            (self.ms, self.drawn) = (Some(0), false);
        }
    }
}

/// The backlight (app/device.rs): what it shows, the automatic level and the measurement behind it.
pub(crate) struct Light {
    /// The level the backlight is at (0: off).
    pub(crate) shown: u8,
    /// Since the last key press.
    pub(crate) idle_ms: u32,
    /// The automatic backlight's state (ambient.rs).
    pub(crate) auto: ambient::Auto,
    /// The level it asks for.
    pub(crate) auto_level: u8,
    /// Since the last light measurement started.
    pub(crate) ms: u32,
    /// The measurement is still running.
    pub(crate) measuring: bool,
}

impl Light {
    /// Off, a measurement due at the first step, the automatic level at its middle until the first reading.
    pub(crate) fn new() -> Self {
        Light {
            shown: 0,
            idle_ms: 0,
            auto: ambient::Auto::default(),
            auto_level: LIGHT_LEVELS[1],
            ms: LIGHT_MS,
            measuring: false,
        }
    }

    /// A key was held (or not) for a pass of `pass_ms`: the idle time restarts or grows.
    pub(crate) fn idle(&mut self, held: bool, pass_ms: u32) {
        self.idle_ms = if held { 0 } else { self.idle_ms.saturating_add(pass_ms) };
    }
}
