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
        (self.map_zoom, self.map_centre, self.map_whole) = (None, None, false);
        if c.on.is_some() && c.on == c.list.get(k).map(|e| e.file) {
            return self.stop_course();
        }
        c.load(k);
        self.say("READING COURSE");
    }

    /// Stops following the course (its row in COURSES, STOP FOLLOWING in the ride menu); the map fits again.
    pub(crate) fn stop_course(&mut self) {
        let Some(c) = self.courses.as_deref_mut().filter(|c| c.on.is_some()) else { return };
        c.on = None;
        (self.map_zoom, self.map_centre, self.map_whole) = (None, None, false);
        self.say("COURSE OFF");
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
            storage::course_file::Progress::Loaded(true) => {
                self.say("COURSE ON");
                // from the list, the course page (as stock's route details lead to riding); a rider who has gone
                // elsewhere while it was read stays there
                if matches!(self.screen, Screen::Menu(Menu::Courses, _)) {
                    (self.depth, self.screen) = (0, Screen::Ride(COURSE));
                    self.show(b);
                }
            }
            storage::course_file::Progress::Loaded(false) => self.say("NOT A COURSE"),
            _ => {}
        }
    }

    /// The beeps of a climb starting.
    pub(crate) const CLIMB_TONES: [(u32, u32); 1] = [(2000, 300)];

    /// Shows and sounds what following the course said: off course (banner and the alert's beeps), back on it, a
    /// climb starting ("CLIMB 3.4 KM 7%", a beep).
    pub(crate) fn course_event(&mut self, ev: course::Event) {
        // seen whatever the screen: the light comes on as for a key, even after its time-out
        self.light.idle_ms = 0;
        match ev {
            course::Event::Off => {
                self.alarm("OFF COURSE");
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
        let Some((c, at)) = self.on_course() else { return self.ahead_text(f) };
        let d = at.along_m;
        let climb = c.climb_at(d).and_then(|(k, on)| Some((*c.climbs().get(k)?, on)));
        match (f, climb) {
            (Field::NextTurn, _) => match c.next_cue(at) {
                Some((cue, m)) => {
                    l.push_str(cue.turn.glyph());
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
                // no turn left before the end: said, not "--" as off course
                None if c.cues_done(at) => l.push_str("END"),
                None => l.push_str("--"),
            },
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
                l.push_str("K|"); // the grade small after the length: the box's scale for the length
                push_grade(&mut l, g);
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
                ui::card::push_km(value, m.saturating_add(50)); // to the nearest 100 m
                // a long name is whole: the unit goes first, then the distance (the label is cut before the value)
                let whole =
                    |v: &Line<12>| ui::LABEL.width(label.as_str()) <= ui::menu_label_w(label.as_str(), v.as_str());
                let mut with_unit = *value;
                with_unit.push_str(" KM");
                if whole(&with_unit) {
                    *value = with_unit;
                } else if !whole(value) {
                    *value = Line::new();
                }
            }
            None => value.push_str("--"),
        }
        Some(if c.on == Some(e.file) { ui::GREEN } else { ui::WHITE })
    }
}
