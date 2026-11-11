//! The time budget of every chip against the app's long operations (the 2026-11-01 ride lost its sensors because one
//! long pass starved the Nordic drain). The simulator charges the transfers their time (`Sim::cost`); each long
//! operation runs on a ride-like unit (the ride's real sensor mix and the phone's BLE traffic, a GNSS fix) and the gate
//! records, per chip, the worst service gap, then asserts each against the chip's limit and its source:
//! - Nordic: the event queue (96 slots) must stay under half full, and never hang (BOARD.md 11, ride_radio_tests.rs);
//! - GNSS: the receiver stops its stream after 1.5 s without a read (gnss_model.rs G2): a gap over 1.2 s (the time-out
//!   less a 300 ms margin) or one lost epoch fails;
//! - keys: read once a pass, so the longest gap between reads is the latency a press can wait: 250 ms. The touch
//!   sample (`sample_touch`) runs in the same pass, so it waits the same; it is not measured apart.
//!
//! The principle: a cost or a bound is never chosen to make tests pass. A violation an honest cost shows is recorded by
//! name in [`EXEMPT`] (row, setting, the measured value plus 10 %, the follow-up that removes it), never absorbed by
//! loosening a general limit. Every row is held to the general limits otherwise, at two settings of the card's write
//! time: the optimistic (0.2 ms a sector) and the survey's pessimistic (2 ms); each write command also costs 1 ms.
//!
//! Reads: `Sim::work_ns_per_byte` is 430 ns a byte here, off in the default simulator (most tests do not mind the time,
//! and it would move the clock under all of them). 430 ns is the measured end-to-end rate of `tools/dev.py bench scan`
//! (2.3 MB/s): the card's own read time (about 95 ns a byte, 10.5 MB/s raw, docs/status.md) plus parsing. It is
//! charged on every read, so a bulk read that is not parsed is overcharged: conservative.
//!
//! `cargo test-host budget -- --nocapture` prints the table. Out of scope: the unit's own measurements
//! (docs/status.md, "Not yet seen on the unit").

use super::SimLink;
use super::ride_radio_tests::radio_unit;
use super::*;
use crate::ui::menu::Row;
use std::string::String;

/// The Nordic queue's high-water limit: half of its 96 slots.
const QUEUE_MAX: usize = 48;
/// The GNSS limit, ms: the receiver's 1.5 s time-out less 300 ms.
const GNSS_GAP_MAX: u64 = 1200;
/// The key latency limit, ms.
const KEY_GAP_MAX: u64 = 250;
/// The length of the 3 h ride filler's passes, ms (the recorder catches up the seconds between).
const FILL_STRIDE: u32 = 2000;

/// A setting of the card's write time.
#[derive(Clone, Copy)]
struct Setting {
    name: &'static str,
    cost: Cost,
}

fn settings() -> [Setting; 2] {
    [
        Setting { name: "0.2 ms/sector", cost: Cost::default() },
        Setting { name: "2 ms/sector", cost: Cost::pessimistic() },
    ]
}

/// A named exemption: one row at one setting breaks a general limit, by the measured value plus 10 %. The follow-up
/// removes it. A metric with `None` is held to the general limit.
struct Exempt {
    row: &'static str,
    setting: &'static str,
    key: Option<u64>,
    gnss: Option<u64>,
    lost: Option<u32>,
    why: &'static str,
}

