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
/// Zoom levels: level `i` is [`mpp16`]`(i)` / 16 metres per pixel; 2 m to about 1000 km.
const MIN_LEVEL: u32 = 2;
const MAX_LEVEL: u32 = 40;
/// The view is fitted again when the position comes this close to the map's edge (pixels).
const MARGIN: i32 = 20;
/// The arrow's length from its centre to its tip (pixels).
const ARROW: i32 = 9;
/// Scale bar lengths offered, metres; the longest that fits a third of the map's width is shown.
const NICE_M: [i32; 15] =
    [10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10_000, 20_000, 50_000, 100_000, 200_000, 500_000];
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
        let (fw, fh) = (i64::from(area.2 - 2 * MARGIN) * 2 / 3, i64::from(area.3 - 2 * MARGIN) * 2 / 3);
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
        let mpp16 = (MIN_LEVEL..MAX_LEVEL).map(mpp16).find(|&m| fits(m)).unwrap_or(mpp16(MAX_LEVEL));
        Some(View { cx: (i64::from(min.0) + sx / 2) as i32, cy: (i64::from(min.1) + sy / 2) as i32, mpp16 })
    }

    /// Where point `p` (metres) falls on screen.
    pub(crate) fn px(&self, area: Rect, p: (i32, i32)) -> (i32, i32) {
        let px = |d: i32| (i64::from(d) * 16).div_euclid(self.mpp16) as i32;
        (area.0 + area.2 / 2 + px(p.0 - self.cx), area.1 + area.3 / 2 - px(p.1 - self.cy))
    }

    /// The position is still well inside the map: no need to fit again.
    fn keeps(&self, area: Rect, track: &Track) -> bool {
        let Some(p) = track.pos else { return true };
        let (x, y) = self.px(area, p);
        x >= area.0 + MARGIN && x < area.0 + area.2 - MARGIN && y >= area.1 + MARGIN && y < area.1 + area.3 - MARGIN
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
        let dir = (track.heading.rem_euclid(36_000_000) + 1_125_000) / 2_250_000 % 16;
        Some(Arrow { x, y, dir: dir as usize, stale: track.stale })
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
}

impl Shown {
    /// The map of `track` (and `route`) in `area` as [`draw`] will show it when drawn whole now.
    pub fn of(track: &Track, area: Rect, route: Option<&Route>) -> Shown {
        let view = View::fit(track, area, route);
        let arrow = view.and_then(|v| Arrow::of(&v, area, track));
        Shown { view, points: track.len, thinned: track.thinned, arrow, route: route.map(|r| r.id) }
    }
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

/// The scale bar, bottom left: a bar of a round length and its label above it.
fn scale_bar(c: &mut impl Canvas, k: Rect, area: Rect, view: &View) {
    let len = |m: i32| (i64::from(m) * 16 / view.mpp16) as i32;
    let Some(&m) = NICE_M.iter().rev().find(|&&m| len(m) <= area.2 / 3) else { return };
    let len = len(m);
    let (x, y) = (area.0 + 8, area.1 + area.3 - 8);
    // a black outline first: white alone vanishes on IGN's light map
    run(c, k, x - 1, y - 3, len + 2, 4, ui::BLACK);
    run(c, k, x - 1, y - 9, 4, 8, ui::BLACK);
    run(c, k, x + len - 3, y - 9, 4, 8, ui::BLACK);
    run(c, k, x, y - 2, len, 2, ui::WHITE);
    run(c, k, x, y - 8, 2, 6, ui::WHITE);
    run(c, k, x + len - 2, y - 8, 2, 6, ui::WHITE);
    let mut l = Line::<8>::new();
    if m >= 1000 {
        l.push_u32(m as u32 / 1000);
        l.push_str(" KM");
    } else {
        l.push_u32(m as u32);
        l.push_str(" M");
    }
    let ty = y - 10 - LABEL.height() as i32;
    if x >= 0 && ty >= 0 {
        text::draw(c, &LABEL, x as u32, ty as u32, Align::Left, l.as_str());
    }
}

/// Draws the map of `track` in `area` as `view` shows it (None: "NO FIX"): the background (the basemap from
/// `base`'s buffer, else black), the course (`route`), the track, the scale bar and the arrow, in that order. Only
/// the part `c` keeps ([`Canvas::clip`]) costs anything: of the course, only the chunks whose box it touches.
pub fn draw(
    c: &mut dyn Canvas,
    area: Rect,
    track: &Track,
    view: Option<View>,
    base: Option<&Basemap>,
    route: Option<&Route>,
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
            for w in pts.windows(2) {
                if let [a, b] = *w {
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
}

/// The view must change: the first position, the position near the edge, the track thinned, another course. The
/// first fix after the last known position keeps the view when it falls inside it (no new basemap).
pub fn refit(area: Rect, track: &Track, shown: &Shown, route: Option<&Route>) -> bool {
    match shown.view {
        None => track.pos.is_some(),
        Some(v) => shown.thinned != track.thinned || !v.keeps(area, track) || shown.route != route.map(|r| r.id),
    }
}

/// Brings the map in `area` on the panel `c` from `shown` to the track now: the whole map when the view must
/// change ([`refit`]; the caller draws the basemap for [`Shown::of`] first), else only the box holding the new
/// segments and the arrow's old and new places, its background copied from `base`.
pub fn update(
    c: &mut dyn Canvas,
    area: Rect,
    track: &Track,
    shown: &mut Shown,
    base: Option<&Basemap>,
    route: Option<&Route>,
) {
    let refit = refit(area, track, shown, route);
    let Some(v) = shown.view.filter(|_| !refit) else {
        if refit {
            *shown = Shown::of(track, area, route);
            draw(c, area, track, shown.view, base, route);
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
        draw(&mut Clip::new(c, d), area, track, shown.view, base, route);
    }
    shown.points = track.len;
    shown.arrow = arrow;
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
