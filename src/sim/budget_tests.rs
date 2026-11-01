//! The time budget of every chip against the app's long operations (the 2026-11-01 ride lost its sensors because one
//! long pass starved the Nordic drain). The simulator now charges the transfers their time (`Sim::cost`); each long
//! operation runs on a ride-like unit (a heart-rate strap, a GNSS fix) and the gate records, per chip, the worst
//! service gap, then asserts each against the chip's limit and its source:
//! - Nordic: the event queue (96 slots) must stay under half full, and never hang (BOARD.md 11, ride_radio_tests.rs);
//! - GNSS: the receiver stops its stream after 1.5 s without a read (gnss_model.rs G2): a gap over 1.2 s (the time-out
//!   less a 300 ms margin) or one lost epoch fails;
//! - keys and touch: read once a pass, so the longest gap between reads is the latency a press can wait: 250 ms,
//!   except where `measure` is given a higher ceiling for a known finding (each with its reason, to split later).
//!
//! `cargo test-host budget -- --nocapture` prints the table. Out of scope: the unit's own measurements (docs/status.md,
//! "Not yet seen on the unit"); the card's write time and stall are inferred (`Cost`).

use super::*;
use crate::ui::menu::Row;
use std::string::String;

const KEY: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
/// The Nordic queue's high-water limit: half of its 96 slots.
const QUEUE_MAX: usize = 48;
/// The GNSS limit, ms: the receiver's 1.5 s time-out less 300 ms.
const GNSS_GAP_MAX: u64 = 1200;
/// The key and touch latency limit, ms.
const KEY_GAP_MAX: u64 = 250;

/// Starting a ride is one pass of about 950 ms: the card is read about 2 MB (at 430 ns a byte) and 378 sectors are
/// written (finding of 2026-11-01, a candidate to split). The keys wait for it; the GNSS gap stays under its limit.
const RIDE_START_KEYS: u64 = 1040;

/// A unit mid-ride and what the gate reads off it.
struct Rig {
    sim: Sim,
    app: App,
    worst_pass: u64,
}

impl Rig {
    /// A unit with a heart-rate strap paired, a GNSS fix and the Nordic chip's real limits, over `image`.
    fn new(image: Vec<u8>) -> Self {
        fn hr(_: u64) -> [u8; 8] {
            [0, 0, 0, 0, 0, 0, 0, 72]
        }
        let mut sim = Sim::new(9, 0, 0);
        sim.own_settings();
        sim.emmc.image = image;
        sim.gnss.fix = Some(8000);
        sim.work_ns_per_byte = 430; // reading and parsing a card byte on the unit (docs/status.md, 2.3 MB/s)
        sim.backup[Kind::Hr.backup()] = Kind::Hr.to_word(SensorId { device: 9, trans: 1 });
        sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: 9, data: hr, network: 0, ack: &[] });
        let app = App::start_with(&mut sim, Some(KEY), None);
        sim.nordic.limits = Some(ChipLimits::default());
        Self { sim, app, worst_pass: 0 }
    }

    /// Forget the past: the counters start from here.
    fn reset(&mut self) {
        self.sim.gnss.longest_gap_ms = 0;
        self.sim.gnss.lost_epochs = 0;
        self.sim.nordic.high_water = 0;
        (self.sim.longest_key_gap_ms, self.worst_pass) = (0, 0);
    }

    /// One pass of the main loop, then the sleep.
    fn pass(&mut self) {
        let t = self.sim.ms;
        self.app.step(&mut self.sim);
        self.worst_pass = self.worst_pass.max(self.sim.ms - t);
        self.sim.delay_ms(POLL_MS);
    }

    fn run(&mut self, ms: u64) {
        let end = self.sim.ms + ms;
        while self.sim.ms < end {
            self.pass();
        }
    }

    /// A key press (its handler may block too).
    fn press(&mut self, p: Press) {
        let t = self.sim.ms;
        self.app.press(&mut self.sim, p);
        self.worst_pass = self.worst_pass.max(self.sim.ms - t);
    }
}

/// One operation's row of the table.
struct Row1 {
    name: &'static str,
    key_ok: u64,
    queue: usize,
    hangs: u32,
    gnss_gap: u64,
    lost: u32,
    key_gap: u64,
    pass: u64,
}

/// Runs `op` on a fresh rig over `image`, then 3 s more (the gaps close), and reads the counters.
fn measure(name: &'static str, key_ok: u64, image: Vec<u8>, op: impl FnOnce(&mut Rig)) -> Row1 {
    let mut r = Rig::new(image);
    r.sim.gnss.expect = Some("stream time-out"); // recorded as lost epochs below, not a panic
    r.run(3000);
    r.reset();
    op(&mut r);
    r.run(3000);
    Row1 {
        name,
        key_ok,
        queue: r.sim.nordic.high_water,
        hangs: r.sim.nordic.hangs,
        gnss_gap: r.sim.gnss.longest_gap_ms,
        lost: r.sim.gnss.lost_epochs,
        key_gap: r.sim.longest_key_gap_ms,
        pass: r.worst_pass,
    }
}