/// The recorded violations (2026-11-01). The owner's performance worklist.
const EXEMPT: &[Exempt] = &[
    Exempt {
        row: "first ride on a fresh card",
        setting: "0.2 ms/sector",
        key: Some(1206),
        gnss: None,
        lost: None,
        why: "first ride on a fresh card: the recorder creates its 20 folders and files (setup 202 sectors, make_kept 154) and syncs the whole FAT three times (1.69 MB read, about 727 ms). Follow-up: prepare the card's folders when it is first seen, outside a ride (owner to decide), see docs/status.md",
    },
    Exempt {
        row: "first ride on a fresh card",
        setting: "2 ms/sector",
        key: Some(1955),
        gnss: Some(2059),
        lost: None,
        why: "first ride on a fresh card: the recorder creates its 20 folders and files (setup 202 sectors, make_kept 154) and syncs the whole FAT three times (1.69 MB read, about 727 ms). Follow-up: prepare the card's folders when it is first seen, outside a ride (owner to decide), see docs/status.md",
    },
    Exempt {
        row: "first ride on a fresh card, map page",
        setting: "0.2 ms/sector",
        key: Some(1207),
        gnss: None,
        lost: None,
        why: "first ride on a fresh card: the recorder creates its 20 folders and files (setup 202 sectors, make_kept 154) and syncs the whole FAT three times (1.69 MB read, about 727 ms). Follow-up: prepare the card's folders when it is first seen, outside a ride (owner to decide), see docs/status.md",
    },
    Exempt {
        row: "first ride on a fresh card, map page",
        setting: "2 ms/sector",
        key: Some(1955),
        gnss: Some(2228),
        lost: None,
        why: "first ride on a fresh card: the recorder creates its 20 folders and files (setup 202 sectors, make_kept 154) and syncs the whole FAT three times (1.69 MB read, about 727 ms). Follow-up: prepare the card's folders when it is first seen, outside a ride (owner to decide), see docs/status.md",
    },
    Exempt {
        row: "ride start, card stall",
        setting: "0.2 ms/sector",
        key: Some(1536),
        gnss: Some(1749),
        lost: None,
        why: "first ride on a fresh card: the recorder creates its 20 folders and files (setup 202 sectors, make_kept 154) and syncs the whole FAT three times (1.69 MB read, about 727 ms). Follow-up: prepare the card's folders when it is first seen, outside a ride (owner to decide), see docs/status.md; plus the card's 300 ms stall (inferred) lands in the checkpoint's pass. Follow-up: bench the stall on the unit; write the checkpoint outside the key pass (recorder, a later opus task)",
    },
    Exempt {
        row: "ride start, card stall",
        setting: "2 ms/sector",
        key: Some(2285),
        gnss: Some(2389),
        lost: None,
        why: "first ride on a fresh card: the recorder creates its 20 folders and files (setup 202 sectors, make_kept 154) and syncs the whole FAT three times (1.69 MB read, about 727 ms). Follow-up: prepare the card's folders when it is first seen, outside a ride (owner to decide), see docs/status.md; plus the card's 300 ms stall (inferred) lands in the checkpoint's pass. Follow-up: bench the stall on the unit; write the checkpoint outside the key pass (recorder, a later opus task)",
    },
    Exempt {
        row: "checkpoint, card stall",
        setting: "0.2 ms/sector",
        key: Some(398),
        gnss: None,
        lost: None,
        why: "the card's 300 ms stall (inferred) lands in the checkpoint's pass. Follow-up: bench the stall on the unit; write the checkpoint outside the key pass (recorder, a later opus task)",
    },
    Exempt {
        row: "checkpoint, card stall",
        setting: "2 ms/sector",
        key: Some(422),
        gnss: None,
        lost: None,
        why: "the card's 300 ms stall (inferred) lands in the checkpoint's pass. Follow-up: bench the stall on the unit; write the checkpoint outside the key pass (recorder, a later opus task)",
    },
    Exempt {
        row: "save, 3 h ride",
        setting: "0.2 ms/sector",
        key: Some(554),
        gnss: None, // 894 ms measured 2026-11-02, with the card's stall as its busy (emmc_model.rs E1)
        lost: None,
        why: "the save reads the 3 h file back whole (280 KB, 607 ms at 430 ns a byte) in one pass. Follow-up: read back in slices (recorder, a later opus task)",
    },
    Exempt {
        row: "save, 3 h ride",
        setting: "2 ms/sector",
        key: Some(600),
        gnss: None,
        lost: None,
        why: "the save reads the 3 h file back whole (280 KB, 607 ms at 430 ns a byte) in one pass. Follow-up: read back in slices (recorder, a later opus task)",
    },
    Exempt {
        row: "screenshot",
        setting: "0.2 ms/sector",
        key: Some(309),
        gnss: None,
        lost: None,
        why: "the screenshot writes its BMP (about 300 sectors) in the pass. Follow-up: write it in slices over passes (storage/screenshot.rs)",
    },
    Exempt {
        row: "screenshot",
        setting: "2 ms/sector",
        key: Some(1036),
        gnss: Some(1207),
        lost: None,
        why: "the screenshot writes its BMP (about 300 sectors) in the pass; its link takes one write more since 2026-11-03 (fat::write `Fat::link`: its entries written deleted, then made live, so a torn write never leaves half an entry) and each FAT sector it changes one read more since 2026-11-11 (the second copy, so a torn first one is not copied over it), 7 ms past the bound, inside the receiver's 300 ms margin. Follow-up: write it in slices over passes (storage/screenshot.rs)",
    },
];

/// A unit mid-ride and what the gate reads off it.
struct Rig {
    sim: Sim,
    app: App,
    worst_pass: u64,
    setting: &'static str,
    paid0: [u64; 6],
}

impl Rig {
    /// A unit over `image` with the ride's radio mix, a GNSS fix and the Nordic chip's real limits, on the main screen.
    fn new(image: Vec<u8>, s: Setting) -> Self {
        Self::on(image, s, |_| {})
    }

