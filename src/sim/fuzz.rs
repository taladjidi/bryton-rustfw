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

/// Uniform xorshift32 (`bytes` leans on markers): for generators that draw numbers, not bytes.
fn xorshift(seed: u32) -> impl FnMut() -> u32 {
    let mut x = seed.max(1);
    move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    }
}

#[test]
fn nordic_packets() {
    let mut rx = crate::nrf::Rx::new();
    // random bytes (the stream out of step), then frames built on purpose: a length header and an advertising
    // report, an ANT event (broadcast, acknowledged, channel closed) or random bytes; random bytes alone never
    // make a 44-byte ANT event or an advertising report (code review: the decoders were never reached)
    let mut reached = [0usize; 5];
    let look = |p: &[u8], reached: &mut [usize; 5]| {
        let _ = crate::nrf::event_id(p);
        reached[0] += usize::from(crate::nrf::ant_broadcast(p).is_some());
        reached[1] += usize::from(crate::nrf::ant_closed(p).is_some());
        reached[3] += usize::from(crate::nrf::ant_tx_failed(p).is_some());
        reached[4] += usize::from(crate::nrf::ant_tx_completed(p).is_some());
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
                    match r() % 6 {
                        0 | 1 => p[42] = 0x07, // EVENT_CHANNEL_CLOSED
                        2 => p[42] = 0x06,     // EVENT_TRANSFER_TX_FAILED
                        3 => p[42] = 0x05,     // EVENT_TRANSFER_TX_COMPLETED
                        _ => {}
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
    // index 0: the random broadcasts (seeds 1 to 4), 1: the structured ones (seeds 5 to 8)
    let (mut beats, mut alphas, mut rmssds) = ([0usize; 2], [0usize; 2], [0usize; 2]);
    for seed in 1..=8u32 {
        let random = seed <= 4;
        let k = usize::from(!random);
        let (mut dec, mut live, mut hrv) = (Decoders::default(), Live::default(), crate::hrv::Hrv::default());
        let mut r = xorshift(seed);
        let (mut t, mut count) = (0u16, 0u8);
        let frames: std::vec::Vec<[u8; 8]> = if random {
            let v: std::vec::Vec<u8> = bytes(seed, 1 << 18).collect();
            v.chunks_exact(8).map(|c| core::array::from_fn(|i| c[i])).collect()
        } else {
            // a strap's broadcasts: the beat time and count in step (one beat, or two on page 4 with the earlier
            // one), spans of about 800 ms with a few percent of jitter, so the DFA and RMSSD see clean beats
            (0..1 << 14)
                .map(|_| -> [u8; 8] {
                    let span = 780 + (r() % 60) as u16;
                    let t1 = t.wrapping_add(span);
                    let p = t.wrapping_add(1 + (r() % u32::from(span - 1)) as u16);
                    let two = r().is_multiple_of(4);
                    let d = if two {
                        [4, 0, p as u8, (p >> 8) as u8, t1 as u8, (t1 >> 8) as u8, count.wrapping_add(2), 60]
                    } else {
                        [0, 0, 0, 0, t1 as u8, (t1 >> 8) as u8, count.wrapping_add(1), 60]
                    };
                    (t, count) = (t1, d[6]);
                    d
                })
                .collect()
        };
        for d in frames {
            dec.decode(Kind::Hr, d, 2105, &mut live);
            for &rr in dec.beats.rr() {
                assert!(Beats::RANGE.contains(&rr));
                hrv.push(rr);
                beats[k] += 1;
            }
            if random {
                hrv.push(u16::from_le_bytes([d[0], d[1]])); // raw intervals, 0 and 0xffff too
            }
            alphas[k] += usize::from(hrv.alpha1().is_some());
            rmssds[k] += usize::from(hrv.rmssd().is_some());
        }
    }
    assert!(beats[0] > 0 && beats[1] >= 10_000, "beats: {beats:?}");
    assert!(alphas[1] > 0 && rmssds[1] > 0, "alpha1 and rmssd values, structured: {alphas:?} {rmssds:?}");
}

/// Random beat streams (gaps, bursts, zeros, 0xffff, very long intervals) into the full copy's `hrv` messages
/// between records and checkpoints: the file decodes strictly at every checkpoint and at the end, its beats are the
/// ones fed (0xffff, FIT's invalid, aside), in order. Then into a full copy's recorder with no card: the buffer
/// fills, the beats wait in their queue, and past it are dropped and counted, never a panic.
#[test]
fn beat_streams_into_the_full_copy() {
    use crate::fit::{Sample, Writer, tests::decode};
    use crate::recorder::{Kind, Recorder};
    // a trainer that comes and goes (fit.rs, "The trainer"), any values, its pages 80 / 81 sometimes
    let trainer = |rnd: &mut dyn FnMut() -> u32| {
        (!rnd().is_multiple_of(4)).then(|| crate::fit::TrainerSample {
            target_w: rnd().is_multiple_of(2).then(|| rnd() as u16),
            trainer_w: (!rnd().is_multiple_of(3)).then(|| rnd() as u16),
            meter_w: (!rnd().is_multiple_of(3)).then(|| rnd() as u16),
            fe_state: rnd() as u8,
            status: rnd() as u8,
            ble: rnd().is_multiple_of(2),
            cmds: rnd() as u8,
            ack_fails: rnd() as u8,
            device: rnd().is_multiple_of(5).then(|| crate::fit::TrainerDevice {
                maker: rnd() as u16,
                model: rnd().is_multiple_of(2).then(|| rnd() as u16),
                hw: None,
                serial: Some(rnd()),
                sw_x100: Some(rnd() as u16),
            }),
        })
    };
    let (mut messages, mut dropped, mut trainer_recs) = (0, 0, 0);
    for seed in 1..=40u32 {
        let mut rnd = xorshift(seed);
        let beat = |rnd: &mut dyn FnMut() -> u32| match rnd() % 10 {
            0 => 0,
            1 => 0xffff,
            2 => 0xfffe,
            3 => (rnd() % 60_000) as u16, // up to a minute: a strap lost and found
            _ => 300 + (rnd() % 1700) as u16,
        };
        let (mut f, t0) = (Vec::new(), 1_000_000);
        let mut w = Writer::begin_with(&mut f, t0, true).unwrap();
        let mut fed = Vec::new();
        let mut t = t0;
        for _ in 0..200 {
            t += 1 + rnd() % 3 * (rnd() % 2); // a second, or a gap
            let trainer = trainer(&mut rnd);
            w.record(
                &mut f,
                &Sample { time: t, hr: Some(150), artifacts_pct: Some(rnd() as u8), trainer, ..Default::default() },
            )
            .unwrap();
            let n = match rnd() % 8 {
                0 => 0,
                1 => 6 + rnd() % 15, // a burst: several messages at once
                _ => rnd() % 5,
            };
            let mut k = 0;
            while k < n {
                let batch: Vec<u16> = (0..(1 + rnd() % 5).min(n - k)).map(|_| beat(&mut rnd)).collect();
                k += batch.len() as u32;
                w.hrv(&mut f, &batch).unwrap();
                fed.extend(batch.iter().copied().filter(|&v| v != 0xffff));
                messages += 1;
            }
            if rnd().is_multiple_of(30) {
                w.checkpoint(&mut f).unwrap();
                decode(&f).unwrap();
            }
        }
        w.finish(&mut f, t + 1).unwrap();
        let m = decode(&f).unwrap();
        let rr: Vec<u16> = m
            .iter()
            .filter(|x| x.0 == 78)
            .flat_map(|x| x.1.iter().map(|v| v.1 as u16))
            .filter(|&v| v != 0xffff)
            .collect();
        assert_eq!(rr, fed, "seed {seed}");
        trainer_recs += m.iter().filter(|x| x.0 == 20 && crate::fit::tests::field(x, 240).is_some()).count();

        // the recorder, no card: its buffer fills, its queue too
        let mut r = Recorder::of(Kind::Full);
        r.start(t0, *b"261023100000", false);
        for s in 0..400 {
            let trainer = trainer(&mut rnd);
            r.second(&Sample { time: t0 + 1 + s, trainer, ..Default::default() }, [0; crate::ride::RIDE_WORDS]);
            for _ in 0..rnd() % 8 {
                r.beat(beat(&mut rnd));
            }
        }
        dropped += r.beats_dropped;
    }
    assert!(
        messages > 10_000 && dropped > 1000 && trainer_recs > 4000,
        "the writer and the queue reached: {messages} messages, {dropped} dropped, {trainer_recs} trainer records"
    );
}

/// Random broadcasts into every ANT decoder (speed, cadence, both, power, heart rate, the two shifting ones), with
/// restarts: no panic (a u32 overflow did panic here), speeds and cadences within what a bike does. Half the
/// messages carry a page the shifting decoders read (random bytes would rarely), and the test checks they were
/// reached: gears, battery, button presses, within what the formats can say.
#[test]
fn ant_decoders() {
    use crate::sensors::{Decoders, Kind, Live, Metric, SHIFT_STATUS};
    let (mut gears, mut batteries, mut presses, mut statuses) = (0, 0, 0, 0);
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
            // page 82 on every profile: the status kept is a level, 1 to 5
            if let Some(s) = dec.battery_status(kind) {
                assert!((1..=5).contains(&s), "status {s}");
                statuses += 1;
            }
        }
    }
    assert!(gears > 1000 && batteries > 1000 && presses > 100, "reached: {gears} {batteries} {presses}");
    assert!(statuses > 1000, "page 82 reached on the profiles: {statuses}");
}

/// Random broadcasts with the structure of the strap's page 7 (battery, with or without the toggle bit) and of common
/// page 80 (a maker's id from the table or a random one, 0 and 0xffff among them) into every kind's decoder: no panic,
/// a percent at most 100 and only the heart rate's, a maker id that is none of 0 and 0xffff. The test checks both
/// pages were reached (their counts over the seeds), and that the names resolve for the ids of the table.
#[test]
fn strap_battery_and_maker_pages() {
    use crate::sensors::{Decoders, Kind, Live, maker_name};
    const MAKERS: [u16; 22] =
        [1, 6, 7, 9, 32, 41, 51, 69, 70, 107, 112, 115, 123, 134, 135, 140, 263, 267, 268, 310, 0, 0xffff];
    let (mut percents, mut makers, mut names) = (0usize, 0usize, 0usize);
    for seed in 1..=8u32 {
        let (mut dec, mut live) = (Decoders::default(), Live::default());
        let v: std::vec::Vec<u8> = bytes(seed, 1 << 17).collect();
        for c in v.chunks_exact(9) {
            let kind = Kind::ALL[usize::from(c[8]) % Kind::ALL.len()];
            let mut d: [u8; 8] = core::array::from_fn(|i| c[i]);
            match c[8] >> 5 {
                0 => d[0] = 7,
                1 => d[0] = 0x87,
                2 => {
                    d[0] = 0x50;
                    if c[6] & 1 == 0 {
                        d[4..6].copy_from_slice(&MAKERS[usize::from(c[7]) % MAKERS.len()].to_le_bytes());
                    }
                }
                _ => {}
            }
            dec.decode(kind, d, 2105, &mut live);
            if let Some(p) = dec.battery_percent(kind) {
                assert!(p <= 100 && kind == Kind::Hr, "percent {p} of {kind:?}");
                percents += 1;
            }
            if let Some(m) = dec.maker(kind) {
                assert!(m != 0 && m != 0xffff, "maker {m:x}");
                makers += 1;
                names += usize::from(maker_name(m).is_some());
            }
        }
    }
    assert!(percents > 100 && makers > 1000 && names > 1000, "reached: {percents} {makers} {names}");
}

/// Random calibration pages (power meter page 0x01, ids mixed in so the replies are reached) into the reply decoder
/// and the calibration state machine, with requests, retries and time passing: no panic, and every reply kind seen.
#[test]
fn power_calibration_pages() {
    use crate::powercal::{Cal, Reply, reply};
    let (mut zero, mut auto, mut done) = (0, 0, 0);
    for seed in 9..=12u32 {
        let mut c = Cal::default();
        let v: std::vec::Vec<u8> = bytes(seed, 1 << 16).collect();
        for ch in v.chunks_exact(9) {
            let mut d: [u8; 8] = core::array::from_fn(|i| ch[i]);
            if ch[8] & 1 == 0 {
                d[0] = 1;
                d[1] = [0xac, 0xaf, 0x12, 0xaa, 0xab, ch[1]][usize::from(ch[8] >> 1) % 6];
            }
            match ch[8] >> 4 {
                0 => c.start_zero(),
                1 => c.toggle_auto(),
                2 => c.unsent(),
                3 => c.reset(),
                _ => {}
            }
            let r = reply(d);
            zero += usize::from(matches!(r, Some(Reply::Zero { .. })));
            auto += usize::from(matches!(r, Some(Reply::Auto(_))));
            if let Some(r) = r {
                done += usize::from(c.heard(r));
            }
            let _ = (c.due(), c.tick(u32::from(ch[0]) * 40));
        }
    }
    assert!(zero > 500 && auto > 500 && done > 50, "reached: {zero} {auto} {done}");
}

/// Random bytes from the computer, with command words mixed in, into the dev link protocol.
#[test]
fn dev_link_input() {
    use crate::dev::{Session, Stats};
    let mut sim = super::Sim::new(9, 0, 0);
    let app = crate::App::start(&mut sim, None);
    let words: [&[u8]; 13] = [
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
        b"prof ",
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
            for imperial in [false, true] {
                let _ = crate::ui::card::Card::of(&ride.stats(250), imperial);
            }
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

/// Random power curves into the critical power fit (power_curve.rs `fit_cp`): bests from 0 to 2000 W, some missing,
/// some shaped like a rider's; the fit must get past its checks often (the envelope found) and give a CP, W' and
/// quality in range, never a panic.
#[test]
fn power_curves() {
    use crate::power_curve::{Curve, DURATIONS, N, fit_cp};
    let mut next = xorshift(17);
    let (mut fits, mut trusted) = (0, 0);
    for _ in 0..200_000 {
        let mut c = Curve::default();
        let (cp, wp) = (next() % 450 + 50, next() % 40_000 + 1);
        for (w, &t) in c.watts.iter_mut().zip(&DURATIONS) {
            *w = match next() % 8 {
                0 => 0,                      // no best that long
                1 => (next() % 2001) as u16, // anything
                _ => {
                    let model = cp + wp / u32::from(t);
                    (model - model * (next() % 12) / 100).min(2000) as u16 // a best up to 11 % under the model
                }
            };
        }
        let Some(f) = fit_cp(&c) else { continue };
        fits += 1;
        trusted += usize::from(f.trusted());
        assert!(f.cp > 0 && f.cp < 2000 && f.w_prime > 0 && f.w_prime < 200_000, "{f:?} {c:?}");
        assert!(f.points <= f.of && usize::from(f.of) <= N, "{f:?}");
    }
    assert!(fits > 50_000 && trusted > 10_000, "{fits} fits, {trusted} trusted");
}

/// Full copies (alpha1 in a developer field) with bytes changed, through the reader with alpha1 and the threshold
/// fit (hrvt.rs) as FITNESS reads them (storage/thresholds.rs): never a panic, and the alpha1 path reached.
#[test]
fn full_copies() {
    use crate::fitread::{Msg, Reader};
    let good = crate::storage::thresholds::tests::full_copy(1_000_000, 900, |t| (140 + t / 5) as u16, true);
    let (mut noise, mut alpha) = (bytes(23, 1 << 20), 0usize);
    for k in 0..3000 {
        let mut bad = good.clone();
        for _ in 0..k % 5 {
            let at = (usize::from(noise.next().unwrap_or(0)) << 8 | usize::from(noise.next().unwrap_or(0))) % bad.len();
            bad[at] = noise.next().unwrap_or(0);
        }
        let (mut r, mut fit) = (Reader::with_alpha1(), crate::hrvt::RideFit::new());
        r.push_slice(&bad, |m| {
            if let Msg::Record(x) = m {
                alpha += usize::from(x.alpha1_milli.is_some());
                fit.sample(x.time, x.power, x.alpha1_milli);
            }
        });
        let t = fit.thresholds();
        assert!(t.vt1.is_none_or(|w| w < 2000) && t.vt2.is_none_or(|w| w < 2000), "{t:?}");
    }
    assert!(alpha > 100_000, "{alpha} records with alpha1");
}

/// Workout files (storage/workout_file.rs): a good file with bytes changed, and random messages, each behind a good
/// header and with its CRC made right, so the decoder (not the CRC check) gets them.
#[test]
fn workout_files() {
    use crate::storage::workout_file::{
        Kind, STEPS, parse,
        tests::{intervals, two_targets},
    };
    let crc = |f: &mut Vec<u8>| {
        let n = f.len() - 2;
        let c = crate::fit::crc(0, &f[..n]);
        f[n..].copy_from_slice(&c.to_le_bytes());
    };
    let (mut ok, mut noise) = (0, bytes(11, 1 << 20));
    // the second file has secondary targets (fields 19 to 22): the fuzz must reach them still read after the changes
    let (mut second, mut seconds) = (0u32, 0u32);
    for good in [intervals(), two_targets()] {
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
                second += w.steps().iter().filter(|s| s.target2.kind != Kind::Open).count() as u32;
                seconds += u32::from(w.steps().iter().any(|s| s.target2.kind != Kind::Open));
            }
        }
    }
    assert!(ok > 1000, "{ok} still workouts");
    assert!(second > 5000 && seconds > 2000, "{second} secondary targets read in {seconds} workouts");
    let good = two_targets();
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

/// Course files (storage/course_file.rs into course.rs): the synthetic card's hill as Bryton's route, a Garmin course
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
    use crate::storage::course_file::Gpx;
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
            past.start(&r, 250, true);
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
    let mut auto = crate::gnss::Adaptive::default();
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
                    let pvt = crate::gnss::pvt(c, i, p);
                    pvts += usize::from(pvt.is_some());
                    auto.fix(pvt, true); // GPS POWER AUTO on whatever the receiver says: no panic
                }
            }
        }
    }
    assert!(pvts > 1000, "NAV-PVT decoded: {pvts}");
}

/// GPS POWER AUTO fed rides in stretches of 100 s, each either steady (fast, straight, a good fix) or anything (speed,
/// heading, satellites, accuracy at random, fixes lost, the ride paused): cyclic tracking is reached, and never entered
/// twice within [`crate::gnss::AUTO_STEADY_S`] seconds.
#[test]
fn gps_power_auto() {
    use crate::gnss::{AUTO_STEADY_S, Adaptive, Pvt};
    let mut entered = 0;
    for seed in 1..=8u32 {
        let mut r = xorshift(seed);
        let mut a = Adaptive::default();
        let (mut heading, mut last_entry, mut steady) = (0i32, None::<u32>, true);
        for s in 0..20_000u32 {
            if s % 100 == 0 {
                steady = !r().is_multiple_of(3);
            }
            let (pvt, riding) = if steady {
                heading = heading.wrapping_add((r() % 100_001) as i32 - 50_000); // half a degree either way
                let p = Pvt {
                    fix: 3,
                    fix_ok: true,
                    sats: 10,
                    h_acc_mm: 2000,
                    speed_mm_s: 6000 + r() % 9000,
                    heading,
                    ..Pvt::default()
                };
                (Some(p), true)
            } else {
                heading = r() as i32;
                let p = Pvt {
                    fix: (r() % 6) as u8,
                    fix_ok: !r().is_multiple_of(4),
                    sats: (r() % 20) as u8,
                    h_acc_mm: r() % 20_000,
                    speed_mm_s: r() % 20_000,
                    heading,
                    ..Pvt::default()
                };
                ((!r().is_multiple_of(50)).then_some(p), !r().is_multiple_of(10))
            };
            let was = a.saver();
            if a.fix(pvt, riding) && !was {
                assert!(last_entry.is_none_or(|t| s - t > u32::from(AUTO_STEADY_S)), "seed {seed} at {s}: flapping");
                last_entry = Some(s);
                entered += 1;
            }
        }
    }
    assert!(entered > 8, "cyclic tracking reached {entered} times");
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
    let (mut decoded, mut still_decoded, mut refused) = (0usize, 0usize, 0usize);
    for good in &tiles {
        let (off, len) = (0..1024)
            .find_map(|i| crate::ign::entry(good[16 + 8 * i..24 + 8 * i].try_into().unwrap(), good.len() as u32))
            .unwrap();
        let blob = &good[off as usize..(off + len) as usize]; // the first tile the index names
        decoded += usize::from(crate::ign::decode(&mut blob.iter().copied(), &mut out));
        for _ in 0..200 {
            let mut bad = blob.to_vec();
            for _ in 0..1 + next() % 8 {
                let at = next() as usize % bad.len();
                bad[at] = next() as u8;
            }
            let cut = next() as usize % (bad.len() + 1);
            let _ = crate::ign::decode(&mut bad[..cut].iter().copied(), &mut out);
            let ok = crate::ign::decode(&mut bad.iter().copied(), &mut out);
            still_decoded += usize::from(ok);
            refused += usize::from(!ok);
        }
    }
    assert_eq!(decoded, tiles.len(), "every fixture's first tile decodes");
    assert!(still_decoded > 0 && refused > 0, "reached: {still_decoded} mutations decode, {refused} refused");
}

