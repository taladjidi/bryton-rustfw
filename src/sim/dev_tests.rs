//! The dev link (dev.rs) driven by a fake computer ([`Pc`]): a screenshot equals the simulated panel, a good image
//! loads, bad ones are refused.

use super::*;
use crate::dev::{Effect, Session, Stats};

fn app_at(h: u64, m: u64, s: u64) -> (Sim, App) {
    let mut sim = Sim::new(h, m, s);
    let mut app = App::start(&mut sim, None);
    app.step(&mut sim);
    (sim, app)
}

/// A minimal image as tools/pack.py makes it: SP, reset vector, tag, size word.
fn image(size: u32) -> Vec<u8> {
    let mut img = vec![0u8; size as usize];
    img[0..4].copy_from_slice(&0x2002_0000u32.to_le_bytes());
    img[4..8].copy_from_slice(&0xd000_0101u32.to_le_bytes());
    img[0x94..0x98].copy_from_slice(b"R039");
    img[0x98..0x9c].copy_from_slice(&size.to_le_bytes());
    img
}

fn load(s: &mut Session, app: &App, pc: &mut Pc, img: &[u8], sum: u32) -> Effect {
    let cmd = format!("load {} {}\n", img.len(), sum);
    assert_eq!(s.feed(cmd.as_bytes(), app, Stats::default(), pc), Effect::None);
    let mut effect = Effect::None;
    for chunk in img.chunks(512) {
        effect = s.feed(chunk, app, Stats::default(), pc);
    }
    effect
}

#[test]
fn peek_and_poke_registers_only() {
    let (_, app) = app_at(1, 2, 3);
    let mut pc = Pc::default();
    let mut s = Session::new();
    s.feed(b"poke 0x40013014 4098\npeek 0x40013010 2\n", &app, Stats::default(), &mut pc);
    assert_eq!(pc.got, b"ok\n00000000\n00001002\n");
    for bad in [
        "peek 0x10000000",
        "peek 0x2001fffc 2",
        "poke 0x20000000 1",
        "poke 0xd0000000 1",
        "peek 0x40013001",
        "peek 0x5ffffffc 2",
        "peek 0x40000000 65",
        "peek 0xe000e010",
        "peek 0xe000e33c 2",
        "peek 0x",
        "poke 0x40013000",
        // what could brick the unit or lose its clock: the flash controller (KEYR, OPTKEYR, CR, OPTSR), the
        // watchdogs (a start lasts until a power cycle), RCC_BDCR (BDRST wipes the backup registers, stops the RTC)
        "poke 0x52002004 1",
        "poke 0x52002008 1",
        "poke 0x5200201c 1",
        "poke 0x58004800 1",
        "poke 0x58004c00 1",
        "poke 0x50003000 1",
        "poke 0x40002c00 1",
        "poke 0x58024470 1",
    ] {
        pc.got.clear();
        s.feed(format!("{bad}\n").as_bytes(), &app, Stats::default(), &mut pc);
        assert!(pc.got.starts_with(b"err usage"), "{bad}");
    }
    assert_eq!(pc.regs.len(), 1);
    // read only there: still peeked
    pc.got.clear();
    s.feed(b"peek 0x52002004\npeek 0x58024470\n", &app, Stats::default(), &mut pc);
    assert_eq!(pc.peeked, [0x4001_3010, 0x4001_3014, 0x5200_2004, 0x5802_4470]);
}