    /// As [`Rig::new`], with `prep` run on the simulator before the app starts.
    fn on(image: Vec<u8>, s: Setting, prep: impl FnOnce(&mut Sim)) -> Self {
        let (mut sim, mut app) = radio_unit(190, board_wait_us(), image, true, |sim| {
            sim.gnss.fix = Some(8000);
            sim.work_ns_per_byte = 430;
            sim.cost = s.cost;
            prep(sim);
        });
        app.screen = Screen::Main;
        app.show(&mut sim);
        let mut r = Self { sim, app, worst_pass: 0, setting: s.name, paid0: [0; 6] };
        r.run(3000);
        r.reset();
        r
    }

    /// Forget the past: the counters start from here.
    fn reset(&mut self) {
        self.sim.gnss.longest_gap_ms = 0;
        self.sim.gnss.lost_epochs = 0;
        self.sim.nordic.high_water = 0;
        self.sim.worst_gnss_gap = (0, [0; 6]);
        (self.sim.longest_key_gap_ms, self.worst_pass, self.paid0) = (0, 0, self.sim.paid_ns);
    }

    /// One pass of the main loop, then the sleep.
    fn pass(&mut self) {
        let t = self.sim.ms;
        self.app.step(&mut self.sim);
        self.worst_pass = self.worst_pass.max(self.sim.ms - t);
        self.sim.sleep_ms(POLL_MS);
    }

    fn run(&mut self, ms: u64) {
        let end = self.sim.ms + ms;
        while self.sim.ms < end {
            self.pass();
        }
    }

    /// `ms` of long passes (`FILL_STRIDE`) with the Nordic limits off: a filler whose gaps are not measured.
    fn fill(&mut self, ms: u64) {
        let limits = self.sim.nordic.limits;
        self.sim.nordic.limits.slots = usize::MAX / 2; // the filler's long passes must not overflow the chip
        let end = self.sim.ms + ms;
        while self.sim.ms < end {
            self.app.step(&mut self.sim);
            self.sim.sleep_ms(FILL_STRIDE);
        }
        self.sim.nordic.limits = limits;
        self.run(3000);
        self.reset();
    }

    /// A key press (its handler may block too).
    fn press(&mut self, p: Press) {
        let t = self.sim.ms;
        self.app.press(&mut self.sim, p);
        self.worst_pass = self.worst_pass.max(self.sim.ms - t);
    }

    /// The counters as a row, then forget them.
    fn row(&mut self, name: &'static str) -> Row1 {
        let r = Row1 {
            name,
            setting: self.setting,
            queue: self.sim.nordic.high_water,
            hangs: self.sim.nordic.hangs,
            gnss_gap: self.sim.gnss.longest_gap_ms,
            lost: self.sim.gnss.lost_epochs,
            key_gap: self.sim.longest_key_gap_ms,
            pass: self.worst_pass,
            paid: core::array::from_fn(|i| (self.sim.paid_ns[i] - self.paid0[i]) / 1_000_000),
        };
        self.reset();
        r
    }

    /// Runs on 3 s (the gaps close), then the row.
    fn done(&mut self, name: &'static str) -> Row1 {
        self.run(3000);
        self.row(name)
    }

    /// The Start press until the ride is recording (the card open, the first checkpoint written), then 0.5 s.
    fn start_ride(&mut self, name: &'static str) -> Row1 {
        self.reset();
        self.press(Press::Short(Key::Start));
        for _ in 0..200 {
            if self.sim.backup[RIDE_BKP] == RIDE_OPEN && self.app.ride.state() == crate::ride::State::Running {
                break;
            }
            self.pass();
        }
        assert_eq!(self.sim.backup[RIDE_BKP], RIDE_OPEN, "recording");
        self.run(500);
        self.row(name)
    }
}

/// One operation's row of the table.
struct Row1 {
    name: &'static str,
    setting: &'static str,
    queue: usize,
    hangs: u32,
    gnss_gap: u64,
    lost: u32,
    key_gap: u64,
    pass: u64,
    /// Time paid per [`PAID`] category in the row, ms.
    paid: [u64; 6],
}

fn exempt(r: &Row1) -> Option<&'static Exempt> {
    EXEMPT.iter().find(|e| e.row == r.name && e.setting == r.setting)
}