/// The USB drive (msc.rs): random command blocks behind a valid CBW header, and damaged CBWs, with random host
/// lengths, directions and data packets (sometimes none): no panic, a well-formed CSW for every one answered.
#[test]
fn usb_drive_commands() {
    use crate::msc::tests::{Host, cbw, ram};
    use crate::msc::{Drive, LOG, PACKET};
    const OPS: [u8; 13] = [0x00, 0x03, 0x12, 0x1a, 0x1b, 0x1e, 0x23, 0x25, 0x28, 0x2a, 0x2f, 0x35, 0x5a];
    // what the drive logged: READ(10) and WRITE(10) that passed, those that failed, an opcode it does not have
    // (refused), a CBW not valid (0xfd)
    let (mut reads, mut writes, mut failed, mut refused, mut damaged) = (0usize, 0usize, 0usize, 0usize, 0usize);
    for seed in 1..=4u32 {
        let v: Vec<u8> = bytes(seed, 1 << 18).collect();
        let (mut d, mut h, mut m) = (Drive::new(), Host::default(), ram());
        d.insert();
        for (k, c) in v.chunks_exact(24).enumerate() {
            let len = u32::from(u16::from_le_bytes([c[0], c[1]])) % 8192;
            let mut cb = [0u8; 16];
            cb.copy_from_slice(&c[2..18]);
            cb[0] = OPS.get(usize::from(c[2] % 14)).copied().unwrap_or(c[3]);
            if c[3] % 6 == 0 {
                cb[0] = 0x7f; // not an opcode the drive has
            }
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
            let before = d.log.n;
            d.packet(&packet, &mut h, &mut m);
            // every entry this packet logged (the ring keeps the last LOG)
            for i in before.max(d.log.n.saturating_sub(LOG as u32))..d.log.n {
                let (op, _, _, status, _) = d.log.ring[i as usize % LOG];
                match (op, status) {
                    (0x28, 0) => reads += 1,
                    (0x2a, 0) => writes += 1,
                    (0x28 | 0x2a, _) => failed += 1,
                    (0x7f, _) => refused += usize::from(status != 0),
                    (0xfd, _) => damaged += 1,
                    _ => {}
                }
            }
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
    assert!(
        reads > 0 && writes > 0 && failed > 0 && refused > 0 && damaged > 0,
        "reached: {reads} {writes} {failed} {refused} {damaged}"
    );
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
    let (mut answers, mut notifs, mut fresh, mut stored, mut rejected, mut bonds) = (0usize, 0, 0, 0, 0, 0);
    let (mut sim, mut app) = started();
    for seed in 1..=8u32 {
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
        if seed > 4 {
            // structured: an answer (uid, then the three attributes with random lengths) in fragments of 1 to 20 bytes,
            // a notification of 0 to 11 bytes (short ones are refused), and a stored sector, half of them damaged
            let mut r = xorshift(seed);
            let mut ds2 = crate::phone::DataSource::default();
            for _ in 0..200 {
                let mut frame = std::vec![0u8];
                frame.extend(r().to_le_bytes());
                for id in [0u8, 1, 3] {
                    let n = (r() % 40) as usize;
                    frame.push(id);
                    frame.extend((n as u16).to_le_bytes());
                    frame.extend((0..n).map(|_| b'a' + (r() % 26) as u8));
                }
                let mut rest = frame.as_slice();
                while !rest.is_empty() {
                    let k = (1 + (r() % 20) as usize).min(rest.len());
                    let (c, tail) = rest.split_at(k);
                    rest = tail;
                    answers += usize::from(ds2.push(c).is_some());
                }
                let mut note = [0u8; 12];
                note[0] = (r() % 3) as u8;
                note[1] = r() as u8;
                note[2] = (r() % 8) as u8;
                for b in note[4..].iter_mut() {
                    *b = r() as u8;
                }
                let len = (r() % 12) as usize;
                let parsed = crate::phone::Notif::parse(&note[..len]);
                assert_eq!(parsed.is_some(), len >= 8, "{len} bytes");
                if let Some(n) = parsed {
                    notifs += 1;
                    fresh += usize::from(n.fresh());
                }
                let mut sector = [0u8; 512];
                sector[..4].copy_from_slice(b"RBL1");
                let end = 40 + crate::phone::ATTRS;
                for b in sector[4..end].iter_mut() {
                    *b = r() as u8;
                }
                let crc = crate::fit::crc(0, &sector[..end]);
                sector[end..end + 2].copy_from_slice(&crc.to_le_bytes());
                let damaged = r().is_multiple_of(2);
                if damaged {
                    let at = r() as usize % (end + 2);
                    sector[at] ^= (r() as u8) | 1; // a CRC-16 catches any one byte
                }
                match crate::phone::Stored::from_sector(&sector) {
                    Some(st) => {
                        assert!(!damaged, "a damaged sector was accepted");
                        stored += 1;
                        bonds += usize::from(st.bond.is_some());
                    }
                    None => {
                        assert!(damaged, "an intact sector was refused");
                        rejected += 1;
                    }
                }
            }
        }
    }
    assert!(
        answers > 0 && notifs > 0 && fresh > 0,
        "reached: {answers} answers, {notifs} notifications, {fresh} fresh"
    );
    assert!(stored > 0 && rejected > 0 && bonds > 0, "reached: {stored} stored, {rejected} rejected, {bonds} bonds");
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
    let mut reach = [0usize; 5]; // the screen an input found: Quick, PowerCal, RampResult, About, Notif
    let mut arms = [0usize; 5]; // and an arm taken there: left, asked, left, left, scrolled
    // STORAGE: the menu reached by an input, an oldest ride's row selected, its card opened (read over the passes)
    let mut storage = [0usize; 3];
    for seed in 1..=16u32 {
        let mut sim = Sim::new(9, 0, 0);
        if seed.is_multiple_of(2) {
            sim.emmc.image = crate::storage::trash::tests::volume(); // rides: STORAGE's oldest rows exist
        }
        let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8])); // a stand-in ANT+ key: sensor rows act
        app.theme = crate::ui::tests::metric_ends()[seed as usize / 2 % 2]; // menu rows of 44 and of 56 px
        app.repaint(&mut sim);
        let mut noise = bytes(seed * 7919, 1 << 13);
        let mut next = || noise.next().unwrap_or(0);
        let mut from_storage = false;
        for k in 0..300 {
            let (a, b) = (next(), next());
            let (rows, pitch) = (app.menu_rows() as u8, app.theme.menu_row);
            let row_y = |r: u8| (ui::LIST_Y + u32::from(r % rows) * pitch + 10) as u16;
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
            if a % 37 == 5 {
                // STORAGE opened at a row of its own (the rows of the rides on the card, on the even seeds)
                let rows = app.rows(Menu::Storage).max(1);
                app.open(Screen::Menu(Menu::Storage, usize::from(b) % rows));
                app.show(&mut sim);
            }
            if a % 47 == 11 {
                // the screens no walk of rows and gestures reaches by chance: each opened as its row or gesture
                // opens it (the panel: a swipe down; CALIBRATE POWER; ABOUT; a notice tapped), with the state it shows
                match b % 5 {
                    0 => app.open_quick(),
                    1 => {
                        // a power meter paired and heard (as powercal_tests' unit): CALIBRATE POWER asks
                        app.radio.sensors[crate::sensors::Kind::Power as usize] =
                            Sensor::Paired(SensorId { device: 7, trans: 5 });
                        app.radio.heard_ms[crate::sensors::Kind::Power as usize] = 0;
                        app.open_powercal();
                    }
                    2 => {
                        app.ramp.result = Some(crate::ramp::Outcome { best_w: 220, ftp: 165, secs: 900, aet: None });
                        app.open(Screen::RampResult);
                    }
                    3 => app.open(Screen::About),
                    _ => {
                        // three notices of 13 wrapped lines (ui::NOTIF_LINES is 12): one scrolls one line; the one opened is k = 0..3
                        const LONG: &str = "cccccccccc cccccccccc cccccccccc cccccccccc cccccccccc cccccccccc cccccccccc cccccccccc cccccccccc cccccccccc dddddddddd";
                        for n in 0..3u8 {
                            let mut a = crate::phone::Attributes { uid: u32::from(n) + 1, ..Default::default() };
                            a.title.push_str("aaaaaaaaaa bbbbbbbbbb"); // two lines: a notice longer than the page scrolls
                            a.message.push_str(LONG);
                            app.phone.inbox.add(&a, (9, 41 + n));
                        }
                        app.open(Screen::Notif(usize::from(b / 5) % 3, 0));
                    }
                }
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
            let on_old = matches!(before, Screen::Menu(Menu::Storage, s)
                if matches!(app.row(Menu::Storage, s), Some(menu::Row::OldRide(_))));
            storage[0] += usize::from(matches!(before, Screen::Menu(Menu::Storage, _)));
            let was_locked = app.strip.locked;
            app.input(&mut sim, i);
            if on_old && matches!(i, Input::Select | Input::Touch(_)) {
                storage[1] += 1;
                from_storage = true; // the card reads over later passes: the flag waits for it
            }
            // the screen the input found, and an arm of its own taken: left (Quick, RampResult, About), the meter
            // asked (PowerCal), a scroll (Notif)
            match before {
                Screen::Quick => {
                    reach[0] += 1;
                    arms[0] += usize::from(app.screen() != Screen::Quick);
                }
                Screen::PowerCal => {
                    reach[1] += 1;
                    arms[1] += usize::from(app.powercal.asking());
                }
                Screen::RampResult => {
                    reach[2] += 1;
                    arms[2] += usize::from(app.screen() != Screen::RampResult);
                }
                Screen::About => {
                    reach[3] += 1;
                    arms[3] += usize::from(app.screen() != Screen::About);
                }
                Screen::Notif(k, t) => {
                    reach[4] += 1;
                    arms[4] += usize::from(matches!(app.screen(), Screen::Notif(k2, t2) if k2 == k && t2 != t));
                }
                _ => {}
            }
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
                    let band = (u32::from(y) - ui::LIST_Y) / app.theme.menu_row;
                    let band = band.min(app.menu_rows() as u32 - 1); // the 4 px under the last row are its own
                    let r = menu::first_shown(s, app.menu_rows(), app.rows(m)) + band as usize;
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
            if from_storage && app.screen() == Screen::Past {
                storage[2] += 1;
                from_storage = false;
            }
            if !matches!(app.screen(), Screen::Menu(Menu::Storage, _) | Screen::Past) {
                from_storage = false;
            }
            // the device list is redrawn once a second at most (LIST_REDRAW_MS): a report may wait for it
            // (a banner said by this input is drawn by the next pass: skipped until it is)
            let waiting = app.notes.ms.is_some() && !app.notes.drawn;
            // STORAGE's rows and figures change as its survey goes on (redrawn once a second, LIST_REDRAW_MS)
            let surveying = matches!(app.screen(), Screen::Menu(Menu::Storage, _)) && !app.survey.done();
            let nearby_list = app.screen() == Screen::Nearby && app.list_dirty;
            let quiet = !(waiting || surveying || nearby_list);
            if k % 60 == 0 && quiet {
                super::tests::assert_consistent(&sim, &app);
            }
        }
    }
    std::println!("reached: {selects} selects, {acts} acts, {backs} backs, {asks} asked, {off} off, {riding} riding");
    std::println!(
        "screens reached (inputs): quick {}, power cal {}, ramp result {}, about {}, notif {}",
        reach[0],
        reach[1],
        reach[2],
        reach[3],
        reach[4]
    );
    std::println!(
        "arms taken: quick left {}, asked {}, ramp left {}, about left {}, notif scrolled {}",
        arms[0],
        arms[1],
        arms[2],
        arms[3],
        arms[4]
    );
    assert!(reach.iter().all(|&n| n > 0), "every screen reached: {reach:?}");
    assert!(storage.iter().all(|&n| n > 0), "STORAGE, an oldest ride selected, its card opened: {storage:?}");
    assert!(arms.iter().all(|&n| n > 0), "every arm taken: {arms:?}");
    std::println!("touch lock: {locks} toggles, {locked_touches} touches while locked");
    assert!(locks > 30 && locked_touches > 30, "the lock reached: {locks} toggles, {locked_touches} touches");
    assert!(selects > 50 && acts > 20 && backs > 20 && asks > 10, "{selects} {acts} {backs} {asks}");
    assert!(off > 50 && riding > 5, "touch off {off}, taps and holds while recording {riding}");
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
    for seed in 1..=36u32 {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8]));
        app.set_course_memory(crate::storage::course_file::tests::memory());
        let mut x = seed.wrapping_mul(2_654_435_761) | 1;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        let mut tapped = 0;
        app.theme = crate::ui::tests::metric_ends()[seed as usize % 2];
        app.repaint(&mut sim);
        app.sw.meta.auto = if seed % 3 == 0 { 1 } else { 0 }; // the other seeds walk as they did
        for k in 0..2500 {
            let (a, b) = (next() % 100, next());
            // a tap on the row tapped last half the time: the second tap that acts
            tapped = if b % 2 == 0 { tapped } else { b / 2 % app.menu_rows() as u32 };
            let row_y = (ui::LIST_Y + tapped * app.theme.menu_row + 10) as u16;
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
            if let Screen::Menu(Menu::Page(n), _) = app.screen()
                && n < PAGES
                && b % 3 == 0
            {
                // a page's menu: now and then the cursor on one of the switch rows, so the walk acts on them
                let ops: Vec<usize> = (0..app.rows(Menu::Page(n)))
                    .filter(|&r| matches!(app.row(Menu::Page(n), r), Some(crate::menu::Row::PageOp(..))))
                    .collect();
                if let Some(&r) = ops.get(b as usize / 3 % ops.len().max(1)) {
                    app.screen = Screen::Menu(Menu::Page(n), r);
                    app.update_menu_rows(&mut sim, Menu::Page(n), r);
                }
            }
            let (in_menu, avg) = (matches!(app.screen(), Screen::Menu(..)), app.settings.wkt_avg);
            let snap = (app.sw.meta, app.pages);
            app.input(&mut sim, i);
            if matches!(app.screen(), Screen::End(_)) && b % 4 != 0 {
                // SAVE most times, as Start held (or END RIDE, asked) did before the end-ride screen: a ride ended
                // now and then leaves the walk the menus off a ride (the end-ride screen: end_ride_screen_by_chance)
                app.input(&mut sim, Input::Select); // SAVE is selected when the screen opens
            }
            // the effects of the page switches, not only their rows selected
            let moved = app.pages != snap.1 && app.pages.iter().all(|p| snap.1.contains(p));
            for (what, hit) in [
                ("shown", app.sw.meta.off != snap.0.off),
                ("moved", moved),
                ("renamed", app.sw.meta.names != snap.0.names && app.sw.meta.off == snap.0.off),
            ] {
                *reached.entry(what.into()).or_default() += u32::from(hit);
            }
            if stored(&sim) != before {
                let act = matches!(i, Input::Select | Input::Touch(Gesture::Tap(..)))
                    || editing && matches!(i, Input::Back | Input::Touch(Gesture::Swipe(Dir::Right)));
                // a tap on a power average field of a ride page changes the window, kept
                let avg_tap = !in_menu
                    && matches!(i, Input::Touch(Gesture::Tap(..)))
                    && app.settings.wkt_avg == (avg + 1) % 3
                    && stored(&sim)
                        .iter()
                        .zip(&before)
                        .enumerate()
                        .all(|(n, (a, b))| a == b || n == 1 && (a ^ b) & !(3 << 14) == 0);
                assert!(act && in_menu || avg_tap, "seed {seed} step {k}: {i:?} changed a stored setting");
                acts += 1;
            }
            if let Screen::Menu(m, s) = app.screen() {
                assert!(s < app.rows(m).max(1), "{m:?} row {s}: inside the menu");
                let name = std::format!("{m:?}");
                *reached.entry(name.split('(').next().unwrap_or("").into()).or_default() += 1;
                // the page switch rows (SHOW, MOVE, RENAME) and AUTO SCROLL, selected
                let ops = matches!(app.row(m, s), Some(crate::menu::Row::PageOp(..) | crate::menu::Row::AutoScroll));
                *reached.entry("switch rows".into()).or_default() += u32::from(ops);
            }
            rides += u32::from(app.screen() == Screen::Summary);
            if k % 100 == 7
                && app.auto_secs() > 0
                && app.ride.state() == crate::ride::State::Running
                && matches!(app.screen(), Screen::Ride(n) if n < PAGES)
                && app.more_on.is_none()
            {
                // untouched for longer than the longest step: the auto scroll turns the page
                let page = app.screen();
                for _ in 0..22_000 / POLL_MS {
                    app.step(&mut sim);
                    sim.delay_ms(POLL_MS);
                }
                *reached.entry("auto scrolled".into()).or_default() += u32::from(app.screen() != page);
            }
            for _ in 0..1 + b % 3 {
                // one pass at least: a banner said by the input is drawn by the pass after it
                app.step(&mut sim);
                sim.delay_ms(POLL_MS);
            }
            if k % 250 == 0 && !(app.screen() == Screen::Nearby && app.list_dirty) {
                // the course list's lengths come in while it shows and are drawn at the next list redraw
                // (LIST_REDRAW_MS): a pass taken between the two shows the new length in the screenshot only
                if matches!(app.screen(), Screen::Menu(Menu::Courses, _)) {
                    for _ in 0..LIST_REDRAW_MS / POLL_MS + 1 {
                        app.step(&mut sim);
                        sim.delay_ms(POLL_MS);
                    }
                }
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
        "PageList",
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
    assert!(reached.get("switch rows").is_some_and(|&n| n > 10), "page switch rows reached: {reached:?}");
    for what in ["shown", "moved", "renamed", "auto scrolled"] {
        assert!(reached.get(what).is_some_and(|&n| n > 0), "{what} never happened: {reached:?}");
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
            // a delete asked on a card is answered with Start two times in three (else any input drops it)
            let i = match r % 100 {
                _ if app.confirm == Some(PAST_DELETE) && r % 3 != 0 => Input::Select,
                _ if app.screen() == Screen::Past && r % 4 == 0 => Input::End, // asks
                0..=24 => Input::Next,
                25..=34 => Input::Prev,
                35..=64 => Input::Select,
                65..=74 => Input::End, // asks to delete a past ride (a card shown), as a tap on its DELETE
                75..=79 => Input::Touch(crate::touch::Gesture::Tap(
                    (ui::card::DELETE_TAP.0 + 10) as u16,
                    (ui::card::DELETE_TAP.1 + 10) as u16,
                )),
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
            refused += u32::from(riding && app.notes.text.as_str() == "NOT DURING A RIDE");
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
    let (mut good, mut bad, mut more_read, mut indoor_read, mut mtb_read) = (0u32, 0u32, 0usize, 0usize, 0usize);
    let (mut meta_read, mut meta_fallback) = (0usize, 0usize);
    let p = Page::preset(5, [Field::Hr; 8]).unwrap();
    let whole = Saved {
        seq: 9,
        pages: [p; crate::PAGES],
        count: 3,
        words: [1; 6],
        was: [2; 6],
        more: [None; crate::PAGES],
        indoor: Some(crate::page::Set::indoor_default()),
        mtb: Some(crate::page::Set::mtb_default()),
        meta: crate::page::Meta::DEFAULT,
    }
    .to_sector();
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
            // and the MORE variants behind it: flags, pages as random as those, their own CRC
            let more = at + 2;
            s[more] = if seed % 5 == 0 { 0 } else { s[more] | 1 };
            for page in s[more + 1..more + 1 + crate::PAGES * 13].chunks_exact_mut(13) {
                let used = 1 + usize::from(page[12] % 5);
                for (r, b) in page[..5].iter_mut().enumerate() {
                    *b = if r < used { (1 + *b % 3) << 4 | (1 + *b / 4 % 2) } else { 0 };
                }
                for b in &mut page[5..] {
                    *b %= Field::ALL.len() as u8 + 2;
                }
            }
            let end = more + 1 + crate::PAGES * 13;
            let crc = crate::fit::crc(crate::fit::crc(0, &s[..at]), &s[more..end]);
            s[end..end + 2].copy_from_slice(&crc.to_le_bytes());
            // and the indoor set behind that: a count, pages, MORE flags and variants as random, a CRC chained to
            // the MORE part's (now and then left wrong, now and then the count out of range)
            let ind = end + 2;
            s[ind] = if seed % 7 == 0 { s[ind] } else { 1 + (seed % 2) as u8 };
            let flags = ind + 1 + crate::PAGES * 13;
            s[flags] |= 1;
            let (a, b) = s[ind + 1..flags + 1 + crate::PAGES * 13].split_at_mut(flags - ind);
            for page in a[..flags - ind - 1].chunks_exact_mut(13).chain(b.chunks_exact_mut(13)) {
                let used = 1 + usize::from(page[12] % 5);
                for (r, b) in page[..5].iter_mut().enumerate() {
                    *b = if r < used { (1 + *b % 3) << 4 | (1 + *b / 4 % 2) } else { 0 };
                }
                for b in &mut page[5..] {
                    *b %= Field::ALL.len() as u8 + 2;
                }
            }
            if seed % 3 != 0 {
                // mostly the pages and variants above again (so the indoor set reads as often as they do)
                s.copy_within(60..60 + crate::PAGES * 13, ind + 1);
                s.copy_within(more + 1..more + 1 + crate::PAGES * 13, flags + 1);
            }
            let ind_end = flags + 1 + crate::PAGES * 13;
            let crc = crate::fit::crc(crc, &s[ind..ind_end]);
            let ind_crc = crc;
            let crc = if seed % 11 == 0 { crc ^ 1 } else { crc };
            s[ind_end..ind_end + 2].copy_from_slice(&crc.to_le_bytes());
            // and the MTB set behind that (four pages at most): the same pages again or random ones, a count now and
            // then out of range, a CRC chained to the indoor part's (now and then wrong)
            let mtb = ind_end + 2;
            let mflags = mtb + 1 + 4 * 13;
            let mtb_end = mflags + 1 + 4 * 13;
            s[mtb] = if seed % 13 == 0 { 5 + (seed % 3) as u8 } else { 1 + (seed % 4) as u8 };
            s[mflags] = (seed >> 3) as u8 & 0xf;
            let (pa, pb) = s[mtb + 1..mtb_end].split_at_mut(mflags - mtb);
            for page in pa[..mflags - mtb - 1].chunks_exact_mut(13).chain(pb.chunks_exact_mut(13)) {
                let used = 1 + usize::from(page[12] % 5);
                for (r, b) in page[..5].iter_mut().enumerate() {
                    *b = if r < used { (1 + *b % 3) << 4 | (1 + *b / 4 % 2) } else { 0 };
                }
                for b in &mut page[5..] {
                    *b %= Field::ALL.len() as u8 + 2;
                }
            }
            if seed % 4 != 0 {
                s.copy_within(60..60 + 4 * 13, mtb + 1);
                s.copy_within(more + 1..more + 1 + 4 * 13, mflags + 1);
            }
            let crc = crate::fit::crc(ind_crc, &s[mtb..mtb_end]);
            let crc = if seed % 17 == 0 { crc ^ 1 } else { crc };
            s[mtb_end..mtb_end + 2].copy_from_slice(&crc.to_le_bytes());
            // and the switches behind that: flags (the present bit now and then missing), an OFF mask, names of random
            // codes (now and then out of the alphabet), a CRC chained to the MTB part's (now and then wrong)
            let meta = mtb_end + 2;
            let meta_end = meta + 2 + crate::PAGES * 3;
            s[meta] = if seed % 19 == 0 { s[meta] & 0x7f } else { s[meta] | 0x80 };
            for (k, name) in s[meta + 2..meta_end].chunks_exact_mut(3).enumerate() {
                // four character codes of the alphabet (37 of 64 values), now and then raw bytes
                let codes = bytes(seed * 7 + k as u32, 4);
                let bits = codes
                    .enumerate()
                    .fold(0u32, |acc, (i, x)| acc | (u32::from(x) % if seed % 31 == 0 { 64 } else { 37 }) << (6 * i));
                if seed % 23 != 0 {
                    name.copy_from_slice(&bits.to_le_bytes()[..3]);
                }
            }
            let mtb_good = crate::fit::crc(ind_crc, &s[mtb..mtb_end]);
            let crc = crate::fit::crc(mtb_good, &s[meta..meta_end]);
            let crc = if seed % 29 == 0 { crc ^ 1 } else { crc };
            s[meta_end..meta_end + 2].copy_from_slice(&crc.to_le_bytes());
        }
        let mut file = [0u8; 1024];
        file[..512].copy_from_slice(&s);
        match Saved::newest(&file) {
            Some(saved) => {
                assert!((1..=crate::PAGES).contains(&saved.count));
                assert!(saved.pages[..saved.count].iter().all(|p| p.well_formed() && p.roomy()));
                assert!(saved.more.iter().flatten().all(|p| p.well_formed() && p.roomy()));
                more_read += saved.more.iter().flatten().count();
                assert!(saved.meta.off < 1 << crate::PAGES && saved.meta.auto < 4);
                assert!(saved.meta.names.iter().flatten().all(|c| crate::page::ALPHABET.contains(c)));
                meta_read += usize::from(saved.meta != crate::page::Meta::DEFAULT);
                // codes past the alphabet under a good CRC: the defaults, the pages whole
                if seed % 2 == 1 && seed % 31 == 0 && seed % 19 != 0 && seed % 23 != 0 && seed % 29 != 0 {
                    meta_fallback += usize::from(saved.meta == crate::page::Meta::DEFAULT);
                }
                if let Some(m) = saved.mtb {
                    assert!((1..=crate::page::MTB_PAGES).contains(&m.count));
                    assert!(m.pages[..m.count].iter().all(|p| p.well_formed() && p.roomy()));
                    assert!(m.more.iter().flatten().all(|p| p.well_formed() && p.roomy()));
                    mtb_read += 1;
                    if seed % 2 == 1 {
                        // the MTB pages, each drawn as the ride's pages (a profile fixed in BIKE)
                        app.mtb = m;
                        app.settings.profile = Some(crate::ride::Profile::Mtb);
                        for k in 0..m.count {
                            app.screen = crate::Screen::Ride(k);
                            app.show(&mut sim);
                            drawn += 1;
                        }
                        app.settings.profile = None;
                    }
                }
                if let Some(i) = saved.indoor {
                    assert!((1..=crate::PAGES).contains(&i.count));
                    assert!(i.pages[..i.count].iter().all(|p| p.well_formed() && p.roomy()));
                    assert!(i.more.iter().flatten().all(|p| p.well_formed() && p.roomy()));
                    indoor_read += 1;
                    if seed % 2 == 1 {
                        // the indoor pages, each drawn as the ride's pages (a profile fixed in BIKE)
                        app.indoor = i;
                        app.settings.profile = Some(crate::ride::Profile::Indoor);
                        for k in 0..i.count {
                            app.screen = crate::Screen::Ride(k);
                            app.show(&mut sim);
                            drawn += 1;
                        }
                        app.settings.profile = None;
                    }
                }
                good += 1;
                if seed % 2 == 1 {
                    // random pages: each drawn
                    for p in &saved.pages[..saved.count] {
                        app.pages[0] = *p;
                        app.screen = crate::Screen::Ride(0);
                        app.show(&mut sim);
                        drawn += 1;
                    }
                    // each MORE variant too, over a page, and the default one
                    for m in saved.more.iter().copied().chain([None]) {
                        (app.more[0], app.more_on) = (m, Some(0));
                        app.screen = crate::Screen::Ride(0);
                        app.show(&mut sim);
                        assert!(app.more_shown().is_some());
                        drawn += 1;
                    }
                    app.more_on = None;
                }
            }
            None => bad += 1,
        }
    }
    std::println!(
        "pages.bin copies: {good} read, {bad} refused; {drawn} random pages drawn; {more_read} MORE read; {indoor_read} indoor sets read; {mtb_read} MTB sets read"
    );
    assert!(good > 20 && bad > 1000 && drawn > 20 && more_read > 20, "{good} {bad} {drawn} {more_read}");
    assert!(indoor_read > 20, "the indoor part reached: {indoor_read}");
    assert!(mtb_read > 20, "the MTB part reached: {mtb_read}");
    assert!(meta_read > 20, "the switches part reached: {meta_read}");
    assert!(meta_fallback > 0, "a good CRC over bad name codes fell back to the defaults: {meta_fallback}");
}

/// Keys and taps by chance in the page editor and its actions (app/editor.rs), with a ride started and paused at
/// times: no panic, 1 to 6 pages all well formed, the panel equal to the screenshot, the editor never open while
/// recording; splits, merges, pages added and deleted all reached.
#[test]
fn page_editor_inputs() {
    use super::*;
    let (mut splits, mut merges, mut added, mut deleted) = (0, 0, 0, 0);
    let (mut mores, mut more_edits, mut more_kept) = (0, 0, 0);
    let (mut indoor_edits, mut indoor_changed, mut mtb_edits) = (0, 0, 0);
    for seed in 1..=48u32 {
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
                app.pages_set =
                    [ride::Profile::Indoor, ride::Profile::Road, ride::Profile::Mtb][usize::from(a / 3) % 3]; // the indoor set now and then (a ride's own while it is open)
                let n = app.view_count();
                app.screen = Screen::Ride(usize::from(b) % n);
                app.edit_page(&mut sim, usize::from(b) % n, usize::from(next()) % 8);
                app.show(&mut sim);
            }
            let i = match a % 16 {
                12 | 13 => Input::Touch(Gesture::Swipe(Dir::Up)), // MORE
                14 => Input::Touch(Gesture::Swipe(Dir::Down)),
                15 if b % 2 == 0 => Input::Touch(Gesture::Swipe(Dir::Left)),
                15 => Input::Touch(Gesture::Swipe(Dir::Right)),
                0..=2 => Input::Next,
                3 => Input::Prev,
                4..=6 => Input::Select,
                7 => Input::Back,
                8 => Input::Touch(Gesture::Tap(u16::from(b) % 240, u16::from(next()) % 320)),
                9 => Input::Touch(Gesture::Tap(
                    120,
                    (ui::LIST_Y + u32::from(b) % app.menu_rows() as u32 * app.theme.menu_row + 10) as u16,
                )),
                10 if b % 8 == 0 => {
                    app.start_pause(); // a ride recording, or paused again
                    continue;
                }
                _ => Input::Touch(Gesture::Hold(u16::from(b) % 240, u16::from(next()) % 320)),
            };
            let (count, cells) = (app.view_count(), app.edit.map(|e| e.work.cells()));
            let indoor = app.indoor;
            let was_more = app.edit.is_some_and(|e| e.more);
            app.input(&mut sim, i);
            for _ in 0..1 + usize::from(b % 3) {
                app.step(&mut sim);
                sim.delay_ms(POLL_MS);
            }
            added += usize::from(app.view_count() > count);
            deleted += usize::from(app.view_count() < count);
            indoor_edits += usize::from(app.edit.is_some() && app.view_set() == ride::Profile::Indoor);
            indoor_changed += usize::from(app.indoor != indoor);
            assert!(app.mtb.count <= crate::page::MTB_PAGES && app.mtb.count >= 1);
            mtb_edits += usize::from(app.edit.is_some() && app.view_set() == ride::Profile::Mtb);
            if let (Some(c), Some(e)) = (cells, app.edit) {
                splits += usize::from(e.work.cells() > c);
                merges += usize::from(e.work.cells() < c);
            }
            assert!((1..=PAGES).contains(&app.page_count()));
            assert!(app.pages()[..app.page_count()].iter().all(|p| p.well_formed()));
            assert!(app.more.iter().flatten().all(|p| p.well_formed()));
            assert!((1..=PAGES).contains(&app.indoor.count));
            assert!(app.indoor.pages[..app.indoor.count].iter().all(|p| p.well_formed()));
            assert!(app.indoor.more.iter().flatten().all(|p| p.well_formed()));
            assert_eq!(app.strip.more, app.more_shown().is_some(), "{:?}", app.screen());
            if let Some(n) = app.more_shown() {
                assert!(n < app.page_count(), "MORE up over page {n}");
                mores += 1;
            }
            more_edits += usize::from(app.edit.is_some_and(|e| e.more));
            more_kept += usize::from(was_more && app.edit.is_none() && app.more.iter().any(Option::is_some));
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
    std::println!("MORE reached: {mores} passes up, {more_edits} in the editor, {more_kept} kept");
    std::println!("indoor set reached: {indoor_edits} passes in its editor, {indoor_changed} changes");
    assert!(indoor_edits > 50 && indoor_changed > 5, "{indoor_edits} {indoor_changed}");
    assert!(mtb_edits > 0, "the MTB set reached: {mtb_edits}");
    assert!(splits > 10 && merges > 10 && added > 3 && deleted > 3, "{splits} {merges} {added} {deleted}");
    assert!(mores > 50 && more_edits > 5 && more_kept > 0, "{mores} {more_edits} {more_kept}");
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
        for round in 0..14 {
            // (14 rounds: SAVE is selected when the screen opens, so a Start ends a round sooner than RESUME did)
            let what = std::format!("seed {seed} round {round}");
            app.press(&mut sim, Press::Short(Key::Start));
            pass(&mut sim, &mut app, 3000);
            app.press(&mut sim, Press::Short(Key::Start));
            app.input(&mut sim, Input::End);
            assert_eq!(app.screen(), Screen::End(ui::END_SAVE), "{what}");
            let name = app.rec.clean.name();
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
                if app.ride.state() == ride::State::Running {
                    // RESUME, the one way to run it from here: paused again, then as Back
                    assert!(matches!(was, Screen::End(_)), "{what}: {i:?} on {was:?}");
                    app.start_pause();
                }
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
            banner_gone |= t > 5000 && app.notes.ms.is_none();
            tone_done |= t > 2000 && sim.tones.last() == Some(&0);
            dark |= app.light.shown == 0;
            opens = sim.nordic.got.iter().filter(|p| p.starts_with(&[6, 0xc4, 0])).count();
            // on time, whatever the passes: never early, never later than a few passes
            if t < 3900 {
                assert!(app.notes.ms.is_some(), "seed {seed}: banner early at {t} ms");
            }
            if t < SEARCH_REST_MS as u64 / 2 {
                assert!(opens <= 1, "seed {seed}: the rest cut short at {t} ms");
            }
            if t < 9900 {
                assert_ne!(app.light.shown, 0, "seed {seed}: backlight off early at {t} ms");
            }
        }
        assert!(banner_gone && tone_done && dark, "seed {seed}: {banner_gone} {tone_done} {dark}");
        assert!(opens >= 2, "seed {seed}: the strap searched again ({opens} opens, longest pass {worst} ms)");
    }
}

