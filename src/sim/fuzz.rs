//! Layer 4 of the testing strategy (AGENTS.md): random bytes through every parser and decoder that takes bytes
//! from outside (the Nordic chip, the GNSS receiver). A panic there would freeze the unit.

extern crate std;
use std::vec::Vec;
/// xorshift32: deterministic, so a failure reproduces.
fn bytes(seed: u32, n: usize) -> impl Iterator<Item = u8> {
    let mut x = seed;
    (0..n).map(move |_| {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        // lengths and markers more often than chance, so the parsers get past their headers
        match x % 16 {
            0 => 0xb5,
            1 => 0x62,
            2 => 44,
            3 => 0,
            _ => (x >> 8) as u8,
        }
    })
}

#[test]
fn nordic_packets() {
    let mut rx = crate::nrf::Rx::new();
    // random bytes (the stream out of step), then frames built on purpose: a length header and an advertising
    // report, an ANT event (broadcast, acknowledged, channel closed) or random bytes; random bytes alone never
    // make a 44-byte ANT event or an advertising report (code review: the decoders were never reached)
    let mut reached = [0usize; 3];
    let look = |p: &[u8], reached: &mut [usize; 3]| {
        let _ = crate::nrf::event_id(p);
        reached[0] += usize::from(crate::nrf::ant_broadcast(p).is_some());
        reached[1] += usize::from(crate::nrf::ant_closed(p).is_some());
        if let Some(r) = crate::nrf::adv_report(p) {
            reached[2] += 1;
            let _ = (crate::nrf::ad_name(r.data), crate::nrf::ad_maker(r.data));
        }
    };
    for seed in 1..=4u32 {
        for b in bytes(seed, 1 << 18) {
            if let Some(p) = rx.push(b) {
                look(p, &mut reached);
            }
        }
        // a fresh receiver: the random bytes above leave it waiting for a length of up to 64 KB
        let mut rx = crate::nrf::Rx::new();
        let mut v = bytes(seed + 100, 1 << 20);
        let mut r = || v.next().unwrap_or(0);
        for _ in 0..4000 {
            let mut p = std::vec::Vec::new();
            match r() % 4 {
                0 => {
                    // BLE_GAP_EVT_ADV_REPORT: [2][1d 00][conn][peer 7][direct 7][rssi][flags][dlen][present][data]
                    p.extend([2, 0x1d, 0]);
                    p.extend((0..20).map(|_| r())); // up to present, index 22: the data from 23
                    let n = usize::from(r() % 40);
                    p[21] = n as u8;
                    let mut data = std::vec::Vec::new();
                    while data.len() < n {
                        let len = 1 + r() % 12;
                        data.push(len);
                        data.push([0x01, 0x09, 0x08, 0xff, 0x03][usize::from(r() % 5)]);
                        data.extend((1..len).map(|_| r()));
                    }
                    data.truncate(n + usize::from(r() % 3)); // sometimes longer than dlen says
                    p.extend(data);
                }
                1 | 2 => {
                    // ANT event, 44 bytes: a broadcast / acknowledged message, or a channel event
                    p.push(8);
                    p.extend((0..43).map(|_| r()));
                    p[2] = [0x4e, 0x4f, 0x40, r()][usize::from(r() % 4)];
                    if r() % 3 == 0 {
                        p[42] = 0x07; // EVENT_CHANNEL_CLOSED
                    }
                    if r() % 8 == 0 {
                        p.truncate(usize::from(r() % 44)); // a short one
                    }
                }
                _ => p.extend((0..r() % 64).map(|_| r())),
            }
            let n = p.len() as u16;
            for b in n.to_le_bytes().into_iter().chain(p) {
                if let Some(q) = rx.push(b) {
                    look(q, &mut reached);
                }
            }
        }
    }
    assert!(reached.iter().all(|&n| n > 100), "broadcasts, closed channels, adv reports reached: {reached:?}");
}

/// Random heart-rate broadcasts into the beat decoder, and its intervals and random ones into the HRV: no panic,
/// every interval in range.
#[test]
fn heart_rate_beats() {
    use crate::sensors::{Beats, Decoders, Kind, Live};
    for seed in 1..=4u32 {
        let (mut dec, mut live, mut hrv) = (Decoders::default(), Live::default(), crate::hrv::Hrv::default());
        let v: std::vec::Vec<u8> = bytes(seed, 1 << 18).collect();
        for c in v.chunks_exact(8) {
            let d = core::array::from_fn(|i| c[i]);
            dec.decode(Kind::Hr, d, 2105, &mut live);
            for &rr in dec.beats.rr() {
                assert!(Beats::RANGE.contains(&rr));
                hrv.push(rr);
            }
            hrv.push(u16::from_le_bytes([d[0], d[1]]));
            let _ = (hrv.alpha1(), hrv.rmssd());
        }
    }
}

/// Random broadcasts into every ANT decoder (speed, cadence, both, power, heart rate, the two shifting ones), with
/// restarts: no panic (a u32 overflow did panic here), speeds and cadences within what a bike does. Half the
/// messages carry a page the shifting decoders read (random bytes would rarely), and the test checks they were
/// reached: gears, battery, button presses, within what the formats can say.
#[test]
fn ant_decoders() {
    use crate::sensors::{Decoders, Kind, Live, Metric, SHIFT_STATUS};
    let (mut gears, mut batteries, mut presses) = (0, 0, 0);
    for seed in 5..=8u32 {
        let (mut dec, mut live) = (Decoders::default(), Live::default());
        let v: std::vec::Vec<u8> = bytes(seed, 1 << 18).collect();
        for (n, c) in v.chunks_exact(9).enumerate() {
            let kind = Kind::ALL[usize::from(c[8]) % Kind::ALL.len()];
            if n % 97 == 0 {
                dec.restart(kind);
            }
            let mut d: [u8; 8] = core::array::from_fn(|i| c[i]);
            if c[8] & 0x80 != 0 {
                d[0] = [0, 1, 4, 0x52][usize::from(c[8] >> 3) % 4];
            }
            live.tick(1000); // values age out within 5 messages
            dec.decode(kind, d, 2105, &mut live);
            assert!(live.get(Metric::Speed).is_none_or(|s| s <= 2_700), "{:?}", live.get(Metric::Speed));
            if let Some(g) = live.get(Metric::Gear) {
                assert!(g & 0xff > 0, "rear from 1 (the front 0 for a 1x): {g:x}");
                gears += 1;
            }
            if let Some(b) = live.get(Metric::ShiftBattery) {
                assert!(b <= 100 || (SHIFT_STATUS + 1..=SHIFT_STATUS + 5).contains(&b), "battery {b:x}");
                batteries += 1;
            }
            presses += core::mem::take(&mut dec.press).iter().flatten().count();
        }
    }
    assert!(gears > 1000 && batteries > 1000 && presses > 100, "reached: {gears} {batteries} {presses}");
}

/// Random bytes from the computer, with command words mixed in, into the dev link protocol.
#[test]
fn dev_link_input() {
    use crate::dev::{Session, Stats};
    let mut sim = super::Sim::new(9, 0, 0);
    let app = crate::App::start(&mut sim, None);
    let words: [&[u8]; 12] = [
        b"peek ",
        b"poke ",
        b"load ",
        b"key ",
        b"0x4",
        b"\n",
        b"shot\n",
        b"bat\n",
        b"sector ",
        b"emmc\n",
        b"bench map ",
        b" -",
    ];
    let mut maps = 0;
    for seed in 1..=4u32 {
        let (mut session, mut pc) = (Session::new(), super::Pc::default());
        let mut buf = std::vec::Vec::new();
        for (k, b) in bytes(seed, 1 << 16).enumerate() {
            if b % 5 == 0 {
                buf.extend_from_slice(words[usize::from(b) % words.len()]);
            } else {
                buf.push(b);
            }
            if k % 64 == 63 {
                maps += usize::from(matches!(
                    session.feed(&buf, &app, Stats::default(), &mut pc),
                    crate::dev::Effect::BenchMap(..)
                ));
                buf.clear();
                pc.got.clear();
            }
        }
    }
    // `bench map` lines by structure (random bytes almost never make one): numbers valid, negative, huge, broken
    let tokens: [&str; 8] = ["7", "-250", "99", "0x10", "-", "4294967296", "-2147483648", "60"];
    let (mut session, mut pc) = (Session::new(), super::Pc::default());
    for c in bytes(9, 3 << 12).collect::<std::vec::Vec<_>>().chunks_exact(3) {
        let t = |b: u8| tokens[usize::from(b) % tokens.len()];
        let line = std::format!("bench map {} {} {}\n", t(c[0]), t(c[1]), t(c[2]));
        if let crate::dev::Effect::BenchMap(level, ..) = session.feed(line.as_bytes(), &app, Stats::default(), &mut pc)
        {
            assert!(level <= 60);
            maps += 1;
        }
        pc.got.clear();
    }
    assert!(maps > 0, "bench map reached");
}