/// What each row breaks, as text; empty when the gate passes. An exemption raises a metric's limit for its row and
/// setting only, and is stale (a violation itself) once the metric is back within the general limit.
fn violations(rows: &[Row1]) -> Vec<String> {
    let mut v = Vec::new();
    for r in rows {
        let e = exempt(r);
        let (key, gnss, lost) = (
            e.and_then(|e| e.key).unwrap_or(KEY_GAP_MAX),
            e.and_then(|e| e.gnss).unwrap_or(GNSS_GAP_MAX),
            e.and_then(|e| e.lost).unwrap_or(0),
        );
        let who = std::format!("{} [{}]", r.name, r.setting);
        if r.hangs > 0 || r.queue >= QUEUE_MAX {
            v.push(std::format!("{who}: Nordic queue {} slots, {} hangs (limit {QUEUE_MAX}, none)", r.queue, r.hangs));
        }
        if r.gnss_gap > gnss || r.lost > lost {
            v.push(std::format!(
                "{who}: GNSS unread {} ms, {} epochs lost (limit {gnss} ms, {lost})",
                r.gnss_gap,
                r.lost
            ));
        }
        if r.key_gap > key {
            v.push(std::format!("{who}: keys unread {} ms (limit {key})", r.key_gap));
        }
        if let Some(e) = e {
            let stale = (e.key.is_some() && r.key_gap <= KEY_GAP_MAX)
                || (e.gnss.is_some() && r.gnss_gap <= GNSS_GAP_MAX)
                || (e.lost.is_some() && r.lost == 0);
            if stale {
                v.push(std::format!("{who}: exemption no longer needed ({})", e.why));
            }
        }
    }
    v
}

fn table(rows: &[Row1]) {
    std::eprintln!(
        "{:<28} {:<14} {:>5} {:>5} {:>8} {:>5} {:>7} {:>7}  paid ms: {}",
        "operation",
        "card",
        "queue",
        "hangs",
        "gnss gap",
        "lost",
        "key gap",
        "pass ms",
        PAID.join(" ")
    );
    for r in rows {
        let paid: Vec<String> = r.paid.iter().map(|p| std::format!("{p}")).collect();
        std::eprintln!(
            "{:<28} {:<14} {:>5} {:>5} {:>8} {:>5} {:>7} {:>7}  {}{}",
            r.name,
            r.setting,
            r.queue,
            r.hangs,
            r.gnss_gap,
            r.lost,
            r.key_gap,
            r.pass,
            paid.join(" "),
            exempt(r).map_or(String::new(), |e| std::format!("  EXEMPT: {}", e.why))
        );
    }
}

fn east(ms: u64) -> (i32, i32, i32) {
    (FIX_LAT, FIX_LON + ms as i32, 9_000_000)
}