/// Notification answers on the Data Source (ANCS): made-up attribute lists with long, short, empty and non-ASCII
/// values (a lying length too), cut into fragments of random size, then kept in the inbox and wrapped. No panic, the
/// inbox keeps the newest eight, every wrapped line fits, and the whole answers were reached (counted).
#[test]
fn notification_answers() {
    use crate::phone::{DataSource, Inbox, wrap};
    let (mut whole, mut cut) = (0, 0);
    let mut inbox = Inbox::default();
    for seed in 1..=40u32 {
        let r: Vec<u8> = bytes(seed * 2654, 6000).collect();
        let mut at = 0;
        let mut take = |n: usize| {
            at += n;
            r.get(at - n..at).unwrap_or(&[]).to_vec()
        };
        let mut ds = DataSource::default();
        for round in 0..6usize {
            let lens = [usize::from(take(1)[0]) % 40, usize::from(take(1)[0]) % 40, usize::from(take(1)[0]) % 200];
            let mut a = std::vec![0u8, round as u8, 0, 0, 0];
            for (id, n) in [0u8, 1, 3].into_iter().zip(lens) {
                let lie = take(1)[0] % 16 == 0; // a length longer than the value
                a.push(id);
                a.extend(((n + usize::from(lie) * 300) as u16).to_le_bytes());
                a.extend(take(n));
            }
            let mut left = &a[..];
            while !left.is_empty() {
                let n = (usize::from(take(1)[0]) % 40 + 1).min(left.len());
                let (frag, rest) = left.split_at(n);
                left = rest;
                match ds.push(frag) {
                    Some(attrs) => {
                        whole += 1;
                        inbox.add(&attrs, (seed as u8 % 24, round as u8));
                        for k in 0..crate::phone::INBOX {
                            if let Some(n) = inbox.get(k) {
                                for cols in [1, 7, 19] {
                                    assert!(wrap(n.message.as_str(), cols).all(|l| l.len() <= cols && !l.is_empty()));
                                }
                            }
                        }
                    }
                    None => cut += 1,
                }
            }
        }
    }
    assert!(whole > 20 && cut > 20, "{whole} whole, {cut} cut");
    assert_eq!(inbox.len(), crate::phone::INBOX);
    assert!(inbox.get(crate::phone::INBOX).is_none());
}

/// Pages of the FE-C profile (random bytes behind a page number chosen from the ones decoded, a few others) into the
/// decoder, and random driving of the trainer model (calibrations started and cancelled, pages 1 and 2 of random
/// bytes and of the profile's structure): no panic, every page reached, every command a page of the profile with its
/// fields in range, nothing but the calibration request while one runs, and every step of a calibration reached.
#[test]
fn fec_pages_and_commands() {
    use crate::fec::{self, seen};
    use crate::trainer::{Command, Drive, Reading, Trainer};
    let mut got = 0u8;
    let mut pages = [0usize; 4];
    let mut cals = [0usize; 6]; // calibrations: speed, coast, done, failed, no answer, a request sent
    for seed in 1..=4u32 {
        let mut v = bytes(seed, 1 << 16);
        let mut r = || v.next().unwrap_or(0);
        let (mut reading, mut t) = (Reading::default(), Trainer::default());
        for _ in 0..4000 {
            let page = [16, 25, 25, 54, 71, 80, 81, 1, 2, 2, r()][usize::from(r() % 11)];
            let mut d: [u8; 8] = core::array::from_fn(|_| r());
            d[0] = page;
            if page == 2 {
                // the profile's structure: spin-down pending, the speed condition 1 or 2, a target speed
                d[1] = [0x80, 0x40, 0xc0, r()][usize::from(r() % 4)];
                d[2] = (d[2] & 0x3f) | [0x40, 0x80, 0x40, 0x80, r()][usize::from(r() % 5)] & 0xc0;
            }
            if page == 1 {
                d[1] = [0x80, 0x40, 0, r()][usize::from(r() % 4)];
            }
            fec::decode(d, &mut reading);
            got |= reading.seen;
            if let Some(h) = reading.power {
                assert!(h < 0xfff);
            }
            t.reading = reading;
            match r() % 40 {
                0 => t.start_calibration(),
                1 => t.cancel_calibration(),
                _ => {}
            }
            // the rider and the course at random
            let drive = Drive {
                erg_w: (r() % 2 == 0).then(|| u16::from_le_bytes([r(), r()])),
                step: usize::from(r() % 4),
                grade_x100: (r() % 2 == 0).then(|| i16::from_le_bytes([r(), r()])),
                riding: r() % 8 != 0,
                paused: r() % 4 == 0,
                match_q16: (r() % 2 == 0).then(|| i32::from(r()) * 400),
            };
            match r() % 4 {
                0 => t.step_by(&drive, r() % 2 == 0, r() % 2 == 0),
                1 => t.cycle_mode(&drive),
                _ => {}
            }
            let user = match t.setup(u16::from(r())) {
                Some(crate::trainer::Setup::User) => {
                    Some(Command::User { weight_cg: u16::from_le_bytes([r(), r()]), bike_50g: 190, wheel_mm: 668 })
                }
                _ => None,
            };
            let was = t.calibrating();
            let due = t.due(u32::from(r()) * 20, r() % 8 != 0, &drive);
            cals[match t.cal() {
                crate::trainer::Cal::Speed => 0,
                crate::trainer::Cal::Coast => 1,
                crate::trainer::Cal::Done(_) => 2,
                crate::trainer::Cal::Failed => 3,
                crate::trainer::Cal::NoAnswer => 4,
                _ => 5,
            }] += 1;
            if was && t.calibrating() {
                assert!(matches!(due, None | Some(Command::Calibrate(_))), "{due:?} during a calibration");
            }
            if let Some(Command::Calibrate(req)) = due {
                cals[5] += 1;
                let p = fec::encode(Command::Calibrate(req));
                assert!(p[0] == 1 && p[1] != 0 && p[1] & 0x3f == 0, "{p:?}");
                continue;
            }
            match r() % 7 {
                0 => t.unsent(), // a refusal or a TX_FAILED, whatever went out last
                1 => t.acked(),  // a TX_COMPLETED, a write's answer
                _ => {}
            }
            for c in [due, Some(t.command(&drive)), user].into_iter().flatten() {
                let p = fec::encode(c);
                pages[match p[0] {
                    0x30 => {
                        assert!(p[7] <= 200);
                        0
                    }
                    0x31 => {
                        assert!(u16::from_le_bytes([p[6], p[7]]) <= crate::trainer::MAX_W * 4);
                        1
                    }
                    0x33 => {
                        assert!(u16::from_le_bytes([p[5], p[6]]) <= 40_000);
                        2
                    }
                    0x37 => 3,
                    other => panic!("page {other:#x}"),
                }] += 1;
            }
            if r() % 50 == 0 {
                t.lost();
                reading = Reading::default();
            }
        }
    }
    let all = seen::P16 | seen::P25 | seen::P54 | seen::P71 | seen::P80 | seen::P81 | seen::P1 | seen::P2;
    assert_eq!(got, all, "every page decoded");
    assert!(cals[..5].iter().all(|&n| n > 20) && cals[5] > 20, "every calibration step reached: {cals:?}");
    assert!(pages.iter().all(|&n| n > 100), "every control page sent: {pages:?}");
}