#[test]
fn info_answers() {
    let (_, app) = app_at(1, 2, 3);
    let mut pc = Pc::default();
    Session::new().feed(b"info\n", &app, Stats { uptime_ms: 1234, ..Stats::default() }, &mut pc);
    // held 65: the keys at the start (bit 0) and, a first start, the settings' file to write (bit 6)
    // the app's up_ms at the start is not the test's business: matched as a number
    let got = std::string::String::from_utf8(pc.got.clone()).unwrap();
    let (head, rest) = got.split_once(" appup ").unwrap();
    let (n, tail) = rest.split_once(", ").unwrap();
    assert!(n.parse::<u32>().is_ok(), "appup is a number: {n}");
    assert_eq!(
        format!("{head} appup N, {tail}").as_bytes(),
        format!(
            "bryton-rustfw {} {} up 1234 ms, awake 0 permille, cpu 0 permille, loop early 0 0 0 0 0 stops 0 0 0 0 0 0 appup N, radio resets 0 resyncs 0, ant events lost 0, ant HR 0 SPEED 0 CAD 0 SPD+CAD 0 POWER 0 SHIFT 0 DI2 0 TRAIN 0, climb tile us last 0 max 0 slice max 0, auto off held 65, ride idle, card writes yes, ride stop built no, power steps 0x00000000, usb recoveries 0, phone link mtu 23 int 0 phy 0 dle 0 ext 0 boost 0 0 0 last read 0 B 0 ms hvn 0 sent 0 full 0 turns 0\n",
            env!("CARGO_PKG_VERSION"),
            env!("GIT_DESCRIBE")
        )
        .as_bytes()
    );
}

/// `where` names the screen, the cursor row and the row count, as the menus draw them.
#[test]
fn where_names_the_cursor_row() {
    let (mut sim, mut app) = app_at(1, 2, 3);
    let ask = |app: &App| {
        let mut pc = Pc::default();
        Session::new().feed(b"where\n", app, Stats::default(), &mut pc);
        std::string::String::from_utf8(pc.got).unwrap()
    };
    assert_eq!(ask(&app), "where OVERVIEW\n");
    app.press(&mut sim, Press::Long(Key::Page));
    app.press(&mut sim, Press::Short(Key::Page));
    let Screen::Menu(m, i) = app.screen() else { panic!("{:?}", app.screen()) };
    let label = app.menu_row(m, i).unwrap().0;
    assert_eq!(ask(&app), format!("where MENU|{i}/{}|{}\n", app.rows(m), label.as_str()));
    assert_eq!(i, 1);
    // other screens by kind
    app.screen = Screen::Ride(2);
    assert_eq!(ask(&app), "where RIDE 2\n");
    app.screen = Screen::Nearby;
    assert_eq!(ask(&app), "where SCREEN NEARBY\n");
    app.screen = Screen::Laps(3);
    assert_eq!(ask(&app), "where SCREEN LAPS\n");
    // a cursor past the last row answers an empty label; the answer stays short (labels are 16 characters at most)
    app.screen = Screen::Menu(Menu::Settings, 99);
    let a = ask(&app);
    assert!(a.starts_with("where SETTINGS|99/") && a.ends_with("|\n") && a.len() <= 54, "{a}");
    // the longest titles and labels: cut by their types, never past the line
    for m in [Menu::Notifs, Menu::BtTrainers, Menu::Segments, Menu::Climbs] {
        app.screen = Screen::Menu(m, 0);
        let a = ask(&app);
        assert!(a.len() <= 54 && a.ends_with('\n') && a.matches('\n').count() == 1, "{a}");
    }
}

/// The screenshot (rendered row by row from the app's state) must equal what the panel shows.
#[test]
fn shot_equals_the_panel() {
    let (sim, app) = app_at(12, 34, 56);
    let mut pc = Pc::default();
    Session::new().feed(b"shot\n", &app, Stats::default(), &mut pc);
    let header = b"shot 240 320\n";
    assert_eq!(&pc.got[..header.len()], header);
    let px: Vec<u16> = pc.got[header.len()..].chunks_exact(2).map(|b| u16::from_be_bytes([b[0], b[1]])).collect();
    assert!(px == sim.screen);
}

#[test]
fn load_runs_a_good_image() {
    let (_, app) = app_at(0, 0, 0);
    let (mut s, mut pc) = (Session::new(), Pc::default());
    let img = image(3000);
    let sum = img.iter().map(|&b| u32::from(b)).sum();
    assert_eq!(load(&mut s, &app, &mut pc, &img, sum), Effect::Run(3000));
    assert_eq!(pc.got, b"ready\nok\n");
    assert!(pc.staged == img);
}