/// FIT files from the card (stock's rides): random bytes behind a valid header, and every kind of damage to a
/// valid ride, through the reader (with the position, as a past ride's card reads it), the power curve, the
/// history's totals and the track.
#[test]
fn fit_files() {
    use crate::fitread::{Msg, Reader};
    let feed = |data: &mut dyn Iterator<Item = u8>| {
        let (mut r, mut roll) = (Reader::with_position(), crate::power_curve::Rolling::new());
        let (mut acc, mut track) = (crate::history::Acc::new(), crate::map::Track::new());
        for b in data {
            let Some(m) = r.push(b) else { continue };
            acc.take(&m);
            if let Msg::Record(x) = m {
                roll.sample(x.time, x.power);
                if let Some((lat, lon)) = x.pos.filter(|p| p.0.unsigned_abs() <= 1 << 30) {
                    track.add(lat / 2, lon / 2, 0); // about degrees x 1e-7, as the card converts them
                }
            }
        }
        let file = crate::fat::File { cluster: 0, size: 0, dir: false };
        if let Some(ride) = acc.ride(file, (2026, 1, 1, 0, 0)) {
            let _ = crate::ui::card::Card::of(&ride.stats(250));
        }
        let _ = crate::map::View::fit_whole(&track, (0, 0, 240, 110));
        let _ = crate::power_curve::fit_cp(&crate::power_curve::Curve { watts: roll.best, day: [0; 13] });
    };
    // random messages behind a valid header; a new file each time the reader gives up, so it gets past the
    // first message (most random bytes are a data message on no definition)
    let head = [14u8, 0x10, 0, 0, 0xff, 0x0f, 0, 0, b'.', b'F', b'I', b'T', 0, 0];
    let (mut files, mut deep) = (0, 0);
    for seed in 1..=4u32 {
        let mut r = Reader::new();
        head.iter().for_each(|&b| _ = r.push(b));
        for (i, b) in bytes(seed, 1 << 20).enumerate() {
            // definitions more often than chance
            let _ = r.push(if i % 7 == 0 { 0x40 | (b & 0x2f) } else { b });
            if r.result().is_some() {
                files += 1;
                deep += usize::from(r.offset() > 64);
                r = Reader::new();
                head.iter().for_each(|&b| _ = r.push(b));
            }
        }
    }
    assert!(files > 1000 && deep > 100, "{files} files, {deep} past 64 bytes");
    let mut ride = Vec::new();
    let (mut w, _) = crate::fit::Writer::start(&mut ride, 1_000_000).unwrap();
    for t in 0..300u32 {
        let s = crate::fit::Sample { time: 1_000_001 + t, power: Some(t as u16), hr: Some(120), ..Default::default() };
        w.record(&mut ride, &s).unwrap();
    }
    w.finish(&mut ride, 1_000_400).unwrap();
    let mut noise = bytes(9, 1 << 16);
    for _ in 0..2000 {
        let mut bad = ride.clone();
        for _ in 0..1 + noise.next().unwrap_or(0) % 4 {
            let at = (usize::from(noise.next().unwrap_or(0)) << 8 | usize::from(noise.next().unwrap_or(0))) % bad.len();
            bad[at] = noise.next().unwrap_or(0);
        }
        feed(&mut bad.into_iter());
    }
}

/// Workout files (storage/workout.rs): a good file with bytes changed, and random messages, each behind a good
/// header and with its CRC made right, so the decoder (not the CRC check) gets them.
#[test]
fn workout_files() {
    use crate::storage::workout::{STEPS, parse, tests::intervals};
    let crc = |f: &mut Vec<u8>| {
        let n = f.len() - 2;
        let c = crate::fit::crc(0, &f[..n]);
        f[n..].copy_from_slice(&c.to_le_bytes());
    };
    let (mut ok, mut noise) = (0, bytes(11, 1 << 20));
    let good = intervals();
    for _ in 0..20_000 {
        let mut bad = good.clone();
        for _ in 0..1 + noise.next().unwrap_or(0) % 6 {
            let at = 14 + usize::from(noise.next().unwrap_or(0)) % (bad.len() - 16);
            bad[at] = noise.next().unwrap_or(0);
        }
        crc(&mut bad);
        if let Ok(w) = parse(&bad) {
            assert!(w.steps().len() <= STEPS);
            ok += 1;
        }
    }
    assert!(ok > 1000, "{ok} still workouts");
    for seed in 1..=4u32 {
        for len in [8usize, 64, 600, 4000] {
            let body: Vec<u8> = bytes(seed + len as u32, len)
                .enumerate()
                .map(|(i, b)| if i % 9 == 0 { 0x40 | (b & 0x2f) } else { b })
                .collect();
            let mut f = good[..14].to_vec();
            f[4..8].copy_from_slice(&(len as u32).to_le_bytes());
            let head = crate::fit::crc(0, &f[..12]);
            f[12..14].copy_from_slice(&head.to_le_bytes());
            f.extend(body);
            f.extend([0, 0]);
            crc(&mut f);
            let _ = parse(&f);
            let _ = parse(&f[..f.len() / 2]);
        }
    }
}

