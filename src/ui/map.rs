//! The map: the ride's track (breadcrumbs from the GNSS fixes), the position and heading as an arrow, north up,
//! scaled to fit the whole track, with a scale bar. A ride page field ([`crate::ui::Field::Map`]), so it can fill a
//! page or share one with values.
//!
//! - **Projection:** a local plane in metres, equirectangular around the first fix (x east, y north). At bike scale
//!   its error is far below a pixel.
//! - **Track:** at most [`POINTS`] points, one every `step` metres; when full, every other point is dropped and the
//!   step doubles, so a ride of any length fits, evenly spaced. No heap.
//! - **Zoom:** the view fits the whole track in the middle two thirds of the map, at a scale of 2^k or 1.5 x 2^k
//!   metres per pixel; it is fitted again (one full redraw) only when the position nears the edge, so a loop is drawn once and a ride out
//!   redraws each time it grows by about half.
//! - **Manual zoom:** a map box at least [`ZOOM_BOX`] (the full-page map) has "+" and "-" buttons top right
//!   ([`buttons`], 48 x 48 pixels, 8 apart, for a gloved finger), drawn by [`draw`] like the rest of the map. Each tap is one
//!   level ([`zoom`]) from the one shown, finer or coarser than the fit alike (indoors the fit is already the
//!   finest it takes, [`FIT_LEVEL`]: the owner's "+" and "-" did nothing, 2026-10-25), from [`MIN_LEVEL`] (1.5 m a
//!   pixel: IGN's zoom 16 drawn about 1:1 in France) to [`MAX_LEVEL`]. At a manual level the view follows the rider
//!   ([`View::follow`]): it is centred a sixth of the map ahead of them, and centred again only when the arrow
//!   leaves the middle three fifths, so the basemap is drawn once per re-centre, not every second. The third
//!   button, a crosshair under "-", shown at any manual level, goes back to the automatic fit.
//! - **Pan:** at a manual level, a drag on the map moves it with the finger ([`pan`]), once at the release (one
//!   basemap per view, no redraw while the finger moves), at most [`PAN_SCREENS`] maps' widths past the track and
//!   the course. The view then stays where it was put ([`Zoom::centre`]) instead of following; "+" and "-" zoom
//!   around the centre of the view panned to; the crosshair goes back to the fit.
//! - **Redraw:** otherwise only the box around the new segment and the arrow's old and new places is drawn, from
//!   the state, through [`Clip`]: the same [`draw`] paints the whole map, a dirty box, or one screenshot row (each
//!   segment is a run per row, clipped to [`Canvas::clip`]), so they agree by construction.
//! - **Basemap:** [`draw`] copies the background from the basemap's buffer ([`crate::tiles`]), drawn once per view,
//!   so a dirty box restores the roads under the arrow without drawing them again. "NO MAP" (top left) when no
//!   cached tiles cover the view; black as before when the basemap has no memory. IGN's raster tiles, when packs
//!   cover the view ([`crate::ign`]), come through the same buffer, their "(c) IGN" drawn into it.

use crate::text::{self, Align, Line, Style};
use crate::tiles::{Basemap, Layer};
use crate::ui::{self, Canvas, H, LABEL, W};

/// Track points kept at most (8 bytes each).
pub const POINTS: usize = 512;
/// Spacing of the track points until the track first fills, metres.
const FIRST_STEP_M: i32 = 10;
/// The finest zoom level: level `i` is [`mpp16`]`(i)` / 16 metres per pixel, so 1.5 m (IGN's zoom 16 drawn 1.1x
/// at 45 N, 1.6 m a tile pixel there).
pub const MIN_LEVEL: u32 = 1;
/// The finest level the fit takes, 2 m a pixel: finer is the rider's choice ("+"), since a re-centre costs a
/// basemap.
pub const FIT_LEVEL: u32 = 2;
/// The coarsest zoom level, about 1000 km a pixel.
pub const MAX_LEVEL: u32 = 40;
/// The smallest map box with the zoom buttons, pixels: the full-page map (layout 1, 240 x 296), not half a page.
pub const ZOOM_BOX: (i32, i32) = (200, 200);
/// A zoom button's side, pixels: a gloved finger's size.
const BUTTON: i32 = 48;
/// The gap between two buttons, pixels (adjacent targets with opposite meanings: a glove must not hit both).
const GAP: i32 = 8;
/// A panned view's centre stays within this many map widths (or heights) of the track and the course.
pub const PAN_SCREENS: i32 = 2;

/// A zoom set by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zoom {
    /// Its level ([`mpp16`]).
    pub level: u32,
    /// Panned: the view's centre, metres on the track's plane; None: the view follows the rider.
    pub centre: Option<(i32, i32)>,
}

/// A button on the map ([`button_at`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// "+": a level finer.
    In,
    /// "-": a level coarser.
    Out,
    /// The crosshair, while panned: follow the rider again.
    Centre,
}
/// The view is fitted again when the position comes this close to the map's edge (pixels).
const MARGIN: i32 = 20;
/// The arrow's length from its centre to its tip (pixels).
const ARROW: i32 = 9;
/// sin of the 16 compass directions (22.5 degrees apart, clockwise from north), x 256.
const SIN16: [i32; 16] = [0, 98, 181, 237, 256, 237, 181, 98, 0, -98, -181, -237, -256, -237, -181, -98];

/// A rectangle in screen pixels, signed so shapes may start off it: x, y, width, height.
pub type Rect = (i32, i32, i32, i32);

/// cos(latitude) x 65536, latitude in degrees x 1e-7 (Bhaskara I's approximation, within 0.2 %).
///
/// ```
/// use bryton_rustfw::map::cos_q16;
/// assert_eq!(cos_q16(0), 65536);
/// assert!((cos_q16(600_000_000) - 32768).abs() < 100); // cos 60 = 0.5
/// assert_eq!(cos_q16(-900_000_000), 0);
/// ```
pub fn cos_q16(lat: i32) -> i64 {
    let t = i64::from(lat / 10_000); // 1e-3 degrees
    let t2 = t * t;
    ((32_400_000_000 - 4 * t2) << 16) / (32_400_000_000 + t2)
}