#[test]
fn load_refuses_bad_images_and_recovers() {
    let (_, app) = app_at(0, 0, 0);
    let (mut s, mut pc) = (Session::new(), Pc::default());
    let img = image(3000);
    assert_eq!(load(&mut s, &app, &mut pc, &img, 1), Effect::None);
    assert!(pc.got.ends_with(b"err checksum\n"));
    let mut bad = image(3000);
    bad[0x94] = b'X';
    let sum = bad.iter().map(|&b| u32::from(b)).sum();
    pc = Pc::default();
    assert_eq!(load(&mut s, &app, &mut pc, &bad, sum), Effect::None);
    assert!(pc.got.ends_with(b"err no R### tag at 0x94\n"));
    pc = Pc::default();
    assert_eq!(s.feed(b"load 99999999 0\nreset\n", &app, Stats::default(), &mut pc), Effect::Reset);
    assert!(pc.got.starts_with(b"err usage"));
    assert!(pc.got.ends_with(b"ok\n"));
}

/// `bench map`: parsed with negative offsets, bad ones refused; the app draws the full-page map's basemap at the
/// position moved by them, IGN tiles decoded (the synthetic packs), and says so; `err nofix` without a position.
#[test]
fn bench_map() {
    let (sim, app) = app_at(9, 0, 0);
    let mut pc = Pc::default();
    let mut s = Session::new();
    assert_eq!(s.feed(b"bench map 6 -300 120\n", &app, Stats::default(), &mut pc), Effect::BenchMap(6, -300, 120));
    assert_eq!(s.feed(b"bench map 6 x 1\n", &app, Stats::default(), &mut pc), Effect::None);
    assert!(pc.got.starts_with(b"err usage: bench map"));
    drop(sim);

    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::tiles::tests::image_with_packs();
    let aid = crate::gnss::Aid { offset_15min: Some(8), pos: Some(crate::tiles::tests::ORIGIN) };
    [sim.backup[17], sim.backup[18]] = aid.to_words();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    let s = app.bench_map(&mut sim, 4, 100, 0).expect("the last known position");
    assert!(s.ign && s.ign_tiles > 0 && s.px > 0, "{s:?}");
    let mut pc = Pc::default();
    crate::dev::send_bench_map(&mut pc, Some((1500, s)));
    assert!(pc.got.starts_with(b"map 000005dc "), "{:?}", std::string::String::from_utf8_lossy(&pc.got));
    let mut pc = Pc::default();
    crate::dev::send_bench_map(&mut pc, None);
    assert_eq!(pc.got, b"err nofix\n");
}

/// The profiler on the Sim: each pass times its sections end to end (each read of the counter ends one section and
/// begins the next), right across the counter's wrap; `prof` answers them, `prof reset` asks for a new window.
#[test]
fn prof_times_the_sections_and_answers() {
    let (mut sim, mut app) = app_at(1, 2, 3);
    app.prof().reset();
    sim.cycles_step = 1000;
    sim.cycles = u32::MAX - 2500; // wraps in the first pass
    // the awake clock starts from this counter, not from 0
    app.prof().clock = Default::default();
    app.awake_at = app.prof().clock.us(&mut sim);
    for _ in 0..4 {
        app.step(&mut sim);
    }
    let p = app.prof_figures().clone();
    let slot = |s: crate::prof::Sec| p.slots[s as usize];
    use crate::prof::Sec;
    // every pass runs these, each one counter read long (no other read between two laps on an idle overview)
    for s in [Sec::Second, Sec::Battery, Sec::Gnss, Sec::Strip, Sec::Draw, Sec::Keys, Sec::Recorder, Sec::Phone] {
        assert_eq!(slot(s).count, 4, "{s:?}");
        assert!(slot(s).total >= 4000 && slot(s).max >= 1000, "{s:?}: {:?}", slot(s));
        assert!(slot(s).max < 1 << 20, "{s:?}: no wrap mistaken for a 21 s run");
    }
    assert_eq!(slot(Sec::Light).count, 4, "light once a pass");
    assert_eq!(slot(Sec::Step).count, 0, "the main loop times the whole step");
    let mut pc = Pc::default();
    let mut s = Session::new();
    assert_eq!(s.feed(b"prof\n", &app, Stats::default(), &mut pc), Effect::None);
    let text = std::string::String::from_utf8(pc.got.clone()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "prof 0 0 0 0 0 0");
    assert_eq!(lines.len(), 2 + crate::prof::SECTIONS);
    assert_eq!(lines.last(), Some(&"end"));
    let draw = slot(Sec::Draw);
    assert!(lines.contains(&format!("draw 4 {} {}", draw.total, draw.max).as_str()), "{text}");
    pc.got.clear();
    assert_eq!(s.feed(b"prof reset\n", &app, Stats::default(), &mut pc), Effect::ProfReset);
    assert_eq!(pc.got, b"ok\n");
    pc.got.clear();
    s.feed(b"prof now\n", &app, Stats::default(), &mut pc);
    assert!(pc.got.starts_with(b"err usage"));
}