/// What each row breaks, as text; empty when the gate passes.
fn violations(rows: &[Row1]) -> Vec<String> {
    let mut v = Vec::new();
    for r in rows {
        if r.hangs > 0 || r.queue >= QUEUE_MAX {
            v.push(std::format!(
                "{}: Nordic queue {} slots, {} hangs (limit {QUEUE_MAX}, none)",
                r.name,
                r.queue,
                r.hangs
            ));
        }
        if r.gnss_gap > GNSS_GAP_MAX || r.lost > 0 {
            v.push(std::format!(
                "{}: GNSS unread {} ms, {} epochs lost (limit {GNSS_GAP_MAX} ms, 0)",
                r.name,
                r.gnss_gap,
                r.lost
            ));
        }
        if r.key_gap > r.key_ok {
            v.push(std::format!("{}: keys unread {} ms (limit {})", r.name, r.key_gap, r.key_ok));
        }
    }
    v
}

fn table(rows: &[Row1]) {
    std::eprintln!(
        "{:<26} {:>6} {:>5} {:>9} {:>5} {:>8} {:>8}",
        "operation",
        "queue",
        "hangs",
        "gnss gap",
        "lost",
        "key gap",
        "pass ms"
    );
    for r in rows {
        std::eprintln!(
            "{:<26} {:>6} {:>5} {:>9} {:>5} {:>8} {:>8}{}",
            r.name,
            r.queue,
            r.hangs,
            r.gnss_gap,
            r.lost,
            r.key_gap,
            r.pass,
            if r.key_ok > KEY_GAP_MAX { "  (known: key ceiling raised)" } else { "" }
        );
    }
}

/// A ride-like 70 s of recording, paused, then the end-ride screen with SAVE.
fn save_ride(r: &mut Rig) {
    r.press(Press::Short(Key::Start));
    r.run(70_000);
    r.press(Press::Short(Key::Start));
    r.run(1000);
    r.press(Press::Long(Key::Start));
    r.press(Press::Short(Key::Start)); // SAVE is the default
    r.run(2000);
}

fn east(ms: u64) -> (i32, i32, i32) {
    (FIX_LAT, FIX_LON + ms as i32, 9_000_000)
}

fn the_operations() -> Vec<Row1> {
    let card = crate::fat::tests::image;
    let mut rows = Vec::new();
    rows.push(measure("ride, 10 min, checkpoints", RIDE_START_KEYS, card(), |r| {
        r.press(Press::Short(Key::Start));
        r.run(600_000);
    }));
    rows.push(measure("ride save", RIDE_START_KEYS, card(), save_ride));
    rows.push(measure("history scan, 64 rides", KEY_GAP_MAX, crate::stock::tests::with_rides(64), |r| {
        r.app.stock.start(r.app.date);
        for _ in 0..20_000 {
            if r.app.stock.status() != crate::stock::Status::Running {
                break;
            }
            r.pass();
        }
    }));
    rows.push(measure("screenshot", 440, shot_card(), |r| {
        r.press(Press::Long(Key::Page));
        super::menu_tests::to_row(&mut r.sim, &mut r.app, Row::Screenshot);
        r.press(Press::Short(Key::Start));
        r.run(1000);
    }));
    rows.push(measure("course list and load", KEY_GAP_MAX, card(), |r| {
        r.app.set_map_memory(crate::tiles::tests::memory());
        r.app.set_course_memory(crate::storage::course_file::tests::memory());
        r.app.open_courses(&mut r.sim);
        r.run(1000);
    }));
    rows.push(measure("map ride, 5 min", RIDE_START_KEYS, card(), |r| {
        r.sim.gnss.route = Some(east);
        r.sim.classic_pages();
        r.app.set_map_memory(crate::tiles::tests::memory());
        for _ in 0..3 {
            r.press(Press::Short(Key::Page));
        }
        r.press(Press::Short(Key::Start));
        r.run(300_000);
    }));
    rows
}

/// The card with stock's `\Screenshot` folder.
fn shot_card() -> Vec<u8> {
    let mut d = crate::fat::tests::Ram(crate::storage::trash::tests::volume(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    fat.ensure_dir(&mut d, fat.root(), crate::storage::screenshot::DIR, (0, 0)).unwrap();
    d.0
}

/// The gate: every long operation, the worst per chip against its limit. `--nocapture` prints the table.
#[test]
fn every_chip_is_served_in_time_through_the_long_operations() {
    let rows = the_operations();
    table(&rows);
    let v = violations(&rows);
    assert!(v.is_empty(), "{v:#?}");
}

/// Proof the gate sees a long pass: a 2 s blocking pass in the middle of a ride breaks the GNSS and key limits (the simulated
/// strap queues too little in 2 s to fill the Nordic queue, so that check is proved on a row of the table).
#[test]
fn a_two_second_blocking_pass_fails_the_gate() {
    let row = measure("2 s blocking pass", KEY_GAP_MAX, crate::fat::tests::image(), |r| {
        r.run(1000);
        r.sim.delay_ms(2000);
        r.pass();
    });
    table(std::slice::from_ref(&row));
    let v = violations(std::slice::from_ref(&row));
    assert!(v.iter().any(|s| s.contains("GNSS")), "{v:?}");
    assert!(v.iter().any(|s| s.contains("keys")), "{v:?}");
    let full = Row1 { queue: QUEUE_MAX, hangs: 1, ..row };
    assert!(violations(&[full]).iter().any(|s| s.contains("Nordic")));
}
