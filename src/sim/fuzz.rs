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
    for seed in 1..=4u32 {
        for b in bytes(seed, 1 << 20) {
            if let Some(p) = rx.push(b) {
                let _ = crate::nrf::event_id(p);
                let _ = crate::nrf::ant_broadcast(p);
                let _ = crate::nrf::ant_closed(p);
                if let Some(r) = crate::nrf::adv_report(p) {
                    let _ = (crate::nrf::ad_name(r.data), crate::nrf::ad_maker(r.data));
                }
            }
        }
    }
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

/// Random broadcasts into every ANT+ decoder (speed, cadence, both, power, heart rate), with restarts: no panic
/// (a u32 overflow did panic here), speeds and cadences within what a bike does.
#[test]
fn ant_decoders() {
    use crate::sensors::{Decoders, Kind, Live, Metric};
    for seed in 5..=8u32 {
        let (mut dec, mut live) = (Decoders::default(), Live::default());
        let v: std::vec::Vec<u8> = bytes(seed, 1 << 18).collect();
        for (n, c) in v.chunks_exact(9).enumerate() {
            let kind = Kind::ALL[usize::from(c[8]) % Kind::ALL.len()];
            if n % 97 == 0 {
                dec.restart(kind);
            }
            dec.decode(kind, core::array::from_fn(|i| c[i]), 2105, &mut live);
            assert!(live.get(Metric::Speed).is_none_or(|s| s <= 2_700), "{:?}", live.get(Metric::Speed));
        }
    }
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
/// and GPX, with bytes changed (the FIT ones with their CRC made right, so the reader gets past it), and random
/// text of GPX's own marks; each read into a course, which is then followed from random places (near it and
/// anywhere on the globe), asked for its profile, climbs and ascent, and drawn on a map.
#[test]
fn course_files() {
    use crate::course::{Course, Tracker};
    use crate::fitread::{Msg, Reader};
    use crate::storage::course::Gpx;
    let mut d = crate::fat::tests::Ram(crate::fat::tests::image(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let files = ["PlanTrip/Hill.fit", "Courses/Hill_Garmin.fit", "Courses/Hill.gpx"]
        .map(|p| crate::fat::tests::read_all(&mut fat, &mut d, p).unwrap());
    let mut c = std::boxed::Box::new(Course::new());
    let read = |f: &[u8], gpx: bool, c: &mut Course| {
        c.clear();
        if gpx {
            Gpx::new().push_slice(f, &mut |p| c.add(p));
        } else {
            Reader::with_position().push_slice(f, |m| {
                if let Msg::Record(r) = m
                    && let Some((lat, lon)) = r.pos
                {
                    c.push(crate::history::degrees_e7(lat), crate::history::degrees_e7(lon), r.alt_dm);
                }
            });
        }
        c.finish();
    };
    let mut noise = bytes(21, 1 << 24);
    let mut next = move || u32::from_le_bytes([0; 4].map(|_| noise.next().unwrap_or(0)));
    let exercise = |c: &Course, next: &mut dyn FnMut() -> u32| {
        let mut t = Tracker::new();
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
        let gpx = i == 2;
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
            read(&bad, gpx, &mut c);
            exercise(&c, &mut next);
            read(&bad[..bad.len() / 3], gpx, &mut c);
        }
    }
    const MARKS: [&str; 19] = [
        "<trkpt", "<rtept", " lat=\"", " lon='", "\"", "'", ">", "/>", "</trkpt>", "<ele>", "</ele>", "-", ".", "1",
        "9", " ", "<!--", "=", "999999", // long digit runs (decimal's overflow)
    ];
    for _ in 0..8 {
        let mut text = Vec::new();
        while text.len() < 64 * 1024 {
            text.extend_from_slice(MARKS[next() as usize % MARKS.len()].as_bytes());
        }
        read(&text, true, &mut c);
        exercise(&c, &mut next);
    }
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
            assert!(crate::ui::layout(p.layout).is_some() && p.layout != 0);
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
        assert!(app.drive_on().is_ok());
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
        assert!(app.map_zoom.is_none_or(|l| (MIN_LEVEL..MAX_LEVEL).contains(&l)), "{:?}", app.map_zoom);
        assert!(app.map_view(0).is_none_or(|v| v.mpp16 >= 32), "no finer than 2 m a pixel");
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

#[test]
fn ubx_frames() {
    let mut parser = crate::gnss::Parser::new();
    for seed in 1..=4u32 {
        for b in bytes(seed, 1 << 20) {
            if let Some((c, i, p)) = parser.push(b) {
                let _ = crate::gnss::pvt(c, i, p);
            }
        }
    }
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
            h.out = (0..c[23] % 20).map(|i| std::vec![i; PACKET]).collect();
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
    let (mut selects, mut acts, mut backs) = (0, 0, 0);
    for seed in 1..=12u32 {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        let mut noise = bytes(seed * 7919, 1 << 13);
        let mut next = || noise.next().unwrap_or(0);
        for k in 0..300 {
            let (a, b) = (next(), next());
            let row_y = |r: u8| (ui::LIST_Y + u32::from(r % ui::MENU_ROWS as u8) * ui::MENU_ROW + 10) as u16;
            let i = match a % 14 {
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
            app.input(&mut sim, i);
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
            for _ in 0..usize::from(b % 4) {
                app.step(&mut sim);
                sim.delay_ms(POLL_MS);
            }
            if k % 60 == 0 {
                super::tests::assert_consistent(&sim, &app);
            }
        }
    }
    assert!(selects > 50 && acts > 20 && backs > 20, "reached: {selects} selects, {acts} acts, {backs} backs");
}