/// Where (`lat`, `lon`) (degrees x 1e-7) is on the plane of `origin` (its latitude, longitude and [`cos_q16`] of its
/// latitude): metres east and north.
///
/// ```
/// use bryton_rustfw::map::{cos_q16, project};
/// assert_eq!(project((0, 0, cos_q16(0)), 8983, 0), (0, 99)); // 8983e-7 degrees of latitude: 100 m
/// ```
pub fn project((lat0, lon0, cos): (i32, i32, i64), lat: i32, lon: i32) -> (i32, i32) {
    // 1e-7 degrees of a great circle is 0.011132 m (WGS84 equator; inferred close enough for a drawing)
    let x = (i64::from(lon) - i64::from(lon0)) * 11_132 * cos / 65_536 / 1_000_000;
    let y = (i64::from(lat) - i64::from(lat0)) * 11_132 / 1_000_000;
    (x as i32, y as i32)
}

/// A course drawn under the track ([`crate::course`]): its points in its own plane, the bounding box of each
/// [`Route::CHUNK`] of them (so a dirty box draws only the chunks it touches), where its plane's origin is on the
/// track's plane, and its id (a new course fits the map again).
#[derive(Clone, Copy)]
pub struct Route<'a> {
    /// Points, metres east and north of the course's first point.
    pub pts: &'a [(i32, i32)],
    /// Per chunk of points: min x, min y, max x, max y.
    pub boxes: &'a [(i32, i32, i32, i32)],
    /// Bit `i`: no segment into point `i` (a gap in the course, [`crate::course::is_gap`]).
    pub gaps: &'a [u32],
    /// The course's origin on the track's plane, metres.
    pub off: (i32, i32),
    /// Changes with the course.
    pub id: u32,
}

impl Route<'_> {
    /// Points per box.
    pub const CHUNK: usize = 64;

    /// South-west and north-east corners on the track's plane.
    fn bounds(&self) -> Option<((i32, i32), (i32, i32))> {
        let (o, b) = (self.off, self.boxes);
        let min = (b.iter().map(|b| b.0).min()?, b.iter().map(|b| b.1).min()?);
        let max = (b.iter().map(|b| b.2).max()?, b.iter().map(|b| b.3).max()?);
        Some(((min.0 + o.0, min.1 + o.1), (max.0 + o.0, max.1 + o.1)))
    }
}

/// The colour of a course on the map: magenta, apart from the track's orange and the basemap's roads and water.
pub const COURSE: u16 = 0xc81f;

/// The ride's track, in metres from its first fix.
pub struct Track {
    /// The first fix (degrees x 1e-7) and cos of its latitude x 65536.
    origin: Option<(i32, i32, i64)>,
    pts: [(i32, i32); POINTS],
    len: usize,
    step_m: i32,
    /// Counts the thinnings: the points drawn before one are no longer the track.
    thinned: u32,
    /// The last position and heading of motion (degrees x 1e-5).
    pos: Option<(i32, i32)>,
    heading: i32,
    /// The position is the last one known from an earlier power-up ([`Track::seed`]), not a fix yet.
    stale: bool,
    /// Everything the track went through: south-west and north-east corners.
    min: (i32, i32),
    max: (i32, i32),
}

impl Default for Track {
    fn default() -> Self {
        Self::new()
    }
}

impl Track {
    /// No fix yet.
    pub const fn new() -> Self {
        Track {
            origin: None,
            pts: [(0, 0); POINTS],
            len: 0,
            step_m: FIRST_STEP_M,
            thinned: 0,
            pos: None,
            heading: 0,
            stale: false,
            min: (0, 0),
            max: (0, 0),
        }
    }

    /// Before any fix: the last position known (degrees x 1e-7, kept from an earlier power-up), shown as such until
    /// the first fix replaces it. Ignored once there is a track.
    pub fn seed(&mut self, lat: i32, lon: i32) {
        if self.origin.is_none() {
            self.origin = Some((lat, lon, cos_q16(lat)));
            (self.pos, self.min, self.max, self.stale) = (Some((0, 0)), (0, 0), (0, 0), true);
        }
    }

    /// The first fix or the last known position (degrees x 1e-7): the plane's origin.
    pub fn origin(&self) -> Option<(i32, i32)> {
        self.origin.map(|(lat, lon, _)| (lat, lon))
    }

    /// The position shown is the last one known, not a fix.
    pub fn stale(&self) -> bool {
        self.stale
    }

    /// A good fix: latitude and longitude in degrees x 1e-7, heading of motion in degrees x 1e-5.
    pub fn add(&mut self, lat: i32, lon: i32, heading: i32) {
        let o = *self.origin.get_or_insert((lat, lon, cos_q16(lat)));
        let p = project(o, lat, lon);
        if self.pos.is_none() || self.stale {
            (self.min, self.max, self.stale) = (p, p, false); // the first fix: the last known position goes
        }
        self.min = (self.min.0.min(p.0), self.min.1.min(p.1));
        self.max = (self.max.0.max(p.0), self.max.1.max(p.1));
        self.pos = Some(p);
        self.heading = heading;
        let far = self.points().last().is_none_or(|&q| {
            let (dx, dy) = (i64::from(p.0 - q.0), i64::from(p.1 - q.1));
            dx * dx + dy * dy >= i64::from(self.step_m) * i64::from(self.step_m)
        });
        if !far {
            return;
        }
        if self.len == POINTS {
            for i in 0..POINTS / 2 {
                self.pts[i] = self.pts[2 * i];
            }
            self.len = POINTS / 2;
            self.step_m = self.step_m.saturating_mul(2);
            self.thinned = self.thinned.wrapping_add(1);
        }
        if let Some(slot) = self.pts.get_mut(self.len) {
            *slot = p;
            self.len += 1;
        }
    }

    /// A ride read back from its file is whole: its end is shown as a dot (no heading), as a last known position.
    pub fn end(&mut self) {
        self.stale = self.pos.is_some();
    }

    /// The points kept, oldest first (metres east and north of the first fix).
    pub fn points(&self) -> &[(i32, i32)] {
        self.pts.get(..self.len).unwrap_or(&[])
    }

    /// The last position, metres east and north of the first fix.
    pub fn pos(&self) -> Option<(i32, i32)> {
        self.pos
    }
}

