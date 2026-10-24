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
    let words: [&[u8]; 10] =
        [b"peek ", b"poke ", b"load ", b"key ", b"0x4", b"\n", b"shot\n", b"bat\n", b"sector ", b"emmc\n"];
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
                let _ = session.feed(&buf, &app, Stats::default(), &mut pc);
                buf.clear();
                pc.got.clear();
            }
        }
    }
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
            Gpx::new().push_slice(f, &mut |lat, lon, e| c.push(lat, lon, e));
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
        crate::map::draw(&mut f, (0, 24, 240, 296), &track, view, None, route.as_ref());
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
    const MARKS: [&str; 18] = [
        "<trkpt", "<rtept", " lat=\"", " lon='", "\"", "'", ">", "/>", "</trkpt>", "<ele>", "</ele>", "-", ".", "1",
        "9", " ", "<!--", "=",
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

/// Stock's settings files: random text with the INI's own characters mixed in.
#[test]
fn ini_files() {
    let words: [&[u8]; 8] = [b"[Page1]\n", b"Grid9=", b"type=", b"[UserProfile]\n", b"CP=", b",", b"\r\n", b"ID="];
    for seed in 1..=4u32 {
        let mut t = Vec::new();
        for b in bytes(seed, 1 << 14) {
            if b % 3 == 0 {
                t.extend_from_slice(words[usize::from(b) % words.len()]);
            } else {
                t.push(if b % 3 == 1 { b'0' + b % 10 } else { b });
            }
        }
        let _ = crate::stock::import(&t, &t, &t);
    }
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
            let _ = past.read(&mut d, &r, 250);
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