/// The app with a trainer that sends random pages: it keeps running, the power and speed it feeds stay within what
/// the formats can say, and commands go out once the rider takes the trainer over.
#[test]
fn fec_pages_into_the_app() {
    use crate::sim::{AntSensor, Sim, run};
    use crate::{
        sensors::{Kind, Metric},
        *,
    };
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.backup[14] = Kind::Trainer.to_word(SensorId { device: 5555, trans: 0xf1 });
    sim.nordic.sensors.push(AntSensor { device_type: 0x11, id: 5555, data: |_| [0; 8], network: 0, ack: &[] });
    sim.nordic.trainer.fuzz = Some(0x1234_5679);
    let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8]));
    app.press(&mut sim, Press::Short(Key::Start));
    let mut seen_bits = 0;
    for k in 0..400 {
        run(&mut sim, &mut app, 250);
        seen_bits |= app.trainer.reading.seen;
        if k == 20 {
            app.open(Screen::Trainer);
            app.press(&mut sim, Press::Short(Key::Page));
        }
        if let Some(p) = app.live.get(Metric::Power) {
            assert!(p < 0xfff);
        }
        if let Some(s) = app.live.get(Metric::Speed) {
            assert!(s <= 2400, "{s}");
        }
    }
    assert_eq!(seen_bits, 0xff, "every page reached the app");
    assert!(sim.nordic.trainer.got.iter().any(|p| matches!(p[0], 0x30 | 0x31 | 0x33)), "commands went out");
}

/// The Bluetooth trainer's link (app/ftms.rs, drivers/ftms.rs): with a trainer connected on connection 1, random BLE
/// events on connection 0, 1 or any other (every id the link decodes, the trainer's notifications and indications on
/// its own handles with made-up Indoor Bike Data, Machine Status and Control Point responses, connections as a
/// central, their time-outs), between passes of the main loop that let it connect again. No panic, and each path
/// reached: events taken by the trainer's link, its data, status and responses. And random bytes into the FTMS
/// formats.
#[test]
fn ftms_link_events() {
    use super::ble_sim::{FT_CP, FT_DATA, FT_STATUS};
    use super::*;
    const IDS: [u16; 16] =
        [0x10, 0x11, 0x13, 0x1b, 0x1f, 0x21, 0x23, 0x52, 0x55, 0x56, 0x30, 0x32, 0x33, 0x36, 0x38, 0x39];
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.nordic.ftms = Some(FtmsSim::new());
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 4000);
    app.sensors_store.bt = sim.nordic.ftms.as_ref().map(FtmsSim::peer); // paired
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.ftms.conn, Some(1), "connected before the fuzz");
    let mut reached = [0usize; 4]; // owned, data, status, responses
    let mut gatt = [0usize; 5]; // Services, Chars, Descs, Read, ConnParamRequest
    let mut listed = [0usize; 3]; // advertising reports: listed, with a name, past the cap
    for seed in 1..=4u32 {
        let v: Vec<u8> = bytes(seed, 1 << 16).collect();
        for (k, c) in v.chunks_exact(48).enumerate() {
            // a third of them the trainer's notifications and indications
            let id = match c[42] % 4 {
                0 => 0x39,
                1 => 0x36, // a Read answer, shaped below
                _ => IDS[usize::from(c[0]) % IDS.len()],
            };
            let mut p = std::vec![2];
            p.extend(id.to_le_bytes());
            let conn = match c[47] % 4 {
                0 => 0,
                1 | 2 => 1,
                _ => u16::from_le_bytes([c[45], c[46]]),
            };
            p.extend(conn.to_le_bytes());
            if id == 0x39 && c[44] % 2 == 0 {
                // a notification or indication on one of the trainer's handles, with an FTMS-shaped value
                let handle = [FT_DATA, FT_CP, FT_STATUS][usize::from(c[43]) % 3];
                let value: Vec<u8> = match handle {
                    FT_CP => std::vec![0x80, [0x00, 0x04, 0x05, 0x11, c[3]][usize::from(c[4]) % 5], c[5] % 7],
                    FT_STATUS => std::vec![[0xff, 0x01, 0x08, c[3]][usize::from(c[4]) % 4]],
                    _ => c[3..3 + usize::from(c[2]) % 24].to_vec(),
                };
                p.extend([0, 0, 0, 0]); // status, error handle
                p.extend(handle.to_le_bytes());
                p.push(1 + c[6] % 2);
                p.extend((value.len() as u16).to_le_bytes());
                p.extend(&value);
            } else if id == 0x36 && c[44] % 4 != 0 {
                let value = &c[3..3 + usize::from(c[2]) % 8];
                p.extend([c[5] % 2 * 3, 0]); // status
                p.extend([0, 0, c[6] % 100, 0, 0, 0]); // error handle, the handle, offset
                p.extend((value.len() as u16).to_le_bytes());
                p.extend(value);
            } else {
                p.extend(&c[2..2 + usize::from(c[1]) % 42]);
            }
            if let Some(e) = crate::ble::event(&p)
                && app.ftms_owns(&e)
            {
                reached[0] += 1;
                use crate::ble::Event as E;
                match e {
                    E::Services { .. } => gatt[0] += 1,
                    E::Chars { .. } => gatt[1] += 1,
                    E::Descs { .. } => gatt[2] += 1,
                    E::Read { .. } => gatt[3] += 1,
                    E::ConnParamRequest { .. } => gatt[4] += 1,
                    _ => {}
                }
                if let crate::ble::Event::Hvx { handle, data, .. } = e {
                    let n = if handle == FT_DATA {
                        1
                    } else if handle == FT_STATUS {
                        2
                    } else {
                        usize::from(handle == FT_CP && crate::ftms::response(data).is_some()) * 3
                    };
                    reached[n] += usize::from(n > 0);
                }
            }
            app.phone_event(&mut sim, &p);
            if app.ftms.conn.is_none() {
                app.ftms_down(); // the link made again at once (not after the backoff), so the fuzz keeps reaching it
                run(&mut sim, &mut app, 1000);
            } else if k % 64 == 0 {
                run(&mut sim, &mut app, 500);
            }
        }
        // advertising data made of AD structures (flags, FTMS service data, UUID lists, names, junk), cut short now and then
        app.ftms.searching = true;
        for c in v.chunks_exact(24) {
            let mut ad = std::vec::Vec::new();
            for part in c[..21].chunks_exact(7) {
                match part[0] % 6 {
                    0 => ad.extend([2, 0x01, 6]),
                    1 => ad.extend([6, 0x16, 0x26, 0x18, 1 | part[1], part[2] | 0x20, part[3] & 1]),
                    2 => ad.extend([6, 0x16, 0x26, 0x18, part[1], part[2], part[3]]), // any flags and machine type
                    3 => ad.extend([5, 0x03, 0x0d, 0x18, 0x26, 0x18]),
                    4 => {
                        let n = usize::from(part[1] % 7);
                        ad.extend([n as u8 + 1, 0x09]);
                        ad.extend(&part[2..2 + n.min(5)]);
                        ad.extend(b"SIM BIKE".iter().take(n.saturating_sub(5))); // up to the length given
                    }
                    _ => ad.extend([part[1] % 9, part[2], part[3], part[4]]), // a structure whose length lies
                }
            }
            ad.truncate(usize::from(c[21]) % (ad.len() + 1));
            let r =
                crate::nrf::AdvReport { addr_type: c[22] % 2, addr: [c[23] % 7, 1, 2, 3, 4, 5], rssi: -50, data: &ad };
            if app.ftms.found(&r) {
                listed[0] += 1;
                listed[1] += usize::from(app.ftms.found.iter().take(app.ftms.n_found).any(|d| !d.name().is_empty()));
            }
            listed[2] += usize::from(app.ftms.n_found == app.ftms.found.len());
        }
        app.ftms.searching = false;
        for c in v.chunks(13) {
            let mut r = crate::trainer::Reading::default();
            crate::ftms::bike_data(c, &mut r);
            let _ = (crate::ftms::modes(c), crate::ftms::range(c), crate::ftms::response(c));
            let _ = (crate::ftms::advertised(c), crate::ftms::control_lost(c));
            let _ = crate::ftms::as_fec((c[0], c.get(1).copied().unwrap_or(0)));
        }
    }
    std::println!("ftms fuzz reached {reached:?} {gatt:?} {listed:?}");
    assert!(reached.iter().all(|&n| n > 50), "owned events, data, status, responses: {reached:?}");
    assert!(gatt.iter().all(|&n| n > 5), "Services, Chars, Descs, Read, ConnParamRequest: {gatt:?}");
    assert!(listed.iter().all(|&n| n > 5), "listed, named, past the cap: {listed:?}");
}

/// The FE-C over BLE trainer's link (drivers/fecble.rs, app/ftms.rs): with the D500 connected on connection 1,
/// notifications on its characteristic that are frames (valid with either checksum, wrong checksum, cut short,
/// wrong sync, length or message id, every page the app decodes), mixed with random BLE events on connections 0, 1
/// or any other, between passes that let the link be made again; advertising data with 128-bit lists (the service's
/// base, other UUIDs, cut short); and random bytes into the parsers. No panic, and each path reached: frames
/// accepted (with each checksum) and refused, every decoded page, the listing of FE-C trainers.
#[test]
fn fecble_link_events() {
    use super::ble_sim::FC_NOTIFY;
    use super::*;
    const IDS: [u16; 12] = [0x10, 0x11, 0x13, 0x1b, 0x21, 0x23, 0x52, 0x55, 0x30, 0x32, 0x33, 0x38];
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.nordic.ftms = Some(FtmsSim::new_fec());
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 4000);
    app.sensors_store.bt = sim.nordic.ftms.as_ref().map(FtmsSim::peer);
    app.sensors_store.bt_fec = true;
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.ftms.conn, Some(1), "connected before the fuzz");
    let mut frames = [0usize; 3]; // accepted with the XOR, with the sum, refused
    let mut seen = 0u8;
    let mut listed = [0usize; 2]; // FE-C trainers listed, advertising data without the service
    for seed in 1..=4u32 {
        let v: Vec<u8> = bytes(seed, 1 << 16).collect();
        for (k, c) in v.chunks_exact(24).enumerate() {
            let mut p = std::vec![2];
            if c[0] % 3 != 0 {
                // a notification on the trainer's characteristic: a frame, damaged one way or another
                let number = [16, 25, 54, 71, 80, 81, c[1], c[2]][usize::from(c[3]) % 8];
                let page = [number, c[4], c[5], c[6], c[7], c[8], c[9], c[10]];
                let mut f = crate::fecble::broadcast(page, c[11] % 2 == 0).to_vec();
                match c[12] % 8 {
                    0 => f[12] ^= 1 + c[13] % 255, // a wrong checksum
                    1 => f.truncate(usize::from(c[13]) % 13),
                    2 => f[0] = c[13],
                    3 => f[1] = c[13],
                    4 => f[2] = c[13],
                    5 => f.extend(&c[14..14 + usize::from(c[13]) % 8]), // longer than a frame
                    _ => {}
                }
                match crate::fecble::parse(&f) {
                    Some((_, sum)) => frames[usize::from(sum)] += 1,
                    None => frames[2] += 1,
                }
                p.extend(0x39u16.to_le_bytes());
                p.extend(1u16.to_le_bytes());
                p.extend([0, 0, 0, 0]);
                p.extend(FC_NOTIFY.to_le_bytes());
                p.push(1);
                p.extend((f.len() as u16).to_le_bytes());
                p.extend(&f);
            } else {
                p.extend(IDS[usize::from(c[1]) % IDS.len()].to_le_bytes());
                p.extend(u16::from(c[2] % 3).to_le_bytes());
                p.extend(&c[3..3 + usize::from(c[4]) % 20]);
            }
            app.phone_event(&mut sim, &p);
            seen |= app.trainer.reading.seen;
            if app.ftms.conn.is_none() {
                app.ftms_retry(); // at once, the chip is not reset: its UUID base stays registered
                run(&mut sim, &mut app, 1000);
            } else if k % 64 == 0 {
                run(&mut sim, &mut app, 500);
            }
        }
        // advertising data: 128-bit lists (the base or not), flags, FTMS-less 16-bit lists, junk, cut short
        app.ftms.searching = true;
        for c in v.chunks_exact(24) {
            let mut ad = std::vec::Vec::new();
            for part in c[..20].chunks_exact(5) {
                match part[0] % 5 {
                    0 => ad.extend([2, 0x01, 6]),
                    1 => {
                        ad.extend([17, 0x06 + part[1] % 2]);
                        ad.extend(crate::fecble::BASE);
                    }
                    2 => {
                        ad.extend([17, 0x07]);
                        ad.extend([part[1]; 16]);
                    }
                    3 => ad.extend([5, 0x03, 0x18, 0x18, 0x16, 0x18]),
                    _ => ad.extend([part[1] % 20, part[2], part[3], part[4]]), // a structure whose length lies
                }
            }
            ad.truncate(usize::from(c[21]) % (ad.len() + 1));
            let r =
                crate::nrf::AdvReport { addr_type: c[22] % 2, addr: [c[23] % 7, 1, 2, 3, 4, 5], rssi: -50, data: &ad };
            let fec = crate::fecble::advertised(&ad) == Some(crate::fecble::Proto::Fec);
            listed[0] += usize::from(app.ftms.found(&r) && fec);
            listed[1] += usize::from(crate::fecble::advertised(&ad).is_none());
        }
        app.ftms.searching = false;
        for c in v.chunks(13) {
            let _ = crate::fecble::parse(c);
            let _ = crate::fecble::advertised(c);
        }
    }
    std::println!("fecble fuzz reached {frames:?} {seen:#x} {listed:?}");
    assert!(frames.iter().all(|&n| n > 50), "accepted with each checksum, refused: {frames:?}");
    assert_eq!(seen & 0x3f, 0x3f, "every page decoded");
    assert!(listed.iter().all(|&n| n > 5), "FE-C trainers listed, advertising without the service: {listed:?}");
}

/// Random sections and riders into the climb planner (pacing.rs): it never panics, never gives a power outside CP to
/// CP + W' / 60 or a time that is not finite, and the seeds do reach the solver (plans that spend W', plans with
/// sections of every sign of grade).
#[test]
fn climb_planner() {
    let (mut solved, mut spent) = (0, 0);
    for seed in 1..400u32 {
        let b: Vec<u8> = bytes(seed, 8 * crate::pacing::MAX + 16).collect();
        let w = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let n = usize::from(b[0]) % (crate::pacing::MAX + 4);
        let secs: Vec<crate::pacing::Sec> = (0..n)
            .map(|k| crate::pacing::Sec { len_m: w(2 + 4 * k) % 1500, grade_x10: (w(4 + 4 * k) % 400) as i16 - 150 })
            .collect();
        let r = crate::pacing::Rider {
            cp: f32::from(w(4) % 500),
            w_full: f32::from(w(6)) * 0.4,
            budget: f32::from(w(8)) * 0.6,
            mass: f32::from(b[10]) - 20.0,
        };
        let p = crate::pacing::plan(&secs, &r);
        if r.cp < 1.0 {
            assert_eq!(p.n, 0, "seed {seed}");
        } else if n > 0 {
            solved += 1;
            let hi = r.cp + r.w_full / 60.0 + 1.0;
            assert!(p.watts[..usize::from(p.n)].iter().all(|&x| f32::from(x) >= r.cp - 1.0 && f32::from(x) <= hi));
            spent += usize::from(p.watts[..usize::from(p.n)].iter().any(|&x| f32::from(x) > r.cp + 1.0));
        }
    }
    assert!(solved > 150 && spent > 50, "reached the solver: {solved} plans, {spent} spending W'");
}

/// The map files' tiles (storage/route.rs, inflate.rs): the real gzip streams of the synthetic file with bytes
/// changed and cut, and random bytes behind a valid gzip header, into the inflater. Whatever comes out is the
/// original or an error; the errors seen are every kind of them (the fuzz reaches the inflater's checks).
#[test]
fn gzip_streams() {
    use crate::inflate::{Error, Scratch, gunzip};
    let file = crate::route::tests::fixture();
    let starts: Vec<usize> = (0..file.len() - 3).filter(|&i| file[i..i + 3] == [0x1f, 0x8b, 8]).collect();
    assert_eq!(starts.len(), 8, "a stream a tile");
    let mut noise = bytes(77, 1 << 22);
    let mut next = move || u32::from_le_bytes([0; 4].map(|_| noise.next().unwrap_or(0)));
    let mut mem = std::vec![0u16; 48 * 1024];
    let mut small = std::vec![0u16; 64];
    let (mut ok, mut kinds) = (0, [0u32; 4]);
    let run = |gz: &[u8], mem: &mut Vec<u16>, want: Option<&[u8]>| -> Result<usize, Error> {
        let mut out = Scratch(mem);
        let r = gunzip(&mut gz.iter().copied(), &mut out);
        // the same stream in slices of 1 to 64 bytes, the input taken up again where the inflater says: the same
        // answer, the same bytes (the resumable inflater, which the card's reads in slices rely on)
        let mut w = std::vec![0u16; out.0.len()];
        let mut o2 = Scratch(&mut w);
        let mut inf = crate::inflate::Inflater::new();
        let r2 = loop {
            let budget = 1 + (gz.len() * 31 + inf.written() * 7) % 64;
            let mut src = gz.iter().copied().skip(inf.taken as usize);
            match inf.step(&mut src, &mut o2, budget) {
                Ok(None) => {}
                Ok(Some(n)) => break Ok(n),
                Err(e) => break Err(e),
            }
        };
        assert_eq!(r, r2, "sliced or whole");
        if let Ok(n) = r {
            assert!((0..n).all(|i| out.get(i) == o2.get(i)), "sliced or whole: other bytes");
        }
        if let (Ok(n), Some(w)) = (&r, want) {
            assert!((0..*n).map(|i| out.get(i).unwrap()).eq(w.iter().copied()), "an answer that is not the original");
        }
        r
    };
    for (k, &a) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(file.len());
        let good = &file[a..end];
        let n = run(good, &mut mem, None).expect("the real stream");
        let want: Vec<u8> = (0..n).map(|i| Scratch(&mut mem).get(i).unwrap()).collect();
        ok += 1;
        for _ in 0..300 {
            let mut bad = good.to_vec();
            for _ in 0..1 + next() % 6 {
                let at = next() as usize % bad.len();
                bad[at] = next() as u8;
            }
            if next().is_multiple_of(3) {
                let cut = next() as usize % (bad.len() + 1);
                bad.truncate(cut);
            }
            let m = if next().is_multiple_of(4) { &mut small } else { &mut mem };
            if let Err(e) = run(&bad, m, Some(&want)) {
                kinds[match e {
                    Error::Truncated => 0,
                    Error::Bad => 1,
                    Error::TooBig => 2,
                    Error::Crc => 3,
                }] += 1;
            }
        }
    }
    // random deflate behind a valid header, in a small buffer too
    for _ in 0..4000 {
        let mut gz = std::vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];
        gz.extend((0..next() % 300).map(|_| next() as u8));
        let m = if next().is_multiple_of(2) { &mut mem } else { &mut small };
        if let Err(e) = run(&gz, m, None) {
            kinds[match e {
                Error::Truncated => 0,
                Error::Bad => 1,
                Error::TooBig => 2,
                Error::Crc => 3,
            }] += 1;
        }
    }
    std::println!("gzip fuzz: {ok} real streams, errors truncated / bad / too big / crc: {kinds:?}");
    assert!(kinds.iter().all(|&n| n > 20), "{kinds:?}");
}