/// Metres per pixel x 16 at zoom level `i`: 1, 1.5, 2, 3, 4, 6... (steps of about 1.4).
///
/// ```
/// assert_eq!([2, 3, 4, 5].map(bryton_rustfw::map::mpp16), [32, 48, 64, 96]);
/// ```
pub fn mpp16(i: u32) -> i64 {
    (if i.is_multiple_of(2) { 16 } else { 24 }) << (i / 2)
}

/// What part of the plane the map shows: its centre (metres) and its scale, `mpp16` / 16 metres per pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub(crate) cx: i32,
    pub(crate) cy: i32,
    pub(crate) mpp16: i64,
}

impl View {
    /// The view of `track` (and of `route`, when there is one) in `area`: all of them in the middle two thirds, at
    /// the smallest scale; None before a fix.
    pub fn fit(track: &Track, area: Rect, route: Option<&Route>) -> Option<View> {
        let (mut fw, fh) = (i64::from(area.2 - 2 * MARGIN) * 2 / 3, i64::from(area.3 - 2 * MARGIN) * 2 / 3);
        if let Some([plus, _]) = buttons(area) {
            // the track's right end stays clear of the buttons ([`under_buttons`]): left of them by half an arrow
            fw = fw.min(2 * i64::from(plus.0 - ARROW / 2 - 1 - (area.0 + area.2 / 2)));
        }
        let (min, max) = match route.and_then(Route::bounds) {
            Some((a, b)) => {
                ((a.0.min(track.min.0), a.1.min(track.min.1)), (b.0.max(track.max.0), b.1.max(track.max.1)))
            }
            None => (track.min, track.max),
        };
        Self::fit_box(track, min, max, fw, fh)
    }

    /// The view of a whole ride's `track` in `area` (it will not grow): all of the map but a few pixels at the
    /// edges; None without a position.
    pub fn fit_whole(track: &Track, area: Rect) -> Option<View> {
        Self::fit_in(track, i64::from(area.2 - 16), i64::from(area.3 - 16))
    }

    /// The track in `fw` x `fh` pixels at the smallest scale.
    fn fit_in(track: &Track, fw: i64, fh: i64) -> Option<View> {
        Self::fit_box(track, track.min, track.max, fw, fh)
    }

    /// The box from `min` to `max` in `fw` x `fh` pixels at the smallest scale; None while `track` has no position.
    fn fit_box(track: &Track, min: (i32, i32), max: (i32, i32), fw: i64, fh: i64) -> Option<View> {
        track.pos?;
        let (sx, sy) = (i64::from(max.0) - i64::from(min.0), i64::from(max.1) - i64::from(min.1));
        let fits = |m: i64| sx * 16 / m <= fw && sy * 16 / m <= fh;
        let mpp16 = (FIT_LEVEL..MAX_LEVEL).map(mpp16).find(|&m| fits(m)).unwrap_or(mpp16(MAX_LEVEL));
        Some(View { cx: (i64::from(min.0) + sx / 2) as i32, cy: (i64::from(min.1) + sy / 2) as i32, mpp16 })
    }

    /// The view of `track` in `area` at `mpp16`, zoomed in: centred a sixth of the map ahead of the rider (behind
    /// them is where they have been), so the arrow crosses the middle part before the next re-centre; None before a
    /// position.
    pub fn follow(track: &Track, area: Rect, mpp16: i64) -> Option<View> {
        let p = track.pos?;
        let ahead = if track.stale { 0 } else { i64::from(area.2.min(area.3) / 6) * mpp16 / 16 };
        let dir = Arrow::dir(track.heading);
        let at = |s: i32| (i64::from(s) * ahead / 256) as i32;
        Some(View { cx: p.0 + at(SIN16[dir]), cy: p.1 + at(SIN16[(dir + 4) % 16]), mpp16 })
    }

    /// Where point `p` (metres) falls on screen.
    pub(crate) fn px(&self, area: Rect, p: (i32, i32)) -> (i32, i32) {
        let px = |d: i32| (i64::from(d) * 16).div_euclid(self.mpp16) as i32;
        (area.0 + area.2 / 2 + px(p.0 - self.cx), area.1 + area.3 / 2 - px(p.1 - self.cy))
    }

    /// Zoomed in, the arrow is still in the middle three fifths of the map: no need to centre again.
    fn follows(&self, area: Rect, track: &Track) -> bool {
        let Some(p) = track.pos else { return true };
        let (x, y) = self.px(area, p);
        let (mx, my) = (area.2 / 5, area.3 / 5);
        x >= area.0 + mx && x < area.0 + area.2 - mx && y >= area.1 + my && y < area.1 + area.3 - my
    }

    /// The position is still well inside the map and clear of the buttons ([`under_buttons`]): no need to fit
    /// again.
    fn keeps(&self, area: Rect, track: &Track) -> bool {
        let Some(p) = track.pos else { return true };
        let (x, y) = self.px(area, p);
        !under_buttons(area, (x, y))
            && x >= area.0 + MARGIN
            && x < area.0 + area.2 - MARGIN
            && y >= area.1 + MARGIN
            && y < area.1 + area.3 - MARGIN
    }
}

/// The arrow as drawn: its centre and one of 16 directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrow {
    x: i32,
    y: i32,
    dir: usize,
    /// The last known position, not a fix: a grey dot, no direction.
    stale: bool,
}

impl Arrow {
    /// The arrow for the track's last position.
    fn of(view: &View, area: Rect, track: &Track) -> Option<Arrow> {
        let (x, y) = view.px(area, track.pos?);
        Some(Arrow { x, y, dir: Self::dir(track.heading), stale: track.stale })
    }

    /// The nearest of the 16 directions to `heading` (degrees x 1e-5).
    fn dir(heading: i32) -> usize {
        ((heading.rem_euclid(36_000_000) + 1_125_000) / 2_250_000 % 16) as usize
    }

    /// The box it covers, outline included.
    fn rect(&self) -> Rect {
        let r = ARROW + 3;
        (self.x - r, self.y - r, 2 * r + 1, 2 * r + 1)
    }

    /// Its corners at `size` pixels from the centre: the tip ahead, the two others behind at +-135 degrees.
    fn corners(&self, size: i32) -> [(i32, i32); 3] {
        let at = |d: usize, len: i32| {
            let (s, c) = (SIN16[d % 16], SIN16[(d + 4) % 16]);
            (self.x + s * len / 256, self.y - c * len / 256) // screen y points south
        };
        [at(self.dir, size), at(self.dir + 6, size * 3 / 4), at(self.dir + 10, size * 3 / 4)]
    }
}