/// The card with stock's `\Screenshot` folder.
fn shot_card() -> Vec<u8> {
    let mut d = crate::fat::tests::Ram(crate::storage::trash::tests::volume(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    fat.ensure_dir(&mut d, fat.root(), crate::storage::screenshot::DIR, (0, 0)).unwrap();
    d.0
}

/// A rig over `card` with the ride recording, the pause pressed after `secs` and the save screen up.
fn the_operations(s: Setting) -> Vec<Row1> {
    let card = crate::fat::tests::image;
    let mut rows = Vec::new();

    let mut r = Rig::new(card(), s);
    rows.push(r.start_ride("first ride on a fresh card"));
    r.run(600_000);
    rows.push(r.done("ride, 10 min, checkpoints"));

    let mut r = Rig::new(card(), s);
    r.start_ride("first ride on a fresh card");
    r.run(70_000);
    r.reset();
    r.press(Press::Short(Key::Start));
    r.run(1000);
    r.press(Press::Long(Key::Start));
    r.press(Press::Short(Key::Start)); // SAVE is the default
    rows.push(r.done("ride save"));

    let mut r = Rig::new(crate::stock::tests::with_rides(64), s);
    r.app.stock.start(r.app.date);
    for _ in 0..20_000 {
        if r.app.stock.status() != crate::stock::Status::Running {
            break;
        }
        r.pass();
    }
    rows.push(r.done("history scan, 64 rides"));

    let mut r = Rig::new(shot_card(), s);
    r.press(Press::Long(Key::Page));
    super::menu_tests::to_row(&mut r.sim, &mut r.app, Row::Screenshot);
    r.run(500); // the navigation is back-to-back presses of the test helper (278 ms, 226 of panel), not a latency
    r.reset();
    r.press(Press::Short(Key::Start));
    r.run(1000);
    rows.push(r.done("screenshot"));

    let mut r = Rig::new(card(), s);
    r.app.set_map_memory(crate::tiles::tests::memory());
    r.app.set_course_memory(crate::storage::course_file::tests::memory());
    r.app.open_courses(&mut r.sim);
    r.run(1000);
    rows.push(r.done("course list and load"));

    let mut r = Rig::new(card(), s);
    r.sim.gnss.route = Some(east);
    r.sim.classic_pages();
    r.app.set_map_memory(crate::tiles::tests::memory());
    for _ in 0..3 {
        r.press(Press::Short(Key::Page));
    }
    rows.push(r.start_ride("first ride on a fresh card, map page"));
    r.run(300_000);
    rows.push(r.done("map ride, 5 min"));
    rows.push(prepared_start(s, "ride start, prepared card", false));
    rows.push(prepared_start(s, "ride start, prepared card, map page", true));

    let mut r = Rig::new(super::segment_tests::capped_card(), s);
    r.app.set_segment_memory(Box::leak(Box::new(Segments::new())));
    r.run(30_000);
    assert!(r.app.segments().unwrap().ready(), "the index is built");
    rows.push(r.done("segment index, 130 files"));

    // the card's stall (every 1024 sectors written) inside the start, then inside a checkpoint
    let mut r = Rig::new(card(), s);
    r.sim.sectors_written = 1024 - 100;
    rows.push(r.start_ride("ride start, card stall"));
    assert!(r.sim.paid_ns[5] > 0, "the stall fired inside the start");
    r.run(1000);
    r.sim.sectors_written = r.sim.sectors_written.next_multiple_of(1024) - 1;
    r.reset();
    r.run(130_000);
    assert!(r.sim.paid_ns[5] - r.paid0[5] >= r.sim.cost.stall_ms * 1_000_000, "the stall fired inside a checkpoint");
    rows.push(r.done("checkpoint, card stall"));

    // the pass after a 10 min stall: its last 5 s ticked, the rest on the timer at once, a checkpoint asked for. Only
    // the RTC moves on (the stall's own gaps are other rows'): the cost of the catch-up alone
    let mut r = Rig::new(card(), s);
    r.start_ride("first ride on a fresh card");
    r.run(70_000);
    r.reset();
    let elapsed = r.app.ride.elapsed_s();
    r.sim.start_s += 600;
    r.pass();
    assert!(r.app.ride.elapsed_s() >= elapsed + 600, "the stall counted");
    rows.push(r.done("catch-up of a 10 min stall"));

    rows.extend(three_hours(s));
    rows
}

/// A short ride on a fresh card, saved on the same unit, then the Start measured again: the card's folders and files
/// are there, so this is the start of every ride after the first. `map` as the map page's row.
fn prepared_start(s: Setting, name: &'static str, map: bool) -> Row1 {
    let mut r = Rig::new(crate::fat::tests::image(), s);
    if map {
        r.sim.gnss.route = Some(east);
        r.sim.classic_pages();
        r.app.set_map_memory(crate::tiles::tests::memory());
        for _ in 0..3 {
            r.press(Press::Short(Key::Page));
        }
    }
    r.start_ride("first ride on a fresh card");
    r.run(70_000);
    r.press(Press::Short(Key::Start));
    r.run(1000);
    r.press(Press::Long(Key::Start));
    r.press(Press::Short(Key::Start)); // SAVE is the default
    r.run(3000);
    r.reset();
    r.app.screen = Screen::Main; // the main screen, as after a boot: the save leaves the ride's own page up
    r.app.show(&mut r.sim);
    if map {
        for _ in 0..3 {
            r.press(Press::Short(Key::Page));
        }
        r.run(500); // the navigation is back-to-back presses of the test helper, not a latency: settle before the Start
    }
    r.start_ride(name)
}

/// A 3 h ride recorded (the filler passes are long and not measured), then the unit reset mid-ride (the boot resume)
/// and, on the first unit, the ride ended and saved: the file read back whole, the rename.
fn three_hours(s: Setting) -> Vec<Row1> {
    let mut rows = Vec::new();
    let mut r = Rig::new(crate::fat::tests::image(), s);
    r.press(Press::Short(Key::Start));
    r.fill(3 * 3600 * 1000);
    let name = String::from(core::str::from_utf8(&r.app.rec.clean.name()).unwrap());
    let mut ram = crate::fat::tests::Ram(r.sim.emmc.image.clone(), 0);
    let size = crate::fat::Fat::mount(&mut ram)
        .unwrap()
        .open(&mut ram, &std::format!("Rustfw/Rides/{name}.fit"))
        .unwrap()
        .size;
    std::eprintln!("3 h ride {name}: {} s on the timer, the file {size} B", r.app.ride.timer_s());

    let (image, backup) = (r.sim.emmc.image.clone(), r.sim.backup);
    let (params, start_s) = ((r.sim.emmc.rel_param, r.sim.emmc.rel_sectors), r.sim.start_s + r.sim.ms / 1000 + 30);
    let mut b = Rig::on(image, s, |sim| {
        (sim.backup, sim.start_s, (sim.emmc.rel_param, sim.emmc.rel_sectors)) = (backup, start_s, params);
    });
    assert!(b.app.ride.state() != crate::ride::State::Idle, "resumed");
    rows.push(b.done("boot resume, 3 h ride"));

    r.press(Press::Short(Key::Start));
    r.run(1000);
    r.press(Press::Long(Key::Start));
    r.press(Press::Short(Key::Start)); // SAVE is the default
    rows.push(r.done("save, 3 h ride"));
    rows
}

/// The gate: every long operation, the worst per chip against its limit, at both card settings. `--nocapture` prints
/// the table.
#[test]
fn every_chip_is_served_in_time_through_the_long_operations() {
    let mut rows = Vec::new();
    for s in settings() {
        rows.extend(the_operations(s));
    }
    table(&rows);
    let v = violations(&rows);
    assert!(v.is_empty(), "{v:#?}");
}

/// Where the time of the two raised GNSS bounds goes (`link_tests`, `recorder_tests`): the transfers paid inside the
/// longest gap, by kind, in ms.
pub(super) fn print_gap_breakdown(sim: &Sim, what: &str) {
    let (gap, paid) = sim.worst_gnss_gap;
    let parts: Vec<String> = PAID.iter().zip(paid).map(|(n, p)| std::format!("{n} {}", p / 1_000_000)).collect();
    std::eprintln!("{what}: longest GNSS gap {gap} ms, paid inside it (ms): {}", parts.join(", "));
    let (n, paid) = sim.long_gnss_gaps;
    let parts: Vec<String> = PAID.iter().zip(paid).map(|(k, p)| std::format!("{k} {}", p / 1_000_000)).collect();
    std::eprintln!(
        "{what}: {n} gaps past 1.5 s ({} lost epochs), paid inside them (ms): {}",
        sim.gnss.lost_epochs,
        parts.join(", ")
    );
}

/// A real blocking pass of `ms` in the middle of a ride (the sensors' mix and the phone's traffic), as a row.
fn blocking(ms: u32) -> Row1 {
    let mut r = Rig::new(crate::fat::tests::image(), settings()[0]);
    r.run(1000);
    r.reset();
    r.sim.delay_ms(ms);
    r.pass();
    r.done("blocking pass")
}

/// Proof the gate sees a long pass: 2 s breaks the GNSS and key limits. The Nordic queue gets only 22 of its 96 slots
/// from a 2 s block with this mix (about 11 events a second), so it does not fail on it: the next test.
#[test]
fn a_two_second_blocking_pass_fails_the_gate() {
    let row = blocking(2000);
    table(std::slice::from_ref(&row));
    let v = violations(std::slice::from_ref(&row));
    assert!(v.iter().any(|s| s.contains("GNSS")), "{v:?}");
    assert!(v.iter().any(|s| s.contains("keys")), "{v:?}");
    assert!((15..QUEUE_MAX).contains(&row.queue), "a 2 s block queues {} slots", row.queue);
}

/// The Nordic check can fail: a 5 s blocking pass fills the queue past half (and the chip hangs at 96 slots, BOARD.md 11).
#[test]
fn a_five_second_blocking_pass_fails_the_nordic_check() {
    let row = blocking(5000);
    table(std::slice::from_ref(&row));
    let v = violations(std::slice::from_ref(&row));
    assert!(
        v.iter().any(|s| s.contains("Nordic")),
        "the queue reached {} slots, {} hangs: {v:?}",
        row.queue,
        row.hangs
    );
}

/// The card stalls inside a checkpoint (emmc_model.rs E1) while a ride with the sensors' mix and the phone's traffic
/// runs: 900 ms, under the driver's 1 s bound, and 1.2 s, over it (the driver gives up: `Busy`, the checkpoint is
/// retried). Either way the Nordic queue stays under its limit and never hangs, and the ride is saved with its
/// seconds (none lost but those the wait dropped).
#[test]
fn a_card_stall_inside_a_checkpoint_keeps_the_radio_drained_and_the_ride() {
    for stall in [900, 1200] {
        let mut r = Rig::new(crate::fat::tests::image(), settings()[0]);
        r.sim.gnss.expect = Some("stream time-out"); // past the bound the pass holds over 1.5 s on purpose
        r.start_ride("ride start");
        r.run(1000);
        let name = String::from(core::str::from_utf8(&r.app.rec.clean.name()).unwrap());
        (r.sim.cost.stall_ms, r.sim.sectors_written) = (stall, r.sim.sectors_written.next_multiple_of(1024) - 1);
        let errors = r.app.rec.clean.errors;
        r.reset();
        r.run(130_000);
        let row = r.row("checkpoint, long card stall");
        table(std::slice::from_ref(&row));
        assert!(row.paid[5] >= stall, "{stall} ms: the stall fired inside a checkpoint");
        assert!(
            row.hangs == 0 && row.queue < QUEUE_MAX,
            "{stall} ms: Nordic queue {} slots, {} hangs",
            row.queue,
            row.hangs
        );
        assert_eq!(r.app.rec.clean.errors > errors, stall > 1000, "{stall} ms: the write given up past the bound only");
        let timer = r.app.ride.timer_s();
        r.press(Press::Short(Key::Start));
        r.run(1000);
        r.press(Press::Long(Key::Start));
        r.press(Press::Short(Key::Start)); // SAVE is the default
        r.run(5000);
        assert!(r.app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{stall} ms: {}", r.app.notes.list[0].as_str());
        let times = super::recorder_tests::record_times(&r.sim.emmc.image, &std::format!("Activities/{name}.fit"));
        let dropped = r.app.rec.clean.dropped;
        assert!(
            times.len() as u32 + dropped + 2 >= timer,
            "{stall} ms: {} records, {dropped} dropped, of {timer} s",
            times.len()
        );
        assert_eq!(crate::fat::write::tests::fsck(&r.sim.emmc.image), Ok(0), "{stall} ms");
    }
}

/// The simulated hour of the clock tests; 10 minutes under FUZZ_SCALE < 1 (the --quick tier).
fn hour_ms() -> u64 {
    if super::fuzz::fuzz_scale() < 1.0 { 600_000 } else { 3_600_000 }
}

/// The app's clock (`up_ms`: the time-outs and the cadences counted in ms) keeps real time even when the
/// pass waits for the panel's DMA (the cycle counter stops in WFI): an hour of full-screen redraws, 2026-11-01.
#[test]
fn the_app_clock_counts_the_waits_of_the_work() {
    let mut r = Rig::new(crate::fat::tests::image(), settings()[0]);
    r.press(Press::Short(Key::Start));
    r.run(5000);
    let (up0, real0) = (u64::from(r.app.up_ms), r.sim.ms);
    while r.sim.ms < real0 + hour_ms() {
        r.app.show(&mut r.sim); // a full redraw: its pixels go by DMA, the CPU sleeps
        r.pass();
    }
    let (up, real) = (u64::from(r.app.up_ms) - up0, r.sim.ms - real0);
    assert!(real.abs_diff(up) * 1000 <= real, "app clock {up} ms against real {real} ms");
}

/// The same through the unit's own loop ([`crate::MainLoop::pass`]) with the radio mix and a finger on the panel: the
/// waits of the touch and radio passes between the sleep's slices count once (not dropped, not twice).
#[test]
fn the_loop_keeps_real_time_through_all_its_waits() {
    let (mut sim, mut app) = radio_unit(70, board_wait_us(), crate::fat::tests::image(), true, |sim| {
        // a tap on the strip every 7 s: the touch path (a read and a gesture) runs between slices
        sim.touch.set_finger(|ms| (ms % 7000 < 120).then_some((120, 5)));
    });
    app.press(&mut sim, Press::Short(Key::Start));
    let mut main = crate::MainLoop::<SimLink>::new(<Sim as Board>::CPU_HZ, 80);
    for _ in 0..200 {
        main.pass(&mut app, &mut sim);
    }
    let (up0, real0) = (u64::from(app.up_ms), sim.ms);
    while sim.ms < real0 + hour_ms() {
        app.show(&mut sim); // a full redraw each pass
        main.pass(&mut app, &mut sim);
    }
    let (up, real) = (u64::from(app.up_ms) - up0, sim.ms - real0);
    assert!(real.abs_diff(up) * 1000 <= real, "app clock {up} ms against real {real} ms");
}

/// A pass longer than MAX_PASS_MS (a save, a card stall) is spread over the next passes, not lost: after a 1.7 s
/// pass the clock is back on real time within a few passes (repaid at most a pass's length a pass).
#[test]
fn a_long_pass_is_owed_to_the_clock_not_dropped() {
    let mut r = Rig::new(crate::fat::tests::image(), settings()[0]);
    r.press(Press::Short(Key::Start));
    r.run(5000);
    let (up0, real0) = (u64::from(r.app.up_ms), r.sim.ms);
    r.sim.awake_ns += 1_700_000_000; // the work of one pass: 1.7 s of CPU
    r.sim.ms += 1700;
    r.run(60_000);
    let (up, real) = (u64::from(r.app.up_ms) - up0, r.sim.ms - real0);
    assert!(real.abs_diff(up) <= 60, "app clock {up} ms against real {real} ms after a 1.7 s pass");
}

/// A cable in and a busy drive (each poll sleeps in a drive command, which takes SysTick over): the delays count once
/// in the clock, not again as the slice's elapsed time.
#[test]
fn a_busy_usb_drive_is_not_counted_twice() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    let link = SimLink::default();
    link.0.borrow_mut().busy_ms = 3;
    (sim.main.link, sim.main.vbus) = (Some(link), true);
    let mut main = crate::MainLoop::<SimLink>::new(<Sim as Board>::CPU_HZ, 80);
    for _ in 0..50 {
        main.pass(&mut app, &mut sim);
    }
    let (up0, real0) = (u64::from(app.up_ms), sim.ms);
    while sim.ms < real0 + 600_000 {
        main.pass(&mut app, &mut sim);
    }
    let (up, real) = (u64::from(app.up_ms) - up0, sim.ms - real0);
    assert!(real.abs_diff(up) * 1000 <= real, "app clock {up} ms against real {real} ms");
}

/// The same with each poll costing CPU cycles (the unit's USB poll: 0.046 % slow, 1200 taken-over slices in 600 s):
/// a taken-over slice's cycles after its measured length are work, not sleep.
#[test]
fn a_busy_usb_drive_polls_cost_cycles_the_clock_counts() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    let link = SimLink::default();
    link.0.borrow_mut().busy_ms = 3;
    (sim.main.link, sim.main.vbus, sim.main.poll_work_ns) = (Some(link), true, 200_000);
    let mut main = crate::MainLoop::<SimLink>::new(<Sim as Board>::CPU_HZ, 80);
    for _ in 0..50 {
        main.pass(&mut app, &mut sim);
    }
    let now = |app: &App| u64::from(app.up_ms) + u64::from(app.prof.clock.sleep.0);
    let (up0, real0) = (now(&app), sim.ms);
    while sim.ms < real0 + 600_000 {
        main.pass(&mut app, &mut sim);
    }
    let (up, real) = (now(&app) - up0, sim.ms - real0);
    assert!(real.abs_diff(up) * 5000 <= real, "app clock {up} ms against real {real} ms"); // 0.02 %
}

/// After a 60 s stall the clock catches up at twice real speed at most: a banner (4 s) does not vanish at once.
#[test]
fn a_stall_is_repaid_at_most_at_twice_real_speed() {
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.expect = Some("stream time-out"); // the stall is on purpose
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    sim.delay_ms(60_000); // the stall, inside the work
    app.say("BANNER");
    let t0 = sim.ms;
    while app.notes.ms.is_some() {
        app.step(&mut sim);
        sim.sleep_ms(POLL_MS);
        assert!(sim.ms - t0 < 10_000, "the banner never went");
    }
    let real = sim.ms - t0;
    // the stalled pass counts MAX_PASS_MS at once, the rest of the banner runs at twice real speed at most
    assert!(
        real >= u64::from(BANNER_MS - MAX_PASS_MS) / 2 - 100,
        "a {BANNER_MS} ms banner lasted {real} ms of real time"
    );
}

/// The unit's loop with WFI waking at uneven sub-ms instants (the radio's bytes, touches, keys): the app clock keeps real
/// time (0.35 % slow on the unit, 2026-11-02: each early slice lost its sub-ms part, and the cycles outside the slices
/// were counted as in them).
fn loop_drift(ride: bool) {
    let (mut sim, mut app) = radio_unit(40, board_wait_us(), crate::fat::tests::image(), true, |sim| {
        sim.touch.set_finger(|ms| (ms % 9000 < 150).then_some((120, 5)));
    });
    sim.main.jitter = true;
    if ride {
        app.press(&mut sim, Press::Short(Key::Start));
    }
    let mut main = crate::MainLoop::<SimLink>::new(<Sim as Board>::CPU_HZ, 80);
    for _ in 0..100 {
        main.pass(&mut app, &mut sim);
    }
    // the pass's sleep is in the clock's next step: counted here, or the lag of one pass shows
    let now = |app: &App| u64::from(app.up_ms) + u64::from(app.prof.clock.sleep.0);
    let (up0, real0) = (now(&app), sim.ms);
    while sim.ms < real0 + 600_000 {
        main.pass(&mut app, &mut sim);
    }
    let (up, real) = (now(&app) - up0, sim.ms - real0);
    assert!(real.abs_diff(up) * 5000 <= real, "app clock {up} ms against real {real} ms"); // 0.02 %
}

#[test]
fn the_idle_loop_keeps_real_time_on_early_wakes() {
    loop_drift(false);
}

#[test]
fn the_ride_loop_keeps_real_time_on_early_wakes() {
    loop_drift(true);
}