/// `App::serve` answers the eMMC effects (what main.rs used to do inline), flushes, and leaves the keys, the reset
/// and the image run to the board.
#[test]
fn serve_answers_the_card_effects() {
    let (mut sim, mut app) = app_at(9, 0, 0);
    let (mut s, mut pc) = (Session::new(), Pc::default());
    for e in [Effect::None, Effect::Reset, Effect::Run(4), Effect::Key(Press::Short(Key::Back)), Effect::ProfReset] {
        assert!(!app.serve(&mut sim, &mut pc, &s, e));
    }
    assert!(pc.got.is_empty());
    assert!(app.serve(&mut sim, &mut pc, &s, Effect::Sector(7)));
    let hex: std::string::String = emmc_sector(7).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(pc.got, format!("{hex}\n").as_bytes());
    pc.got.clear();
    assert!(app.serve(&mut sim, &mut pc, &s, Effect::Bench(0, 4, 2)));
    assert!(pc.got.starts_with(b"bench "), "{:?}", pc.got);
    pc.got.clear();
    assert_eq!(s.feed(b"bench step 5\n", &app, Stats::default(), &mut pc), Effect::BenchStep(5));
    assert_eq!(s.feed(b"bench step 17\n", &app, Stats::default(), &mut pc), Effect::None);
    assert!(pc.got.starts_with(b"err usage: bench step"));
    pc.got.clear();
    let (up, slept) = (app.up_ms, app.prof.clock.sleep);
    assert!(app.serve(&mut sim, &mut pc, &s, Effect::BenchStep(5)));
    // 4 of the 5 steps ran with no sleep: far less than the 4 loop sleeps a stale value would count
    assert!(app.up_ms - up < 4 * crate::POLL_MS, "{} ms", app.up_ms - up);
    assert_eq!(app.prof.clock.sleep, slept);
    let line = std::string::String::from_utf8_lossy(&pc.got).into_owned();
    let w: std::vec::Vec<&str> = line.split_whitespace().collect();
    assert_eq!((w[0], w[1], w[7]), ("step", "us", "ratio"), "{line}");
    assert!(w[2..7].iter().all(|n| n.parse::<u32>().is_ok()) && (w[8].contains('.') || w[8] == "n/a"), "{line}");
    pc.got.clear();
    crate::dev::send_bench_step(&mut pc, &[300, 0, 0]);
    assert_eq!(pc.got, b"step us 300 0 0 ratio n/a\n");
    pc.got.clear();
    crate::dev::send_bench_step(&mut pc, &[300, 100, 100]);
    assert_eq!(pc.got, b"step us 300 100 100 ratio 3.0\n");
    pc.got.clear();
    assert!(app.serve(&mut sim, &mut pc, &s, Effect::BenchMap(4, 0, 0)));
    assert!(pc.got.starts_with(b"err nofix") || pc.got.starts_with(b"map"), "{:?}", pc.got);
    pc.got.clear();
    s.feed(b"ls /\n", &app, Stats::default(), &mut pc);
    pc.got.clear();
    assert!(app.serve(&mut sim, &mut pc, &s, Effect::Ls));
    assert!(pc.got.starts_with(b"err ") || pc.got.ends_with(b"end\n"), "{:?}", pc.got);
}