/// Tile messages built from random parts (the base, deltaBits, transport, nodes with random keys, elevations, ways and
/// edges, unknown fields, wire types that do not exist), whole, damaged and cut, into the tile decoder, several into
/// one graph until it is full. A tile is whole or refused (the graph as it was), the nodes of a tile found where
/// they are, and the fuzz reaches nodes, edges, refusals and a full graph.
#[test]
fn route_tiles() {
    use crate::inflate::Scratch;
    use crate::route::{Error, Graph, NODES};
    let mut next = xorshift(91);
    fn varint(v: u64, out: &mut Vec<u8>) {
        let mut v = v;
        loop {
            out.push((v & 0x7f) as u8 | if v >> 7 != 0 { 0x80 } else { 0 });
            v >>= 7;
            if v == 0 {
                return;
            }
        }
    }
    fn field(f: u32, wire: u32, out: &mut Vec<u8>) {
        varint(u64::from(f << 3 | wire), out);
    }
    let zz = |v: i32| (((v << 1) ^ (v >> 31)) as u32).into();
    let mut mem = std::vec![0u16; 160 * 1024]; // room for a tile of 3000 nodes
    let (mut tiles_ok, mut nodes_in, mut refused, mut fulls, mut edges_in) = (0u32, 0u32, 0u32, 0u32, 0u32);
    let mut g = std::boxed::Box::new(Graph::new());
    for round in 0..600u32 {
        if round % 6 == 0 {
            g.clear();
        }
        let bits = match next() % 10 {
            0 => next() % 40,
            1 => 2 + next() % 30,
            _ => 20,
        };
        let (lat0, lon0) = ((next() % 2_000_000) as i32 - 1_000_000, (next() % 2_000_000) as i32 - 1_000_000);
        let mut m = Vec::new();
        let corrupt = next().is_multiple_of(6); // some messages have fields of wire types that do not exist
        let near = |next: &mut dyn FnMut() -> u32| -> u64 {
            let half = u64::from(next()) & ((1u64 << bits.clamp(2, 31)) - 1);
            let small = u64::from(next() % 3000) | (u64::from(next() % 2) << (bits.clamp(2, 31) - 1));
            let (hi, lo) =
                if next().is_multiple_of(2) { (half, u64::from(next())) } else { (small, u64::from(next() % 3000)) };
            ((hi << bits.clamp(2, 31)) | (lo & ((1u64 << bits.clamp(2, 31)) - 1)))
                & if next().is_multiple_of(40) { u64::MAX } else { (1u64 << (2 * bits.clamp(1, 31))) - 1 }
        };
        for (f, v) in
            [(1, zz(lat0)), (2, zz(lon0)), (3, u64::from(bits)), (4, if next().is_multiple_of(20) { 3 } else { 2 })]
        {
            if !next().is_multiple_of(30) {
                field(f, 0, &mut m);
                varint(v, &mut m);
            }
        }
        let n = match next() % 12 {
            0 => NODES as u32 / 2 + next() % 2000,
            1 => 0,
            _ => 1 + next() % 120,
        };
        for _ in 0..n {
            let mut node = Vec::new();
            if !next().is_multiple_of(50) {
                field(1, 0, &mut node);
                varint(near(&mut next), &mut node);
            }
            if !next().is_multiple_of(8) {
                field(2, 0, &mut node);
                varint(
                    if next().is_multiple_of(10) { u64::from(next()) << 20 } else { u64::from(next() % 40_000) },
                    &mut node,
                );
            }
            let mut ways = Vec::new();
            for _ in 0..next() % 5 {
                varint(
                    if next().is_multiple_of(4) { u64::MAX - u64::from(next()) } else { u64::from(next()) },
                    &mut ways,
                );
            }
            if !next().is_multiple_of(6) {
                field(3, 2, &mut node);
                varint(ways.len() as u64, &mut node);
                node.extend(ways);
            }
            for _ in 0..next() % 5 {
                let mut e = Vec::new();
                field(1, 0, &mut e);
                varint(near(&mut next), &mut e);
                field(2, 0, &mut e);
                varint(u64::from(next() % 256), &mut e);
                if next().is_multiple_of(20) {
                    field(9, 5, &mut e);
                    e.extend([1, 2, 3, 4]);
                }
                field(4, 2, &mut node);
                varint(e.len() as u64, &mut node);
                node.extend(e);
                edges_in += 1;
            }
            if next().is_multiple_of(25) {
                let wire = [0, 1, 2, 5, 3, 4, 6, 7][next() as usize % if corrupt { 8 } else { 4 }];
                field(40 + next() % 20, wire, &mut node);
                match wire {
                    0 => varint(u64::from(next()), &mut node),
                    1 => node.extend((0..8).map(|_| next() as u8)),
                    2 => {
                        let k = next() % 10;
                        varint(u64::from(k), &mut node);
                        node.extend((0..k).map(|_| next() as u8));
                    }
                    5 => node.extend((0..4).map(|_| next() as u8)),
                    _ => node.extend((0..next() % 12).map(|_| next() as u8)),
                }
            }
            field(5, 2, &mut m);
            varint(node.len() as u64, &mut m);
            m.extend(node);
        }
        if next().is_multiple_of(5) {
            let at = next() as usize % (m.len() + 1);
            m.truncate(at);
        }
        if next().is_multiple_of(5) && !m.is_empty() {
            for _ in 0..1 + next() % 4 {
                let at = next() as usize % m.len();
                m[at] = next() as u8;
            }
        }
        if m.len() > 2 * mem.len() {
            continue;
        }
        let mut s = Scratch(&mut mem);
        for (i, &b) in m.iter().enumerate() {
            assert!(s.set(i, b));
        }
        let before = (g.len(), g.nodes().to_vec());
        let (x, y) = (round % 12, round / 12 % 12);
        match g.decode(x, y, &s, m.len()) {
            Ok(k) => {
                tiles_ok += 1;
                nodes_in += k as u32;
                assert_eq!(g.len(), before.0 + k);
                for node in g.nodes().iter().skip(before.0) {
                    let i = g.find(node.lat, node.lon).expect("a node of a tile is found");
                    let at = g.node(i).unwrap();
                    assert_eq!((at.lat, at.lon), (node.lat, node.lon));
                }
            }
            Err(e) => {
                refused += 1;
                fulls += u32::from(e == Error::Full);
                assert_eq!((g.len(), g.nodes()), (before.0, &before.1[..]), "refused whole: the graph as it was");
            }
        }
    }
    std::println!(
        "route fuzz: {tiles_ok} tiles, {nodes_in} nodes, {edges_in} edges made, {refused} refused, {fulls} full"
    );
    assert!(tiles_ok > 100 && nodes_in > 3000 && refused > 50 && fulls > 3, "{tiles_ok} {nodes_in} {refused} {fulls}");
}

