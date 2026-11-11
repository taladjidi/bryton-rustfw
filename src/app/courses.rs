//! The course followed: the course memory (from main.rs), the list read from the card, the course loaded and
//! followed, its events (off course, back on, climbs, turn-by-turn cues) said and beeped, and its fields (distance
//! and ascent left, the next turn).

use crate::*;

/// The grade (% x 10) after a climb field's "K|", whole percent, with its "%" when the 8 characters hold it ("1.2K|-10"
/// at -10 %: the sign and the number are what count).
pub(crate) fn push_grade(l: &mut Line<8>, g: i32) {
    if g < 0 {
        l.push_str("-");
    }
    l.push_u32((g.unsigned_abs() + 5) / 10);
    if l.as_str().len() < 8 {
        l.push_str("%");
    }
}

/// The climb fields (a course's and the road ahead's alike): on a climb, what is left of it and the grade ahead
/// ("--" when the profile has no sample there); before it, its length and grade. `ClimbGain` is the rise left or the
/// whole gain. `grade_ahead` and `alt_at` are the source's profile.
pub(crate) fn climb_text(
    l: &mut Line<8>,
    f: Field,
    k: crate::course::Climb,
    on: bool,
    d: u32,
    grade_ahead: impl Fn(u32) -> Option<i32>,
    alt_at: impl Fn(u32) -> Option<i32>,
) {
    if f == Field::ClimbGain {
        if on {
            let top = alt_at(k.end_m()).zip(alt_at(d)).map_or(0, |(t, h)| (t - h).max(0) / 10);
            l.push_u32(top as u32);
        } else {
            l.push_u32(k.gain_m);
        }
        return;
    }
    let m = if on { k.end_m().saturating_sub(d) } else { k.len_m };
    if m < 10_000 {
        crate::push_trip(l, m, false);
    } else {
        l.push_u32(m / 1000);
    }
    l.push_str("K|"); // the grade small after the length: the box's scale for the length
    match if on { grade_ahead(d) } else { Some(k.grade_x10() as i32) } {
        Some(g) => push_grade(l, g),
        None => l.push_str("--"),
    }
}

impl App {
    // -----------------------------------------------------------------------------------------------------------
    // Courses (storage/course_file.rs: the files; course.rs: the model, where the rider is, the climbs)
    // -----------------------------------------------------------------------------------------------------------

    /// Memory for the courses (main.rs keeps it in AXI SRAM: about 70 KB); without it there are none.
    pub fn set_course_memory(&mut self, mem: &'static mut storage::course_file::Courses) {
        self.courses = Some(mem);
    }