/// Course files (storage/course.rs into course.rs): the synthetic card's hill as Bryton's route, a Garmin course
/// and GPX, the zigzag as a FIT course with course points and as GPX, with bytes changed (the FIT ones with their
/// CRC made right, so the reader gets past it), and random text of GPX's own marks, cue elements and turn words
/// among them; each read into a course (its cues placed, or its turns found), which is then followed from random
/// places (near it and anywhere on the globe) and along its own points (so cues are said), asked for its profile,
/// climbs, ascent and next cue, and drawn on a map. The fuzz reaches the cues: course points and GPX cues read,
/// cues kept, cues said (counted over the seeds).
#[test]
fn course_files() {
    use crate::course::{Course, Event, Item, Tracker, Turn};
    use crate::fitread::{Msg, Reader};
    use crate::storage::course::Gpx;
    let mut d = crate::fat::tests::Ram(crate::fat::tests::image(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let paths =
        ["PlanTrip/Hill.fit", "Courses/Hill_Garmin.fit", "Courses/Hill.gpx", "Courses/Turns.fit", "Courses/Zigzag.gpx"];
    let files = paths.map(|p| crate::fat::tests::read_all(&mut fat, &mut d, p).unwrap());
    let mut c = std::boxed::Box::new(Course::new());
    // what was reached: cues read from files, cues kept, cues said
    let (read_cues, mut kept, mut said) = (core::cell::Cell::new(0u32), 0u32, 0u32);
    // the file's points as read (degrees x 1e-7), to ride along
    let mut way: Vec<(i32, i32)> = Vec::new();
    let mut read = |f: &[u8], gpx: bool, c: &mut Course, way: &mut Vec<(i32, i32)>| {
        c.clear();
        way.clear();
        let mut take = |it: Item| {
            match it {
                Item::Point(p) => way.push((p.lat, p.lon)),
                Item::Cue(..) | Item::CueHere(_) => read_cues.set(read_cues.get() + 1),
            }
            c.take(it);
        };
        if gpx {
            Gpx::new().push_slice(f, &mut take);
        } else {
            Reader::with_position().push_slice(f, |m| match m {
                Msg::Record(r) => {
                    if let Some((lat, lon)) = r.pos {
                        let (lat, lon) = (crate::history::degrees_e7(lat), crate::history::degrees_e7(lon));
                        let dist_m = r.dist_cm.map(|cm| cm / 100);
                        take(Item::Point(crate::course::Pt { lat, lon, alt_dm: r.alt_dm, split: false, dist_m }));
                    }
                }
                Msg::CoursePoint { pos: Some((lat, lon)), kind: Some(k), dist_cm } => {
                    let (lat, lon) = (crate::history::degrees_e7(lat), crate::history::degrees_e7(lon));
                    take(Item::Cue(lat, lon, Turn::from_fit(k), dist_cm.map(|cm| cm / 100)));
                }
                _ => {}
            });
        }
        c.finish();
        kept += c.cues().len() as u32;
        assert!(c.cues().windows(2).all(|w| w[0].at_m <= w[1].at_m), "cues in order");
        assert!(c.cues().iter().all(|q| q.at_m <= c.len_m()), "cues on the course");
    };
    let mut noise = bytes(21, 1 << 24);
    let mut next = move || u32::from_le_bytes([0; 4].map(|_| noise.next().unwrap_or(0)));
    let mut exercise = |c: &Course, next: &mut dyn FnMut() -> u32, way: &[(i32, i32)]| {
        let mut t = Tracker::new();
        for &(lat, lon) in way.iter().step_by(2) {
            if let Some(Event::Cue(..)) = c.follow(&mut t, lat, lon) {
                said += 1;
            }
            let _ = c.next_cue(&t);
        }
        for k in 0..40 {
            let (lat, lon) = if k % 4 == 3 {
                (next() as i32, next() as i32) // anywhere, or off the globe
            } else {
                ((next() % 400_000) as i32 - 200_000, (next() % 400_000) as i32 - 200_000) // within 2 km
            };
            let _ = c.follow(&mut t, lat, lon);
            let _ = (c.grade_ahead(t.along_m), c.ascent_left(&t), c.climb_at(t.along_m), c.alt_at(next()));
        }
        for k in 0..64 {
            let _ = c.column(next() % 10_000, next() % 10_000, 64, k);
        }
        let mut f = crate::ui::tests::Frame::new();
        let mut track = crate::map::Track::new();
        track.add(0, 0, 0);
        let route = c.route((0, 0));
        let view = crate::map::View::fit(&track, (0, 24, 240, 296), route.as_ref());
        crate::map::draw(&mut f, (0, 24, 240, 296), &track, view, None, route.as_ref(), false);
    };
    for (i, good) in files.iter().enumerate() {
        let gpx = paths[i].ends_with(".gpx");
        for _ in 0..200 {
            let mut bad = good.clone();
            for _ in 0..1 + next() % 8 {
                let at = 14 + next() as usize % (bad.len() - 16);
                bad[at] = next() as u8;
            }
            if !gpx {
                let n = bad.len() - 2;
                let crc = crate::fit::crc(0, &bad[..n]);
                bad[n..].copy_from_slice(&crc.to_le_bytes());
            }
            read(&bad, gpx, &mut c, &mut way);
            exercise(&c, &mut next, &way);
            read(&bad[..bad.len() / 3], gpx, &mut c, &mut way);
        }
    }
    const MARKS: [&str; 33] = [
        "<trkpt", "<rtept", " lat=\"", " lon='", "\"", "'", ">", "/>", "</trkpt>", "<ele>", "</ele>", "-", ".", "1",
        "9", " ", "<!--", "=", "999999", // long digit runs (decimal's overflow)
        "<wpt", "</wpt>", "<sym>", "</sym>", "<type>", "</type>", "<name>", "</name>", "Left", "TSHR", "u-turn",
        "Slight", " right", "WATER",
    ];
    // a whole point or waypoint now and then, often with a cue element, so cues have a course to go on
    let pt = |next: &mut dyn FnMut() -> u32| {
        let (lat, lon) = (next() % 20_000, next() % 20_000);
        let tag = ["trkpt", "rtept", "wpt"][next() as usize % 3];
        let el = ["sym", "type", "name"][next() as usize % 3];
        let word = ["Left", "TSHR", "u-turn", "slight right", "Cafe", "WATER"][next() as usize % 6];
        std::format!("<{tag} lat=\"0.{lat:07}\" lon=\"0.{lon:07}\"><{el}>{word}</{el}></{tag}>")
    };
    let from_files = read_cues.get();
    for _ in 0..8 {
        let mut text = Vec::new();
        while text.len() < 64 * 1024 {
            if next() % 8 == 0 {
                text.extend_from_slice(pt(&mut next).as_bytes());
            }
            text.extend_from_slice(MARKS[next() as usize % MARKS.len()].as_bytes());
        }
        read(&text, true, &mut c, &mut way);
        exercise(&c, &mut next, &way);
    }
    let gpx = read_cues.get() - from_files;
    std::println!("course fuzz: {from_files} cues read from the files, {gpx} from GPX marks, {kept} kept, {said} said");
    assert!(from_files > 1000 && gpx > 500 && kept > 1000 && said > 100, "{from_files}, {gpx}, {kept}, {said}");
}

/// Random shapes into the turn finder (course/cues.rs): random walks of random steps (1 to 300 m, and the same
/// point again), random turns (a little, a lot, back), gaps between segments, long enough to be thinned; the
/// turns found are in order and on the course, and some are found.
#[test]
fn turn_finder_shapes() {
    use crate::course::{Course, Pt};
    let mut noise = bytes(33, 1 << 22);
    let mut next = move || u32::from_le_bytes([0; 4].map(|_| noise.next().unwrap_or(0)));
    let mut c = std::boxed::Box::new(Course::new());
    let mut found = 0;
    for _ in 0..60 {
        c.clear();
        let (mut x, mut y, mut heading) = (0.0f64, 0.0f64, 0.0f64);
        for _ in 0..next() % 8000 {
            heading += match next() % 4 {
                0 => f64::from(next() % 360).to_radians(),
                1 => core::f64::consts::PI,
                _ => (f64::from(next() % 21) - 10.0).to_radians(),
            };
            let step = if next() % 50 == 0 { 0.0 } else { f64::from(1 + next() % 300) };
            (x, y) = (x + step * heading.cos(), y + step * heading.sin());
            let (lat, lon) = ((y / 0.011_132) as i32, (x / 0.011_132) as i32);
            c.add(Pt { lat, lon, alt_dm: None, split: next() % 500 == 0, dist_m: None });
        }
        c.finish();
        let cues = c.cues();
        assert!(cues.len() <= crate::course::CUES && cues.windows(2).all(|w| w[0].at_m <= w[1].at_m));
        assert!(cues.iter().all(|q| q.at_m <= c.len_m() && q.turn.is_turn()), "{cues:?}");
        found += cues.len();
    }
    assert!(found > 1000, "turns found: {found}");
}

/// Stock's settings files: random text with the INI's own sections, keys and characters mixed in, through every
/// part of the import (the profile, the sensors, DeviceData's bike and backlight, the bike's pages and reminders):
/// no panic, and what comes out is in range (a page's layout holds its fields, the reminders on the menu's steps).
#[test]
fn ini_files() {
    const SECTIONS: [&str; 12] = [
        "Page1",
        "Page2",
        "Page3",
        "Page4",
        "UserProfile",
        "DeviceSys",
        "Reminder1",
        "Reminder2",
        "Reminder6",
        "FTP",
        "MHR",
        "Sensor1",
    ];
    const KEYS: [&str; 20] = [
        "type",
        "isEnabled",
        "name",
        "Grid1",
        "Grid4",
        "Grid6",
        "Grid9",
        "ActBike",
        "brightnessAuto",
        "status",
        "WhichReminder",
        "ReminderFreq",
        "ReminderType",
        "Timer",
        "HR",
        "BaseVale",
        "CP",
        "Weight_kg",
        "ID",
        "Wheel",
    ];
    let mut reached = [0usize; 2];
    for seed in 1..=2000u32 {
        // many short files (a lookup takes a section's first copy), of sections and key = value lines (an INI's structure), each value one of: a small number, a list of field
        // ids, a huge number, raw bytes
        let mut t = Vec::new();
        let mut v = bytes(seed, 600);
        while let (Some(a), Some(b), Some(c)) = (v.next(), v.next(), v.next()) {
            if a % 5 == 0 {
                t.extend_from_slice(std::format!("[{}]\r\n", SECTIONS[usize::from(b) % SECTIONS.len()]).as_bytes());
                continue;
            }
            t.extend_from_slice(KEYS[usize::from(b) % KEYS.len()].as_bytes());
            t.push(b'=');
            match a % 4 {
                0 => t.extend_from_slice(std::format!("{}", c % 8).as_bytes()),
                1 => {
                    let ids: Vec<_> = (0..1 + c % 12).map(|_| std::format!("{}", v.next().unwrap_or(0))).collect();
                    t.extend_from_slice(ids.join(",").as_bytes());
                }
                2 => t.extend_from_slice(
                    std::format!("{}", u32::from(c).wrapping_mul(16_843_009).wrapping_add(1000)).as_bytes(),
                ),
                _ => t.extend((0..c % 16).map(|_| v.next().unwrap_or(0))),
            }
            t.extend_from_slice(b"\r\n");
        }
        let mut i = crate::stock::import(&t, &t, &t);
        assert!(i.device(&t) <= 2, "ActBike: one of stock's three bikes");
        for p in i.pages.iter().flatten() {
            assert!(p.well_formed());
        }
        for m in [i.eat_min, i.drink_min].into_iter().flatten() {
            assert!((5..=120).contains(&m) && m % 5 == 0, "{m}");
        }
        assert!(i.hr_alert.is_none_or(|h| (100..=230).contains(&h)));
        reached[0] += i.pages.iter().flatten().count();
        reached[1] += [i.eat_min, i.drink_min, i.hr_alert].iter().flatten().count();
    }
    std::println!("ini fuzz reached {} pages, {} reminders", reached[0], reached[1]);
    assert!(reached[0] > 0 && reached[1] > 0, "the fuzz reaches the pages and the reminders");
}

/// Touch sampled inside USB drive commands (app/drive.rs `Touching`): random reads and writes from the computer
/// while a random finger comes and goes, through the app on the simulated board: no panic, and a gesture found
/// inside a command is handed over once (the next sample finds none waiting).
#[test]
fn touch_during_drive_commands() {
    use super::*;
    use crate::msc::tests::{Host, cbw, rw10};
    use crate::ui::UsbDot;
    use std::{cell::Cell, rc::Rc};
    for seed in 1..=4u32 {
        let noise: Rc<Vec<u8>> = Rc::new(bytes(seed, 1 << 12).collect());
        let k = Rc::new(Cell::new(0usize));
        let (n, kk) = (noise.clone(), k.clone());
        let mut sim = Sim::new(9, 0, 0).finger(move |_| {
            let i = kk.get();
            kk.set(i + 1);
            let b = n.get(i % n.len()).copied().unwrap_or(0);
            (b % 4 != 0).then_some((u16::from(b) % 240, u16::from(b.wrapping_mul(7)) % 320))
        });
        sim.emmc.image = crate::fat::tests::image();
        sim.own_settings();
        let mut app = App::start(&mut sim, None);
        app.set_usb(&mut sim, UsbDot::Ready);
        assert!(app.drive_on(&mut sim).is_ok());
        let mut h = Host::default();
        let mut gestures = 0;
        for (tag, c) in noise.chunks_exact(4).take(200).enumerate() {
            let (lba, count) = (u32::from(c[0]) * 8, u16::from(c[1] % 64) + 1);
            let read = c[2] % 4 != 0;
            let op = if read { 0x28 } else { 0x2a };
            if !read {
                h.out = (0..count).map(|_| std::vec![c[3]; 512]).collect();
            }
            let len = u32::from(count) * 512;
            app.drive_packet(&mut sim, &cbw(tag as u32, len, read, &rw10(op, lba, count)), &mut h);
            h.got.clear();
            gestures += usize::from(app.touch(&mut sim, 8).is_some());
            assert!(app.touch_pending.is_none(), "handed over at the first sample after the command");
        }
        assert!(gestures > 0, "seed {seed}: gestures found inside commands");
    }
}

/// Random taps on the map page while riding, most on or near the zoom buttons (map.rs), and drags of random
/// vectors through the touch path, with the ride going on between them: no panic, the zoom level in range, a pan
/// kept near the track, the panel equal to the screenshot, and the buttons and the pan reached.
#[test]
fn map_zoom_taps() {
    use super::*;
    use crate::map::{MAX_LEVEL, MIN_LEVEL};
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.gnss.route = Some(super::tests::tiles_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    sim.gnss.fix = Some(500);
    app.press(&mut sim, Press::Short(Key::Start));
    let mut noise = bytes(7, 1 << 12);
    let mut next = || noise.next().unwrap_or(0);
    let (mut changes, mut pans) = (0, 0);
    for k in 0..400 {
        // half the taps in the buttons' corner (x 180..240, y 0..170: "+", "-", "re-centre"), the rest anywhere
        let (a, b) = (u16::from(next()), u16::from(next()));
        let (x, y) = if a % 2 == 0 { (180 + a % 60, b % 170) } else { (a % 240, (b + a) % 320) };
        let before = (app.map_zoom, app.map_centre);
        if k % 3 == 0 {
            // a drag: a swipe changes the page while the map is fitted; back to the map
            let by = (i32::from(next()) - 128, i32::from(next()) - 128);
            super::tests::drag(&mut sim, &mut app, (i32::from(x % 240), i32::from(30 + y % 280)), by);
            if app.screen() != Screen::Ride(2) {
                app.screen = Screen::Ride(2);
                app.show(&mut sim);
            }
            pans += u32::from(app.map_centre.is_some() && app.map_centre != before.1);
        } else {
            app.input(&mut sim, Input::Touch(Gesture::Tap(x, y)));
        }
        changes += u32::from(app.map_zoom != before.0);
        if let (Some(c), Some(v)) = (app.map_centre, app.map_view(0)) {
            let p = app.track.pos().unwrap_or_default();
            let far = 3 * 296 * v.mpp16 as i32 / 16 + 100_000; // PAN_SCREENS heights from the track (100 km)
            assert!((c.0 - p.0).abs() < far && (c.1 - p.1).abs() < far, "{c:?} {v:?}");
        }
        assert!(app.map_zoom.is_none_or(|l| (MIN_LEVEL..=MAX_LEVEL).contains(&l)), "{:?}", app.map_zoom);
        assert!(app.map_view(0).is_none_or(|v| v.mpp16 >= 24), "no finer than 1.5 m a pixel");
        for _ in 0..usize::from(next() % 8) {
            app.step(&mut sim);
            sim.delay_ms(POLL_MS);
        }
        if k % 50 == 0 {
            super::tests::assert_consistent(&sim, &app);
        }
    }
    assert_eq!(app.screen(), Screen::Ride(2), "taps on the map leave the page");
    assert!(changes > 20, "the buttons reached: {changes} zoom changes");
    assert!(pans > 10, "the pan reached: {pans} pans");
}

/// A damaged volume under the scan of stock's rides: it ends (done or failed), never hangs or panics.
#[test]
fn ride_scan_on_a_damaged_volume() {
    let img = crate::fat::tests::image();
    let mut noise = bytes(5, 1 << 20);
    let mut scan = crate::stock::Scan::new();
    for _ in 0..40 {
        let mut bad = img.clone();
        for _ in 0..64 {
            let hi = usize::from(noise.next().unwrap_or(0)) << 16;
            let at = (hi | usize::from(noise.next().unwrap_or(0)) << 8 | usize::from(noise.next().unwrap_or(0)))
                % bad.len().min(4 << 20);
            bad[at] = noise.next().unwrap_or(0);
        }
        let mut d = crate::fat::tests::Ram(bad, 0);
        scan.start((2026, 10, 23));
        let mut passes = 0;
        while scan.step(&mut d, 1 << 16) {
            passes += 1;
            assert!(passes < 100_000, "the scan does not end");
        }
        // and each ride it found opened as a card (the volume as damaged)
        let mut past = crate::history::Past::new();
        for i in 0..scan.index.len() {
            let r = *scan.index.get(i).unwrap();
            past.start(&r, 250);
            for _ in 0..1000 {
                if past.step(&mut d, 1 << 16).is_some() {
                    break;
                }
            }
            assert!(!past.reading(), "a card is read in 64 MB at most");
        }
    }
}

/// Random and damaged caches of the ride scan (`\Rustfw\scan.bin`, storage/stock.rs `Cache`) on the synthetic
/// volume: entries of random bytes (some with a plausible date, so they are counted), written whole (a right CRC:
/// taken, and counted at once) or then damaged on the card (refused whole). Nothing panics, and once the scan has
/// listed the rides, what it shows is what the rides are (random entries name no ride: dropped).
#[test]
fn scan_caches() {
    use crate::stock::{ENTRY, Scan, Status, Summary};
    let img = crate::fat::tests::image();
    let today = (2026, 10, 23);
    let mut plain = Scan::new();
    plain.start(today);
    while plain.step(&mut crate::fat::tests::Ram(img.clone(), 0), 1 << 20) {}
    let want = Summary { bytes: 0, ..plain.out };
    let mut noise = bytes(9, 1 << 24);
    let (mut taken, mut refused) = (0, 0);
    for round in 0..80 {
        let mut d = crate::fat::tests::Ram(img.clone(), 0);
        let c = crate::stock::tests::cache();
        let n = usize::from(noise.next().unwrap_or(0)) % 48;
        for i in 0..n {
            let mut e = [0u8; ENTRY];
            e.iter_mut().for_each(|b| *b = noise.next().unwrap_or(0));
            if i % 2 == 0 {
                // a ride of 2020 to 2027, counted, in or out of the window
                let y = 2020 + u16::from(e[8] % 8);
                e[8..10].copy_from_slice(&y.to_le_bytes());
                (e[10], e[11], e[14]) = (1 + e[10] % 12, 1 + e[11] % 28, 1);
            }
            c.push_raw(&e);
        }
        c.write(&mut d, (0, 0)).unwrap();
        if round % 2 == 1 {
            let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
            let f = fat.open(&mut d, "Rustfw/scan.bin").unwrap();
            let at = fat.lba(f.cluster) as usize * crate::fat::SECTOR;
            for _ in 0..1 + usize::from(noise.next().unwrap_or(0)) % 4 {
                let k = (usize::from(noise.next().unwrap_or(0)) << 8 | usize::from(noise.next().unwrap_or(0)))
                    % f.size as usize;
                d.0[at + k] ^= 1 << (noise.next().unwrap_or(0) % 8);
            }
        }
        let mut s = Scan::new();
        s.set_cache(crate::stock::tests::cache());
        s.load(&mut d, today);
        if s.ready() {
            taken += 1;
            assert_eq!(s.cache().map(|c| c.len()), Some(n), "taken whole");
        } else {
            refused += 1;
            assert_eq!(s.cache().map(|c| c.len()), Some(0), "refused whole");
        }
        s.start(today);
        let mut passes = 0;
        while s.step(&mut d, 1 << 12) {
            passes += 1;
            assert!(passes < 10_000, "the scan does not end");
        }
        assert_eq!((s.status(), Summary { bytes: 0, ..s.out }), (Status::Done, want), "round {round}");
        assert_eq!(s.cache().map(|c| c.len()), Some(6), "the rides only");
    }
    assert!(taken >= 40 && refused >= 30, "the reader reached: {taken} taken, {refused} refused");
}

#[test]
fn ubx_frames() {
    let mut parser = crate::gnss::Parser::new();
    // random bytes, then UBX frames built on purpose (sync, class, id, length, payload, Fletcher checksum): NAV-PVT
    // with random content, other messages, bad checksums, garbage between; random bytes alone never pass the
    // checksum, so the decoder was never reached (code review)
    let mut pvts = 0;
    for seed in 1..=4u32 {
        for b in bytes(seed, 1 << 18) {
            if let Some((c, i, p)) = parser.push(b) {
                let _ = crate::gnss::pvt(c, i, p);
            }
        }
        let mut v = bytes(seed + 200, 1 << 20);
        let mut r = || v.next().unwrap_or(0);
        for _ in 0..3000 {
            let (class, id, len) = match r() % 4 {
                0 | 1 => (0x01, 0x07, 92),
                2 => (0x01, 0x07, usize::from(r())),
                _ => (r(), r(), usize::from(r() % 120)),
            };
            let mut f = std::vec![0xb5, 0x62, class, id, len as u8, (len >> 8) as u8];
            f.extend((0..len).map(|_| r()));
            let (mut a, mut b) = (0u8, 0u8);
            for &x in &f[2..] {
                a = a.wrapping_add(x);
                b = b.wrapping_add(a);
            }
            f.extend([a, b ^ u8::from(r() % 10 == 0)]); // now and then a bad checksum
            f.extend((0..r() % 4).map(|_| r())); // garbage between frames
            for x in f {
                if let Some((c, i, p)) = parser.push(x) {
                    pvts += usize::from(crate::gnss::pvt(c, i, p).is_some());
                }
            }
        }
    }
    assert!(pvts > 1000, "NAV-PVT decoded: {pvts}");
}

/// Random bytes, and real LZ4 blocks with random bytes changed, into the LZ4 decoder and the IGN tile decoder: no
/// panic, never a length past the output.
#[test]
fn lz4_blocks() {
    let mut out = std::vec![0u16; crate::ign::TILE];
    let mut noise = bytes(31, 1 << 22);
    for n in [0usize, 1, 7, 64, 1000, 70_000] {
        for _ in 0..32 {
            let v: Vec<u8> = noise.by_ref().take(n).collect();
            let got = crate::ign::lz4(&mut v.iter().copied(), &mut out[..4096], u16::from);
            assert!(got.is_none_or(|k| k <= 4096));
            let _ = crate::ign::decode(&mut v.iter().copied(), &mut out);
        }
    }
    let tiles: Vec<Vec<u8>> = crate::ign::tests::fixture().into_iter().map(|(_, p)| p).collect();
    let mut next = move || u32::from_le_bytes([0; 4].map(|_| noise.next().unwrap_or(0)));
    for good in &tiles {
        let blob = &good[8208..]; // the first tile (the packer writes it first)
        for _ in 0..200 {
            let mut bad = blob.to_vec();
            for _ in 0..1 + next() % 8 {
                let at = next() as usize % bad.len();
                bad[at] = next() as u8;
            }
            let cut = next() as usize % (bad.len() + 1);
            let _ = crate::ign::decode(&mut bad[..cut].iter().copied(), &mut out);
            let _ = crate::ign::decode(&mut bad.iter().copied(), &mut out);
        }
    }
}

/// The USB drive (msc.rs): random command blocks behind a valid CBW header, and damaged CBWs, with random host
/// lengths, directions and data packets (sometimes none): no panic, a well-formed CSW for every one answered.
#[test]
fn usb_drive_commands() {
    use crate::msc::tests::{Host, cbw, ram};
    use crate::msc::{Drive, PACKET};
    const OPS: [u8; 13] = [0x00, 0x03, 0x12, 0x1a, 0x1b, 0x1e, 0x23, 0x25, 0x28, 0x2a, 0x2f, 0x35, 0x5a];
    for seed in 1..=4u32 {
        let v: Vec<u8> = bytes(seed, 1 << 18).collect();
        let (mut d, mut h, mut m) = (Drive::new(), Host::default(), ram());
        d.insert();
        for (k, c) in v.chunks_exact(24).enumerate() {
            let len = u32::from(u16::from_le_bytes([c[0], c[1]])) % 8192;
            let mut cb = [0u8; 16];
            cb.copy_from_slice(&c[2..18]);
            cb[0] = OPS.get(usize::from(c[2] % 14)).copied().unwrap_or(c[3]);
            let n = 1 + usize::from(c[18] % 16);
            let mut packet = cbw(k as u32, len, c[19] & 1 == 0, &cb[..n]);
            if c[20] % 16 == 0 {
                packet[usize::from(c[21]) % 31] ^= c[22] | 1; // a damaged CBW
            }
            // full packets, and now and then odd sizes off the packet grid (a short one is legal from a host)
            let odd = c[22] % 3 == 0;
            h.out = (0..c[23] % 20)
                .map(|i| std::vec![i; if odd { 1 + usize::from(i) * 37 % PACKET } else { PACKET }])
                .collect();
            h.stalled = false;
            d.packet(&packet, &mut h, &mut m);
            if let Some(csw) = h.got.pop() {
                assert_eq!((&csw[..4], csw.len()), (&b"USBS"[..], 13));
                assert!(csw[12] <= 2);
            }
            h.got.clear();
            if h.stalled {
                d.reset();
            }
        }
        assert_eq!(m.0.len(), 100 * 512, "the disk never grows");
    }
}

/// Random BLE events from the chip (every id the phone link decodes, random bodies) into the decoder and into the
/// app's phone link on the simulated board, which answers its commands: no panic, whatever state the link is driven
/// into (connected, encrypted, discovering on made-up services). And random bytes into the phone formats.
#[test]
fn phone_link_events() {
    use super::*;
    const IDS: [u16; 22] = [
        0x10, 0x11, 0x13, 0x14, 0x19, 0x1a, 0x1b, 0x1f, 0x21, 0x23, 0x01, 0x50, 0x52, 0x55, 0x56, 0x30, 0x32, 0x33,
        0x36, 0x38, 0x39, 0x3b,
    ];
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    for seed in 1..=4u32 {
        let v: Vec<u8> = bytes(seed, 1 << 16).collect();
        for c in v.chunks_exact(48) {
            let id = IDS[usize::from(c[0]) % IDS.len()];
            let mut p = std::vec![2];
            p.extend(id.to_le_bytes());
            p.extend(&c[2..2 + usize::from(c[1]) % 46]);
            if c[47] % 4 == 0 {
                p[3..].iter_mut().take(2).for_each(|b| *b = 0); // connection 0, as the chip's first link
            }
            let _ = crate::ble::event(&p);
            app.phone_event(&mut sim, &p);
        }
        let mut ds = crate::phone::DataSource::default();
        let mut now = crate::phone::NowPlaying::default();
        for c in v.chunks(20) {
            let _ = (ds.push(c), crate::phone::Notif::parse(c), crate::phone::current_time(c));
            let _ = crate::phone::local_offset(c);
            now.update(c);
            let _ = crate::phone::ams_commands(c);
        }
        let mut s = [0u8; 512];
        for (d, x) in s.iter_mut().zip(&v) {
            *d = *x;
        }
        s[..4].copy_from_slice(b"RBL1");
        let _ = crate::phone::Stored::from_sector(&s);
    }
}

/// Keys and gestures in any order through the menus and pages (UI review, steps 4, 5 and 7): taps on the 44 px rows,
/// on the title's "<", swipes, holds, keys, between passes of the main loop. No panic, the selection inside its
/// menu (no wrap past the ends), the panel equal to the screenshot, and each new path reached: a tap that only
/// selects, a tap that acts, the "<" that goes back.
#[test]
fn menu_touch_and_keys() {
    use super::*;
    use crate::touch::Dir;
    let (mut selects, mut acts, mut backs, mut asks, mut off, mut riding) = (0, 0, 0, 0, 0, 0);
    let (mut locks, mut locked_touches) = (0, 0);
    for seed in 1..=12u32 {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8])); // a stand-in ANT+ key: sensor rows act
        let mut noise = bytes(seed * 7919, 1 << 13);
        let mut next = || noise.next().unwrap_or(0);
        for k in 0..300 {
            let (a, b) = (next(), next());
            let row_y = |r: u8| (ui::LIST_Y + u32::from(r % ui::MENU_ROWS as u8) * ui::MENU_ROW + 10) as u16;
            if a % 29 == 0 {
                // straight to a row that asks first: a sensor, the phone's FORGET, IMPORT
                let import = menu::SYSTEM.iter().position(|&r| r == menu::Row::Stock(menu::StockRow::Import));
                let at = [
                    (Menu::Sensors, usize::from(b) % 5),
                    (Menu::Phone, menu::PHONE.len() - 1),
                    (Menu::System, import.unwrap_or(0)),
                ];
                app.open(Screen::Menu(at[usize::from(b) % 3].0, at[usize::from(b) % 3].1));
                app.show(&mut sim);
            }
            if a % 31 == 1 {
                app.settings.touch = !app.settings.touch; // TOUCH off and on
            }
            let i = match a % 14 {
                _ if a % 23 == 3 => Input::TouchLock,
                0 | 1 => Input::Next,
                2 => Input::Prev,
                3 => Input::Select,
                4 => Input::Back,
                5 => Input::Menu,
                6..=8 => Input::Touch(Gesture::Tap(120, row_y(b))),
                9 => Input::Touch(Gesture::Tap(10 + u16::from(b % 30), (ui::PAGE_Y + 8) as u16)),
                10 => Input::Touch(Gesture::Swipe([Dir::Left, Dir::Right, Dir::Up, Dir::Down][usize::from(b % 4)])),
                11 => Input::Touch(Gesture::Hold(u16::from(b) % 240, u16::from(next()) % 320)),
                12 => Input::Touch(Gesture::Tap(u16::from(b) % 240, u16::from(next()) % 320)),
                _ => Input::NextHeld,
            };
            let (before, chosen, editing) = (app.screen(), app.menu_chosen, app.editing);
            let (pages, whole) = (app.pages, app.graph_whole);
            let recording = app.ride.state() == crate::ride::State::Running;
            let was_locked = app.strip.locked;
            app.input(&mut sim, i);
            locks += u32::from(i == Input::TouchLock);
            let same = app.screen() == before && app.pages == pages && app.graph_whole == whole;
            if let Input::Touch(g) = i {
                if was_locked {
                    assert!(same, "locked: {g:?} did something");
                    locked_touches += 1;
                }
                if !app.settings.touch {
                    assert!(same, "TOUCH OFF: {g:?} did something");
                    off += 1;
                } else if recording
                    && matches!(before, Screen::Main | Screen::Ride(_))
                    && !app.phone.ringing
                    && matches!(g, Gesture::Tap(..) | Gesture::Hold(..))
                {
                    assert!(same, "recording: {g:?} on {before:?} did something");
                    riding += 1;
                }
            }
            match (before, i) {
                (Screen::Menu(..), Input::Touch(Gesture::Tap(x, y)))
                    if !editing && x < 48 && u32::from(y) < ui::LIST_Y && app.screen() != before =>
                {
                    backs += 1
                }
                (Screen::Menu(m, s), Input::Touch(Gesture::Tap(_, y))) if !editing && u32::from(y) >= ui::LIST_Y => {
                    let r = menu::first_shown(s, ui::MENU_ROWS) + ((u32::from(y) - ui::LIST_Y) / ui::MENU_ROW) as usize;
                    if r == s && chosen {
                        acts += 1;
                    } else if r != s && app.screen() == Screen::Menu(m, r) {
                        selects += 1;
                    }
                }
                _ => {}
            }
            if let Screen::Menu(m, s) = app.screen() {
                assert!(s < app.rows(m).max(1), "{m:?} row {s}: inside the menu");
            }
            asks += u32::from(app.confirm.is_some());
            for _ in 0..usize::from(b % 4) {
                app.step(&mut sim);
                sim.delay_ms(POLL_MS);
            }
            // the device list is redrawn once a second at most (LIST_REDRAW_MS): a report may wait for it
            // (a banner said by this input is drawn by the next pass: skipped until it is)
            let waiting = app.banner_ms.is_some() && !app.banner_drawn;
            if k % 60 == 0 && !(app.screen() == Screen::Nearby && app.list_dirty) && !waiting {
                super::tests::assert_consistent(&sim, &app);
            }
        }
    }
    std::println!("reached: {selects} selects, {acts} acts, {backs} backs, {asks} asked, {off} off, {riding} riding");
    std::println!("touch lock: {locks} toggles, {locked_touches} touches while locked");
    assert!(locks > 30 && locked_touches > 30, "the lock reached: {locks} toggles, {locked_touches} touches");
    assert!(selects > 50 && acts > 20 && backs > 20 && asks > 10, "{selects} {acts} {backs} {asks}");
    assert!(off > 50 && riding > 10, "touch off {off}, taps and holds while recording {riding}");
}