/// `stackfault` (the stack guard's test on the unit) answers `ok` and leaves the overflow to main.rs; never while a
/// ride is open.
#[test]
fn stackfault_only_with_no_ride_open() {
    let (mut sim, mut app) = app_at(9, 0, 0);
    let (mut s, mut pc) = (Session::new(), Pc::default());
    assert_eq!(s.feed(b"stackfault\n", &app, Stats::default(), &mut pc), Effect::StackFault);
    assert_eq!(pc.got, b"ok\n");
    assert!(!app.serve(&mut sim, &mut pc, &s, Effect::StackFault));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), crate::ride::State::Running);
    pc.got.clear();
    assert_eq!(s.feed(b"stackfault\n", &app, Stats::default(), &mut pc), Effect::None);
    assert_eq!(pc.got, b"err ride open\n");
}

/// With a ride open the commands that stall the loop, run a new image, or write a register refuse; the read-only
/// ones still answer.
#[test]
fn ride_open_refuses_the_risky_commands() {
    let (mut sim, mut app) = app_at(9, 0, 0);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), crate::ride::State::Running);
    let (mut s, mut pc) = (Session::new(), Pc::default());
    for cmd in [
        "load 1000 5",
        "reset",
        "find 0 4 0 9",
        "bench 0 4 2",
        "bench scan 4",
        "bench map 4 0 0",
        "bench step 4",
        "sector 3",
        "poke 0x40013014 1",
        "ls /",
        "get /Activities/a.fit 0 16",
        "emmc",
        "bench stall 6",
    ] {
        pc.got.clear();
        assert_eq!(s.feed(format!("{cmd}\n").as_bytes(), &app, Stats::default(), &mut pc), Effect::None, "{cmd}");
        assert_eq!(pc.got, b"err ride open\n", "{cmd}");
    }
    assert!(pc.regs.is_empty());
    pc.got.clear();
    s.feed(b"peek 0x40013010\nprof reset\n", &app, Stats::default(), &mut pc);
    assert_eq!(pc.got, b"00000000\nok\n");
    // settled again: a bench over the cap is refused with its usage
    let (_, app) = app_at(9, 0, 0);
    pc.got.clear();
    s.feed(b"bench 0 4 1001\n", &app, Stats::default(), &mut pc);
    assert!(pc.got.starts_with(b"err usage: bench"), "{:?}", pc.got);
}

/// An image whose upload began with no ride does not run when a ride started before its last byte: the ride stays.
#[test]
fn an_upload_finished_in_a_ride_does_not_run() {
    let (mut sim, mut app) = app_at(9, 0, 0);
    let img = image(4096);
    let sum = img.iter().map(|&b| u32::from(b)).fold(0u32, u32::wrapping_add);
    let (mut s, mut pc) = (Session::new(), Pc::default());
    s.feed(format!("load {} {sum}\n", img.len()).as_bytes(), &app, Stats::default(), &mut pc);
    assert_eq!(pc.got, b"ready\n");
    s.feed(&img[..1000], &app, Stats::default(), &mut pc);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), crate::ride::State::Running);
    pc.got.clear();
    assert_eq!(s.feed(&img[1000..], &app, Stats::default(), &mut pc), Effect::None);
    assert_eq!(pc.got, b"err ride open\n");
    // the same image with no ride runs
    let (_, idle) = app_at(9, 0, 0);
    assert_eq!(load(&mut s, &idle, &mut Pc::default(), &img, sum), Effect::Run(4096));
}

