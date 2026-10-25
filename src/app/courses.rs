//! The course followed: the course memory (from main.rs), the list read from the card, the course loaded and
//! followed, its events (off course, back on, climbs) said and beeped, and its fields (distance and ascent left).

use crate::*;

impl App {
    // -----------------------------------------------------------------------------------------------------------
    // Courses (storage/course.rs: the files; course.rs: the model, where the rider is, the climbs)
    // -----------------------------------------------------------------------------------------------------------

    /// Memory for the courses (main.rs keeps it in AXI SRAM: about 70 KB); without it there are none.
    pub fn set_course_memory(&mut self, mem: &'static mut storage::course::Courses) {
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
        let Some(c) = self.courses.as_deref_mut() else { return self.say("NO COURSE") };
        if c.open(&mut emmc::Volume(&mut self.emmc, b)).is_err() || c.list.is_empty() {
            self.say("NO COURSE");
        }
        self.open(Screen::Menu(Menu::Courses, 0));
    }

    /// Select on course `k`: reads it to follow it ([`App::step_courses`]), or stops following it.
    pub(crate) fn follow_course(&mut self, k: usize) {
        let Some(c) = self.courses.as_deref_mut() else { return };
        // another map (the course in or out of the fit): fitted until "+", not a zoom from before that waited for
        // the fit to grow (review)
        (self.map_zoom, self.map_centre) = (None, None);
        if c.on.is_some() && c.on == c.list.get(k).map(|e| e.file) {
            c.on = None;
            return self.say("COURSE OFF");
        }
        c.load(k);
        self.say("READING COURSE");
    }

    /// Bytes of course files read per pass: as much as the scan of stock's rides while no ride runs (about 40 ms
    /// on the unit), ride or not, since someone waits for it.
    const COURSE_BYTES: u32 = Self::SCAN_IDLE;

    /// The course files go on being read, a bounded amount per pass: the course to follow (then COURSE ON and the
    /// first ride page, or NOT A COURSE), and the lengths of the list while it shows.
    #[inline(never)]
    pub(crate) fn step_courses(&mut self, b: &mut impl Board) {
        let Some(c) = self.courses.as_deref_mut() else { return };
        if c.measuring() && !matches!(self.screen, Screen::Menu(Menu::Courses, _)) {
            return; // the lengths only while the list shows
        }
        match c.step(&mut emmc::Volume(&mut self.emmc, b), Self::COURSE_BYTES) {
            storage::course::Progress::Loaded(true) => {
                self.say("COURSE ON");
                // from the list, the course page (as stock's route details lead to riding); a rider who has gone
                // elsewhere while it was read stays there
                if matches!(self.screen, Screen::Menu(Menu::Courses, _)) {
                    (self.depth, self.screen) = (0, Screen::Ride(COURSE));
                    self.show(b);
                }
            }
            storage::course::Progress::Loaded(false) => self.say("NOT A COURSE"),
            _ => {}
        }
    }

    /// The beeps of a climb starting.
    pub(crate) const CLIMB_TONES: [(u32, u32); 1] = [(2000, 300)];

    /// Shows and sounds what following the course said: off course (banner and the alert's beeps), back on it, a
    /// climb starting ("CLIMB 3.4 KM 7%", a beep).
    pub(crate) fn course_event(&mut self, ev: course::Event) {
        match ev {
            course::Event::Off => {
                self.say("OFF COURSE");
                self.tones.play(&BEEP_ALERT);
            }
            course::Event::Back => self.say("BACK ON COURSE"),
            course::Event::Climb(k) => {
                let mut l = Line::<20>::new();
                l.push_str("CLIMB ");
                ui::card::push_km(&mut l, k.len_m);
                l.push_str(" KM ");
                l.push_u32(k.grade_x10().div_ceil(10).max(1));
                l.push_str("%");
                self.say(l.as_str());
                self.tones.play(&Self::CLIMB_TONES);
            }
        }
    }

    /// The course fields' text: "--" without a course followed and a fix on it.
    pub(crate) fn course_text(&self, f: Field) -> Line<8> {
        let mut l = Line::new();
        let Some((c, at)) = self.on_course() else {
            l.push_str("--");
            return l;
        };
        let d = at.along_m;
        let climb = c.climb_at(d).and_then(|(k, on)| Some((*c.climbs().get(k)?, on)));
        match (f, climb) {
            (Field::ToGo, _) => ui::card::push_km(&mut l, c.len_m().saturating_sub(d)),
            (Field::OffCourse, _) => l.push_u32(at.off_m.unwrap_or(0).min(9999)),
            (Field::UpToGo, _) => l.push_u32(c.ascent_left(at)),
            // on a climb: what is left of it and the grade ahead; before it: its length and grade
            (Field::Climb, Some((k, on))) => {
                let (m, g) = match on {
                    true => (k.end_m().saturating_sub(d), c.grade_ahead(d).unwrap_or(0)),
                    false => (k.len_m, k.grade_x10() as i32),
                };
                if m < 10_000 {
                    ui::card::push_km(&mut l, m);
                } else {
                    l.push_u32(m / 1000);
                }
                l.push_str("K ");
                if g < 0 {
                    l.push_str("-");
                }
                l.push_u32((g.unsigned_abs() + 5) / 10);
                l.push_str("%");
            }
            (Field::ClimbGain, Some((k, true))) => {
                let top = c.alt_at(k.end_m()).zip(c.alt_at(d)).map_or(0, |(t, h)| (t - h).max(0) / 10);
                l.push_u32(top as u32);
            }
            (Field::ClimbGain, Some((k, false))) => l.push_u32(k.gain_m),
            _ => l.push_str("--"),
        }
        l
    }

    /// A course field's colour: OFF COURSE red once off course was said, CLIMB by its grade; None for the others.
    pub(crate) fn course_color(&self, f: Field) -> Option<u16> {
        let (c, at) = self.on_course()?;
        match f {
            Field::OffCourse if at.off_course() => Some(ui::RED),
            Field::Climb => {
                let (k, on) = c.climb_at(at.along_m)?;
                let g = match on {
                    true => c.grade_ahead(at.along_m)?,
                    false => c.climbs().get(k)?.grade_x10() as i32,
                };
                Some(ui::grade_color(g))
            }
            _ => None,
        }
    }

    /// A course's menu row: its name, its length; green when followed.
    pub(crate) fn course_row(&self, k: usize, label: &mut Line<16>, value: &mut Line<12>) -> Option<u16> {
        let c = self.courses.as_deref()?;
        let e = c.list.get(k)?;
        label.push_str(e.name());
        match e.dist_m {
            _ if !e.measured => value.push_str("..."),
            Some(m) => {
                ui::card::push_km(value, m.saturating_add(50)); // to the nearest 100 m
                value.push_str(" KM");
            }
            None => value.push_str("--"),
        }
        Some(if c.on == Some(e.file) { ui::GREEN } else { ui::GREY })
    }
}