/// Keys and taps by chance through the whole menu tree (UI review steps 6 and 8), a ride started and ended, workouts
/// and courses there to start: no panic, the selection inside its menu, every menu reached (the ride menu and its
/// WORKOUT and COURSE menus too), and no stored setting (the settings' registers, the pages, the sensors) changed
/// but by an act in a menu: Start or a tap on a row, or the end of a number's edit (Back or a swipe right keep it).
#[test]
fn menu_tree_by_chance() {
    use super::*;
    use crate::touch::Dir;
    use std::collections::BTreeMap;
    let mut reached = BTreeMap::<std::string::String, u32>::new();
    let watched: Vec<usize> = [WHEEL_BKP, POWER_BKP, ALERTS_BKP]
        .into_iter()
        .chain(PAGE_BKP.iter().flatten().copied())
        .chain(crate::sensors::Kind::ALL.iter().map(|k| k.backup()))
        .collect();
    let (mut acts, mut rides) = (0, 0);
    for seed in 1..=16u32 {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8]));
        app.set_course_memory(crate::storage::course::tests::memory());
        let mut x = seed.wrapping_mul(2_654_435_761) | 1;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        let mut tapped = 0;
        for k in 0..2500 {
            let (a, b) = (next() % 100, next());
            // a tap on the row tapped last half the time: the second tap that acts
            tapped = if b % 2 == 0 { tapped } else { b / 2 % ui::MENU_ROWS as u32 };
            let row_y = (ui::LIST_Y + tapped * ui::MENU_ROW + 10) as u16;
            // down more than up and out, so the deep menus are reached
            let i = match a {
                0..=29 => Input::Next,
                30..=37 => Input::Prev,
                38..=59 => Input::Select,
                60..=69 => Input::Back,
                70..=75 => Input::Menu,
                76 => Input::End,
                77..=79 => Input::NextHeld,
                80..=91 => Input::Touch(Gesture::Tap(120, row_y)),
                92..=93 => Input::Touch(Gesture::Tap(10, (ui::PAGE_Y + 8) as u16)),
                _ => Input::Touch(Gesture::Swipe([Dir::Left, Dir::Right, Dir::Up, Dir::Down][(b % 4) as usize])),
            };
            let stored = |sim: &Sim| watched.iter().map(|&r| sim.backup[r]).collect::<Vec<_>>();
            let (before, editing) = (stored(&sim), app.editing);
            let in_menu = matches!(app.screen(), Screen::Menu(..));
            app.input(&mut sim, i);
            if matches!(app.screen(), Screen::End(_)) && b % 4 != 0 {
                // SAVE most times, as Start held (or END RIDE, asked) did before the end-ride screen: a ride ended
                // now and then leaves the walk the menus off a ride (the end-ride screen: end_ride_screen_by_chance)
                app.input(&mut sim, Input::Next);
                app.input(&mut sim, Input::Select);
            }
            if stored(&sim) != before {
                let act = matches!(i, Input::Select | Input::Touch(Gesture::Tap(..)))
                    || editing && matches!(i, Input::Back | Input::Touch(Gesture::Swipe(Dir::Right)));
                assert!(act && in_menu, "seed {seed} step {k}: {i:?} changed a stored setting");
                acts += 1;
            }
            if let Screen::Menu(m, s) = app.screen() {
                assert!(s < app.rows(m).max(1), "{m:?} row {s}: inside the menu");
                let name = std::format!("{m:?}");
                *reached.entry(name.split('(').next().unwrap_or("").into()).or_default() += 1;
            }
            rides += u32::from(app.screen() == Screen::Summary);
            for _ in 0..1 + b % 3 {
                // one pass at least: a banner said by the input is drawn by the pass after it
                app.step(&mut sim);
                sim.delay_ms(POLL_MS);
            }
            if k % 250 == 0 && !(app.screen() == Screen::Nearby && app.list_dirty) {
                super::tests::assert_consistent(&sim, &app);
            }
        }
    }
    std::println!("reached: {reached:?}; {acts} stored by an act, {rides} ride cards");
    let all = [
        "Main",
        "Settings",
        "Me",
        "Alerts",
        "Display",
        "Bike",
        "System",
        "BuiltIn",
        "Gps",
        "Sensors",
        "Remote",
        "Phone",
        "Pages",
        "Page",
        "Fields",
        "Group",
        "History",
        "Stock",
        "Workouts",
        "Steps",
        "Courses",
        "Ride",
        "WorkoutNow",
        "CourseNow",
    ];
    for m in all {
        assert!(reached.contains_key(m), "{m} never reached: {reached:?}");
    }
    assert!(acts > 50 && rides > 5, "{acts} settings stored, {rides} rides ended");
}