/// Two commands with an effect in one packet: the second waits in the session and runs on the next call, never lost.
#[test]
fn a_packet_of_several_commands_runs_them_all() {
    let (_, app) = app_at(9, 0, 0);
    let (mut s, mut pc) = (Session::new(), Pc::default());
    let e = s.feed(b"key start\nkey back long\nwhere\n", &app, Stats::default(), &mut pc);
    assert_eq!(e, Effect::Key(Press::Short(Key::Start)));
    assert!(s.pending());
    assert_eq!(s.feed(&[], &app, Stats::default(), &mut pc), Effect::Key(Press::Long(Key::Back)));
    assert!(s.pending());
    assert_eq!(s.feed(&[], &app, Stats::default(), &mut pc), Effect::None);
    assert!(!s.pending());
    assert!(pc.got.starts_with(b"ok\nok\nwhere "), "{:?}", pc.got);
    // a line cut by the effect's packet end goes on with the next packet
    pc.got.clear();
    assert_eq!(s.feed(b"key lap\nwhe", &app, Stats::default(), &mut pc), Effect::Key(Press::Short(Key::Lap)));
    assert_eq!(s.feed(&[], &app, Stats::default(), &mut pc), Effect::None);
    assert_eq!(s.feed(b"re\n", &app, Stats::default(), &mut pc), Effect::None);
    assert!(pc.got.starts_with(b"ok\nwhere "), "{:?}", pc.got);
    // a new client (usb.rs: a bus reset, a DTR change) drops what the last one left: its key never comes
    pc.got.clear();
    assert_eq!(s.feed(b"key lap\nkey start\n", &app, Stats::default(), &mut pc), Effect::Key(Press::Short(Key::Lap)));
    s.drop_pending();
    assert!(!s.pending());
    assert_eq!(s.feed(b"where\n", &app, Stats::default(), &mut pc), Effect::None);
    assert!(pc.got.starts_with(b"ok\nwhere "), "{:?}", pc.got);
}

/// An upload the computer abandoned does not swallow the next commands: after 5 s without a byte it is dropped, with an error line.
#[test]
fn a_stalled_upload_is_dropped() {
    let (_, app) = app_at(9, 0, 0);
    let (mut s, mut pc) = (Session::new(), Pc::default());
    let at = |uptime_ms| Stats { uptime_ms, ..Stats::default() };
    s.feed(b"load 1000 5\n", &app, at(100), &mut pc);
    assert_eq!(pc.got, b"ready\n");
    s.feed(&[7; 100], &app, at(4900), &mut pc); // 4.8 s later: still the upload
    assert_eq!(pc.got, b"ready\n");
    pc.got.clear();
    s.feed(b"info\n", &app, at(9901), &mut pc); // 5.001 s of silence: a command
    assert!(pc.got.starts_with(b"err load timeout\nbryton-rustfw"), "{:?}", pc.got);
}

/// A line longer than the buffer is refused whole, never run cut.
#[test]
fn a_long_line_is_refused() {
    let (_, app) = app_at(9, 0, 0);
    let (mut s, mut pc) = (Session::new(), Pc::default());
    let long = format!("get /{} 0 100\n", "a".repeat(70));
    s.feed(long.as_bytes(), &app, Stats::default(), &mut pc);
    assert_eq!(pc.got, b"err line too long\n");
    assert_eq!(s.path(), "");
    pc.got.clear();
    s.feed(format!("peek {}\n", "0".repeat(59)).as_bytes(), &app, Stats::default(), &mut pc); // 64 bytes: fits
    assert!(pc.got.starts_with(b"err usage: peek"), "{:?}", pc.got);
    pc.got.clear();
    s.feed(b"where\n", &app, Stats::default(), &mut pc);
    assert!(pc.got.starts_with(b"where "), "{:?}", pc.got);
}