/// A map on screen: the view it was drawn with, the track points and the arrow drawn (to redraw only changes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// The view; None while "NO FIX" is shown.
    pub view: Option<View>,
    points: usize,
    thinned: u32,
    arrow: Option<Arrow>,
    /// The course drawn ([`Route::id`]).
    route: Option<u32>,
    /// The view is at a zoom set by hand (following the rider, [`View::follow`], or panned), not the fit.
    zoomed: bool,
    /// Zoomed and panned: the view stays at [`Zoom::centre`].
    panned: bool,
}

impl Shown {
    /// The map of `track` (and `route`) in `area` at `zoom` (None: the fit) as [`draw`] will show it when drawn
    /// whole now.
    pub fn of(track: &Track, area: Rect, route: Option<&Route>, zoom: Option<Zoom>) -> Shown {
        let fit = View::fit(track, area, route);
        let zoomed = zoomed(area, fit, zoom);
        let centre = zoom.and_then(|z| z.centre).filter(|_| zoomed.is_some());
        let view = match (zoomed, centre) {
            (Some(mpp16), Some((cx, cy))) => Some(View { cx, cy, mpp16 }),
            (Some(m), None) => View::follow(track, area, m),
            _ => fit,
        };
        let arrow = view.and_then(|v| Arrow::of(&v, area, track));
        let route = route.map(|r| r.id);
        let (zoomed, panned) = (zoomed.is_some(), centre.is_some());
        Shown { view, points: track.len, thinned: track.thinned, arrow, route, zoomed, panned }
    }

    /// The view is at a zoom set by hand (a drag pans it).
    pub fn zoomed(&self) -> bool {
        self.zoomed && self.view.is_some()
    }

    /// The view is panned.
    pub fn panned(&self) -> bool {
        self.panned
    }
}

/// The scale (`mpp16`) of `zoom` in `area` when it applies: a box with the buttons, a position to show (`fit`).
fn zoomed(area: Rect, fit: Option<View>, zoom: Option<Zoom>) -> Option<i64> {
    let m = mpp16(zoom?.level);
    (buttons(area).is_some() && fit.is_some()).then_some(m)
}

/// The arrow centred at `p` is under the buttons, or within half its size of them: the fit keeps clear of them
/// (its track spans the middle two thirds, less on the right: at most x 179 in a 240 pixel map, the buttons from 188), so the arrow
/// and the track's end stay in sight. Zoomed in, following keeps the arrow left of x 192 already.
fn under_buttons(area: Rect, p: (i32, i32)) -> bool {
    let near = |r: Rect| {
        let m = ARROW / 2;
        (r.0 - m..r.0 + r.2 + m).contains(&p.0) && (r.1 - m..r.1 + r.3 + m).contains(&p.1)
    };
    buttons(area).is_some_and(|b| b.into_iter().any(near))
}

/// The crosshair under "-", back to the fit (shown at a manual zoom).
fn centre_button(area: Rect) -> Option<Rect> {
    buttons(area).map(|[_, b]| (b.0, b.1 + b.3 + GAP, b.2, b.3))
}

/// The zoom buttons of a map in `area`, "+" then "-", top right; None in a box smaller than [`ZOOM_BOX`].
///
/// ```
/// use bryton_rustfw::map::buttons;
/// assert_eq!(buttons((0, 24, 240, 296)), Some([(188, 28, 48, 48), (188, 84, 48, 48)]));
/// assert_eq!(buttons((0, 24, 240, 148)), None, "half a page");
/// ```
pub fn buttons(area: Rect) -> Option<[Rect; 2]> {
    let x = area.0 + area.2 - BUTTON - 4;
    (area.2 >= ZOOM_BOX.0 && area.3 >= ZOOM_BOX.1)
        .then_some([(x, area.1 + 4, BUTTON, BUTTON), (x, area.1 + 4 + BUTTON + GAP, BUTTON, BUTTON)])
}

/// The button of the map in `area` (drawn as `shown`) at (`x`, `y`); None elsewhere, or while the map has no view
/// (no buttons drawn).
pub fn button_at(area: Rect, shown: &Shown, x: i32, y: i32) -> Option<Button> {
    let [plus, minus] = buttons(area).filter(|_| shown.view.is_some())?;
    let inside = |r: Rect| (r.0..r.0 + r.2).contains(&x) && (r.1..r.1 + r.3).contains(&y);
    if inside(plus) {
        Some(Button::In)
    } else if inside(minus) {
        Some(Button::Out)
    } else if shown.zoomed && centre_button(area).is_some_and(inside) {
        Some(Button::Centre)
    } else {
        None
    }
}

/// The centre of the map in `area` (drawn zoomed in as `shown`) after a drag of (`dx`, `dy`) pixels: the map moves
/// with the finger, so the view's centre the other way; kept within [`PAN_SCREENS`] map widths of the track and the
/// course. None when the map is not zoomed in.
pub fn pan(
    track: &Track,
    area: Rect,
    route: Option<&Route>,
    shown: &Shown,
    (dx, dy): (i32, i32),
) -> Option<(i32, i32)> {
    let v = shown.view.filter(|_| shown.zoomed)?;
    let m = |px: i32| (i64::from(px) * v.mpp16 / 16) as i32;
    let (min, max) = match route.and_then(Route::bounds) {
        Some((a, b)) => ((a.0.min(track.min.0), a.1.min(track.min.1)), (b.0.max(track.max.0), b.1.max(track.max.1))),
        None => (track.min, track.max),
    };
    let (rx, ry) = (m(area.2 * PAN_SCREENS), m(area.3 * PAN_SCREENS));
    // clamped, but never moved against the drag: after "+" the range is smaller and a centre at its old edge is
    // outside it, where a small drag would have jumped back across (review of 6de15ed)
    let go = |old: i32, new: i32, lo: i32, hi: i32| {
        let c = new.clamp(lo, hi);
        if (c - old).signum() * (new - old).signum() < 0 { old } else { c }
    };
    let cx = go(v.cx, v.cx.saturating_sub(m(dx)), min.0.saturating_sub(rx), max.0.saturating_add(rx));
    let cy = go(v.cy, v.cy.saturating_add(m(dy)), min.1.saturating_sub(ry), max.1.saturating_add(ry));
    Some((cx, cy))
}