/// HISTORY by chance (trash.rs, `App::delete_past`): Page, Lap, Start, Back and taps over the list and the cards of
/// the synthetic volume with a full copy, a ride started and stopped now and then. No panic, the selection inside
/// the list, deletes reached (none while riding), and after each the volume whole and no ride lost: every ride
/// file is in `\Activities` or in `\Rustfw\Deleted`, the list one shorter at once.
#[test]
fn history_by_chance() {
    use super::*;
    use crate::ui::menu::Input;
    let rides = |img: &[u8], dir: &str| {
        let mut d = crate::fat::tests::Ram(img.to_vec(), 0);
        let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
        let Ok(f) = fat.open(&mut d, dir) else { return 0 };
        let mut c = fat.cursor(f).unwrap();
        let mut n = 0;
        while let Some(e) = fat.list(&mut d, &mut c).unwrap() {
            n += u32::from(!e.file.dir && e.name().get(..12).is_some_and(|s| s.iter().all(u8::is_ascii_digit)));
        }
        n
    };
    let (mut deletes, mut refused) = (0, 0);
    for seed in 1..=8u32 {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::storage::trash::tests::volume();
        let all = rides(&sim.emmc.image, "Activities");
        let mut app = App::start(&mut sim, None);
        app.set_scan_cache(crate::stock::tests::cache());
        app.set_map_memory(crate::tiles::tests::memory());
        let mut x = seed.wrapping_mul(2_654_435_761) | 1;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        for k in 0..600 {
            if !matches!(app.screen(), Screen::Menu(Menu::History, _) | Screen::Past) {
                if app.stock.status() == crate::stock::Status::Idle {
                    app.stock.start(app.date);
                }
                app.open(Screen::Menu(Menu::History, 0));
                app.show(&mut sim);
            }
            let r = next();
            let i = match r % 100 {
                0..=24 => Input::Next,
                25..=34 => Input::Prev,
                35..=79 => Input::Select,
                80..=89 => Input::Back,
                _ => Input::Touch(crate::touch::Gesture::Tap(120, (ui::LIST_Y + 10) as u16)),
            };
            if r % 97 == 0 {
                let ev = app.ride.start_pause(); // riding now and then (stopped again below)
                app.recorder(ev);
            }
            let riding = !app.ride_settled();
            let (before, rows) = (rides(&sim.emmc.image, "Activities"), app.stock.index.len());
            app.input(&mut sim, i);
            let after = rides(&sim.emmc.image, "Activities");
            if after != before {
                assert!(!riding, "seed {seed} step {k}: deleted during a ride");
                assert_eq!(after + 1, before, "seed {seed} step {k}: one ride");
                assert_eq!(after + rides(&sim.emmc.image, "Rustfw/Deleted"), all, "seed {seed} step {k}: none lost");
                assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0), "seed {seed} step {k}");
                assert_eq!(app.stock.index.len() + 1, rows, "seed {seed} step {k}: the list at once");
                deletes += 1;
            }
            refused += u32::from(riding && app.banner.as_str() == "NOT DURING A RIDE");
            if app.ride.state() != crate::ride::State::Idle && r % 5 == 0 {
                app.ride = crate::ride::Ride::default();
            }
            if let Screen::Menu(m, s) = app.screen() {
                assert!(s < app.rows(m).max(1), "{m:?} row {s}: inside the menu");
            }
            for _ in 0..1 + r % 3 {
                app.step(&mut sim);
                sim.delay_ms(POLL_MS);
            }
        }
    }
    std::println!("{deletes} deletes, {refused} refused while riding");
    assert!(deletes > 5 && refused > 0, "{deletes} deletes, {refused} refused");
}