/// The whole chain on damaged map files: the synthetic file with random bytes changed (and sometimes cut off by a
/// short file size), a rider along its road, the tiles read from the card, snapped, traced, smoothed, climbs found.
/// Nothing panics; the fuzz reaches traces made, climbs found and said, and tries that fail.
#[test]
fn damaged_maps_under_a_ride() {
    use crate::climbs_ahead::{Ahead, tests::fix};
    use crate::fat::Fat;
    use crate::route::tests::{image_with_map, metres};
    let img = image_with_map();
    let mut d0 = crate::fat::tests::Ram(img.clone(), 0);
    let mut fat = Fat::mount(&mut d0).unwrap();
    let f = fat.open(&mut d0, "Map/MapData/Test/9-49-342.dat").unwrap();
    let base = fat.lba(f.cluster) as usize * crate::fat::SECTOR;
    let _ = metres(0.0, 0.0);
    let mut noise = bytes(5, 1 << 24);
    let mut next = move || u32::from_le_bytes([0; 4].map(|_| noise.next().unwrap_or(0)));
    let mut mem = std::vec![0u16; 48 * 1024];
    let (mut traces, mut climbs, mut said, mut tiles, mut quiet, mut flips) = (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    let mut a = std::boxed::Box::new(Ahead::new());
    for round in 0..120 {
        let mut d = crate::fat::tests::Ram(img.clone(), 0);
        if round > 0 {
            for _ in 0..1 + next() % 6 {
                let at =
                    if next().is_multiple_of(2) { next() as usize % 400 } else { next() as usize % f.size as usize };
                d.0[base + at] = next() as u8;
            }
        }
        *a = Ahead::new();
        a.set_trails(round % 2 == 1);
        // along the road east, a little off it now and then, sometimes the other way or somewhere else
        let (mut x, mut y, mut h) = (100.0 + f64::from(next() % 400), 0.0, 90.0);
        for t in 0..150 {
            x += 8.0 * if h < 180.0 { 1.0 } else { -1.0 };
            if next().is_multiple_of(50) {
                y += f64::from(next() % 200) - 100.0;
            }
            if next().is_multiple_of(200) {
                h = f64::from(next() % 360);
            }
            let fx = if next().is_multiple_of(40) {
                fix(f64::from(next() % 9000), f64::from(next() % 400) - 200.0, h)
            } else {
                fix(x, y, h)
            };
            if a.second(true, Some(fx)).is_some() {
                said += 1;
            }
            // the mode switched at a random second, sometimes while a trace or a tile is in progress
            if next().is_multiple_of(25) {
                flips += u32::from(a.busy() || a.loading());
                a.set_trails(!a.trails());
            }
            let mut steps = 0;
            while a.work(&mut d, &mut mem) && steps < 20 {
                steps += 1;
            }
            let _ = (a.alt_at(next() % 5000), a.grade_ahead(next() % 5000), a.climb_at(next() % 5000), t);
        }
        // the round ends sane: a trace still under way is finished, and then nothing is busy
        let mut steps = 0;
        while a.work(&mut d, &mut mem) {
            steps += 1;
            assert!(steps < 100_000, "a trace that never finishes");
        }
        assert!(!a.busy(), "idle once the work says so");
        traces += a.traces;
        climbs += a.climbs().len() as u32;
        tiles += a.io().0;
        quiet += u32::from(!a.on_road());
    }
    std::println!(
        "damaged maps: {traces} traces, {climbs} climbs, {said} said, {tiles} tiles read, {quiet} rides off the trace, \
         {flips} mode switches mid-work"
    );
    assert!(
        traces > 100 && climbs > 20 && said > 10 && tiles > 300 && quiet > 5 && flips > 0,
        "{traces} {climbs} {said} {tiles} {quiet} {flips}"
    );
}

/// Random road networks (random walks that branch, loop, end, with random elevations, ways and classes) and random
/// riders on them, in many tiles, the graph emptied now and then: snap, trace, smoothing and climbs on whatever they
/// are. Reaches traces, climbs, ends of both kinds.
#[test]
fn random_road_networks() {
    use crate::climbs_ahead::{
        Ahead, End,
        tests::{fix, net},
    };
    /// A card that does not answer: the tiles the graph lacks cannot be read, and that try fails.
    struct Dead;
    impl crate::fat::Disk for Dead {
        fn read(&mut self, _: u32, _: &mut [u8]) -> Result<(), crate::fat::Error> {
            Err(crate::fat::Error::Io)
        }
    }
    let mut next = xorshift(404);
    let mut mem = std::vec![0u16; 48 * 1024];
    let (mut traces, mut climbs, mut stops, mut lengths, mut sent, mut slow) = (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    let mut a = std::boxed::Box::new(Ahead::new());
    for round in 0..150 {
        *a = Ahead::new();
        a.set_trails(round % 2 == 1);
        type Road = (u16, u8, Vec<(f64, f64, i16)>);
        let mut roads: Vec<Road> = Vec::new();
        for r in 0..1 + next() % 5 {
            let (mut x, mut y) = (f64::from(next() % 3000) - 1500.0, f64::from(next() % 3000) - 1500.0);
            let (mut dir, mut z) = (f64::from(next() % 360).to_radians(), 100 + (next() % 300) as i16);
            let wild = if next().is_multiple_of(4) { 1.0 } else { 0.2 };
            let mut pts = Vec::new();
            for _ in 0..20 + next() % 220 {
                pts.push((x, y, if next().is_multiple_of(40) { crate::route::NO_ELEV } else { z }));
                let step = 3.0 + f64::from(next() % 60);
                dir +=
                    (f64::from(next() % 90) - 45.0).to_radians() * if next().is_multiple_of(30) { 3.0 } else { wild };
                (x, y) = (x + step * dir.sin(), y + step * dir.cos());
                z += (next() % 7) as i16 - 3 + if next().is_multiple_of(3) { 3 } else { 0 };
                if next().is_multiple_of(90) {
                    break;
                }
            }
            roads.push((1 + (r % 3) as u16, [0xee, 0x99, 0x55, 0x1e][next() as usize % 4], pts));
        }
        let borrowed: Vec<_> = roads.iter().map(|(w, c, p)| (*w, *c, &p[..])).collect();
        net(&mut a, &borrowed);
        for _ in 0..20 {
            // a rider on a random point of a random road, or off them
            let r = &roads[next() as usize % roads.len()].2;
            let p = r[next() as usize % r.len()];
            let (jx, jy) = (f64::from(next() % 80) - 40.0, f64::from(next() % 80) - 40.0);
            let f = if next().is_multiple_of(6) {
                fix(f64::from(next() % 6000) - 3000.0, f64::from(next() % 6000) - 3000.0, 0.0)
            } else {
                fix(p.0 + jx, p.1 + jy, f64::from(next() % 360))
            };
            for _ in 0..8 {
                if a.second(true, Some(f)).is_some() {
                    sent += 1;
                }
                let mut guard = 0;
                while a.work(&mut Dead, &mut mem) && guard < 30 {
                    guard += 1;
                }
                if a.on_road() && next().is_multiple_of(2) {
                    break;
                }
            }
            slow += u32::from(round % 2 == 1 && a.on_road() && a.on_slow());
            climbs += a.climbs().len() as u32;
            stops += u32::from(a.on_road() && a.end() == End::Stop);
            lengths += u32::from(a.on_road() && a.end() == End::Length);
            let _ = (a.alt_at(next() % 6000), a.grade_ahead(next() % 6000));
            a.reset();
        }
        traces += a.traces;
        let _ = round;
    }
    std::println!(
        "random networks: {traces} traces, {climbs} climbs, {stops} stopped, {lengths} at length, {sent} said"
    );
    assert!(traces > 300 && climbs > 20 && stops > 20 && lengths > 20, "{traces} {climbs} {stops} {lengths}");
    assert!(slow > 0, "trails rounds that ended on a slow edge: {slow}");
}

/// The ramp test's arithmetic (ramp.rs) on random power, cadence and DFA a1 series: missing readings, spikes, a1 out
/// of range, targets of 0 and of 65535 W. Never a panic; the FTP is 75 % of the best minute and the best minute never
/// more than the highest power; a threshold lies among the powers ridden. Half the series have a1 fall as power
/// climbs, so the threshold is found (asserted over the seeds), and many stop on a rule.
#[test]
fn ramp_results() {
    use crate::ramp::Ramp;
    let (mut with_aet, mut stopped, mut with_result) = (0, 0, 0);
    for seed in 1..=300u32 {
        let mut x = seed.wrapping_mul(2_654_435_761) | 1;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x >> 8
        };
        let falling = seed % 2 == 0;
        let (mut r, mut top) = (Ramp::new(), 0u16);
        for s in 0..1500u32 {
            let base = (80 + s / 5) as u16;
            let power = match next() % 40 {
                0 | 1 => None,
                2 => Some(u16::MAX),
                3 => Some((next() * 7) as u16),
                _ => Some(base + (next() % 30) as u16),
            };
            let cadence = [None, Some(0), Some(59), Some(60), Some(90), Some(255)][(next() % 6) as usize];
            let a1 = match next() % 20 {
                0 => None,
                1 => Some((next() * 5) as u16),
                _ if falling => Some((130 - base / 4).clamp(20, 160) + (next() % 6) as u16),
                _ => Some(40 + (next() % 100) as u16),
            };
            let hr = [None, Some(base / 3 + 60), Some(0), Some(u16::MAX)][(next() % 4) as usize];
            // mostly a target the rider meets, sometimes none or an absurd one
            let target = [0, base, base, base, base, u16::MAX][((s / 25 + seed) % 6) as usize]; // in blocks, so streaks happen
            top = top.max(power.unwrap_or(0));
            if r.second(power, if (s / 7 + seed) % 5 == 0 { cadence } else { Some(90) }, target, (a1, hr)) {
                stopped += 1;
            }
        }
        if let Some(o) = r.outcome() {
            with_result += 1;
            assert!(o.best_w <= top && o.ftp.abs_diff(o.best_w * 3 / 4) <= 1, "{o:?} top {top}");
            assert_eq!(o.secs, 1500);
            if let Some(h) = o.aet {
                with_aet += 1;
                assert!(h.watts <= top, "{h:?} over the top {top}");
            }
        }
    }
    assert!(with_result > 250 && with_aet > 20 && stopped > 100, "{with_result} {with_aet} {stopped}");
}

/// Random and damaged copies of `\Rustfw\settings.bin` (app/settings_file.rs `SettingsCopy`), built field by field
/// (the magic, the version, the length, the sequence number, the CRC each right or wrong, the settings' bytes in and
/// out of their ranges, one copy or two): no panic; whatever is read holds only values the menus offer, the newer
/// whole copy wins; and every path is reached: copies refused (magic, version, length, CRC), read whole, read short
/// (the defaults past the length), with a value out of range (its default).
#[test]
fn settings_file() {
    use crate::{SettingsCopy, menu::Setting};
    let mut next = xorshift(0x5e77);
    let d = crate::Settings::from_word(0);
    let (mut refused, mut whole, mut short, mut defaulted) = (0u32, 0u32, 0u32, 0u32);
    let (mut kept, mut fell_back) = (0u32, 0u32); // THEME's names
    let (mut lap_kept, mut lap_fell_back) = (0u32, 0u32); // AUTO LAP's three bytes
    let (mut seg_kept, mut seg_fell_back) = (0u32, 0u32); // LIVE SEGMENTS' and TARGET's bytes
    let (mut un_kept, mut un_fell_back, mut trips_read) = (0u32, 0u32, 0u32); // SOUND, UNITS, TEMP, AUTO PAUSE; the trips
    let (mut auto_on, mut day_night_kept, mut day_night_fell_back) = (0u32, 0u32, 0u32); // AUTO, and DAY and NIGHT's names
    let (mut with_limit, mut wild_limit) = (0u32, 0u32); // limits read, and out of range ones read as 0
    let (mut with_lthr, mut wild_lthr, mut with_smooth, mut wild_smooth) = (0u32, 0u32, 0u32, 0u32);
    for _ in 0..20_000 {
        let mut file = [0u8; 1024];
        let mut want: Option<u32> = None; // the newest copy built whole
        for slot in 0..1 + (next() % 2) as usize {
            let s = &mut file[512 * slot..512 * (slot + 1)];
            let r = next();
            s[..4].copy_from_slice(if r.is_multiple_of(13) { b"RSEX" } else { b"RSET" });
            s[4..6].copy_from_slice(&(if r.is_multiple_of(11) { 2u16 } else { 1 }).to_le_bytes());
            let len: u16 = match r % 7 {
                0 => (next() % 600) as u16,     // past what a copy holds, or anywhere
                1 => (next() % 31) as u16,      // an earlier build's
                2 => 31 + (next() % 60) as u16, // a later build's
                3 => 44 + (next() % 82) as u16, // with the alert limits, LTHR, POWER SMOOTH, AUTO, the names and the segments' bytes: whole or cut
                _ => 31,
            };
            s[6..8].copy_from_slice(&len.to_le_bytes());
            let seq = next() % 8;
            s[8..12].copy_from_slice(&seq.to_le_bytes());
            for b in s[12..508].iter_mut() {
                *b = next() as u8;
            }
            let mut theme: Option<([u8; 12], bool)> = None; // a name written at 59, and whether it reads as one
            let mut lim = [None::<(u16, bool)>; 11]; // the limits written at 28 + 44 + 2n, and whether in range
            let mut lthr_w = None::<(u16, bool)>; // LTHR at 66, POWER SMOOTH at 68: written, and in range
            let mut ps_w = None::<(u8, bool)>;
            let mut auto_w = None::<u8>; // AUTO at 69 (a byte: 0, 1, or out of its form)
            let mut lap_w = [None::<(u8, bool)>; 3]; // AUTO LAP, km, min at 94, 95, 96: written, and in range
            let mut seg_w = [None::<(u8, bool)>; 2]; // LIVE SEGMENTS, TARGET at 97, 98: written, and in range
            let mut un_w = [None::<(u8, bool)>; 4]; // SOUND, UNITS, TEMPERATURE, AUTO PAUSE at 99..102: written, in range
            let mut day_w = None::<([u8; 12], bool)>; // DAY THEME at 70, NIGHT THEME at 82: the name, and whether it is one
            let mut night_w = None::<([u8; 12], bool)>;
            // the settings: mostly what a build writes (a wheel offered, numbers in range), else random
            if !next().is_multiple_of(4) {
                let ok = crate::Settings { ftp: 100 + 5 * (next() % 80) as u16, ..d };
                let good = SettingsCopy { seq, words: [0; 4], settings: ok }.to_sector();
                let n = usize::from(len).min(31);
                s[28..28 + n].copy_from_slice(&good[28..28 + n]);
                if next().is_multiple_of(3) {
                    s[28 + (next() % 31) as usize] = next() as u8; // one byte out of its range, maybe
                }
                if len >= 43 {
                    // THEME's name: a valid stem (padded or all 12), a bad byte, or a NUL and then a byte
                    let stem = |next: &mut dyn FnMut() -> u32, n: usize| {
                        let mut name = [0u8; 12];
                        for b in name.iter_mut().take(n) {
                            *b = b"AZaz09-_"[(next() % 8) as usize];
                        }
                        name
                    };
                    let n = 1 + (next() % 12) as usize;
                    let mut name = stem(&mut next, n);
                    match next() % 4 {
                        0 => name[(next() % n as u32) as usize] = [b'/', 0xff, b' ', b'.'][(next() % 4) as usize],
                        1 if n < 11 => name[n + 1 + (next() % (11 - n as u32)) as usize] = b'X',
                        _ => {}
                    };
                    let bad = name.iter().any(|&b| !(b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == 0))
                        || name.iter().skip_while(|&&b| b != 0).any(|&b| b != 0);
                    s[28 + 31..28 + 43].copy_from_slice(&name);
                    theme = Some((name, !bad));
                }
                for (n, slot) in lim.iter_mut().enumerate() {
                    let at = 28 + 44 + 2 * n;
                    if usize::from(len) >= 44 + 2 * n + 2 {
                        let range = Setting::Limit(n as u8).number().unwrap().0;
                        let ok = next().is_multiple_of(2);
                        let v = if ok {
                            (next() % (u32::from(*range.end()) + 1)) as u16
                        } else {
                            60_000 + (next() % 5000) as u16
                        };
                        s[at..at + 2].copy_from_slice(&v.to_le_bytes());
                        *slot = Some((v, ok));
                    }
                }
                if usize::from(len) >= 68 {
                    // LTHR: 0 (unset), 100 to 230, or one out of range (1 to 99 or above 230 reads unset)
                    let v: u16 =
                        if next().is_multiple_of(4) { 60_000 + (next() % 5000) as u16 } else { (next() % 231) as u16 };
                    let ok = v == 0 || (100..=230).contains(&v);
                    s[28 + 66..28 + 68].copy_from_slice(&v.to_le_bytes());
                    lthr_w = Some((v, ok));
                }
                if len >= 69 {
                    // POWER SMOOTH: an index of the four, or one out of range
                    let v = if next().is_multiple_of(4) { (4 + next() % 252) as u8 } else { (next() % 4) as u8 };
                    s[28 + 68] = v;
                    ps_w = Some((v, usize::from(v) < 4));
                }
                if usize::from(len) >= 70 {
                    // AUTO: a byte, 0 or 1 (on for 1), or one out of its form (off)
                    let v = if next().is_multiple_of(4) { (2 + next() % 254) as u8 } else { (next() % 2) as u8 };
                    s[28 + 69] = v;
                    auto_w = Some(v);
                }
                for (at, w, whole) in [(70usize, &mut day_w, 82u16), (82, &mut night_w, 94)] {
                    if len >= whole {
                        // a name of letters, digits, - and _ (up to 12), or one with a bad byte
                        let mut name = [0u8; 12];
                        let n = 1 + (next() % 12) as usize;
                        for b in name.iter_mut().take(n) {
                            *b = b"AZaz09-_"[(next() % 8) as usize];
                        }
                        let bad = next().is_multiple_of(4);
                        if bad {
                            name[(next() % n as u32) as usize] = b'/';
                        }
                        s[28 + at..28 + at + 12].copy_from_slice(&name);
                        *w = Some((name, !bad));
                    }
                }
                for (k, (slot, top)) in lap_w.iter_mut().zip([2u8, 50, 60]).enumerate() {
                    if usize::from(len) >= 95 + k {
                        // in range, or one past it (and 0 for the two numbers)
                        let v = if next().is_multiple_of(3) {
                            top + 1 + (next() % 100) as u8 % 100
                        } else {
                            (next() % 3) as u8
                        };
                        let v = if k > 0 && v == 0 && next().is_multiple_of(2) { top } else { v };
                        s[28 + 94 + k] = v;
                        *slot = Some((v, if k == 0 { v <= top } else { (1..=top).contains(&v) }));
                    }
                }
                for (k, (slot, top)) in seg_w.iter_mut().zip([2u8, 5]).enumerate() {
                    if usize::from(len) >= 98 + k {
                        // in range, or one past it
                        let v = if next().is_multiple_of(3) {
                            top + (next() % 200) as u8
                        } else {
                            (next() % u32::from(top)) as u8
                        };
                        s[28 + 97 + k] = v;
                        *slot = Some((v, v < top));
                    }
                }
                for (k, (slot, top)) in un_w.iter_mut().zip([2u8, 2, 2, 6]).enumerate() {
                    if usize::from(len) >= 100 + k {
                        let v = if next().is_multiple_of(3) {
                            top + (next() % 200) as u8
                        } else {
                            (next() % u32::from(top)) as u8
                        };
                        s[28 + 99 + k] = v;
                        *slot = Some((v, v < top));
                    }
                }
            }
            let crc = crate::inflate::crc32(s[..508].iter().copied());
            let crc = if r.is_multiple_of(17) { crc ^ 1 } else { crc };
            s[508..].copy_from_slice(&crc.to_le_bytes());
            let is_whole = !r.is_multiple_of(13) && !r.is_multiple_of(11) && !r.is_multiple_of(17) && len <= 480;
            if is_whole {
                want = Some(want.map_or(seq, |w| w.max(seq)));
            } else {
                refused += 1;
            }
            match SettingsCopy::from_sector((&*s).try_into().unwrap()) {
                None => assert!(!is_whole, "a whole copy refused"),
                Some(c) => {
                    assert!(is_whole, "a damaged copy read");
                    let st = c.settings;
                    if let Some((v, ok)) = lthr_w {
                        assert_eq!(st.lthr, if ok { v } else { 0 }, "LTHR written {v}");
                        with_lthr += u32::from(ok && v > 0);
                        wild_lthr += u32::from(!ok);
                    }
                    if let Some((v, ok)) = ps_w {
                        assert_eq!(st.power_smooth, if ok { usize::from(v) } else { 0 }, "POWER SMOOTH written {v}");
                        with_smooth += u32::from(ok && v > 0);
                        wild_smooth += u32::from(!ok);
                    }
                    assert!(crate::WHEELS.contains(&st.wheel_mm));
                    let mut m = st;
                    for k in crate::NUMBERS {
                        let (range, _) = k.number().unwrap();
                        assert!(range.contains(&*m.number_mut(k).unwrap()), "{k:?}");
                    }
                    for (n, w) in lim.iter().enumerate() {
                        let k = Setting::Limit(n as u8);
                        assert!(k.number().unwrap().0.contains(&st.limits[n]), "limit {n}");
                        if let Some((v, ok)) = *w {
                            assert_eq!(st.limits[n], if ok { v } else { 0 }, "limit {n} written {v}");
                            with_limit += u32::from(ok && v > 0);
                            wild_limit += u32::from(!ok);
                        }
                    }
                    // AUTO, DAY THEME, NIGHT THEME: written (a bad one reads off, OUTDOOR, DARK), or the defaults when cut
                    let d = crate::Settings::from_word(0);
                    if let Some(v) = auto_w {
                        assert_eq!(st.auto_theme, v == 1, "AUTO written {v}");
                        auto_on += u32::from(v == 1);
                    } else if usize::from(len) < 70 {
                        assert!(!st.auto_theme, "AUTO is off in a file without it");
                    }
                    for (w, got, def) in [(day_w, st.day_theme, d.day_theme), (night_w, st.night_theme, d.night_theme)]
                    {
                        match w {
                            Some((name, ok)) => {
                                assert_eq!(got, if ok { name } else { def }, "a theme name written {name:?}");
                                day_night_kept += u32::from(ok);
                                day_night_fell_back += u32::from(!ok);
                            }
                            None if usize::from(len) < 82 => assert_eq!(got, def, "a cut file: the default"),
                            None => {}
                        }
                    }
                    for (k, (w, got, def)) in
                        [(lap_w[0], st.auto_lap as u16, 0), (lap_w[1], st.lap_km, 5), (lap_w[2], st.lap_min, 10)]
                            .into_iter()
                            .enumerate()
                    {
                        match w {
                            Some((v, ok)) => {
                                assert_eq!(got, if ok { u16::from(v) } else { def }, "auto lap byte {k} written {v}");
                                lap_kept += u32::from(ok);
                                lap_fell_back += u32::from(!ok);
                            }
                            None if usize::from(len) < 95 + k => assert_eq!(got, def, "a cut file: the default"),
                            None => {}
                        }
                    }
                    for (k, (w, got, def)) in
                        [(seg_w[0], u8::from(st.live_segments), 1), (seg_w[1], st.seg_target as u8, 0)]
                            .into_iter()
                            .enumerate()
                    {
                        match w {
                            Some((v, ok)) => {
                                assert_eq!(got, if ok { v } else { def }, "segments byte {k} written {v}");
                                seg_kept += u32::from(ok);
                                seg_fell_back += u32::from(!ok);
                            }
                            None if usize::from(len) < 98 + k => assert_eq!(got, def, "a cut file: the default"),
                            None => {}
                        }
                    }
                    for (k, (w, got, def, top)) in [
                        (un_w[0], u8::from(st.sound), 1, 2),
                        (un_w[1], u8::from(st.imperial), 0, 2),
                        (un_w[2], u8::from(st.temp_f), 0, 2),
                        (un_w[3], st.auto_pause as u8, 0, 6),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        match w {
                            Some((v, ok)) => {
                                assert_eq!(got, if ok { v } else { def }, "units byte {k} written {v} of {top}");
                                un_kept += u32::from(ok);
                                un_fell_back += u32::from(!ok);
                            }
                            None if usize::from(len) < 100 + k => assert_eq!(got, def, "a cut file: the default"),
                            None => {}
                        }
                    }
                    // the trips and the odometer are whatever the file holds (random bytes), a cut one reads 0
                    for (k, got) in
                        [st.trip_m[0], st.trip_s[0], st.trip_m[1], st.trip_s[1], st.odo_m].into_iter().enumerate()
                    {
                        let at = 28 + 103 + 4 * k;
                        let bytes = s.get(at..at + 4).and_then(|b| <[u8; 4]>::try_from(b).ok());
                        let want = if usize::from(len) >= 107 + 4 * k && bytes.is_some() {
                            bytes.map_or(0, u32::from_le_bytes)
                        } else {
                            0
                        };
                        if usize::from(len) >= 107 + 4 * k {
                            trips_read += 1;
                        }
                        assert_eq!(got, want, "trip word {k}, length {len}");
                    }
                    let again = SettingsCopy::from_sector(&SettingsCopy { seq: c.seq, ..c }.to_sector());
                    assert_eq!(again.map(|a| a.settings.limits), Some(st.limits), "the limits round trip");
                    assert!(st.light_level <= crate::LIGHT_AUTO && st.light_timeout < crate::LIGHT_TIMEOUTS_S.len());
                    assert!(st.auto_off < crate::AUTO_OFF_MIN.len() && st.wkt_avg < crate::WKT_AVGS.len());
                    let mut l = crate::Line::<12>::new();
                    for k in
                        [Setting::Light, Setting::LightOff, Setting::AutoOff, Setting::BikeProfile, Setting::GpsPower]
                    {
                        st.text(k, &mut l);
                    }
                    match theme {
                        Some((name, true)) => {
                            kept += 1;
                            assert_eq!(st.theme, name, "a valid name is kept");
                        }
                        Some((_, false)) => {
                            fell_back += 1;
                            assert_eq!(st.theme, [0; 12], "a bad name falls back to DARK");
                        }
                        None => {}
                    }
                    if len < 31 {
                        short += 1;
                    } else {
                        whole += 1;
                        // a byte a build never writes there read as its default
                        defaulted += u32::from(st.gps_power == d.gps_power && s[28 + 30] > 2);
                    }
                }
            }
        }
        if let Some(c) = SettingsCopy::newest(&file) {
            assert_eq!(Some(c.seq), want, "the newest whole copy");
        } else {
            assert_eq!(want, None);
        }
    }
    std::println!("settings copies: {refused} refused, {whole} whole, {short} short, {defaulted} with a default");
    std::println!("theme names: {kept} kept, {fell_back} fallen back");
    std::println!("AUTO on {auto_on}, day and night names kept {day_night_kept}, fallen back {day_night_fell_back}");
    std::println!("AUTO LAP bytes kept {lap_kept}, fallen back {lap_fell_back}");
    assert!(lap_kept > 0 && lap_fell_back > 0, "AUTO LAP's bytes reached");
    std::println!("LIVE SEGMENTS and TARGET bytes kept {seg_kept}, fallen back {seg_fell_back}");
    assert!(seg_kept > 100 && seg_fell_back > 100, "the segments' bytes reached: {seg_kept} {seg_fell_back}");
    std::println!(
        "SOUND, UNITS, TEMP, AUTO PAUSE bytes kept {un_kept}, fallen back {un_fell_back}; trip words read {trips_read}"
    );
    assert!(
        un_kept > 100 && un_fell_back > 100 && trips_read > 100,
        "the units' bytes: {un_kept} {un_fell_back} {trips_read}"
    );
    assert!(auto_on > 0 && day_night_kept > 0 && day_night_fell_back > 0, "every new path reached");
    assert!(refused > 1000 && whole > 1000 && short > 1000 && defaulted > 10);
    assert!(kept > 100 && fell_back > 100, "{kept} kept, {fell_back} fell back");
    assert!(with_limit > 100 && wild_limit > 100, "{with_limit} limits read, {wild_limit} out of range read as 0");
    std::println!(
        "LTHR {with_lthr} read, {wild_lthr} out of range; POWER SMOOTH {with_smooth} read, {wild_smooth} out of range"
    );
    assert!(with_lthr > 100 && wild_lthr > 100, "LTHR {with_lthr} read, {wild_lthr} out of range");
    assert!(with_smooth > 100 && wild_smooth > 100, "POWER SMOOTH {with_smooth} read, {wild_smooth} out of range");
}

/// The zones, SEX, BIRTH YEAR and HEIGHT bytes of `\Rustfw\settings.bin` (data 123..150, with MAX HR FROM AGE), built by structure: the two
/// flags each right or out of form, the ten bounds rising (the form ZONES writes), unsorted, zero or cut short, the
/// year and height in and out of range, the length anywhere from 123 to 150. No panic; whatever is read is what the
/// module doc says (custom zones only from whole, rising bounds and a custom flag of 1; the year and height unset out
/// of range), and the zones every consumer reads rise; and the fuzz reaches custom zones, refused bounds and a cut.
#[test]
fn settings_file_zone_bytes() {
    use crate::SettingsCopy;
    let mut next = xorshift(0x20e5);
    let (mut custom, mut refused, mut cut, mut years, mut wild_years) = (0u32, 0u32, 0u32, 0u32, 0u32);
    for _ in 0..60_000 {
        let mut sec = SettingsCopy { seq: 1, words: [0; 4], settings: crate::Settings::from_word(0) }.to_sector();
        let len = 123 + (next() % 28) as usize;
        let age_flag = (next() % 3) as u8;
        let flags = [(next() % 3) as u8, (next() % 3) as u8]; // 0, 1, or 2 (out of form)
        let mut up = [0u16; 10];
        let rising = !next().is_multiple_of(3);
        let (mut h, mut p) = (40 + (next() % 30) as u16, 50 + (next() % 50) as u16);
        for (i, v) in up.iter_mut().enumerate() {
            let at = if i < 4 { &mut h } else { &mut p };
            *at += 1 + (next() % 30) as u16;
            *v = if rising { *at } else { (next() % 300) as u16 };
        }
        let year = if next().is_multiple_of(4) { (next() % 3000) as u16 } else { 1920 + (next() % 101) as u16 };
        let height = (next() % 256) as u8;
        let mut bytes = std::vec![flags[0], flags[1]];
        bytes.extend(up.iter().flat_map(|v| v.to_le_bytes()));
        bytes.push((next() % 2) as u8);
        bytes.extend(year.to_le_bytes());
        bytes.push(height);
        bytes.push(age_flag);
        sec[28 + 123..28 + 123 + 27].copy_from_slice(&bytes);
        sec[6..8].copy_from_slice(&(len as u16).to_le_bytes());
        let crc = crate::inflate::crc32(sec[..508].iter().copied());
        sec[508..].copy_from_slice(&crc.to_le_bytes());
        let st = SettingsCopy::from_sector(&sec).expect("a whole copy").settings;
        let z = st.zones().upper();
        assert!(z[..4].windows(2).all(|w| w[0] < w[1]) && z[4..].windows(2).all(|w| w[0] < w[1]), "{z:?} rises");
        let whole = len >= 125 + 20;
        let want_custom = whole && flags[1] == 1 && rising;
        assert_eq!(st.zone_custom, want_custom, "len {len}, flag {}, rising {rising}", flags[1]);
        if want_custom {
            assert_eq!(st.zones().upper(), up);
            custom += 1;
        } else {
            refused += u32::from(whole && flags[1] == 1);
            cut += u32::from(!whole);
        }
        assert_eq!(st.zones_lthr, len > 123 && flags[0] == 1);
        if len >= 148 {
            let ok = (1920..=2020).contains(&year);
            assert_eq!(st.birth_year, if ok { year } else { 0 });
            years += u32::from(ok);
            wild_years += u32::from(!ok);
        } else {
            assert_eq!(st.birth_year, 0, "cut before the year");
        }
        assert_eq!(st.max_hr_age, len >= 150 && age_flag == 1);
        assert_eq!(st.height_cm, if len >= 149 && (100..=230).contains(&height) { u16::from(height) } else { 0 });
    }
    assert!(custom > 1000 && refused > 500 && cut > 1000 && years > 1000 && wild_years > 100);
}

/// The ZONES menu's edits (`Settings::step` on `Setting::Zone`, ZONES BY, RESET, and FTP, MAX HR and LTHR moving under
/// them), in random order: no panic; the zones every consumer reads always rise and stay in what ZONES offers, a bound
/// never crosses its neighbours, the file round trip keeps them; and the fuzz reaches the first edit (computed zones
/// taken as the rider's), a bound held at its neighbour, RESET, and a custom table kept while
/// FTP, MAX HR and LTHR change.
#[test]
fn zones_menu_edits() {
    use crate::{Settings, SettingsCopy, menu};
    use menu::Setting;
    let mut next = xorshift(0x20ae5);
    let (mut first, mut held, mut reset, mut kept) = (0u32, 0u32, 0u32, 0u32);
    for _ in 0..3000 {
        let mut s = Settings::from_word(0);
        for _ in 0..200 {
            let before = (s.zone_custom, s.zones().upper());
            match next() % 12 {
                0 => s.cycle(Setting::ZonesBy),
                1 => {
                    s.cycle(Setting::ZonesReset);
                    reset += u32::from(before.0);
                }
                2 => s.step(Setting::Ftp, next().is_multiple_of(2)),
                3 => s.step(Setting::MaxHr, next().is_multiple_of(2)),
                4 => s.step(Setting::Lthr, next().is_multiple_of(2)),
                _ => {
                    let n = (next() % 10) as u8;
                    let up = !next().is_multiple_of(3);
                    s.step(Setting::Zone(n), up);
                    let after = s.zones().upper();
                    first += u32::from(!before.0 && s.zone_custom);
                    held += u32::from(before.0 && after == before.1);

                    for (k, (a, b)) in after.iter().zip(&before.1).enumerate() {
                        assert!(k == usize::from(n) || a == b, "only bound {n} moves, not {k}");
                    }
                }
            }
            let z = s.zones().upper();
            assert!(z[..4].windows(2).all(|w| w[0] < w[1]) && z[4..].windows(2).all(|w| w[0] < w[1]), "{z:?} rises");
            if s.zone_custom {
                assert!(Settings::zone_bounds_ok(&z), "{z:?} in range");
                kept += u32::from(before.0 && z == before.1 && matches!(s.zone_up, u if u == z));
            }
        }
        let back = SettingsCopy::from_sector(&SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector()).unwrap();
        assert_eq!(back.settings.zones(), s.zones(), "kept through the file");
    }
    assert!(first > 1000 && held > 100 && reset > 100 && kept > 1000, "{first} {held} {reset} {kept}");
}

/// Which wins at a start (app/settings_file.rs), by chance: random backup registers (each tagged as some build writes
/// it, or not: register 11 untagged is a flat backup battery, register 18 without the aiding's tag carries no AUTO
/// OFF) and a random `\Rustfw\settings.bin` (none, whole with the registers' words, whole with one of them changed,
/// damaged). No panic; the settings are those the module doc says (the file's merged with the changed registers), in
/// range; once written, the file and the registers agree; and every path is reached: lost registers, each register
/// winning, the file winning, the migration.
#[test]
fn settings_decision() {
    use super::Sim;
    use super::tests::run;
    use crate::{AID_BKP, ALERTS_BKP, POWER_BKP, Settings, SettingsCopy, WHEEL_BKP};
    let kept = |app: &crate::App| Settings { alt_m: 500, ..app.settings };
    let pristine = crate::fat::tests::image();
    // the same volume with a file: what the migration of a start with this firmware's defaults wrote (slot 1)
    let (with_file, at) = {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = pristine.clone();
        sim.own_settings();
        let mut app = crate::App::start(&mut sim, None);
        run(&mut sim, &mut app, 30_000);
        let img = sim.emmc.image;
        let slot1 = (0..img.len() / 512).map(|k| 512 * k).find(|&a| img[a..a + 4] == *b"RSET").unwrap();
        (img, slot1 - 512)
    };
    let mut next = xorshift(0x7e57);
    let random_settings = |next: &mut dyn FnMut() -> u32| {
        let w = (0xa7 + next() % 6) << 24 | next() & 0x7f_ffff;
        Settings::from_word(w)
            .with_power_word(0xa << 28 | next() & 0x0fff_ffff)
            .with_alerts_word(0x9 << 28 | next() & 0x0fff_ffff)
    };
    let (mut lost, mut took, mut file_wins, mut migrated) = (0u32, [0u32; 4], 0u32, 0u32);
    for seed in 0..160 {
        let mut sim = Sim::new(9, 0, 0);
        // the registers: a build's words, an earlier build's tags, or none
        let s = random_settings(&mut next);
        let [w11, w16, w28, off] = s.register_words();
        let pick = |n: u32, ours: u32, old: u32| match n % 5 {
            0 => 0,
            1 => old,
            _ => ours,
        };
        sim.backup[WHEEL_BKP] = pick(next(), w11, 0xa5 << 24 | 2136);
        sim.backup[POWER_BKP] = pick(next(), w16, 0xc1_03_2e_46);
        sim.backup[ALERTS_BKP] = pick(next(), w28, 0xc2 << 24 | 150 << 16 | 4);
        sim.backup[AID_BKP[1]] = match next() % 3 {
            0 => next() & 0x003f_ffff, // no aiding tag
            _ => 0xa << 28 | off << 22,
        };
        let aid = sim.backup[AID_BKP[1]];
        let regs = [
            sim.backup[WHEEL_BKP],
            sim.backup[POWER_BKP],
            sim.backup[ALERTS_BKP],
            if aid >> 28 == 0xa { aid >> 22 & 7 } else { 0 },
        ];
        let is_lost = !matches!(regs[0] >> 24, 0xa5..=0xac);
        // the file
        let kind = next() % 4;
        sim.emmc.image = if kind == 0 { pristine.clone() } else { with_file.clone() };
        if kind != 0 {
            for slot in 0..2 {
                let mut words = regs;
                if kind == 2 {
                    let k = (next() % 4) as usize;
                    words[k] ^= 1 << (next() % 32); // that register changed since the copy
                }
                let c = SettingsCopy { seq: 10 + next() % 4, words, settings: random_settings(&mut next) };
                let mut sec = c.to_sector();
                if kind == 3 {
                    sec[(next() % 512) as usize] ^= 1 << (next() % 8); // damaged
                }
                sim.emmc.image[at + 512 * slot..at + 512 * (slot + 1)].copy_from_slice(&sec);
            }
        }
        let image: [u8; 1024] = sim.emmc.image[at..at + 1024].try_into().unwrap();
        let newest = if kind == 0 { None } else { SettingsCopy::newest(&image) };
        let mut app = crate::App::start(&mut sim, None);
        let before = kept(&app);
        run(&mut sim, &mut app, u64::from(crate::PAGES_LOAD_MS) + 30_000);
        let got = kept(&app);
        match newest {
            None => {
                migrated += 1;
                if !is_lost {
                    assert_eq!(got, before, "seed {seed}: the registers' kept");
                }
            }
            Some(c) if is_lost => {
                lost += 1;
                assert_eq!(got, c.settings, "seed {seed}: lost registers, the file's");
            }
            Some(c) => {
                let (want, t) = c.settings.with_registers(c.words, regs);
                assert_eq!(got, want, "seed {seed}");
                for (n, t) in took.iter_mut().zip(t) {
                    *n += u32::from(t);
                }
                file_wins += u32::from(!t.contains(&true));
            }
        }
        assert!(crate::WHEELS.contains(&got.wheel_mm));
        let mut m = got;
        for k in crate::NUMBERS {
            assert!(k.number().unwrap().0.contains(&*m.number_mut(k).unwrap()), "seed {seed}: {k:?}");
        }
        // written: the file has the settings, the registers the same words
        assert!(!app.settings_store.dirty, "seed {seed}");
        let mut d = crate::fat::tests::Ram(sim.emmc.image.clone(), 0);
        let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
        let v = crate::fat::tests::read_all(&mut fat, &mut d, "Rustfw/settings.bin").unwrap();
        let c = SettingsCopy::newest(&v.try_into().unwrap()).unwrap();
        assert_eq!(c.settings, got, "seed {seed}: the file");
        assert_eq!(c.words[..3], [WHEEL_BKP, POWER_BKP, ALERTS_BKP].map(|r| sim.backup[r]), "seed {seed}: registers");
    }
    std::println!("settings at a start: {lost} lost, registers took {took:?}, {file_wins} file, {migrated} migrated");
    assert!(lost > 0 && file_wins > 0 && migrated > 0 && took.iter().all(|&n| n > 0));
}

/// Theme files (ui/theme.rs `parse`, `\Rustfw\Themes\*.ini`): generated from the key table (sections, known and
/// unknown keys, good, cut and wild colour values, comments, long lines, NULs, CRLF, bad versions and bases), never a
/// panic; an accepted theme passes the whole-theme checks; and the seeds reach both ends (accepted, and each
/// refusal), every key set from a file at least once.
#[test]
fn theme_files() {
    use crate::theme::{NAMES, Refused, Theme, check, parse};
    let (mut accepted, mut light, mut clamped, mut fields) = (0, 0usize, 0usize, 0usize);
    let (mut refused, mut metric_reach) = ([0usize; 5], [0usize; 3]);
    let mut set = [false; NAMES.len()];
    for seed in 1..4000u32 {
        let mut rnd = xorshift(seed.wrapping_mul(2_654_435_761));
        let mut f = std::string::String::new();
        let nl = if rnd().is_multiple_of(3) { "\r\n" } else { "\n" };
        if !rnd().is_multiple_of(8) {
            f.push_str("[theme]");
            f.push_str(nl);
            match rnd() % 6 {
                0 => {}
                1 => f.push_str(&std::format!("version = {}{nl}", rnd() % 4)),
                2 => f.push_str(&std::format!("version = x{}{nl}", rnd() % 4)),
                _ => f.push_str(&std::format!("version = 1{nl}name = T{}{nl}", rnd() % 100)),
            }
            if rnd().is_multiple_of(3) {
                f.push_str(["base = outdoor", "base = dark", "base = ??", "base = light"][rnd() as usize % 4]);
                f.push_str(nl);
            }
            if rnd().is_multiple_of(3) {
                f.push_str(
                    ["overview = fields", "overview = classic", "overview = grid", "overview = "][rnd() as usize % 4],
                );
                f.push_str(nl);
            }
        }
        f.push_str("[colors]");
        f.push_str(nl);
        for _ in 0..rnd() % 24 {
            let name = if rnd().is_multiple_of(6) { "unknown" } else { NAMES[rnd() as usize % NAMES.len()] };
            let rgb = rnd() & 0xff_ffff;
            let value = match rnd() % 8 {
                0 => std::format!("#{rgb:06X}"),
                1 => std::format!("{rgb:06x}"),
                2 => std::format!("#{:05X}", rgb & 0xf_ffff),
                3 => std::format!("#{rgb:06X} ; note"),
                4 => std::format!("#{}", "G".repeat(1 + rnd() as usize % 8)),
                5 => std::string::String::new(),
                // readable pairs and red often enough for both checks to bite
                6 => ["#FFFFFF", "#000000", "#FF0000", "#F80000"][rnd() as usize % 4].into(),
                _ => "x".repeat(rnd() as usize % 600),
            };
            f.push_str(&std::format!("{name} = {value}{nl}"));
            if rnd().is_multiple_of(40) {
                f.push('\0');
            }
        }
        if rnd().is_multiple_of(2) {
            f.push_str("[metrics]");
            f.push_str(nl);
            for _ in 0..rnd() % 5 {
                let key = ["zone_bar", "frame", "menu_row", "other"][rnd() as usize % 4];
                let value = match rnd() % 7 {
                    6 => std::format!("{}", rnd() % 14),
                    0 => "x".into(),
                    1 => std::format!("-{}", rnd() % 9),
                    2 => std::format!("{}", rnd()),
                    3 => std::format!("{} ; px", rnd() % 70),
                    _ => std::format!("{}", rnd() % 70),
                };
                f.push_str(&std::format!("{key} = {value}{nl}"));
            }
        }
        let bytes = f.as_bytes();
        let bytes = &bytes[..if rnd().is_multiple_of(10) { rnd() as usize % (bytes.len() + 1) } else { bytes.len() }];
        match parse(bytes) {
            Ok(t) => {
                accepted += 1;
                assert!((4..=8).contains(&t.zone_bar) && (1..=4).contains(&t.frame) && (44..=56).contains(&t.menu_row));
                fields += usize::from(t.overview == crate::theme::Overview::Fields);
                metric_reach[0] += usize::from(t.zone_bar != 8);
                metric_reach[1] += usize::from(t.frame != 3);
                metric_reach[2] += usize::from(t.menu_row != 44);
                clamped += usize::from(t.zone_bar == 8 || t.zone_bar == 4 || t.menu_row == 56);
                assert!(
                    [Theme::DARK, Theme::OUTDOOR, Theme::LIGHT].iter().any(|b| check(&t, b).is_ok()),
                    "seed {seed}"
                );
                light += usize::from(bytes.windows(12).any(|w| w == b"base = light") && t != Theme::DARK);
                for (s, (c, d)) in set.iter_mut().zip(t.ink.iter().zip(Theme::DARK.ink)) {
                    *s |= *c != d;
                }
            }
            Err(Refused::NotATheme) => refused[0] += 1,
            Err(Refused::TooNew) => refused[1] += 1,
            Err(Refused::Contrast) => refused[2] += 1,
            Err(Refused::RedNotAlarm) => refused[3] += 1,
            Err(Refused::SameZones) => refused[4] += 1,
        }
    }
    assert!(accepted > 100, "accepted {accepted}");
    assert!(light > 0, "a theme accepted over the light base");
    assert!(fields > 20, "overview = fields reached: {fields}");
    assert!(metric_reach.iter().all(|&n| n > 5) && clamped > 20, "metrics set: {metric_reach:?}, at an end {clamped}");
    assert!(refused.iter().all(|&n| n > 0), "every refusal reached: {refused:?}");
    assert!(set.iter().filter(|&&s| s).count() >= NAMES.len() - 2, "keys set from a file: {set:?}");
}

/// Random raw readings through the sensor conversions: the battery's `millivolts` (the ADC words, any reference) and
/// `percent` of its voltage, the light's `lux10`, and the barometer's 24-bit pressure word (what `baro_read_raw`
/// returns with its temperature) into `alt_mm`. No panic; a percent is at most 100, the calibrated battery reading
/// at most 4950 mV, and a plausible pressure (500 to 1100 hPa) reads between -1000 m and 6000 m.
#[test]
fn raw_samples() {
    let mut r = xorshift(9);
    let (mut volts, mut plausible, mut altitudes) = (0usize, 0usize, 0usize);
    for _ in 0..1 << 18 {
        let (raw, vref, cal) = (r() as u16, r() as u16, r() as u16);
        if let Some(mv) = crate::battery::millivolts(raw, vref, cal) {
            assert!(crate::battery::percent(mv) <= 100, "{mv} mV");
            // VDDA is 3300 mV x cal / vref, and the battery channel is at most 1.5 times it
            assert!(u64::from(mv) <= 4950 * u64::from(cal) / u64::from(vref) + 1, "{mv} mV, cal {cal}, vref {vref}");
            volts += 1;
        }
        // VREFINT at its calibration value: VDDA is 3.3 V, the pin's full scale 4.95 V on the battery
        assert!(crate::battery::millivolts(raw, 24000, 24000).is_some_and(|mv| mv <= 4950));
        let w = r();
        assert_eq!(crate::ambient::lux10(w), w & 0x3_ffff);
        let p = w & 0xff_ffff; // the chip's 24-bit pressure word
        if let Some(h) = crate::altitude::alt_mm(p, crate::altitude::P_STD) {
            if (500 * 4096..=1100 * 4096).contains(&p) {
                assert!((-1_000_000..=6_000_000).contains(&h), "{p} -> {h} mm");
                plausible += 1;
            }
            altitudes += 1;
        }
    }
    assert!(
        volts > 0 && plausible > 0 && altitudes > 0,
        "reached: {volts} volts, {plausible} plausible, {altitudes} altitudes"
    );
}

/// One recovery record's reader against its writer. The intact sector reads back equal (`c[0]`). One to three bytes
/// flipped are refused (`c[1]`), except when the CRC-16 misses the change: every byte is under the CRC over bytes
/// 0..510 (bytes 510..512 are the CRC itself), and a random change passes a 16-bit CRC with probability about 2^-16,
/// so a damaged sector may read back (`c[3]`, bounded by the caller). The boot's recovery does not trust such a record
/// blindly: it also checks the chain and sector 0 against it (`still_ours`), which bounds the harm. A random sector is
/// refused (`c[2]`).
fn recovery_sectors<T: PartialEq + std::fmt::Debug>(
    next: &mut impl FnMut() -> u32,
    original: &T,
    sector: [u8; crate::fat::SECTOR],
    read: impl Fn(&[u8; crate::fat::SECTOR]) -> Option<T>,
    c: &mut [u32; 4],
) {
    assert_eq!(read(&sector).as_ref(), Some(original));
    c[0] += 1;
    let mut bad = sector;
    for _ in 0..1 + next() % 3 {
        let at = next() as usize % crate::fat::SECTOR;
        bad[at] ^= 1 + (next() % 255) as u8; // a nonzero XOR, so the byte changes
    }
    if bad != sector {
        c[1] += 1;
        if let Some(back) = read(&bad) {
            assert_eq!(back, *original, "a damaged sector read back as another record");
            c[3] += 1; // accepted, and equal: the change fell where the record does not look
        }
    }
    let mut raw = [0u8; crate::fat::SECTOR];
    raw.iter_mut().for_each(|b| *b = next() as u8);
    assert_eq!(read(&raw), None, "a random sector was read");
    c[2] += 1;
}

#[test]
fn recovery_records() {
    use crate::fit::RECORD_LEN;
    use crate::recorder::{Phase, rec::Rec};
    use crate::ride::RIDE_WORDS;
    let (mut recorder, mut trash) = ([0u32; 4], [0u32; 4]);
    for seed in 1..=64u32 {
        let mut next = xorshift(seed.wrapping_mul(0x9e37_79b9) ^ 0x5ec0_0001);
        for _ in 0..100 {
            let mut name = [0u8; 12];
            name.iter_mut().for_each(|b| *b = next() as u8);
            let mut fit = [0u8; RECORD_LEN];
            fit.iter_mut().for_each(|b| *b = next() as u8);
            let phase = [Phase::Done, Phase::Starting, Phase::Recording, Phase::Saving][next() as usize % 4];
            let ride: [u32; RIDE_WORDS] = core::array::from_fn(|_| next());
            let r = Rec {
                seq: next(),
                phase,
                name,
                cluster: next(),
                fit,
                ride,
                discard: next().is_multiple_of(2),
                checked: (next().is_multiple_of(2)).then(|| next() as u16),
            };
            recovery_sectors(&mut next, &r, r.to_sector(), Rec::from_sector, &mut recorder);

            // the trash's move record: each file of the move is there or not, with its CRC or without
            let moved = |next: &mut dyn FnMut() -> u32| {
                (!next().is_multiple_of(3)).then(|| (next(), next(), (next().is_multiple_of(2)).then(|| next() as u16)))
            };
            let mut name = [0u8; 12];
            name.iter_mut().for_each(|b| *b = next() as u8);
            let t = crate::trash::Rec {
                pending: next().is_multiple_of(2),
                name,
                suffix: next() as u8,
                files: [moved(&mut next), moved(&mut next)],
            };
            // the older layout's flag 1 (its CRC ignored, so the file reads back without one): some records use it
            let mut sector = t.to_sector();
            let mut expect = t;
            if next().is_multiple_of(4) {
                for k in 0..2 {
                    if sector[32 + 16 * k] == 2 {
                        sector[32 + 16 * k] = 1;
                        expect.files[k] = expect.files[k].map(|(first, size, _)| (first, size, None));
                    }
                }
                let crc = crate::fit::crc(0, &sector[..crate::fat::SECTOR - 2]);
                sector[crate::fat::SECTOR - 2..].copy_from_slice(&crc.to_le_bytes());
            }
            recovery_sectors(&mut next, &expect, sector, crate::trash::Rec::from_sector, &mut trash);
        }
    }
    // a 16-bit CRC misses about 2^-16 of the changes: at most 4 accepted per 6400 damaged sectors, all equal
    for c in [recorder, trash] {
        assert!(c[0] > 0 && c[1] > 0 && c[2] > 0, "reached: {c:?} (intact, damaged, random, accepted)");
        assert!(c[3] * 1600 <= c[1], "accepted {} of {} damaged", c[3], c[1]);
    }
}

/// A live segment file (nav/segment.rs) generated from its layout: the header and the data section with random
/// counts, target codes (some not stock's), times (some falling, some past a u16), a FIT course of the same points,
/// one record more or fewer, without positions or off the globe. Then cut, flipped, its fields set to wild values,
/// appended to, or left whole. Every error the reader can give and the Ok path are reached (counted over the seeds),
/// and no input panics.
#[test]
fn segment_files() {
    use crate::segment::{Error, HEADER, Header, KEPT, POINTS, Segment, tests::raw};
    const CODES: [[u8; 2]; 9] = [*b"05", *b"06", *b"07", *b"09", *b"11", *b"12", *b"13", *b"99", *b"01"];
    // reached: Ok, then each error in the order of the match below
    let mut reached = [0usize; 15];
    for seed in 1..=2000u32 {
        let mut next = xorshift(seed);
        // most files are valid, and each kind of break is rare: a file breaks in a few places, not everywhere
        let np = (next() % 430) as usize;
        let n = 1 + (next() % 7) as usize;
        let codes: Vec<[u8; 2]> = (0..n)
            .map(
                |_| if next().is_multiple_of(12) { CODES[(next() % 9) as usize] } else { CODES[(next() % 7) as usize] },
            )
            .collect();
        let corrupt_times = next().is_multiple_of(8);
        let times: Vec<u32> = (0..np * n)
            .map(|k| {
                let v = 10 * (k / n) as u32 + (k % n) as u32;
                match next() % 300 {
                    0 if corrupt_times => next() % 80_000,
                    1 if corrupt_times => next() % 50,
                    _ => v,
                }
            })
            .collect();
        let records =
            (np + usize::from(next().is_multiple_of(10))).saturating_sub(usize::from(next().is_multiple_of(10)));
        let pos = !next().is_multiple_of(12);
        let lat = if next().is_multiple_of(15) { 95.0 } else { 0.0 };
        let ntf = if next().is_multiple_of(20) { next() % 9 } else { n as u32 };
        let np_field = if next().is_multiple_of(40) { next() % 500 } else { np as u32 };
        let mut f = raw(np_field, ntf, &codes, &times, records, pos, lat);
        let fit_at = HEADER + 8 + 13 * n + 4 * np * n;
        if next().is_multiple_of(3) {
            match next() % 13 {
                0 => f.truncate(next() as usize % (f.len() + 1)),
                1 => {
                    let i = next() as usize % f.len();
                    f[i] ^= 0x5a;
                }
                2 => {
                    if let Some(b) = f.get_mut(fit_at + 8) {
                        *b ^= 1;
                    }
                }
                3 => {
                    if let Some(b) = f.last_mut() {
                        *b ^= 1;
                    }
                }
                4 => {
                    if let Some(b) = f.get_mut(fit_at + 12) {
                        *b = 0;
                    }
                }
                5 => f.push(next() as u8),
                6 => {
                    if let Some(s) = f.get_mut(9..13) {
                        s.copy_from_slice(&next().to_le_bytes());
                    }
                }
                7 => {
                    if let Some(b) = f.get_mut(0x179) {
                        *b = (next() % 10) as u8;
                    }
                }
                8 => {
                    if let Some(s) = f.get_mut(0x17a..0x17c) {
                        s.copy_from_slice(&[b'0' + (next() % 10) as u8, b'0' + (next() % 10) as u8]);
                    }
                }
                9 => {
                    let i = next() as usize % HEADER;
                    f[i] ^= 0xa5;
                }
                10 => {
                    if f.len() > fit_at {
                        let i = fit_at + next() as usize % (f.len() - fit_at);
                        f[i] ^= 0x3c;
                    }
                }
                11 => f.resize(65_537, 0),
                _ => {}
            }
        }
        let mut seg = Segment::new();
        let k = match seg.parse(&f) {
            Ok(()) => {
                assert!((2..=POINTS).contains(&seg.npoints), "seed {seed}: points {}", seg.npoints);
                for i in 0..seg.npoints {
                    assert!(seg.lat[i].unsigned_abs() <= 900_000_000 && seg.lon[i].unsigned_abs() <= 1_800_000_000);
                }
                for i in 1..seg.npoints {
                    for c in (0..KEPT).filter(|&c| seg.kept[c].is_some()) {
                        assert!(seg.times[i - 1][c] <= seg.times[i][c], "seed {seed}: times fall");
                    }
                }
                0
            }
            Err(e) => match e {
                Error::TooBig => 1,
                Error::Short | Error::Io => 2, // Io: only a card gives it
                Error::Targets => 3,
                Error::Target => 4,
                Error::Points => 5,
                Error::FitOffset => 6,
                Error::Times => 7,
                Error::Earth => 8,
                Error::NoPosition => 9,
                Error::FitHeader => 10,
                Error::FitUndefined => 11,
                Error::FitTruncated => 12,
                Error::FitCrc => 13,
                Error::Trailing => 14,
            },
        };
        reached[k] += 1;
        let _ = Header::parse(&f);
    }
    assert!(reached.iter().all(|&c| c > 0), "every error and the Ok path reached: {reached:?}");
}

/// A segment's PR file (nav/segment.rs `PrCopy`, read at a segment's load and before an effort is written)
/// generated from its layout: two copies, each valid, absent, or with one field broken (magic, version, the point
/// count, a time falling, the first time or the total, a flipped byte with or without its CRC put right), the file
/// cut or grown. Every check and the Ok path of a copy are reached (counted over the seeds), and the newest whole
/// copy is always one that parses on its own, the one with the higher sequence number.
#[test]
fn pr_files() {
    use crate::segment::{POINTS, PR_COPY, PrCopy, PrError, pr_copy};
    let mut reached = [0usize; 7]; // Ok, Size, Magic, Version, Points, Crc, Times
    let mut newest = [0usize; 3]; // none, the first, the second
    for seed in 1..=3000u32 {
        let mut next = xorshift(seed);
        let copy = |next: &mut dyn FnMut() -> u32| -> Vec<u8> {
            let np = 2 + (next() as usize % (POINTS - 1));
            let mut t = 0u16;
            let times: Vec<u16> = (0..np)
                .map(|i| {
                    if i > 0 {
                        t = t.saturating_add(1 + (next() % 40) as u16);
                    }
                    t
                })
                .collect();
            let mut c = pr_copy(next() % 50, next() % 3, next(), &times).unwrap().to_vec();
            let crc_fixed = next().is_multiple_of(2);
            let at = |next: &mut dyn FnMut() -> u32| 24 + 2 * (next() as usize % np);
            match next() % 12 {
                0 => c[next() as usize % 4] ^= 1 + (next() % 255) as u8,
                1 => c[4] = (next() % 4) as u8,
                2 => c[6..8].copy_from_slice(&((next() % 3) as u16 * 401).to_le_bytes()),
                3 => {
                    let i = at(next);
                    c[i..i + 2].copy_from_slice(&((next() % 2) as u16).to_le_bytes());
                }
                4 => c[20..24].copy_from_slice(&(next() % 3).to_le_bytes()),
                5 => {
                    let i = next() as usize % 1020;
                    c[i] ^= 1 + (next() % 255) as u8;
                }
                6 => c.truncate(next() as usize % PR_COPY),
                7 => c = std::vec![0u8; PR_COPY], // a copy never written
                _ => {}
            }
            if crc_fixed && c.len() == PR_COPY {
                let crc = crate::inflate::crc32(c[..1020].iter().copied());
                c[1020..].copy_from_slice(&crc.to_le_bytes());
            }
            c
        };
        let (a, b) = (copy(&mut next), copy(&mut next));
        for c in [&a, &b] {
            reached[match PrCopy::parse(c) {
                Ok(p) => {
                    assert!(p.npoints() >= 2 && p.iter().last() == Some(p.total_s as u16), "seed {seed}");
                    assert!(p.iter().zip(p.iter().skip(1)).all(|(x, y)| x <= y), "seed {seed}: falling");
                    0
                }
                Err(PrError::Size) => 1,
                Err(PrError::Magic) => 2,
                Err(PrError::Version) => 3,
                Err(PrError::Points) => 4,
                Err(PrError::Crc) => 5,
                Err(PrError::Times) => 6,
            }] += 1;
        }
        // the file: both copies padded to their place, sometimes cut or grown
        let mut file = std::vec![0u8; 2 * PR_COPY];
        file[..a.len().min(PR_COPY)].copy_from_slice(&a[..a.len().min(PR_COPY)]);
        file[PR_COPY..PR_COPY + b.len().min(PR_COPY)].copy_from_slice(&b[..b.len().min(PR_COPY)]);
        match next() % 8 {
            0 => file.truncate(next() as usize % (2 * PR_COPY)),
            1 => file.extend((0..next() % 600).map(|_| next() as u8)),
            _ => {}
        }
        let whole = |c: &[u8]| c.len() == PR_COPY && PrCopy::parse(c).is_ok();
        let n = PrCopy::newest(&file);
        newest[match n {
            None => 0,
            Some(p) if file.len() >= PR_COPY && PrCopy::parse(&file[..PR_COPY]) == Ok(p) => 1,
            Some(_) => 2,
        }] += 1;
        if let Some(p) = n {
            let (x, y) = (&file[..PR_COPY], &file[PR_COPY..2 * PR_COPY]);
            assert!(whole(x) || whole(y), "seed {seed}");
            for other in [x, y].into_iter().filter_map(|c| PrCopy::parse(c).ok()) {
                assert!(other.seq <= p.seq, "seed {seed}: not the newest");
            }
        }
    }
    assert!(reached.iter().all(|&c| c > 0), "every check and the Ok path reached: {reached:?}");
    assert!(newest.iter().all(|&c| c > 0), "none, the first and the second won: {newest:?}");
}

/// The segment matcher (nav/segment_match.rs) against parsed segments, fed random fixes: a rider that follows the
/// track with noise, then wanders (jumps, U-turns, stops, a clock that stands or jumps), at up to 30 m/s, in the
/// lane of one segment or of a second one of another geometry. In some seeds the segment is a 2 or 3 point one, and
/// in some the wrong file is handed in. No input panics; the events come in a valid order (an Approach or a Start
/// while idle, then a Lost or a Finish, the segment active between), a wrong file only ever loses the segment, a
/// finish's deltas are the elapsed time minus a time inside the segment's span, and over the seeds starts (by both
/// segments), finishes and losses are all reached.
#[test]
fn segment_matcher() {
    use crate::segment::{Segment, tests::synth};
    use crate::segment_match::tests::{at, seg};
    use crate::segment_match::{Event, Matcher};
    let mut other = seg(12);
    other.header.id = 9;
    for p in other.header.start.iter_mut().filter(|p| **p != (0, 0)) {
        p.0 += 8983; // 100 m north
    }
    other.lat.iter_mut().for_each(|v| *v += 8983);
    let (mut starts, mut finishes, mut losts, mut by_other) = (0, 0, 0, 0);
    for seed in 1..=1500u32 {
        let mut next = xorshift(seed);
        let mut m = Matcher::new();
        let mut s = seg(20);
        if seed % 9 == 0 {
            // a tiny segment (2 or 3 points; the reader takes no fewer)
            let mut tiny = Segment::new();
            if tiny.parse(&synth(2 + (next() % 2) as usize, &[*b"07", *b"05"])).is_ok() {
                s = tiny;
            }
        }
        let wrong = seed % 7 == 0; // the file handed in is not the active one's
        let lane = if next().is_multiple_of(2) { 0.0 } else { 100.0 };
        let (mut pos, mut t, mut active) = (-300.0f64, 0u32, false);
        for _ in 0..400 {
            let speed = 3.0 + f64::from(next() % 28);
            let noise = |v: u32| f64::from(v % 41) - 20.0;
            match next() % 40 {
                0 => pos = f64::from(next() % 2000) - 500.0, // a jump
                1 => pos -= 60.0,                            // a step back
                2 => t = t.saturating_add(next() % 400),     // a gap in the fixes
                3 => {}                                      // a stopped clock, same fix
                _ => pos += speed,
            }
            if !next().is_multiple_of(40) {
                t += 1;
            }
            let side = lane + if next().is_multiple_of(25) { 70.0 } else { noise(next()) * 0.3 };
            let (lat, lon) = at(pos + noise(next()) * 0.3, side);
            let ev = match m.active_id() {
                Some(id) => {
                    let file = if wrong == (id == s.header.id) { &other } else { &s };
                    m.step_active(file, lat, lon, t)
                }
                None => m.step_idle([&s.header, &other.header], lat, lon, t),
            };
            match ev {
                Some(Event::Start(id)) => {
                    assert!(!active && m.active_id() == Some(id));
                    (active, starts) = (true, starts + 1);
                    by_other += usize::from(id == 9);
                }
                Some(Event::Lost(_)) => {
                    assert!(active && m.active_id().is_none());
                    (active, losts) = (false, losts + 1);
                }
                Some(Event::Finish { id, elapsed_s, deltas }) => {
                    assert!(active && !wrong && m.active_id().is_none());
                    (active, finishes) = (false, finishes + 1);
                    let file = if id == 9 { &other } else { &s };
                    let last = file.npoints - 1;
                    for (c, d) in deltas.iter().enumerate() {
                        if let Some(d) = d {
                            let target = i64::from(elapsed_s) - i64::from(*d);
                            assert!((0..=i64::from(file.times[last][c])).contains(&target), "{d} {elapsed_s}");
                        }
                    }
                }
                Some(Event::Approach(_)) => assert!(!active),
                None => assert_eq!(active, m.active_id().is_some()),
            }
        }
    }
    assert!(starts > 100 && finishes > 20 && losts > 20 && by_other > 10, "{starts} {finishes} {losts} {by_other}");
}

/// AUTO LAP (BY DISTANCE and BY TIME, 1 km and 1 min) on a ride with Lap and pause pressed at random: no panic, a
/// banner of the lap summary or its replacement always readable, and auto laps fire over the seeds.
#[test]
fn auto_lap_on_a_ride_with_keys() {
    use crate::sim::{Sim, run};
    use crate::*;
    let mut auto = 0u32;
    for seed in 0..8u32 {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        sim.own_settings();
        sim.gnss.route = Some(crate::sim::map_tests::ride_route);
        sim.gnss.fix = Some(8000);
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 2000);
        (app.settings.auto_lap, app.settings.lap_km, app.settings.lap_min) = (1 + seed as usize % 2, 1, 1);
        app.press(&mut sim, Press::Short(Key::Start));
        let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        let mut manual = 0u32;
        for _ in 0..90 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            run(&mut sim, &mut app, 5000);
            match x >> 28 {
                0 => {
                    app.press(&mut sim, Press::Short(Key::Lap));
                    manual += 1;
                }
                1 => app.press(&mut sim, Press::Short(Key::Start)),
                _ => {}
            }
            assert!(app.notes.text.as_str().is_ascii());
        }
        auto += app.ride.laps().saturating_sub(manual.min(app.ride.laps()));
    }
    assert!(auto > 0, "auto laps fired over the seeds");
}