/// The level after a tap on "+" (`zoom_in`) or "-" on the map drawn as `shown`: one from the one shown, within
/// [`MIN_LEVEL`] and [`MAX_LEVEL`], finer or coarser than the fit; None while the map shows no view.
pub fn zoom(shown: &Shown, zoom_in: bool) -> Option<u32> {
    let v = shown.view?;
    let level = (MIN_LEVEL..MAX_LEVEL).find(|&l| mpp16(l) >= v.mpp16).unwrap_or(MAX_LEVEL);
    Some(if zoom_in { level.saturating_sub(1).max(MIN_LEVEL) } else { (level + 1).min(MAX_LEVEL) })
}

/// The map area of a ride field box: the box less its frame lines (ui::draw_field_frame).
pub fn area((x, y, w, h): (u32, u32, u32, u32)) -> Rect {
    let w = w - u32::from(x + w < W);
    let h = h - u32::from(y + h < H - 2);
    (x as i32, y as i32, w as i32, h as i32)
}

/// A canvas that keeps only what falls in a rectangle: a dirty box redrawn on the panel.
pub struct Clip<'a> {
    c: &'a mut dyn Canvas,
    /// x, y, w, h, on screen.
    r: (u32, u32, u32, u32),
}

impl<'a> Clip<'a> {
    /// `c`, kept to `r` and to what `c` itself keeps.
    pub fn new(c: &'a mut dyn Canvas, r: Rect) -> Self {
        let r = cut(r, c.clip());
        Clip { c, r: (r.0 as u32, r.1 as u32, r.2 as u32, r.3 as u32) }
    }
}

impl Canvas for Clip<'_> {
    fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, color: u16) {
        let (x0, y0) = (x.max(self.r.0), y.max(self.r.1));
        let (x1, y1) = ((x + w).min(self.r.0 + self.r.2), (y + h).min(self.r.1 + self.r.3));
        if x0 < x1 && y0 < y1 {
            self.c.fill(x0, y0, x1 - x0, y1 - y0, color);
        }
    }

    fn blit(&mut self, x: u32, y: u32, w: u32, h: u32, px: &mut dyn FnMut(u32, u32) -> u16) {
        let (x0, y0) = (x.max(self.r.0), y.max(self.r.1));
        let (x1, y1) = ((x + w).min(self.r.0 + self.r.2), (y + h).min(self.r.1 + self.r.3));
        if x0 < x1 && y0 < y1 {
            self.c.blit(x0, y0, x1 - x0, y1 - y0, &mut |dx, dy| px(dx + x0 - x, dy + y0 - y));
        }
    }

    fn clip(&self) -> (u32, u32, u32, u32) {
        self.r
    }

    fn smooth(&self) -> bool {
        self.c.smooth()
    }
}