/// Random edits of random pages (page.rs): every result well formed, within 5 rows and 8 cells, every box a value's
/// size at least, no field unreadable in more cells than before (cells move in an edit: matched by field); each edit
/// reached both ways (done and refused).
#[test]
fn page_edits() {
    use crate::page::{CELLS, Page, ROWS};
    use crate::ui::{Field, LAYOUTS};
    let mut done = [0u32; 6];
    let mut refused = [0u32; 6];
    let mut noise = bytes(2024, 1 << 18);
    let mut next = || u32::from(noise.next().unwrap_or(0));
    for _ in 0..2000 {
        let fields: [Field; CELLS] = core::array::from_fn(|_| Field::from_index(next() % Field::ALL.len() as u32));
        let Some(mut p) = Page::preset(1 + (next() % (LAYOUTS.len() as u32 - 1)) as u8, fields) else { continue };
        for _ in 0..20 {
            let k = next() as usize % (p.cells() + 1); // one past the last too
            let f = Field::from_index(next() % Field::ALL.len() as u32);
            let op = next() as usize % 6;
            let r = match op {
                0 => p.split_across(k, f).map(|q| (q, k)),
                1 => p.split_down(k, f).map(|q| (q, k)),
                2 => p.merge(k),
                3 => p.resize(k, next() % 2 == 0).map(|q| (q, k)),
                4 => p.with_field(k, f).map(|q| (q, k)),
                _ => p.merge(k).and_then(|(q, k)| q.split_down(k, f).map(|q| (q, k))),
            };
            match r {
                Ok((q, at)) => {
                    assert!(q.well_formed() && q.used().len() <= ROWS && q.cells() <= CELLS, "{op}: {q:?}");
                    assert!(q.roomy(), "{op} left a box under a value's size: {p:?} -> {q:?}");
                    for f in Field::ALL {
                        assert!(q.misfits_of(f) <= p.misfits_of(f), "{op} made {f:?} unreadable: {p:?} -> {q:?}");
                    }
                    assert!(at < q.cells());
                    done[op] += 1;
                    p = q;
                }
                Err(_) => refused[op] += 1,
            }
        }
    }
    std::println!("page edits done {done:?}, refused {refused:?}");
    assert!(done.iter().chain(&refused).all(|&n| n > 50), "every edit reached both ways: {done:?} {refused:?}");
}