/// STORAGE's survey on a damaged card: FSInfo's count or signature damaged, FAT entries and directory bytes random.
/// Every seed ends (no panic, no hang), and both the FSInfo path and the FAT count and the listing are reached.
#[test]
fn card_survey_on_damaged_volumes() {
    use crate::fat::tests::Ram;
    use crate::volume::Survey;
    let base = crate::storage::trash::tests::volume();
    let (mut fsinfo_ok, mut counted, mut listed, mut failed) = (0, 0, 0, 0);
    for seed in 1..=200u32 {
        let mut next = xorshift(seed);
        let mut img = base.clone();
        let le16 = |i: usize| usize::from(u16::from_le_bytes([img[i], img[i + 1]]));
        let (info, fat0) = (le16(48) * 512, le16(14) * 512);
        let fat_len = (u32::from_le_bytes([img[36], img[37], img[38], img[39]]) as usize) * 512;
        match next() % 3 {
            0 => {}
            1 => img[info + 488..info + 492].copy_from_slice(&next().to_le_bytes()),
            _ => img[info] = next() as u8,
        }
        // half the seeds: the FAT entry of \Activities' first cluster (its listing follows the chain)
        if next().is_multiple_of(2) {
            let mut d = Ram(base.clone(), 0);
            let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
            let first = fat.open(&mut d, "Activities").unwrap().cluster;
            img[fat0 + first as usize * 4 + (next() % 4) as usize] = next() as u8;
        }
        // one seed in eight: BPB_RootClus out of range, the volume refused
        if next().is_multiple_of(8) {
            img[44..48].copy_from_slice(&next().to_le_bytes());
        }
        // and a few random FAT bytes
        for _ in 0..(next() % 64) {
            let i = fat0 + next() as usize % fat_len.min(2048);
            img[i] = next() as u8;
        }
        let whole_info = crate::fat::Fat::mount(&mut Ram(img.clone(), 0))
            .is_ok_and(|f| f.fsinfo_free(&mut Ram(img.clone(), 0)).is_some());
        let mut d = Ram(img, 0);
        let mut s = Survey::new();
        let mut passes = 0;
        while !s.step(&mut d, 1 + seed % 4) {
            passes += 1;
            assert!(passes < 100_000, "the survey ends");
        }
        fsinfo_ok += u32::from(whole_info);
        counted += u32::from(s.free().is_some());
        listed += u32::from(s.rides().is_some());
        failed += u32::from(s.failed());
    }
    assert!(
        fsinfo_ok > 20 && fsinfo_ok < 180 && counted > 40 && listed > 40 && failed > 10,
        "{fsinfo_ok} {counted} {listed} {failed}"
    );
}