/// `bench stall <s>` runs during a ride up to 5 s (longer is refused): the sleep is one gap, the ride's timer takes
/// it on the next pass, and the session answers `stall <ms>`. With no ride, up to 120 s. `bench step` is still
/// refused in the ride.
#[test]
fn bench_stall_runs_during_a_ride() {
    let (_, idle) = app_at(9, 0, 0);
    let (mut sim, mut app) = app_at(9, 0, 0);
    app.press(&mut sim, crate::Press::Short(crate::Key::Start));
    assert_eq!(app.ride.state(), crate::ride::State::Running);
    let t = app.ride.timer_s();
    let (mut s, mut pc) = (Session::new(), Pc::default());
    assert_eq!(s.feed(b"bench stall 120\n", &idle, Stats::default(), &mut pc), Effect::BenchStall(120));
    assert_eq!(s.feed(b"bench stall 6\n", &app, Stats::default(), &mut pc), Effect::None);
    assert_eq!(pc.got, b"err ride open\n");
    pc.got.clear();
    let e = s.feed(b"bench stall 5\n", &app, Stats::default(), &mut pc);
    assert_eq!(e, Effect::BenchStall(5));
    assert!(app.serve(&mut sim, &mut pc, &s, e));
    let answer = std::str::from_utf8(&pc.got).unwrap_or("");
    let ms: u32 = answer.trim().strip_prefix("stall ").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    assert!((4_500..=5_500).contains(&ms), "{answer:?}");
    app.step(&mut sim);
    assert!(app.ride.timer_s().abs_diff(t + 5) <= 1, "timer {} after {t}", app.ride.timer_s());
    pc.got.clear();
    assert_eq!(s.feed(b"bench step 2\n", &app, Stats::default(), &mut pc), Effect::None);
    assert_eq!(pc.got, b"err ride open\n");
}

/// `bench srefresh` and `bench stop` (docs/power.md "Stop mode", phase 1) are the main loop's: parsed here, refused with
/// a ride open (the SDRAM path is under test, and `stop` ends in a reset), `stop`'s `ok` sent before its reset.
#[test]
fn stop_benches_wait_for_a_settled_ride() {
    let (mut sim, mut app) = app_at(9, 0, 0);
    let (mut s, mut pc) = (Session::new(), Pc::default());
    assert_eq!(s.feed(b"bench srefresh 100\n", &app, Stats::default(), &mut pc), Effect::BenchSrefresh(100));
    assert!(pc.got.is_empty());
    assert!(!app.serve(&mut sim, &mut pc, &s, Effect::BenchSrefresh(100)), "the loop's, not serve's");
    use crate::stopbench::Mode;
    for (line, mode) in [(&b"bench stop 1000\n"[..], Mode::Stop), (b"bench stop 1000 usboff\n", Mode::UsbOff)] {
        assert_eq!(s.feed(line, &app, Stats::default(), &mut pc), Effect::BenchStop(1000, mode));
        assert_eq!(pc.got, b"ok\n");
        pc.got.clear();
    }
    assert_eq!(s.feed(b"bench stop 1000 keep\n", &app, Stats::default(), &mut pc), Effect::BenchStop(1000, Mode::Keep));
    assert!(pc.got.is_empty(), "keep answers with its report line");
    for bad in [&b"bench stop 30001\n"[..], b"bench stop 100 kept\n", b"bench stop 100 keep usboff\n"] {
        assert_eq!(s.feed(bad, &app, Stats::default(), &mut pc), Effect::None);
        assert!(pc.got.starts_with(b"err usage: bench stop"), "{:?}", pc.got);
        pc.got.clear();
    }
    // peek and poke reach the mapped peripheral blocks only: the typo that faulted the unit is refused
    s.feed(b"peek 0x584244a8\n", &app, Stats::default(), &mut pc);
    assert!(pc.got.starts_with(b"err usage: peek") && pc.peeked.is_empty(), "{:?}", pc.got);
    pc.got.clear();
    s.feed(b"peek 0x580244a8\n", &app, Stats::default(), &mut pc);
    assert_eq!(pc.peeked, [0x5802_44a8]);
    pc.got.clear();
    s.feed(b"poke 0x40002800 1\n", &app, Stats::default(), &mut pc);
    assert!(pc.got.starts_with(b"err usage: poke") && pc.regs.is_empty(), "{:?}", pc.got);
    pc.got.clear();
    app.press(&mut sim, crate::Press::Short(crate::Key::Start));
    assert_eq!(app.ride.state(), crate::ride::State::Running);
    for line in [&b"bench srefresh 100\n"[..], b"bench stop 1000\n", b"bench stop 1000 keep\n"] {
        pc.got.clear();
        assert_eq!(s.feed(line, &app, Stats::default(), &mut pc), Effect::None);
        assert_eq!(pc.got, b"err ride open\n");
    }
}