/// Random and damaged copies of `\Rustfw\pages.bin` (page.rs `Saved`): no panic; random bytes with a good tag and
/// CRC (so the checks after them run) give only well formed pages with boxes a value's size at least, which draw
/// (every page of them, on the Sim, without a panic), and both kinds are reached.
#[test]
fn pages_file() {
    use crate::page::{Page, Saved};
    use crate::ui::Field;
    let (mut good, mut bad) = (0u32, 0u32);
    let p = Page::preset(5, [Field::Hr; 8]).unwrap();
    let whole = Saved { seq: 9, pages: [p; crate::PAGES], count: 3, words: [1; 6], was: [2; 6] }.to_sector();
    let mut sim = super::Sim::new(9, 0, 0);
    let mut app = crate::App::start(&mut sim, None);
    let mut drawn = 0;
    for seed in 1..=3000u32 {
        let mut s = [0u8; 512];
        for (b, x) in s.iter_mut().zip(bytes(seed, 512)) {
            *b = x;
        }
        if seed % 2 == 0 {
            // a valid copy with a few bytes changed
            s = whole;
            for (i, x) in bytes(seed * 31, 6).enumerate() {
                let at = usize::from(x) + 256 * (i % 2);
                s[at] ^= 1 << (i % 8);
            }
        } else {
            // random rows (1 to 5 in use, now and then a stray byte after them), fields and count behind a good
            // tag, the CRC made to match
            s[..4].copy_from_slice(b"RPG1");
            s[8] = (seed % 8) as u8;
            for page in s[60..60 + crate::PAGES * 13].chunks_exact_mut(13) {
                let used = 1 + usize::from(page[12] % 5);
                for (r, b) in page[..5].iter_mut().enumerate() {
                    *b = match (r < used, *b % 16) {
                        (true, _) => (1 + *b % 3) << 4 | (1 + *b / 4 % 2),
                        (false, 0) => *b | 1,
                        (false, _) => 0,
                    };
                }
                for b in &mut page[5..] {
                    *b %= Field::ALL.len() as u8 + 2;
                }
            }
            let at = 60 + crate::PAGES * 13;
            let crc = crate::fit::crc(0, &s[..at]);
            s[at..at + 2].copy_from_slice(&crc.to_le_bytes());
        }
        let mut file = [0u8; 1024];
        file[..512].copy_from_slice(&s);
        match Saved::newest(&file) {
            Some(saved) => {
                assert!((1..=crate::PAGES).contains(&saved.count));
                assert!(saved.pages[..saved.count].iter().all(|p| p.well_formed() && p.roomy()));
                good += 1;
                if seed % 2 == 1 {
                    // random pages: each drawn
                    for p in &saved.pages[..saved.count] {
                        app.pages[0] = *p;
                        app.screen = crate::Screen::Ride(0);
                        app.show(&mut sim);
                        drawn += 1;
                    }
                }
            }
            None => bad += 1,
        }
    }
    std::println!("pages.bin copies: {good} read, {bad} refused; {drawn} random pages drawn");
    assert!(good > 20 && bad > 1000 && drawn > 20, "{good} {bad} {drawn}");
}

/// Keys and taps by chance in the page editor and its actions (app/editor.rs), with a ride started and paused at
/// times: no panic, 1 to 6 pages all well formed, the panel equal to the screenshot, the editor never open while
/// recording; splits, merges, pages added and deleted all reached.
#[test]
fn page_editor_inputs() {
    use super::*;
    let (mut splits, mut merges, mut added, mut deleted) = (0, 0, 0, 0);
    for seed in 1..=24u32 {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        sim.own_settings();
        let mut app = App::start(&mut sim, None);
        let mut noise = bytes(seed * 104_729, 1 << 14);
        let mut next = || noise.next().unwrap_or(0);
        for k in 0..400 {
            let (a, b) = (next(), next());
            if !matches!(
                app.screen(),
                Screen::Edit(..) | Screen::Menu(Menu::Edit(_) | Menu::Fields(..) | Menu::Group(..), _)
            ) && a % 3 != 0
            {
                // back into the editor, on a page by chance
                app.leave_menus();
                app.screen = Screen::Ride(usize::from(b) % app.page_count());
                app.edit_page(&mut sim, usize::from(b) % app.page_count(), usize::from(next()) % 8);
                app.show(&mut sim);
            }
            let i = match a % 12 {
                0..=2 => Input::Next,
                3 => Input::Prev,
                4..=6 => Input::Select,
                7 => Input::Back,
                8 => Input::Touch(Gesture::Tap(u16::from(b) % 240, u16::from(next()) % 320)),
                9 => Input::Touch(Gesture::Tap(120, (ui::LIST_Y + u32::from(b % 6) * ui::MENU_ROW + 10) as u16)),
                10 if b % 8 == 0 => {
                    app.start_pause(); // a ride recording, or paused again
                    continue;
                }
                _ => Input::Touch(Gesture::Hold(u16::from(b) % 240, u16::from(next()) % 320)),
            };
            let (count, cells) = (app.page_count(), app.edit.map(|e| e.work.cells()));
            app.input(&mut sim, i);
            for _ in 0..1 + usize::from(b % 3) {
                app.step(&mut sim);
                sim.delay_ms(POLL_MS);
            }
            added += usize::from(app.page_count() > count);
            deleted += usize::from(app.page_count() < count);
            if let (Some(c), Some(e)) = (cells, app.edit) {
                splits += usize::from(e.work.cells() > c);
                merges += usize::from(e.work.cells() < c);
            }
            assert!((1..=PAGES).contains(&app.page_count()));
            assert!(app.pages()[..app.page_count()].iter().all(|p| p.well_formed()));
            if app.ride.state() == crate::ride::State::Running {
                assert!(app.edit.is_none() || !matches!(app.screen(), Screen::Edit(..)), "editing while recording");
            }
            if k % 50 == 0 {
                run_a_bit(&mut sim, &mut app);
                super::tests::assert_consistent(&sim, &app);
            }
        }
    }
    std::println!("editor reached: {splits} splits, {merges} merges, {added} pages added, {deleted} deleted");
    assert!(splits > 10 && merges > 10 && added > 3 && deleted > 3, "{splits} {merges} {added} {deleted}");
}

/// Random pressures, altitudes and calibrations into the barometric altitude (altitude.rs): no panic, and every
/// branch reached (a reading, a silence, a calibration by typing and by the receiver, both sources, a sector).
#[test]
fn barometric_altitude() {
    use crate::altitude::{Altimeter, Cal, P_STD};
    let mut reach = [0usize; 9];
    for seed in 1..=4u32 {
        let mut v = bytes(seed, 1 << 20);
        let mut r = move || {
            u32::from_le_bytes([
                v.next().unwrap_or(0),
                v.next().unwrap_or(0),
                v.next().unwrap_or(0),
                v.next().unwrap_or(0),
            ])
        };
        let mut a = Altimeter::new();
        for now in 0..60_000u32 {
            let x = r();
            // pressures: mostly the chip's range, around the standard day, sometimes anything
            let p = match x >> 29 {
                0 => r(),
                1..=2 => 260 * 4096 + r() % (1000 * 4096),
                _ => P_STD - 40_000 + r() % 80_000,
            };
            let alt = match r() % 4 {
                0 => r() as i32,
                1 => (r() % 20_000_000) as i32 - 10_000_000,
                _ => (r() % 3_000_000) as i32,
            };
            let gps = (x & 1 == 0).then_some(alt);
            match x % 12 {
                0 => a.feed(None),
                1 => reach[0] += usize::from(a.calibrate(alt, now)),
                2 => a.from_gps(),
                3 => a.ride_start(now),
                7 => {
                    a.follow(gps);
                    reach[8] += usize::from(a.source(gps).0.is_some());
                }
                4 => reach[1] += usize::from(a.import_stock(r() % 20_000)),
                5 | 6 => a.second(gps, now),
                _ => a.feed(Some(p)),
            }
            if let Some(c) = a.take_dirty() {
                reach[2] += 1;
                assert_eq!(Cal::from_sector(&c.to_sector()), Some(c));
            }
            match a.source(gps) {
                (Some(_), 0) => reach[3] += 1,
                (Some(_), _) => reach[4] += 1,
                _ => {}
            }
            reach[5] += usize::from(a.alt_mm().is_some());
            reach[6] += usize::from(a.progress().is_some());
        }
        // sectors: random bytes, and ones with the magic and a good CRC around random fields
        for _ in 0..2000 {
            let mut s = [0u8; 512];
            s.iter_mut().for_each(|b| *b = r() as u8);
            let _ = Cal::from_sector(&s);
            s[..4].copy_from_slice(b"ALT1");
            s[12] = 1 + (r() % 5) as u8;
            s[4..8].copy_from_slice(&(P_STD - 100_000 + r() % 200_000).to_le_bytes());
            let crc = crate::fit::crc(0, &s[..13]);
            s[13..15].copy_from_slice(&crc.to_le_bytes());
            reach[7] += usize::from(Cal::from_sector(&s).is_some());
        }
    }
    std::println!("barometric altitude reached: {reach:?}");
    assert!(reach.iter().all(|&n| n > 1), "{reach:?}"); // [1]: stock's, only before a calibration of ours
}