/// `r` cut to `k` (empty: zero width or height).
fn cut(r: Rect, k: (u32, u32, u32, u32)) -> Rect {
    let (kx, ky, kw, kh) = (k.0 as i32, k.1 as i32, k.2 as i32, k.3 as i32);
    let (x0, y0) = (r.0.max(kx), r.1.max(ky));
    let (x1, y1) = ((r.0 + r.2).min(kx + kw), (r.1 + r.3).min(ky + kh));
    (x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
}

/// The smallest rectangle holding `a` and `b`.
fn union(a: Option<Rect>, b: Rect) -> Rect {
    let Some(a) = a else { return b };
    let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
    ((x0), (y0), (a.0 + a.2).max(b.0 + b.2) - x0, (a.1 + a.3).max(b.1 + b.3) - y0)
}

/// Fills `w` x `h` from (`x`, `y`), the part inside `k` (signed: shapes can start off screen).
fn run(c: &mut impl Canvas, k: Rect, x: i32, y: i32, w: i32, h: i32, color: u16) {
    let r = cut((x, y, w, h), (k.0 as u32, k.1 as u32, k.2 as u32, k.3 as u32));
    if r.2 > 0 && r.3 > 0 {
        c.fill(r.0 as u32, r.1 as u32, r.2 as u32, r.3 as u32, color);
    }
}

/// A 2-pixel line from `a` to `b`, one run per row, kept to `k`.
fn segment(c: &mut impl Canvas, k: Rect, a: (i32, i32), b: (i32, i32), color: u16) {
    let (top, bottom, left, right) = (a.1.min(b.1), a.1.max(b.1), a.0.min(b.0), a.0.max(b.0));
    if right + 2 <= k.0 || left >= k.0 + k.2 || bottom + 2 <= k.1 || top >= k.1 + k.3 {
        return;
    }
    for y in top.max(k.1 - 1)..=bottom.min(k.1 + k.3 - 1) {
        let (lo, hi) = if a.1 == b.1 {
            (left, right)
        } else {
            // x where the line crosses the row's top and bottom edges (half rows)
            let at = |h2: i64| {
                let (ax, ay) = (i64::from(a.0), i64::from(a.1));
                (ax + i64::from(b.0 - a.0) * (h2 - 2 * ay) / (2 * i64::from(b.1 - a.1))) as i32
            };
            let (p, q) = (at(2 * i64::from(y) - 1), at(2 * i64::from(y) + 1));
            (p.min(q).max(left), p.max(q).min(right))
        };
        run(c, k, lo, y, hi - lo + 2, 2, color);
    }
}

/// A filled triangle, one run per row, kept to `k`.
fn triangle(c: &mut impl Canvas, k: Rect, v: [(i32, i32); 3], color: u16) {
    let top = v.iter().map(|p| p.1).min().unwrap_or(0).max(k.1);
    let bottom = v.iter().map(|p| p.1).max().unwrap_or(0).min(k.1 + k.3 - 1);
    for y in top..=bottom {
        let (mut lo, mut hi) = (i32::MAX, i32::MIN);
        for (a, b) in [(v[0], v[1]), (v[1], v[2]), (v[2], v[0])] {
            if a.1 != b.1 && a.1.min(b.1) <= y && y <= a.1.max(b.1) {
                let x = a.0 + (b.0 - a.0) * (y - a.1) / (b.1 - a.1);
                (lo, hi) = (lo.min(x), hi.max(x));
            }
        }
        if lo <= hi {
            run(c, k, lo, y, hi - lo + 1, 1, color);
        }
    }
}

/// The scale bar, bottom left: a bar of a fixed length and the distance it spans above it. Fixed, so a zoom step
/// always changes the number the way the zoom went (a round length per view made two steps show the same "200 M" with
/// bars of other lengths, which read as a zoom the wrong way: the owner, 2026-10-25). The levels' metres a pixel (1,
/// 1.5, 2, 3, 4, 6...) times 50 px give round enough numbers: 50 M, 75 M, 100 M, 150 M... 1.2 KM, as Garmin's
/// scale steps go.
fn scale_bar(c: &mut impl Canvas, k: Rect, area: Rect, view: &View) {
    // half as long in a box too narrow (a third of its width at most)
    let len = if area.2 / 3 >= SCALE_PX { SCALE_PX } else { SCALE_PX / 2 };
    let m = (i64::from(len) * view.mpp16 / 16).clamp(1, i64::from(i32::MAX)) as u32;
    let (x, y) = (area.0 + 8, area.1 + area.3 - 8);
    // a black outline first: white alone vanishes on IGN's light map
    run(c, k, x - 1, y - 3, len + 2, 4, ui::BLACK);
    run(c, k, x - 1, y - 9, 4, 8, ui::BLACK);
    run(c, k, x + len - 3, y - 9, 4, 8, ui::BLACK);
    run(c, k, x, y - 2, len, 2, ui::WHITE);
    run(c, k, x, y - 8, 2, 6, ui::WHITE);
    run(c, k, x + len - 2, y - 8, 2, 6, ui::WHITE);
    let mut l = Line::<8>::new();
    push_distance(&mut l, m);
    let ty = y - 10 - LABEL.height() as i32;
    if x >= 0 && ty >= 0 {
        text::draw(c, &LABEL, x as u32, ty as u32, Align::Left, l.as_str());
    }
}

/// The scale bar's length, pixels (half in a narrow box).
const SCALE_PX: i32 = 50;

/// `m` metres as the scale bar says it: "75 M" under a kilometre, then "1.2 KM" with one decimal under 10 km, then
/// whole kilometres.
///
/// ```
/// use bryton_rustfw::{map::push_distance, text::Line};
/// let s = |m| { let mut l = Line::<8>::new(); push_distance(&mut l, m); std::string::String::from(l.as_str()) };
/// assert_eq!([s(75), s(1200), s(1000), s(12_800)], ["75 M", "1.2 KM", "1 KM", "12 KM"]);
/// ```
pub fn push_distance<const N: usize>(l: &mut Line<N>, m: u32) {
    match m {
        ..1000 => {
            l.push_u32(m);
            l.push_str(" M");
        }
        1000..10_000 => {
            l.push_u32(m / 1000);
            if m % 1000 >= 100 {
                l.push_str(".");
                l.push_u32(m % 1000 / 100);
            }
            l.push_str(" KM");
        }
        _ => {
            l.push_u32(m / 1000);
            l.push_str(" KM");
        }
    }
}

/// Draws the map of `track` in `area` as `view` shows it (None: "NO FIX"): the background (the basemap from
/// `base`'s buffer, else black), the course (`route`), the track, the scale bar, the arrow and the buttons ("re-centre"
/// too when `panned`), in that order. Only
/// the part `c` keeps ([`Canvas::clip`]) costs anything: of the course, only the chunks whose box it touches.
pub fn draw(
    c: &mut dyn Canvas,
    area: Rect,
    track: &Track,
    view: Option<View>,
    base: Option<&Basemap>,
    route: Option<&Route>,
    manual: bool,
) {
    let mut c = Clip::new(c, area);
    let k = cut(area, c.clip());
    if k.2 == 0 || k.3 == 0 {
        return;
    }
    let layer = match (base, view, track.origin()) {
        (Some(b), Some(v), Some(o)) => b.layer(area, v, o),
        _ => Layer::None,
    };
    match layer {
        Layer::Map(px) => {
            let w = area.2 as u32;
            c.blit(area.0 as u32, area.1 as u32, w, area.3 as u32, &mut |x, y| {
                px.get((y * w + x) as usize).copied().unwrap_or(ui::BLACK)
            });
        }
        Layer::NoMap => {
            run(&mut c, k, area.0, area.1, area.2, area.3, ui::BLACK);
            if area.0 >= 0 && area.1 >= 0 {
                let grey = Style { fg: ui::GREY, ..LABEL };
                text::draw(&mut c, &grey, area.0 as u32 + 8, area.1 as u32 + 8, Align::Left, "NO MAP");
            }
        }
        Layer::None => run(&mut c, k, area.0, area.1, area.2, area.3, ui::BLACK),
    }
    let Some(v) = view else {
        let (x, y) = (area.0 + area.2 / 2, area.1 + area.3 / 2 - LABEL.height() as i32 / 2);
        if x >= 0 && y >= 0 {
            text::draw(&mut c, &Style { fg: ui::GREY, ..LABEL }, x as u32, y as u32, Align::Centre, "NO FIX");
        }
        return;
    };
    if let Some(r) = route {
        let o = r.off;
        for (i, b) in r.boxes.iter().enumerate() {
            let ((x0, y0), (x1, y1)) = (v.px(area, (b.0 + o.0, b.3 + o.1)), v.px(area, (b.2 + o.0, b.1 + o.1)));
            if x1 + 2 < k.0 || x0 >= k.0 + k.2 || y1 + 2 < k.1 || y0 >= k.1 + k.3 {
                continue;
            }
            // the chunk's points and the first of the next (the segment between them)
            let from = i * Route::CHUNK;
            let pts = r.pts.get(from..(from + Route::CHUNK + 1).min(r.pts.len())).unwrap_or(&[]);
            for (j, w) in pts.windows(2).enumerate() {
                if let [a, b] = *w
                    && !crate::course::is_gap(r.gaps, from + j + 1)
                {
                    let (a, b) = ((a.0 + o.0, a.1 + o.1), (b.0 + o.0, b.1 + o.1));
                    segment(&mut c, k, v.px(area, a), v.px(area, b), COURSE);
                }
            }
        }
    }
    for w in track.points().windows(2) {
        if let [a, b] = *w {
            segment(&mut c, k, v.px(area, a), v.px(area, b), ui::ACCENT);
        }
    }
    scale_bar(&mut c, k, area, &v);
    match Arrow::of(&v, area, track) {
        Some(a) if a.stale => {
            // the last known position: a grey dot with a black outline, no direction
            run(&mut c, k, a.x - 5, a.y - 5, 11, 11, ui::BLACK);
            run(&mut c, k, a.x - 3, a.y - 3, 7, 7, ui::GREY);
        }
        Some(a) => {
            triangle(&mut c, k, a.corners(ARROW + 3), ui::BLACK);
            triangle(&mut c, k, a.corners(ARROW), ui::WHITE);
        }
        None => {}
    }
    let [plus, minus] = buttons(area).map_or([None, None], |b| b.map(Some));
    let centre = centre_button(area).filter(|_| manual);
    for (i, b) in [plus, minus, centre].into_iter().enumerate() {
        let Some((x, y, s)) = b.map(|b| (b.0, b.1, b.2)) else { continue };
        // a black square, a white frame and a white bar (and its upright for "+"), 4 pixels thick
        run(&mut c, k, x, y, s, s, ui::BLACK);
        for (fx, fy, fw, fh) in [(0, 0, s, 2), (0, s - 2, s, 2), (0, 0, 2, s), (s - 2, 0, 2, s)] {
            run(&mut c, k, x + fx, y + fy, fw, fh, ui::WHITE);
        }
        match i {
            0 | 1 => run(&mut c, k, x + 10, y + s / 2 - 2, s - 20, 4, ui::WHITE),
            _ => {
                // a crosshair: a ring of 12 pixels' radius and four ticks through it, the middle left open
                let (cx, cy) = (x + s / 2, y + s / 2);
                for dy in -12..12 {
                    let w = isqrt(144 - (dy * 2 + 1) * (dy * 2 + 1) / 4);
                    let inner = isqrt((81 - (dy * 2 + 1) * (dy * 2 + 1) / 4).max(0));
                    if inner > 0 {
                        run(&mut c, k, cx - w, cy + dy, w - inner, 1, ui::WHITE);
                        run(&mut c, k, cx + inner, cy + dy, w - inner, 1, ui::WHITE);
                    } else {
                        run(&mut c, k, cx - w, cy + dy, 2 * w, 1, ui::WHITE);
                    }
                }
                for (tx, ty, tw, th) in [(-16, -1, 10, 2), (6, -1, 10, 2), (-1, -16, 2, 10), (-1, 6, 2, 10)] {
                    run(&mut c, k, cx + tx, cy + ty, tw, th, ui::WHITE);
                }
            }
        }
        if i == 0 {
            run(&mut c, k, x + s / 2 - 2, y + 10, 4, s - 20, ui::WHITE);
        }
    }
}

/// The integer square root of `n` (0 for less than 1), for the crosshair's ring.
fn isqrt(n: i32) -> i32 {
    (0..=n.max(0)).take_while(|r| r * r <= n).last().unwrap_or(0)
}

/// The view must change: the first position, the position near the edge, the track thinned, another course, the
/// zoom changed (`zoom`; None: the fit) or panned, zoomed in and following the arrow out of the middle part. The first fix after
/// the last known position keeps the view when it falls inside it (no new basemap).
pub fn refit(area: Rect, track: &Track, shown: &Shown, route: Option<&Route>, zoom: Option<Zoom>) -> bool {
    let Some(v) = shown.view else { return track.pos.is_some() };
    if shown.thinned != track.thinned || shown.route != route.map(|r| r.id) {
        return true;
    }
    let centre = zoom.and_then(|z| z.centre);
    match zoomed(area, View::fit(track, area, route), zoom) {
        Some(m) if !shown.zoomed || v.mpp16 != m || shown.panned != centre.is_some() => true,
        Some(_) => centre.map_or(!v.follows(area, track), |c| c != (v.cx, v.cy)),
        None => shown.zoomed || !v.keeps(area, track),
    }
}

/// Brings the map in `area` on the panel `c` from `shown` to the track now: the whole map when the view must
/// change ([`refit`], at `zoom`; the caller draws the basemap for [`Shown::of`] first), else only the box holding the new
/// segments and the arrow's old and new places, its background copied from `base`.
pub fn update(
    c: &mut dyn Canvas,
    area: Rect,
    track: &Track,
    shown: &mut Shown,
    base: Option<&Basemap>,
    route: Option<&Route>,
    zoom: Option<Zoom>,
) {
    let refit = refit(area, track, shown, route, zoom);
    let Some(v) = shown.view.filter(|_| !refit) else {
        if refit {
            *shown = Shown::of(track, area, route, zoom);
            draw(c, area, track, shown.view, base, route, shown.zoomed);
        }
        return;
    };
    let mut dirty = None;
    for w in track.points().get(shown.points.saturating_sub(1)..).unwrap_or(&[]).windows(2) {
        let [a, b] = *w else { continue };
        let (a, b) = (v.px(area, a), v.px(area, b));
        let seg = (a.0.min(b.0), a.1.min(b.1), (a.0 - b.0).abs() + 2, (a.1 - b.1).abs() + 2);
        dirty = Some(union(dirty, seg));
    }
    let arrow = Arrow::of(&v, area, track);
    if arrow != shown.arrow {
        for r in [shown.arrow, arrow].into_iter().flatten().map(|a| a.rect()) {
            dirty = Some(union(dirty, r));
        }
    }
    if let Some(d) = dirty {
        draw(&mut Clip::new(c, d), area, track, shown.view, base, route, shown.zoomed);
    }
    shown.points = track.len;
    shown.arrow = arrow;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every zoom step changes the scale bar's number the way the zoom went, the bar the same length (the owner: the
    /// labels changed erratically, a zoom in could read as a zoom out).
    #[test]
    fn scale_bar_numbers_follow_the_zoom() {
        extern crate std;
        let said = |level: u32| {
            let mut l = Line::<8>::new();
            push_distance(&mut l, (i64::from(SCALE_PX) * mpp16(level) / 16) as u32);
            std::string::String::from(l.as_str())
        };
        let m = |level: u32| SCALE_PX as i64 * mpp16(level) / 16;
        for level in MIN_LEVEL..MAX_LEVEL {
            assert!(m(level + 1) > m(level), "level {level}");
            assert_ne!(said(level + 1), said(level), "level {level}: the same label twice");
        }
        assert_eq!(
            (MIN_LEVEL..MIN_LEVEL + 6).map(said).collect::<std::vec::Vec<_>>(),
            ["75 M", "100 M", "150 M", "200 M", "300 M", "400 M"]
        );
    }

    #[test]
    fn projects_metres() {
        let mut t = Track::new();
        t.add(450_000_000, 60_000_000, 0);
        t.add(450_000_000 + 8983, 60_000_000, 0); // 8983e-7 degrees of latitude: 100 m
        assert_eq!(t.pos(), Some((0, 99)));
        t.add(450_000_000, 60_000_000 + 12_704, 0); // at 45 degrees, 100 m east
        let (x, _) = t.pos().unwrap();
        assert!((99..=101).contains(&x), "{x}");
    }

    /// A long straight ride: never more than POINTS points, thinned to twice the spacing each time it fills, the
    /// first and last points kept close to the ride's ends.
    #[test]
    fn thins_when_full() {
        let mut t = Track::new();
        for i in 0..20_000 {
            t.add(450_000_000 + i * 900, 60_000_000, 0); // 10 m north per fix: 200 km
        }
        assert!(t.points().len() <= POINTS && t.points().len() > POINTS / 2);
        assert_eq!(t.points()[0], (0, 0));
        let last = t.points().last().unwrap().1;
        assert!(t.pos().unwrap().1 - last <= t.step_m, "the last point is within a step of the position");
        assert!(t.points().windows(2).all(|w| w[1].1 - w[0].1 >= t.step_m - 2), "evenly spaced");
        assert_eq!(1 << t.thinned, t.step_m / FIRST_STEP_M);
    }

    /// The fitted view puts every point of the track inside the map, away from the edge.
    #[test]
    fn fits_the_track() {
        let mut t = Track::new();
        for i in 0..300 {
            t.add(450_000_000 + i * 900, 60_000_000 + i * 600, 0);
        }
        let area = (0, 24, 240, 296);
        let v = View::fit(&t, area, None).unwrap();
        for &p in t.points() {
            let (x, y) = v.px(area, p);
            assert!(
                (MARGIN..240 - MARGIN).contains(&x) && (24 + MARGIN..320 - MARGIN).contains(&y),
                "{p:?} at {x},{y}"
            );
        }
        assert!(v.keeps(area, &t));
        assert_eq!(View::fit(&Track::new(), area, None), None);
    }

    /// "+" and "-" step one level from the one shown, within MIN_LEVEL (1.5 m) and MAX_LEVEL, whatever the fit.
    #[test]
    fn zoom_levels() {
        let at = |mpp16| Shown { view: Some(View { cx: 0, cy: 0, mpp16 }), ..Shown::default() };
        assert_eq!(mpp16(MIN_LEVEL), 24, "1.5 m a pixel");
        assert_eq!(zoom(&at(32), true), Some(1), "2 m: 1.5 m");
        assert_eq!(zoom(&at(32), false), Some(3), "2 m: 3 m");
        assert_eq!(zoom(&at(24), true), Some(MIN_LEVEL));
        assert_eq!(zoom(&at(mpp16(MAX_LEVEL)), false), Some(MAX_LEVEL));
        assert_eq!(zoom(&Shown::default(), true), None);
    }

    /// The three buttons are 48 x 48 with 8 pixels or more between them, inside the map (UI review 2, P11: 40 pixels
    /// with 4 between, two meanings side by side), and a tap in a gap is no button's.
    #[test]
    fn buttons_are_big_and_apart() {
        let area = (0, 24, 240, 296);
        let [plus, minus] = buttons(area).unwrap();
        let centre = centre_button(area).unwrap();
        let all = [plus, minus, centre];
        assert!(all.iter().all(|r| r.2 == 48 && r.3 == 48 && r.0 + r.2 <= 240 && r.1 + r.3 <= 24 + 296));
        assert!(all.windows(2).all(|w| w[1].1 - (w[0].1 + w[0].3) >= 8), "{all:?}");
        let shown = Shown { zoomed: true, view: Some(View { cx: 0, cy: 0, mpp16: 16 }), ..Shown::default() };
        let at = |r: Rect, dy| button_at(area, &shown, r.0 + 10, r.1 + dy);
        assert_eq!(
            [at(plus, 10), at(minus, 10), at(centre, 10)],
            [Some(Button::In), Some(Button::Out), Some(Button::Centre)]
        );
        assert_eq!([at(plus, 48 + 4), at(minus, 48 + 4)], [None, None], "the gaps");
    }

    /// Fitted, the arrow never sits under the zoom buttons (review of the zoom: riding north-east at 10 m/s it was
    /// under one for 8096 of 20000 s), and a fresh fit does not ask for another at once.
    #[test]
    fn arrow_clear_of_the_buttons() {
        let area = (0, 24, 240, 296);
        let [plus, minus] = buttons(area).unwrap();
        let mut t = Track::new();
        let mut shown = Shown::default();
        let (mut under, mut refits) = (0, 0);
        for s in 0..20_000 {
            t.add(450_000_000 + s * 635, 60_000_000 + s * 898, 4_500_000); // 7.1 m north and east a second
            if refit(area, &t, &shown, None, None) {
                shown = Shown::of(&t, area, None, None);
                refits += 1;
                assert!(!refit(area, &t, &shown, None, None), "a fresh fit keeps at {s} s");
            }
            let (x, y) = shown.view.unwrap().px(area, t.pos().unwrap());
            let inside = |r: Rect| (r.0..r.0 + r.2).contains(&x) && (r.1..r.1 + r.3).contains(&y);
            under += u32::from(inside(plus) || inside(minus));
        }
        assert_eq!(under, 0, "seconds with the arrow under a button");
        // 104, one every 3 minutes on this worst case (straight towards the buttons): the track fits 118 pixels wide,
        // not 134, to stay clear of the 48 pixel buttons (56 with 40 pixel ones); 23 when the arrow could go under
        // them
        assert!(refits <= 110, "{refits} fits");
    }
}