    /// The course followed, as the map draws it (once the map has a plane: a fix, or the last position known).
    pub(crate) fn route(&self) -> Option<map::Route<'_>> {
        let c = self.courses.as_deref().filter(|c| c.on.is_some())?;
        c.course.route(self.track.origin()?)
    }

    /// The course followed and where the rider is on it (placed by a fix at least once).
    pub(crate) fn on_course(&self) -> Option<(&course::Course, &course::Tracker)> {
        let c = self.courses.as_deref().filter(|c| c.on.is_some() && c.at.off_m.is_some())?;
        Some((&c.course, &c.at))
    }

    /// COURSES: the course files on the card, listed from their folders; each one's length is read while the list
    /// shows ([`App::step_courses`]).
    #[inline(never)]
    pub(crate) fn open_courses(&mut self, b: &mut impl Board) {
        let Some(c) = self.courses.as_deref_mut() else { return self.say("NO COURSE\nSEND ONE FROM APP") };
        c.ride_it = false;
        if c.open(&mut emmc::Volume(&mut self.emmc, b)).is_err() || c.list.is_empty() {
            self.say("NO COURSE\nSEND ONE FROM APP");
        }
        self.open(Screen::Menu(Menu::Courses, 0));
    }

    /// The course list again once the phone wrote or deleted a course ([`storage::course_file::Courses::stale`]),
    /// unless a course is being read to follow (its place in the list finds it): lengths measured only while COURSES
    /// shows (a measure pending elsewhere would hold the steps after it in [`App::step_courses`]), and its cursor
    /// kept within a list that shrank.
    pub(crate) fn relist_courses(&mut self, b: &mut impl Board) {
        let shown = matches!(self.screen, Screen::Menu(Menu::Courses, _));
        let lent = self.emmc.lent();
        let Some(c) = self.courses.as_deref_mut().filter(|c| c.stale && !c.loading() && !lent) else { return };
        let d = &mut emmc::Volume(&mut self.emmc, b);
        let _ = if shown { c.open(d) } else { c.relist(d) };
        if let Screen::Menu(Menu::Courses, row) = self.screen {
            let last = self.rows(Menu::Courses).saturating_sub(1);
            self.screen = Screen::Menu(Menu::Courses, row.min(last));
            self.show(b);
        }
    }

    /// Select on course `k`: reads it to follow it ([`App::step_courses`]), or stops following it.
    pub(crate) fn follow_course(&mut self, k: usize) {
        let Some(c) = self.courses.as_deref_mut() else { return };
        // another map (the course in or out of the fit): fitted until "+", not a zoom from before that waited for
        // the fit to grow (review)
        (self.map_view.zoom, self.map_view.centre, self.map_view.whole) = (None, None, false);
        if c.on.is_some() && c.on == c.list.get(k).map(|e| e.file) && !c.ride_it {
            return self.stop_course();
        }
        c.base_m = None;
        c.load(k);
        self.say("READING COURSE");
    }

    /// Backing out of COURSES ends a pending RIDE A COURSE: the next course followed is an ordinary one.
    pub(crate) fn drop_ride_it(&mut self) {
        if let (Screen::Menu(Menu::Courses, _), Some(c)) = (self.screen, self.courses.as_deref_mut()) {
            c.ride_it = false;
        }
    }

    /// RIDE A COURSE (WORKOUTS): lists the courses; the one picked is ridden on the trainer ([`App::begin_virtual`]).
    pub(crate) fn pick_course(&mut self, b: &mut impl Board) {
        if !self.trainer_heard() {
            return self.say("NEEDS TRAINER\nPAIR ONE: SENSORS");
        }
        if self.workout_running() {
            return self.say("WORKOUT RUNNING");
        }
        self.open_courses(b);
        if let Some(c) = self.courses.as_deref_mut() {
            c.ride_it = true;
        }
    }

    /// The course picked is loaded: the ride starts if none runs (indoor, the trainer being heard), and the rider is
    /// placed on the course by the ride's distance (the trainer's speed) from here ([`App::virtual_course_second`]).
    fn begin_virtual(&mut self) {
        self.ftms_retry();
        if self.ride.state() == ride::State::Idle {
            self.start_pause();
        }
        self.choose_profile();
        let dist = self.ride.dist_m();
        if let Some(c) = self.courses.as_deref_mut() {
            (c.at, c.base_m) = (course::Tracker::new(), Some(dist));
        }
    }

    /// Once a second in a virtual ride: the ride's distance, from the trainer's speed, is the place on the course (no fix indoors), and the
    /// course's end says DONE once, with the distance and the time. The ride goes on; the trainer is let go by the
    /// grade (none in the last 20 m).
    pub(crate) fn virtual_course_second(&mut self) {
        // no longer an indoor ride (the trainer silent for TRAINER_QUIET_S, or set by hand): the GPS places the rider
        let (dist, timer) = (self.ride.dist_m(), self.ride.timer_s());
        let off = self.ride.state() == ride::State::Idle || !self.indoors();
        let Some(c) = self.courses.as_deref_mut() else { return };
        let Some(base) = c.base_m else { return };
        if c.on.is_none() || off {
            return c.base_m = None;
        }
        let (len, before) = (c.course.len_m(), c.at.along_m);
        c.at.along_m = dist.saturating_sub(base).min(len);
        c.at.off_m = Some(0);
        if before < len && c.at.along_m == len {
            let mut l = Line::<20>::new();
            l.push_str("DONE ");
            crate::push_trip(&mut l, len, false);
            l.push_str(" KM ");
            push_duration(&mut l, timer, false);
            self.say_as(l.as_str(), Banner::Ride);
            self.tones.play(&Self::STEP_TONE);
        }
    }

    /// Stops following the course (its row in COURSES, STOP FOLLOWING in the ride menu); the map fits again.
    pub(crate) fn stop_course(&mut self) {
        let Some(c) = self.courses.as_deref_mut().filter(|c| c.on.is_some()) else { return };
        (c.on, c.base_m) = (None, None);
        (self.map_view.zoom, self.map_view.centre, self.map_view.whole) = (None, None, false);
        self.say("COURSE OFF");
    }

    /// REVERSE COURSE: the course followed the other way (the rider is placed again by the next fix).
    pub(crate) fn reverse_course(&mut self) {
        let Some(c) = self.courses.as_deref_mut().filter(|c| c.on.is_some()) else { return };
        c.course.reverse();
        c.at = course::Tracker::new();
        (self.map_view.zoom, self.map_view.centre, self.map_view.whole) = (None, None, false);
        self.say("COURSE REVERSED");
    }

    /// BACK TO START: the ride's own track (the map's breadcrumb, already thinned) followed back as a course; it
    /// replaces the course followed. The track is the ride's own (never stock's files).
    #[inline(never)]
    pub(crate) fn back_to_start(&mut self) {
        let (Some(c), Some(o)) = (self.courses.as_deref_mut(), self.track.origin()) else { return };
        if self.track.points().len() < 2 {
            return self.say("NO TRACK YET");
        }
        c.cancel_load(); // a course still being read would be written over the way back
        c.on = None;
        c.course.from_plane(o, self.track.points().iter().rev().copied());
        if c.course.points().len() < 2 {
            return self.say("NO TRACK YET");
        }
        // no file behind it: a cluster no file has
        (c.on, c.at) = (Some(fat::File { cluster: 0, size: 0, dir: false }), course::Tracker::new());
        (self.map_view.zoom, self.map_view.centre, self.map_view.whole) = (None, None, false);
        self.say("BACK TO START");
    }

    /// Bytes of course files read per pass: as much as the scan of stock's rides while no ride runs (about 40 ms
    /// on the unit), and 4 KB in a ride or while its save runs, so a course picked
    /// in a ride never holds the recorder's pass up.
    fn course_bytes(&self) -> u32 {
        // a ride until it is saved (the save and its read-back are the riskiest moment): 4 KB a pass, one chunk
        if self.ride_settled() { Self::SCAN_IDLE } else { 4096 }
    }

    /// The course files go on being read, a bounded amount per pass: the course to follow (then COURSE ON and the
    /// first ride page, or NOT A COURSE), and the lengths of the list while it shows.
    #[inline(never)]
    pub(crate) fn step_courses(&mut self, b: &mut impl Board) {
        let budget = self.course_bytes();
        // in a ride, never in a pass where the recorder has card work (a checkpoint, a start, a save): the course waits
        // a pass, the records do not
        let (rec, riding) = (&self.rec, !self.ride_settled());
        if riding && [&rec.full, &rec.clean].iter().any(|r| r.due() || r.pending()) {
            return;
        }
        let Some(c) = self.courses.as_deref_mut() else { return };
        if c.measuring() && !matches!(self.screen, Screen::Menu(Menu::Courses, _)) {
            return; // the lengths only while the list shows
        }
        let measuring = c.measuring();
        let step = c.step(&mut emmc::Volume(&mut self.emmc, b), budget);
        // a length that came in is drawn in this pass, not at the next throttled list redraw: a screenshot taken
        // between the two disagreed with the panel
        if let (true, Screen::Menu(Menu::Courses, sel)) = (measuring, self.screen) {
            self.update_menu_rows(b, Menu::Courses, sel);
            self.redraw_ms = 0;
        }
        match step {
            storage::course_file::Progress::Loaded(true) => {
                self.say("COURSE ON");
                if self.courses.as_deref_mut().is_some_and(|c| core::mem::take(&mut c.ride_it)) {
                    self.begin_virtual();
                }
                // from the list, the course page (as stock's route details lead to riding); a rider who has gone
                // elsewhere while it was read stays there
                if matches!(self.screen, Screen::Menu(Menu::Courses, _)) {
                    (self.depth, self.screen) = (0, Screen::Ride(COURSE));
                    self.show(b);
                }
            }
            storage::course_file::Progress::Loaded(false) => {
                if let Some(c) = self.courses.as_deref_mut() {
                    c.ride_it = false;
                }
                self.say("NOT A COURSE")
            }
            _ => {}
        }
        if self.courses.as_deref().is_some_and(|c| c.stale && !c.loading()) {
            self.relist_courses(b); // a load that ended: the list it held can be read again
        }
        // the named points along the course: a bounded step of a scan, once the course is on (an extra)
        // never while the recorder has card work (a start, a checkpoint, a save)
        let rec = &self.rec;
        let writing = [&rec.full, &rec.clean].iter().any(|r| r.due() || r.pending());
        if !self.emmc.lent()
            && !writing
            && let (Some(c), Some(p)) = (self.courses.as_deref(), self.pois.as_deref_mut())
            && c.on.is_some()
            && !p.done()
        {
            p.step(&c.course, &mut emmc::Volume(&mut self.emmc, b));
        }
    }

    /// Memory for the named points along a course (main.rs: its own cell, so the course memory's frame stays small);
    /// without it there are none.
    pub fn set_poi_memory(&mut self, mem: &'static mut named::Pois) {
        self.pois = Some(mem);
    }

    /// The named points found along the course followed.
    pub(crate) fn poi_list(&self) -> &[named::Poi] {
        self.pois.as_deref().map_or(&[], |p| p.list())
    }

    /// The next named point of the course followed (a peak or a col) and the distance to it, m: where the rider is
    /// on the course, else from its start.
    pub(crate) fn next_poi(&self) -> Option<(&named::Poi, u32)> {
        let c = self.courses.as_deref().filter(|c| c.on.is_some())?;
        let along = c.at.off_m.map_or(0, |_| c.at.along_m);
        let (_, p) = self.pois.as_deref()?.next(along)?;
        Some((p, p.along_m.saturating_sub(along)))
    }

    /// The beeps of a climb starting.
    pub(crate) const CLIMB_TONES: [(u32, u32); 1] = [(2000, 300)];

    /// Shows and sounds what following the course said: off course (banner and the alert's beeps), back on it, a
    /// climb starting ("CLIMB 3.4 KM 7%", a beep).
    pub(crate) fn course_event(&mut self, ev: course::Event) {
        // the repeat of off course goes to a rider who is riding, not to one stopped or paused (no beep, no light)
        if ev == course::Event::OffAgain && !(self.ride.state() == ride::State::Running && self.ride.moving()) {
            return;
        }
        // seen whatever the screen: the light comes on as for a key, even after its time-out
        self.light.idle_ms = 0;
        match ev {
            course::Event::Off | course::Event::OffAgain => {
                self.alarm("OFF COURSE");
                self.tones.play(&BEEP_ALERT);
            }
            course::Event::Back => self.say("BACK ON COURSE"),
            course::Event::End => {
                let mut l = Line::<20>::new();
                l.push_str("COURSE DONE ");
                crate::push_trip(&mut l, self.courses.as_deref().map_or(0, |c| c.course.len_m()), false);
                l.push_str(" KM");
                self.say_as(l.as_str(), Banner::Ride);
                self.tones.play(&Self::STEP_TONE);
            }
            course::Event::Climb(k) => {
                let mut l = Line::<20>::new();
                l.push_str("CLIMB ");
                crate::push_trip(&mut l, k.len_m, false);
                l.push_str(" KM ");
                l.push_u32(k.grade_x10().div_ceil(10).max(1));
                l.push_str("%");
                self.say_as(l.as_str(), Banner::Ride);
                self.tones.play(&Self::CLIMB_TONES);
            }
            course::Event::Cue(c, d) => {
                let mut l = Line::<20>::new();
                l.push_str(c.turn.glyph());
                l.push_str(" ");
                l.push_str(c.turn.word());
                if d == 0 {
                    l.push_str(" NOW");
                    self.tones.play(&Self::TURN_TONES);
                } else {
                    // to the nearest 50 m, as a voice would say it
                    l.push_str(" ");
                    l.push_u32((d + 25) / 50 * 50);
                    l.push_str(" M");
                    self.tones.play(&Self::CUE_TONES);
                }
                self.say_as(l.as_str(), if d == 0 { Banner::Now } else { Banner::Cue });
            }
        }
    }

    /// The beep of a cue ahead ("LEFT 200 M").
    pub(crate) const CUE_TONES: [(u32, u32); 1] = [(1800, 150)];
    /// The beeps at the turn ("LEFT NOW").
    pub(crate) const TURN_TONES: [(u32, u32); 3] = [(1800, 120), (0, 60), (1800, 120)];

    /// The course fields' text: "--" without a course followed and a fix on it.
    pub(crate) fn course_text(&self, f: Field) -> Line<8> {
        let mut l = Line::new();
        if f == Field::DescentAhead {
            return self.descent_text();
        }
        if f == Field::NextPoi {
            match self.next_poi() {
                Some((_, m)) => crate::map::push_distance(&mut l, m), // "1.7 KM": a bare K reads as thousands
                None => l.push_str("--"),
            }
            return l;
        }
        let Some((c, at)) = self.on_course() else { return self.ahead_text(f) };
        let d = at.along_m;
        let climb = c.climb_at(d).and_then(|(k, on)| Some((*c.climbs().get(k)?, on)));
        match (f, climb) {
            // not on the course yet: the way to its start, a compass point and the distance
            (Field::NextTurn, _) if c.to_start(at).is_some() => {
                let (deg, m) = c.to_start(at).unwrap_or((0, 0));
                match self.gps.pvt.filter(|p| p.good() && p.speed_mm_s >= crate::app::fields::HEADING_MIN_MM_S) {
                    // an arrow relative to the way the rider faces (heading in degrees x 1e-5)
                    Some(p) => l.push_str(Self::turn_toward(deg, p.heading).glyph()),
                    None => l.push_str(
                        ["N", "NE", "E", "SE", "S", "SW", "W", "NW"].get((deg as usize + 22) / 45 % 8).unwrap_or(&"N"),
                    ),
                }
                push_dist(&mut l, m);
            }
            (Field::NextTurn, _) => match c.next_cue(at) {
                Some((cue, m)) => {
                    l.push_str(cue.turn.glyph());
                    push_dist(&mut l, m);
                }
                // no turn left before the end: said, not "--" as off course
                None if c.cues_done(at) => l.push_str("END"),
                None => l.push_str("--"),
            },
            (Field::ToGo, _) => crate::push_trip(&mut l, c.len_m().saturating_sub(d), false),
            (Field::OffCourse, _) => l.push_u32(at.off_m.unwrap_or(0).min(9999)),
            (Field::UpToGo, _) => l.push_u32(c.ascent_left(at)),
            (Field::Climb | Field::ClimbGain, Some((k, on))) => {
                climb_text(&mut l, f, k, on, d, |m| c.grade_ahead(m), |m| c.alt_at(m));
            }
            _ => l.push_str("--"),
        }
        l
    }

    /// The turn that faces bearing `deg` for a rider heading `heading_e5` (degrees x 1e-5).
    pub(crate) fn turn_toward(deg: u32, heading_e5: i32) -> course::Turn {
        use course::Turn::*;
        let rel = (i64::from(deg) * 100_000 - i64::from(heading_e5)).rem_euclid(36_000_000) as u32;
        match (rel + 1_125_000) / 4_500_000 % 8 {
            1 => SlightRight,
            2 => Right,
            3 => SharpRight,
            4 => UTurn,
            5 => SharpLeft,
            6 => Left,
            7 => SlightLeft,
            _ => Straight,
        }
    }

    /// A course field's colour: OFF COURSE red once off course was said (an alarm); None for the others.
    pub(crate) fn course_color(&self, f: Field) -> Option<u16> {
        let (_, at) = self.on_course()?;
        (f == Field::OffCourse && at.off_course()).then_some(ui::RED)
    }

    /// CLIMB's edge bar, by the grade of the climb (the one running, else the next); BLACK elsewhere.
    pub(crate) fn course_bar(&self, f: Field) -> u16 {
        let Some((c, at)) = self.on_course().filter(|_| f == Field::Climb) else { return self.ahead_bar(f) };
        let grade = c.climb_at(at.along_m).and_then(|(k, on)| match on {
            true => c.grade_ahead(at.along_m),
            false => c.climbs().get(k).map(|k| k.grade_x10() as i32),
        });
        grade.map_or(ui::BLACK, ui::grade_color)
    }

    /// A course's menu row: its name, its length; green when followed.
    pub(crate) fn course_row(&self, k: usize, label: &mut Line<16>, value: &mut Line<12>) -> Option<u16> {
        let c = self.courses.as_deref()?;
        let e = c.list.get(k)?;
        label.push_str(e.name());
        match e.dist_m {
            _ if !e.measured => value.push_str("..."),
            Some(m) => {
                // the distance with its unit stays whole, at the right (the owner's choice); the name is cut to the room
                // left (menu_label_w), so a long course name loses its end, never its length
                crate::push_trip(value, m.saturating_add(50), false); // to the nearest 100 m
                value.push_str(" KM");
            }
            None => value.push_str("--"),
        }
        // a name that repeats says what tells it apart: the file's kind, else its place among the same names
        let same = |o: &&storage::course_file::Entry| o.name() == e.name();
        let all = (0..c.list.len()).filter_map(|i| c.list.get(i));
        if all.clone().filter(same).count() > 1 {
            if all.clone().filter(same).any(|o| o.gpx != e.gpx) {
                value.push_str(if e.gpx { " GPX" } else { " FIT" });
            } else {
                value.push_str(" #");
                value.push_u32(1 + all.take(k).filter(same).count() as u32);
            }
        }
        Some(if c.on == Some(e.file) { ui::GREEN } else { ui::WHITE })
    }
}

/// A distance as the next turn's field shows it: 10 m steps under a kilometre, then km.
pub(crate) fn push_dist(l: &mut Line<8>, m: u32) {
    match m {
        0..1000 => {
            l.push_u32(m / 10 * 10);
            l.push_str("M");
        }
        1000..100_000 => {
            l.push_u32(m / 1000);
            l.push_str(".");
            l.push_u32(m / 100 % 10);
            l.push_str("K");
        }
        _ => {
            l.push_u32((m / 1000).min(9999));
            l.push_str("K");
        }
    }
}