/// STORAGE's glue on a damaged card (app/survey.rs): the survey's passes, the oldest rows, their cards and Back from
/// a card to STORAGE, with FSInfo and the FAT damaged on some seeds. Nothing panics, the survey stays consistent with
/// the screen, and each step of the glue is reached over the seeds.
#[test]
fn storage_glue_on_a_damaged_card() {
    use super::*;
    use crate::ui::menu::{Input, Row};
    let base = crate::storage::trash::tests::volume();
    let (mut opened, mut back, mut done) = (0u32, 0u32, 0u32);
    for seed in 1..=24u32 {
        let mut next = xorshift(seed * 31 + 7);
        let mut img = base.clone();
        let info = usize::from(u16::from_le_bytes([img[48], img[49]])) * 512;
        let fat0 = usize::from(u16::from_le_bytes([img[14], img[15]])) * 512;
        let fat_len = (u32::from_le_bytes([img[36], img[37], img[38], img[39]]) as usize) * 512;
        if next().is_multiple_of(2) {
            img[info + 488..info + 492].copy_from_slice(&next().to_le_bytes());
        }
        if next().is_multiple_of(4) {
            for _ in 0..32 {
                let i = fat0 + next() as usize % fat_len.min(2048);
                img[i] = next() as u8;
            }
        }
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = img;
        let mut app = App::start(&mut sim, None);
        // the card's start-up work (the recorder checks the card) ends before STORAGE is read: the survey waits for it
        super::tests::run(&mut sim, &mut app, 3000);
        app.open(Screen::Menu(Menu::Storage, 0));
        app.show(&mut sim);
        let mut pending = false;
        for k in 0..400 {
            let (a, b) = (next(), next());
            if !matches!(app.screen(), Screen::Menu(Menu::Storage, _) | Screen::Past) {
                // a walk off STORAGE (Back to the overview, where Select starts a ride): back to it, the rides untouched
                app.open(Screen::Menu(Menu::Storage, 0));
                app.show(&mut sim);
            }
            let before = app.screen();
            let i = match a % 10 {
                0..=2 => Input::Prev,
                3..=5 => Input::Select,
                // DELETE is asked on a card (End), never a ride key: the ride screens are not this fuzz's
                6 if before == Screen::Past => Input::End,
                6 | 7 => Input::Prev,
                _ => Input::Touch(crate::touch::Gesture::Tap(
                    120,
                    (ui::LIST_Y + 10 + app.theme.menu_row * (b % app.menu_rows() as u32)) as u16,
                )),
            };
            let on_old = matches!(before, Screen::Menu(Menu::Storage, s)
                if matches!(app.row(Menu::Storage, s), Some(Row::OldRide(_))));
            app.input(&mut sim, i);
            if on_old && matches!(i, Input::Select | Input::Touch(_)) {
                pending = true; // the card reads over later passes
            }
            if before == Screen::Past && matches!(app.screen(), Screen::Menu(Menu::Storage, _)) {
                back += 1; // a card closed (Start, Back, or a delete): STORAGE again, its survey restarted or going on
            }
            for _ in 0..(b % 4) as usize {
                app.step(&mut sim);
                sim.delay_ms(POLL_MS);
            }
            if pending && app.screen() == Screen::Past {
                opened += 1;
                pending = false;
            }
            if !matches!(app.screen(), Screen::Menu(Menu::Storage, _) | Screen::Past) {
                pending = false;
            }
            let surveying = matches!(app.screen(), Screen::Menu(Menu::Storage, _)) && !app.survey.done();
            if k % 50 == 0 && !surveying {
                super::tests::assert_consistent(&sim, &app);
            }
        }
        done += u32::from(app.survey.done());
    }
    std::println!("storage glue: {opened} cards opened from the rows, {back} back to STORAGE, {done} surveys done");
    assert!(opened > 0 && back > 0 && done > 0, "opened {opened}, back {back}, done {done}");
}

/// The segment index (app/segments.rs): folders of segment files with broken headers and names (a flipped byte
/// in the header, a cut file, a name that is not the id, a stray file), read by the app on the Sim. Nothing panics, the
/// build ends, every candidate is either indexed or counted as skipped, and over the seeds both indexed and skipped
/// files are reached.
#[test]
fn segment_index_folders() {
    use crate::segment::tests::synth;
    let (mut indexed, mut skipped, mut loaded, mut failed) = (0usize, 0usize, 0usize, 0usize);
    for seed in 1..=24u32 {
        let mut noise = bytes(seed, 4096);
        let mut next = move || noise.next().unwrap_or(0) as usize;
        let mut files: Vec<(std::string::String, Vec<u8>)> = Vec::new();
        let mut candidates = 0;
        for k in 0..6 + next() % 6 {
            let mut f = synth(20, &[*b"07", *b"05"]);
            f[1..5].copy_from_slice(&(7000 + k as u32).to_le_bytes());
            let mut name = std::format!("{}.seg", 7000 + k);
            for _ in 0..next() % 4 {
                match next() % 7 {
                    0 | 1 => {
                        let i = next() % 0x200;
                        if let Some(b) = f.get_mut(i) {
                            *b ^= 1 << (next() % 8);
                        }
                    }
                    2 => f.truncate(1 + next() % 0x400),
                    3 => name = std::format!("{}.seg", 9000 + k), // not the id
                    4 => name = std::format!("{}.dat", 7000 + k),
                    6 => {
                        let i = 0x200 + next() * 3 % f.len().max(1); // the body
                        if let Some(b) = f.get_mut(i) {
                            *b ^= 0x10;
                        }
                    }
                    5 => f.get_mut(0x179).iter_mut().for_each(|b| **b = next() as u8),
                    _ => {}
                }
            }
            candidates += usize::from(name.ends_with(".seg"));
            files.push((name, f));
        }
        let files: Vec<(&str, Vec<u8>)> = files.iter().map(|(n, f)| (n.as_str(), f.clone())).collect();
        let image = super::segment_tests::card(&files);
        let mut sim = super::Sim::new(9, 0, 0);
        sim.emmc.image = image.clone();
        let mut app = crate::App::start(&mut sim, None);
        app.set_segment_memory(std::boxed::Box::leak(std::boxed::Box::new(crate::Segments::new())));
        super::tests::run(&mut sim, &mut app, 12_000);
        let s = app.segments().unwrap();
        assert!(s.ready(), "seed {seed}");
        assert_eq!(s.indexed() + usize::from(s.skipped), candidates, "seed {seed}");
        // every indexed file loaded: whole ones load, the ones with a broken body fail, neither panics
        let mut seg = crate::Segments::new();
        let mut disk = crate::fat::tests::Ram(image, 0);
        for id in s.ids() {
            match seg.load(&mut disk, id) {
                Ok(()) => loaded += 1,
                Err(()) => failed += 1,
            }
        }
        (indexed, skipped) = (indexed + s.indexed(), skipped + usize::from(s.skipped));
    }
    assert!(indexed > 20 && skipped > 20, "indexed {indexed}, skipped {skipped}");
    assert!(loaded > 10 && failed > 0, "loaded {loaded}, failed {failed}");
}

#[test]
fn sun_times_anywhere() {
    // any latitude, longitude, date and zone (16 h either way, 15 min steps): no panic, the times in range, the sun up
    // and down both reached (the polar cases give None)
    let (mut rose, mut total) = (0, 0);
    let mut next = xorshift(7);
    for _ in 0..20_000 {
        let lat = ((next() % 1_800_000_001) as i64 - 900_000_000) as i32;
        let lon = ((next() % 3_600_000_001) as i64 - 1_800_000_000) as i32;
        let days = 20_000 + (next() % 5_000) as i32;
        let zone = ((next() % 129) as i32 - 64) * 15;
        total += 1;
        if let Some((rise, set)) = crate::sun::rise_set(lat, lon, days, zone) {
            assert!(rise < 1440 && set >= rise && set < 2 * 1440, "{lat} {lon} {days} {zone}: {rise} {set}");
            rose += 1;
        }
    }
    assert!(rose > 0 && rose < total, "the sun up and the polar days both reached: {rose} of {total}");
}
