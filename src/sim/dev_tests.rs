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
    ] {
        pc.got.clear();
        s.feed(format!("{bad}\n").as_bytes(), &app, Stats::default(), &mut pc);
        assert!(pc.got.starts_with(b"err usage"), "{bad}");
    }
    assert_eq!(pc.regs.len(), 1);
}

#[test]
fn info_answers() {
    let (_, app) = app_at(1, 2, 3);
    let mut pc = Pc::default();
    Session::new().feed(b"info\n", &app, Stats { uptime_ms: 1234, awake_permille: 0, cpu_permille: 0 }, &mut pc);
    // held 65: the keys at the start (bit 0) and, a first start, the settings' file to write (bit 6)
    assert_eq!(
        pc.got,
        format!(
            "bryton-rustfw {} {} up 1234 ms, awake 0 permille, cpu 0 permille, radio resets 0, climb tile us last 0 max 0 slice max 0, auto off held 65, ride idle\n",
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