/// Passes until the banner is gone (the strip and the page under it drawn again).
fn run_a_bit(sim: &mut super::Sim, app: &mut crate::App) {
    for _ in 0..100 {
        app.step(sim);
        crate::Board::delay_ms(sim, crate::POLL_MS);
    }
}

/// Keys and taps by chance on the end-ride screen and its question (app/riding.rs `end_input`), one ride after
/// another: no panic, the ride never running again from there (paused until it ends), and every ride ended in exactly
/// one place, as chosen: SAVE in `\Activities` and `\Rustfw\Activities` (the card shows), DISCARD in
/// `\Rustfw\Discarded` and its `Full` (the overview shows), never lost; RESUME back to the paused ride. Each of the
/// three is reached (counted over the seeds), the volume stays clean.
#[test]
fn end_ride_screen_by_chance() {
    use super::recorder_tests::places;
    use super::*;
    use crate::touch::Dir;
    let (mut saves, mut discards, mut resumes, mut asked) = (0, 0, 0, 0);
    for seed in 1..=6u32 {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        let mut app = App::start(&mut sim, None);
        let mut x = seed.wrapping_mul(2_654_435_761) | 1;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        let pass = |sim: &mut Sim, app: &mut App, ms: u32| {
            for _ in 0..ms / POLL_MS {
                app.step(sim);
                sim.delay_ms(POLL_MS);
            }
        };
        pass(&mut sim, &mut app, 2000);
        for round in 0..8 {
            let what = std::format!("seed {seed} round {round}");
            app.press(&mut sim, Press::Short(Key::Start));
            pass(&mut sim, &mut app, 3000);
            app.press(&mut sim, Press::Short(Key::Start));
            app.input(&mut sim, Input::End);
            assert_eq!(app.screen(), Screen::End(0), "{what}");
            let name = app.recorder.name();
            let mut steps = 0;
            while app.ride.state() != ride::State::Idle {
                steps += 1;
                assert!(steps < 2000, "{what}: never ended");
                let (a, b) = (next() % 100, next());
                // a button's centre, a little off, most taps; anywhere else the rest
                let (bx, by, w, h) = ui::END_BUTTONS[(b % 3) as usize];
                let (jx, jy) = ((b >> 8) % 40, (b >> 16) % 40);
                let tap = if a % 5 == 0 {
                    ((b >> 8) % ui::W, (b >> 16) % ui::H)
                } else {
                    (bx + w / 2 + jx - 20, by + h / 2 + jy - 20)
                };
                let i = match a {
                    0..=19 => Input::Next,
                    20..=34 => Input::Prev,
                    35..=44 => Input::Select,
                    45..=52 => Input::Back,
                    53..=56 => Input::End,
                    57..=59 => Input::Menu,
                    60..=61 => Input::NextHeld,
                    62..=63 => Input::Media,
                    64..=91 => Input::Touch(Gesture::Tap(tap.0 as u16, tap.1 as u16)),
                    _ => Input::Touch(Gesture::Swipe([Dir::Left, Dir::Right, Dir::Up, Dir::Down][(b % 4) as usize])),
                };
                let was = app.screen();
                app.input(&mut sim, i);
                assert_ne!(app.ride.state(), ride::State::Running, "{what}: {i:?} on {was:?}");
                asked += u32::from(matches!(app.screen(), Screen::Discard(_)) && !matches!(was, Screen::Discard(_)));
                match (app.ride.state(), app.screen()) {
                    (ride::State::Paused, Screen::End(_) | Screen::Discard(_)) => {}
                    (ride::State::Paused, s) => {
                        // RESUME: the ride paused on its page (or what a page's input opened from there); back to it
                        resumes += u32::from(matches!(was, Screen::End(_) | Screen::Discard(_)));
                        if !matches!(s, Screen::Main | Screen::Ride(_)) {
                            app.input(&mut sim, Input::Back);
                            app.input(&mut sim, Input::Back);
                            app.input(&mut sim, Input::Back);
                        }
                        if app.screen() == Screen::Media {
                            app.input(&mut sim, Input::Back);
                        }
                        app.input(&mut sim, Input::End);
                    }
                    _ => {}
                }
                pass(&mut sim, &mut app, POLL_MS * (1 + b % 3));
            }
            let discarded = app.screen() == Screen::Main;
            assert!(discarded || app.screen() == Screen::Summary, "{what}: {:?}", app.screen());
            pass(&mut sim, &mut app, 3000);
            let want: [&[&str]; 2] = if discarded {
                discards += 1;
                [&["Rustfw/Discarded"], &["Rustfw/Discarded/Full"]]
            } else {
                saves += 1;
                [&["Activities"], &["Rustfw/Activities"]]
            };
            assert_eq!(places(&sim, name), want, "{what}: in one place, as chosen");
            assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0), "{what}");
            super::tests::back(&mut sim, &mut app); // the card, if it shows (a banner up: cleared first)
            assert_eq!(app.screen(), Screen::Main, "{what}");
            pass(&mut sim, &mut app, 1000);
        }
    }
    std::println!("end-ride screen by chance: {saves} saved, {discards} discarded, {resumes} resumed, {asked} asked");
    assert!(saves > 5 && discards > 5 && resumes > 20 && asked > 10, "{saves} {discards} {resumes} {asked}");
}

/// Random key masks through `Keys::poll` (the touch lock's chord): no panic, and a chord only when Back and Lap have
/// both been down for the last second, once per hold; neither of the two acts while the other is down with it.
#[test]
fn keys_chord() {
    use crate::{CHORD_MS, Key, Keys, Press};
    let pair = Key::Back as u8 | Key::Lap as u8;
    let (mut chords, mut shorts) = (0u32, 0u32);
    for seed in 1..=20u32 {
        let mut k = Keys::default();
        let mut noise = bytes(seed * 104_729, 1 << 12);
        let (mut mask, mut both_ms, mut chord_seen, mut run_fired) = (0u8, 0u32, false, false);
        while let Some(n) = noise.next() {
            // hold the mask 50 to 450 ms, then change one or two keys, often to both Back and Lap
            let dt = 50 * (1 + u32::from(n % 9));
            let p = k.poll(mask, dt);
            both_ms = if mask & pair == pair { both_ms + dt } else { 0 };
            if mask & pair == 0 {
                chord_seen = false;
            }
            run_fired &= both_ms != 0;
            match p {
                Some(Press::Chord) => {
                    assert!(both_ms >= CHORD_MS && !run_fired, "chord at {both_ms} ms, again in one run");
                    (chord_seen, run_fired) = (true, true);
                    chords += 1;
                }
                Some(Press::Short(key) | Press::Long(key) | Press::Repeat(key)) => {
                    assert!(!(chord_seen && pair & key as u8 != 0), "{p:?} after a chord");
                    shorts += u32::from(matches!(p, Some(Press::Short(_))));
                }
                None => {}
            }
            mask = match noise.next().unwrap_or(0) % 6 {
                0 | 1 => pair,
                2 => 0,
                3 => mask ^ Key::Back as u8,
                4 => mask ^ Key::Lap as u8,
                _ => mask ^ Key::Page as u8,
            };
        }
    }
    std::println!("keys: {chords} chords, {shorts} short presses");
    assert!(chords > 20 && shorts > 50, "reached: {chords} chords, {shorts} shorts");
}

/// Random pass lengths (5 ms to a second, as a slow card or a deeper sleep gives) through `App::step`: the timers
/// fire in real time whatever the passes, and nothing panics. Reach: a banner ends, a tone ends, the backlight
/// times out, a silent strap's channel closes and is searched again, in every seed.
#[test]
fn random_pass_lengths() {
    use super::tests::STRAP;
    use crate::{App, Board, Key, Kind, Press, SEARCH_REST_MS, Screen};
    static MELODY: [(u32, u32); 2] = [(1000, 300), (2000, 300)];
    for seed in 1..=6u32 {
        let mut sim = super::Sim::new(9, 0, 0);
        sim.backup[Kind::Hr.backup()] = STRAP.to_word();
        let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8]));
        app.settings.light_level = 2;
        app.settings.light_timeout = 1;
        app.press(&mut sim, Press::Short(Key::Page));
        assert!(matches!(app.screen(), Screen::Ride(_)));
        app.notice("FUZZ");
        app.tones.play(&MELODY);
        let mut r = bytes(seed, 1 << 16).map(u32::from);
        let (mut banner_gone, mut tone_done, mut dark, mut opens) = (false, false, false, 0usize);
        let (t0, mut worst) = (sim.ms, 0u64);
        while sim.ms - t0 < 200_000 {
            let pass = 5 + (r.next().unwrap_or(0) << 8 | r.next().unwrap_or(0)) % 996;
            app.step(&mut sim);
            sim.sleep = (pass, 0);
            sim.delay_ms(pass);
            worst = worst.max(u64::from(pass));
            let t = sim.ms - t0;
            banner_gone |= t > 5000 && app.banner_ms.is_none();
            tone_done |= t > 2000 && sim.tones.last() == Some(&0);
            dark |= app.light_shown == 0;
            opens = sim.nordic.got.iter().filter(|p| p.starts_with(&[6, 0xc4, 0])).count();
            // on time, whatever the passes: never early, never later than a few passes
            if t < 3900 {
                assert!(app.banner_ms.is_some(), "seed {seed}: banner early at {t} ms");
            }
            if t < SEARCH_REST_MS as u64 / 2 {
                assert!(opens <= 1, "seed {seed}: the rest cut short at {t} ms");
            }
            if t < 9900 {
                assert_ne!(app.light_shown, 0, "seed {seed}: backlight off early at {t} ms");
            }
        }
        assert!(banner_gone && tone_done && dark, "seed {seed}: {banner_gone} {tone_done} {dark}");
        assert!(opens >= 2, "seed {seed}: the strap searched again ({opens} opens, longest pass {worst} ms)");
    }
}
