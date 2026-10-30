//! The overview's totals from stock's rides, the STOCK DATA menu, the fitness estimates, the ride history and
//! HISTORY's delete, on the Sim.

use super::menu_tests::to_row;
use super::tests::run; // not the parent's `run`, which the glob would give
use super::tests::*;
use super::*;
use crate::ui::Field;

/// The overview's totals: stock's rides are read by themselves a while after start-up, then the week, year and
/// all-time distance show (with the ride running added).
#[test]
fn overview_totals_from_stocks_rides() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let w = PageCfg {
        layout: 4,
        fields: [Field::WeekKm, Field::WeekH, Field::YearKm, Field::TotalKm, Field::Time, Field::Time],
    }
    .to_words();
    (sim.backup[26], sim.backup[27]) = (w[0], w[1]);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.field_text(Field::TotalKm).as_str(), "--", "not read yet");
    run(&mut sim, &mut app, u64::from(STOCK_SCAN_MS) + 20_000);
    assert_eq!(app.stock.status(), crate::stock::Status::Done, "read by itself");
    assert_eq!(app.field_text(Field::WeekKm).as_str(), "19");
    assert_eq!(app.field_text(Field::TotalKm).as_str(), "37");
    assert_consistent(&sim, &app);
    sim.save("overview_totals");
}

/// The scan's cache (storage/stock.rs `Cache`): the first start reads every ride and writes `\Rustfw\scan.bin`;
/// the next start shows the overview's totals from it at its first pass, and reads no ride. Prints what each
/// start cost with the unit's cost of reading and parsing (about 430 ns a byte): passes, the longest pass's work,
/// the time the scan took.
#[test]
fn overview_totals_from_the_cache_at_once() {
    let fields = [Field::WeekKm, Field::TotalKm, Field::Time, Field::Time, Field::Time, Field::Time];
    let page = PageCfg { layout: 2, fields };
    let start = |image: std::vec::Vec<u8>| {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = image;
        sim.work_ns_per_byte = 430;
        let w = page.to_words();
        (sim.backup[26], sim.backup[27]) = (w[0], w[1]);
        let mut app = App::start(&mut sim, None);
        app.set_scan_cache(crate::stock::tests::cache());
        (sim, app)
    };
    let (mut sim, mut app) = start(crate::stock::tests::with_rides(64));
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.field_text(Field::TotalKm).as_str(), "--", "no cache yet: nothing to show before the scan");
    run(&mut sim, &mut app, u64::from(STOCK_SCAN_MS));
    let (t0, mut passes, mut longest, mut last) = (sim.ms, 0, 0, 0);
    while app.stock.status() == crate::stock::Status::Running {
        longest = longest.max(last);
        let us = sim.awake_ns / 1000;
        run(&mut sim, &mut app, 1);
        (passes, last) = (passes + 1, sim.awake_ns / 1000 - us);
    }
    longest = longest.max(last);
    // the cache's first write: both FATs compared first, spread over passes (2026-10-25 review: in one pass, the
    // unit's 15 MB of FATs froze the loop 3 to 4 s)
    let (t1, mut writing, mut longest_write) = (sim.ms, 0, 0);
    while app.stock.unsaved() {
        let us = sim.awake_ns / 1000;
        run(&mut sim, &mut app, 1);
        (writing, longest_write) = (writing + 1, longest_write.max(sim.awake_ns / 1000 - us));
        assert!(writing < 10_000);
    }
    let fat_sectors =
        crate::fat::Fat::mount(&mut crate::fat::tests::Ram(sim.emmc.image.clone(), 0)).unwrap().fat_sectors();
    std::println!(
        "first start: {} rides read, {} bytes, {passes} passes, longest {longest} us of work, {} ms; the cache \
         written in {writing} passes (FATs of {fat_sectors} sectors), longest {longest_write} us, {} ms",
        app.stock.read,
        app.stock.out.bytes,
        t1 - t0,
        sim.ms - t1
    );
    // the budget, and the step that passed it: a FAT step reads 16 KB (7 ms at the test's 430 ns a byte)
    let bound = App::SCAN_IDLE_US + 2 * App::SYNC_SECTORS * 512 * 430 / 1000 + 500;
    assert!(longest < u64::from(bound), "each pass bounded by time: {longest} us");
    assert!(longest_write < u64::from(bound), "the write's passes too: {longest_write} us");
    assert!(writing > 1, "spread: {writing} passes");
    assert_eq!(app.stock.read, 70);
    let total = app.field_text(Field::TotalKm);
    assert!(!app.stock.unsaved(), "written");
    let mut d = crate::fat::tests::Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    assert_eq!(fat.open(&mut d, "Rustfw/scan.bin").map(|f| f.size), Ok(64 + 70 * 64));
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));

    // the next start: the totals at the first pass, then a scan that reads no ride
    let (mut sim2, mut app2) = start(sim.emmc.image.clone());
    run(&mut sim2, &mut app2, u64::from(POLL_MS));
    assert_eq!(app2.field_text(Field::TotalKm).as_str(), total.as_str(), "from the cache, at once");
    assert_eq!(app2.field_text(Field::WeekKm).as_str(), "19");
    run(&mut sim2, &mut app2, u64::from(STOCK_SCAN_MS) + 2000);
    assert_eq!(app2.stock.status(), crate::stock::Status::Done);
    std::println!("next start: {} rides read, {} bytes", app2.stock.read, app2.stock.out.bytes);
    assert_eq!((app2.stock.read, app2.stock.out.bytes, app2.stock.unsaved()), (0, 0, false));
    assert_eq!(app2.field_text(Field::TotalKm).as_str(), total.as_str());
    assert_consistent(&sim2, &app2);
}

/// Back held 3 s switches off even while a scan makes every pass long (each read here costs 250 ms of work):
/// the hold counts the passes' real time ([`Board::awake_us`]), not their number.
#[test]
fn back_held_switches_off_during_a_slow_scan() {
    let mut sim = Sim::new(9, 0, 0).keys(|ms| if (5000..9300).contains(&ms) { Key::Back as u8 } else { 0 });
    sim.emmc.image = crate::stock::tests::with_rides(64);
    sim.work_ns_per_byte = 30_000;
    let mut app = App::start(&mut sim, None);
    app.set_scan_cache(crate::stock::tests::cache());
    app.stock.start(app.date);
    let left = 4000u64.saturating_sub(sim.ms);
    run(&mut sim, &mut app, left);
    assert_eq!(app.stock.status(), crate::stock::Status::Running);
    // the other card loads (pages, altitude) come due by the real time now and make some passes a second long: the
    // hold is sampled once a pass, so it is longer here
    run(&mut sim, &mut app, 6000);
    assert!(sim.standby, "off, released at 9.3 s");
    assert_eq!(app.stock.status(), crate::stock::Status::Running, "while the scan ran");
}

/// The stock data menu over the synthetic volume (tools/fat_image.py) through the eMMC model: READ RIDES
/// reads stock's rides (a bounded amount per pass), the rows show the totals and CP, IMPORT applies stock's
/// settings. Prints what the scan cost: bytes and reads (the model takes no time to read; on the unit the
/// read speed decides).
#[test]
fn stock_data_menu() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Stock));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Stock, 0));
    let value = |app: &App, i| std::string::String::from(app.menu_row(Menu::Stock, i).unwrap().1.as_str());
    assert_eq!(value(&app, 1), "--", "the scan HISTORY started is not done");
    app.press(&mut sim, Press::Short(Key::Start));
    let (t0, cmds0) = (sim.ms, sim.emmc.cmds.len());
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim, &mut app, u64::from(POLL_MS));
        assert!(sim.ms - t0 < 60_000);
    }
    let reads = sim.emmc.cmds[cmds0..].iter().filter(|c| c.0 == 17 || c.0 == 18).count();
    std::println!("scan: {} bytes, {} reads, {} passes", app.stock.out.bytes, reads, (sim.ms - t0) / 50);
    assert_eq!(app.stock.status(), crate::stock::Status::Done);
    let at = |r| menu::FITNESS.iter().position(|&x| x == r).unwrap();
    use menu::StockRow as S;
    let rows = [S::Scan, S::WeekKm, S::AllKm].map(at);
    assert_eq!(rows.map(|i| value(&app, i)), ["4", "19", "37"].map(std::string::String::from));
    let cp = crate::power_curve::fit_cp(&app.stock.out.curve).unwrap();
    assert_eq!(value(&app, 1), std::format!("{} W", cp.cp));
    run(&mut sim, &mut app, 1000);
    assert_consistent(&sim, &app);
    sim.save("stock");
    assert!(cp.trusted());
    // the first start took stock's FTP, 265 W: the bests give a floor under it, nothing to apply, the settings ride
    assert_eq!(app.settings.ftp, 265);
    assert_eq!(value(&app, at(S::EstFtp)), std::format!(">={} W CP", cp.cp));
    assert_eq!(value(&app, at(S::ApplyFtp)), "--");
    assert_eq!(value(&app, at(S::Model)), "SETTINGS", "CP under the FTP setting");
    assert_eq!(value(&app, at(S::Hrvt1)), "--", "no full copy on this volume");
    assert_eq!(value(&app, at(S::HrvRides)), "0/0 OF 0");
    app.settings.ftp = 200; // a setting under what the bests show
    assert_eq!(value(&app, at(S::EstFtp)), std::format!("{} W CP", cp.cp));
    assert_eq!(value(&app, at(S::ApplyFtp)), std::format!("{} W", (cp.cp + 2) / 5 * 5));
    assert_eq!(value(&app, at(S::Model)), "CP FIT");

    // estimates only raise the settings: a CP just under the FTP setting keeps the settings' pair, and a W'
    // setting above the fit's is not offered (and not lowered)
    app.settings.ftp = cp.cp + 3;
    assert_eq!(value(&app, at(S::Model)), "SETTINGS", "CP under the FTP setting");
    app.settings.ftp = 200;
    app.settings.wprime_j = *menu::WPRIME_RANGE.end();
    assert_eq!(value(&app, at(S::ApplyWPrime)), "--", "the fit's W' is under the setting");
    app.settings.wprime_j = (cp.w_prime as u16).saturating_sub(5000).max(*menu::WPRIME_RANGE.start());
    assert!(value(&app, at(S::ApplyWPrime)).ends_with(" KJ"), "above the setting: offered");
    app.settings.wprime_j = *menu::WPRIME_RANGE.start(); // under the fit's, so APPLY W' has something to apply

    // APPLY FTP, then APPLY W': asked, then the estimate into the setting, saved
    for (row, want) in [(S::ApplyFtp, (cp.cp + 2) / 5 * 5), (S::ApplyWPrime, ((cp.w_prime + 250) / 500 * 500) as u16)] {
        back(&mut sim, &mut app); // HISTORY
        to_row(&mut sim, &mut app, menu::Row::Open(Menu::Stock));
        app.press(&mut sim, Press::Short(Key::Start));
        to_row(&mut sim, &mut app, menu::Row::Stock(row));
        let got = |app: &App| if row == S::ApplyFtp { app.settings.ftp } else { app.settings.wprime_j };
        let before = got(&app);
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(got(&app), before, "asked first");
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(got(&app), want);
    }
    let saved = Settings::from_word(sim.backup[WHEEL_BKP]).with_power_word(sim.backup[POWER_BKP]);
    assert_eq!((saved.ftp, saved.wprime_j), (app.settings.ftp, app.settings.wprime_j), "saved");
    run(&mut sim, &mut app, 5000); // the banner gone

    app.press(&mut sim, Press::Short(Key::Back)); // HISTORY
    app.press(&mut sim, Press::Short(Key::Back)); // MENU
    to_row(&mut sim, &mut app, menu::Row::Stock(menu::StockRow::Import)); // SETTINGS > SYSTEM > IMPORT
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start)); // sure
    assert_eq!((app.settings.ftp, app.settings.max_hr), (265, 188));
    let hr = crate::nrf::SensorId { device: 1234, trans: 0 };
    assert_eq!(sim.backup[Kind::Hr.backup()], hr.to_word());
    use crate::ui::Field::*;
    let p = app.pages();
    assert_eq!((p[0].cells(), &p[0].fields[..4]), (4, &[Power, Hr, Cadence, Speed][..]));
    assert_eq!((p[1].cells(), &p[1].fields[..2]), (2, &[Dist, Timer][..]), "ActBike=1: Bike2");
    assert_eq!((app.settings.weight_kg, app.settings.wprime_j), (73, 21_000), "72.5 kg rounded, W' in J");
    assert!(app.notes.list[0].as_str().ends_with("IMPORTED 8"), "{}", app.notes.list[0].as_str());
    assert_eq!(Settings::from_word(sim.backup[WHEEL_BKP]).ftp, 265, "kept");
}

/// FITNESS reads the recent full copies when it shows; a reading that failed (no `\Rustfw\Activities` here, or the
/// card lent meanwhile) is tried again at the next visit, not left failed until a restart.
#[test]
fn fitness_reads_the_full_copies_again_after_a_failure() {
    use crate::thresholds::Status;
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.set_threshold_memory(std::boxed::Box::leak(std::boxed::Box::new(crate::thresholds::Scan::new())));
    let status = |app: &App| app.thresholds.as_deref().map(|s| s.status());
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Stock));
    run(&mut sim, &mut app, 200);
    assert_eq!(status(&app), Some(Status::Idle), "not before FITNESS shows");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 500);
    assert_eq!(status(&app), Some(Status::Failed), "no folder of full copies on this volume");
    back(&mut sim, &mut app);
    run(&mut sim, &mut app, 200);
    assert_eq!(status(&app), Some(Status::Idle), "to be read again");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 500);
    assert_eq!(status(&app), Some(Status::Failed), "read again");
}

/// The ride's CP and W' are taken at its start and kept (app/fitness.rs): a ride started while the scan reads keeps
/// the settings' pair after the scan has the curve, for W' balance and the pacer alike; the next ride takes the fit's.
#[test]
fn ride_model_latched_at_start() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.settings.ftp = 250; // stock's 265 W would keep the settings (the fit's CP under it)
    let settings = (app.settings.ftp, app.settings.wprime_j);
    app.stock.start(app.date); // READ RIDES
    app.start_pause(); // a ride, before the curve is read
    assert_eq!(app.ride.state(), ride::State::Running);
    assert_eq!(app.cp_model(), settings);
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim, &mut app, u64::from(POLL_MS));
        assert!(sim.ms < 120_000);
    }
    let fit = app.cp_fit().unwrap();
    assert!(fit.trusted() && (fit.cp, fit.w_prime as u16) != settings, "{fit:?}");
    assert_eq!(app.cp_model(), settings, "kept for the whole ride");
    let r = app.pace_rider();
    assert_eq!((r.cp, r.w_full), (f32::from(settings.0), f32::from(settings.1)), "the pacer too");
    let value = |app: &App, r| {
        let i = menu::FITNESS.iter().position(|&x| x == r).unwrap();
        std::string::String::from(app.menu_row(Menu::Stock, i).unwrap().1.as_str())
    };
    assert_eq!(value(&app, menu::StockRow::Model), "SETTINGS", "the ride's");
    app.start_pause(); // pause
    app.end_ride();
    assert_eq!(app.ride.state(), ride::State::Idle);
    let want = (fit.cp, fit.w_prime as u16);
    assert_eq!(app.cp_model(), want, "between rides: what a ride would take now");
    assert_eq!(value(&app, menu::StockRow::Model), "CP FIT");
    run(&mut sim, &mut app, 3000);
    app.start_pause(); // the next ride
    assert_eq!(app.cp_model(), want);
}

/// The owner's colour rule: no status colour is the banner orange or the alarm red (the CLIMB bar, the course
/// profile, the DFA graph use the zone palette).
#[test]
fn grade_and_dfa_colours_avoid_the_banner_and_alarm() {
    for x10 in -200..400 {
        let c = ui::grade_color(x10);
        assert!(c != ui::ACCENT && c != ui::RED, "grade {x10}");
    }
    for v in 0..200 {
        let c = ride::dfa_color(v);
        assert!(c != ui::ACCENT && c != ui::RED, "alpha1 {v}");
    }
    assert!(ride::dfa_color(80) != ride::dfa_color(60) && ride::dfa_color(60) != ride::dfa_color(30));
}

/// A 1000 W target's half-width is whole ("1000|50", not "1000|+-5" cut from 50), and a grade keeps its sign and
/// number when the "%" does not fit.
#[test]
fn long_target_and_grade_texts_are_not_cut() {
    use crate::storage::workout_file::{Kind, Length, Step, Target, Workout};
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    let power = |lo, hi| Step {
        len: Length::Time(60),
        target: Target { kind: Kind::Power, lo, hi, pct: false, zone: 0 },
        target2: Target::OPEN,
        intensity: 5,
    };
    app.plan.workout = Workout::built("X", &[power(200, 220), power(950, 1050), power(995, 1005)]);
    for (at, want) in [(0, "210|+-10"), (1, "1000|50"), (2, "1000|+-5")] {
        let steps = app.plan.steps();
        let mut p = crate::workout::Player::start(steps, at, 0).0;
        p.second(steps, true, 0);
        app.plan.player = Some(p);
        assert_eq!(app.field_text(Field::StepTarget).as_str(), want);
    }
    let grade = |head: &str, g| {
        let mut l = Line::<8>::new();
        l.push_str(head);
        push_grade(&mut l, g);
        std::string::String::from(l.as_str())
    };
    assert_eq!(
        [grade("12K|", -100), grade("1.2K|", -100), grade("1.2K|", 57), grade("1.2K|", -57)],
        ["12K|-10%", "1.2K|-10", "1.2K|6%", "1.2K|-6%"]
    );
}

/// A new ride forgets the last one's climb plan (a frozen TOP time, the old W' budget).
#[test]
fn a_new_ride_forgets_the_pace_plan() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.pace.key = Some((1000, 3, 0));
    app.start_pause();
    assert_eq!(app.pace.key, None);
}

/// The first start over a volume stock used (the synthetic one, tools/fat_image.py) takes stock's settings and
/// pages by itself; the next start leaves what was set since alone and does not touch the card.
#[test]
fn first_start_imports_stock_settings() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let app = App::start(&mut sim, None);
    assert_eq!((app.settings.ftp, app.settings.max_hr), (265, 188));
    use crate::ui::Field::*;
    let p = app.pages();
    assert_eq!((p[0].cells(), &p[0].fields[..4]), (4, &[Power, Hr, Cadence, Speed][..]));
    assert_eq!((p[1].cells(), &p[1].fields[..2]), (2, &[Dist, Timer][..]), "ActBike=1: Bike2");
    assert_eq!(p[2], crate::DEFAULT_PAGES[2].into(), "stock had two data pages: the third as it was");
    assert!(app.notes.list[0].as_str().ends_with("IMPORTED 8"), "{}", app.notes.list[0].as_str());
    assert_eq!(sim.backup[Kind::Hr.backup()], crate::nrf::SensorId { device: 1234, trans: 0 }.to_word());
    assert_eq!(Settings::from_word(sim.backup[WHEEL_BKP]).ftp, 265, "kept");
    sim.save("first_start");

    // the owner changes FTP and a page here; the next start keeps them and reads nothing from the card
    let mut app = app;
    app.settings.ftp = 300;
    app.save_misc(&mut sim);
    app.pages[0].fields[0] = Wbal;
    crate::save_page(&mut sim, 0, &app.pages[0]);
    sim.emmc.cmds.clear();
    let app = App::start(&mut sim, None);
    assert_eq!((app.settings.ftp, app.pages()[0].fields[0]), (300, Wbal), "not imported again");
    assert!(sim.emmc.cmds.is_empty(), "the card is not read at a later start");

    // no stock data (a card without a volume): the defaults, nothing said, and not tried again
    let mut sim = Sim::new(9, 0, 0);
    let app = App::start(&mut sim, None);
    assert_eq!((app.settings.ftp, app.pages()[0]), (crate::ride::FTP, crate::DEFAULT_PAGES[0].into()));
    assert!(app.notes.list.iter().all(|n| !n.as_str().contains("IMPORTED")));
    assert_eq!(sim.backup[WHEEL_BKP] >> 24, 0xa7, "this firmware's settings from now on");

    // a strap paired and a page set here, no setting ever changed (register 11 never written): not a first start
    // (review of ecb8b98: stock's sensors and pages replaced the owner's)
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let strap = crate::nrf::SensorId { device: 77, trans: 1 };
    sim.backup[Kind::Hr.backup()] = strap.to_word();
    let page = PageCfg { layout: 1, fields: [Wbal; 6] };
    for (&r, w) in crate::PAGE_BKP[0].iter().zip(page.to_words()) {
        sim.backup[r] = w;
    }
    let app = App::start(&mut sim, None);
    assert_eq!((sim.backup[Kind::Hr.backup()], app.pages()[0]), (strap.to_word(), page.into()), "kept");
    assert_eq!(app.settings.ftp, crate::ride::FTP, "nothing imported");
}

/// The ride history over the synthetic volume (tools/fat_image.py): HISTORY starts the scan of stock's rides and
/// lists them newest first as they come; Select opens a ride's card (its track, profile and numbers, read from
/// its file once), Back returns to the list. Prints what opening a card read.
#[test]
fn ride_history() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::History));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::History, 0));
    assert_eq!(app.stock.status(), crate::stock::Status::Running, "opening it starts the scan");
    let row =
        |app: &App, i| app.menu_row(Menu::History, i).map(|(l, v, _)| std::format!("{} {}", l.as_str(), v.as_str()));
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim, &mut app, u64::from(POLL_MS));
        assert!(sim.ms < 60_000);
    }
    run(&mut sim, &mut app, 1000);
    let rows: Vec<_> = (1..6).map(|i| row(&app, i)).collect();
    assert_eq!(row(&app, 0).unwrap(), "FITNESS >", "first, what the rides give");
    let want = ["THU 22/10/2026 5.0", "TUE 20/10/2026 14.4", "THU 01/10/2026 12.0", "SAT 15/03/2025 6.0"];
    assert_eq!(rows, want.map(|r| Some(r.into())).into_iter().chain([None]).collect::<Vec<_>>(), "newest first");
    assert_consistent(&sim, &app);
    sim.save("history_list");

    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Page));
    let (cmds0, t0) = (sim.emmc.cmds.len(), sim.ms);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.notes.text.as_str(), "READING RIDE");
    while app.screen() != Screen::Past {
        run(&mut sim, &mut app, u64::from(POLL_MS));
        assert!(sim.ms - t0 < 5000, "the card opens");
    }
    let reads = sim.emmc.cmds[cmds0..].iter().filter(|c| c.0 == 17 || c.0 == 18).count();
    let file = app.stock.index.get(1).unwrap().file.size;
    assert_eq!(app.past.bytes, file, "the whole file, once");
    std::println!(
        "card: {} bytes of FIT in {reads} reads, {} ms simulated; at 2.5 MB/s on the unit about {} ms",
        app.past.bytes,
        sim.ms - t0,
        app.past.bytes / 2500
    );
    // what the records give: 2 min at 400 W then 250 W, a second a record
    let mut p = crate::power::Power::new();
    (0..1800).for_each(|t| p.sample(Some(if t < 120 { 400 } else { 250 }), true));
    let np = p.np().unwrap();
    let tss = crate::power::tss(1800, np, app.settings.ftp);
    let card = app.past.card;
    assert_eq!(card.title.as_str(), "20 OCT 08:00");
    let stats: Vec<_> = card.stats.iter().map(|(l, v)| std::format!("{l} {}", v.as_str())).collect();
    let want = [
        "TIME 0:30".into(),
        "KM 14.4".into(),
        "AVG KM/H 28.8".into(),
        "ASCENT M 80".into(),
        std::format!("NP {np}"),
        "AVG W 260".into(),
        "AVG HR 140".into(),
        "MAX HR 140".into(),
        std::format!("TSS {tss}"),
        "KJ 468".into(),
    ];
    assert_eq!(stats, want);
    assert!(app.past.track.points().len() > 100, "the loop's track");
    let h = ui::card::ELEV.3;
    let bars: Vec<u32> = (0..ui::card::ELEV.2).map(|i| app.past.elev(i, h).0).collect();
    assert!(bars.iter().all(|&b| b >= 1) && bars.iter().any(|&b| b > h / 2), "two hills over the profile");
    assert!(bars[0] < h / 4 && bars[bars.len() / 4] > h / 2, "low at the start, a top a quarter in");
    run(&mut sim, &mut app, 5000); // long enough for the banner to go
    assert_consistent(&sim, &app);
    sim.save("history_card");

    back(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Menu(Menu::History, 2), "back to the list, on that ride");
    // the ride without a track: its card all the same, "NO FIX" on the map
    app.press(&mut sim, Press::Short(Key::Lap));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 500);
    assert_eq!((app.screen(), app.past.card.title.as_str()), (Screen::Past, "22 OCT 10:00"));
    assert!(app.past.track.points().is_empty());
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
}

/// DELETE in the history (trash.rs) over the synthetic volume with a full copy of the 20 Oct ride: on a ride's card,
/// Start held asks (any other key drops it; Start alone dismisses the card, as on the ride summary), Start moves the ride into `\Rustfw\Deleted` and its full copy into
/// `\Rustfw\Deleted\Full`; the list, the totals and the power curve lose it at once (no ride read again), the list
/// keeps its place, the scan's cache is written without it, the volume stays whole. A name taken in
/// `\Rustfw\Deleted` gets a suffix. Refused during a ride and with a ride waiting to be resumed or saved.
#[test]
fn delete_rides() {
    use crate::storage::trash::tests::{file, volume};
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = volume();
    {
        // a file of the 22 Oct ride's name in \Rustfw\Deleted already
        let mut d = crate::fat::tests::Ram(core::mem::take(&mut sim.emmc.image), 0);
        let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
        let mine = fat.open(&mut d, "Rustfw").unwrap();
        let del = fat.ensure_dir(&mut d, mine, "Deleted", (0, 0)).unwrap();
        fat.create(&mut d, del, "261022100000.fit", (0, 0)).unwrap();
        sim.emmc.image = d.0;
    }
    let mut app = App::start(&mut sim, None);
    app.set_scan_cache(crate::stock::tests::cache());
    app.set_map_memory(crate::tiles::tests::memory());
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::History));
    app.press(&mut sim, Press::Short(Key::Start));
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim, &mut app, u64::from(POLL_MS));
        assert!(sim.ms < 60_000);
    }
    let row =
        |app: &App, i| app.menu_row(Menu::History, i).map(|(l, v, _)| std::format!("{} {}", l.as_str(), v.as_str()));
    let all0 = app.stock.out.all;
    let (curve0, read0) = (app.stock.out.curve, app.stock.read);
    let open_card = |sim: &mut Sim, app: &mut App, n: usize| {
        for _ in 0..20 {
            match app.screen() {
                Screen::Menu(Menu::History, s) if s < n => app.press(sim, Press::Short(Key::Page)),
                Screen::Menu(Menu::History, s) if s > n => app.press(sim, Press::Short(Key::Lap)),
                _ => break,
            }
        }
        assert_eq!(app.screen(), Screen::Menu(Menu::History, n));
        app.press(sim, Press::Short(Key::Start));
        for _ in 0..100 {
            run(sim, app, u64::from(POLL_MS));
            if app.screen() == Screen::Past {
                break;
            }
        }
        assert_eq!(app.screen(), Screen::Past);
    };
    // the 20 Oct ride (row 2): Start dismisses; asked (Start held), dropped by another key, asked again, deleted
    open_card(&mut sim, &mut app, 2);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::History, 2), "Start dismisses the card");
    assert!(file(&sim.emmc.image, "Activities/261020080000.fit").is_some(), "and deletes nothing");
    open_card(&mut sim, &mut app, 2);
    app.press(&mut sim, Press::Long(Key::Start));
    assert_eq!(app.notes.text.as_str(), "DELETE RIDE?\nSTART: YES BACK: NO");
    assert_eq!(app.confirm, Some(PAST_DELETE));
    app.press(&mut sim, Press::Short(Key::Lap));
    app.press(&mut sim, Press::Short(Key::Back)); // the banner, then (nothing asked) the list
    assert!(file(&sim.emmc.image, "Activities/261020080000.fit").is_some(), "only asked: Lap dropped it");
    open_card(&mut sim, &mut app, 2);
    app.press(&mut sim, Press::Long(Key::Start));
    let (clean, full) = (
        file(&sim.emmc.image, "Activities/261020080000.fit").unwrap(),
        file(&sim.emmc.image, "Rustfw/Activities/261020080000.fit").unwrap(),
    );
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::History, 2), "back to the list, in its place");
    assert_eq!(app.notes.text.as_str(), "RIDE DELETED");
    let img = &sim.emmc.image;
    assert_eq!(file(img, "Activities/261020080000.fit"), None);
    assert_eq!(file(img, "Rustfw/Activities/261020080000.fit"), None);
    assert_eq!(file(img, "Rustfw/Deleted/261020080000.fit"), Some(clean), "kept, under its name");
    assert_eq!(file(img, "Rustfw/Deleted/Full/261020080000.fit"), Some(full), "the full copy too");
    assert_eq!(crate::fat::write::tests::fsck(img), Ok(0));
    let rows: Vec<_> = (1..5).map(|i| row(&app, i)).collect();
    let want = ["THU 22/10/2026 5.0", "THU 01/10/2026 12.0", "SAT 15/03/2025 6.0"];
    assert_eq!(rows, want.map(|r| Some(r.into())).into_iter().chain([None]).collect::<Vec<_>>(), "at once");
    let all = app.stock.out.all;
    assert_eq!((all.rides, all.dist_m, all.time_s), (all0.rides - 1, all0.dist_m - 14_400, all0.time_s - 1800));
    assert_ne!(app.stock.out.curve, curve0, "its 400 W bests gone from the curve");
    assert_eq!((app.stock.read, app.stock.status()), (read0, crate::stock::Status::Done), "no ride read again");
    // the cache written without it, between rides
    run(&mut sim, &mut app, 1000);
    assert!(!app.stock.unsaved());
    let size = |img: &[u8], p: &str| {
        let mut d = crate::fat::tests::Ram(img.to_vec(), 0);
        let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
        fat.open(&mut d, p).map(|f| f.size)
    };
    assert_eq!(size(&sim.emmc.image, "Rustfw/scan.bin"), Ok(64 + 64 * u32::from(all.rides + app.stock.out.skipped)));
    assert_consistent(&sim, &app);

    // the 22 Oct ride: its name taken in \Rustfw\Deleted, a suffix; this time by a tap on DELETE, then a tap on it
    open_card(&mut sim, &mut app, 1);
    let (dx, dy, dw, dh) = ui::card::DELETE_TAP;
    let tap_delete = |sim: &mut Sim, app: &mut App| {
        app.input(sim, Input::Touch(Gesture::Tap((dx + dw / 2) as u16, (dy + dh / 2) as u16)))
    };
    tap_delete(&mut sim, &mut app);
    assert_eq!((app.screen(), app.confirm), (Screen::Past, Some(PAST_DELETE)), "a tap asks");
    sim.save("past_delete_asks");
    tap_delete(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Menu(Menu::History, 1));
    assert!(file(&sim.emmc.image, "Rustfw/Deleted/261022100000-1.fit").is_some_and(|f| !f.is_empty()));
    assert_eq!(file(&sim.emmc.image, "Rustfw/Deleted/261022100000.fit"), Some(Vec::new()), "the other kept");
    assert_eq!(row(&app, 1).unwrap(), "THU 01/10/2026 12.0");

    // refused during a ride, and with a ride waiting to be resumed or saved
    open_card(&mut sim, &mut app, 1);
    let ev = app.ride.start_pause();
    app.recorder(ev);
    app.press(&mut sim, Press::Long(Key::Start));
    assert_eq!(app.notes.text.as_str(), "NOT DURING A RIDE");
    assert_eq!(app.screen(), Screen::Past);
    app.ride = crate::ride::Ride::default();
    app.rec.resume_pending = true;
    app.press(&mut sim, Press::Long(Key::Start));
    assert_eq!(app.notes.text.as_str(), "NOT DURING A RIDE", "a ride waiting");
    assert!(file(&sim.emmc.image, "Activities/261001070000.fit").is_some());
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
}

/// A delete a power cut left halfway (the ride in neither folder: its chain named by no entry, trash.rs) is finished
/// when the next start's scan of the rides begins (30 s in; the card is not touched at boot), before it reads them:
/// the ride in `\Rustfw\Deleted`, the volume whole, the history without it.
#[test]
fn a_delete_cut_halfway_is_finished_at_start() {
    use crate::storage::trash::tests::{cut_midway, file};
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = cut_midway();
    assert!(crate::fat::write::tests::fsck(&sim.emmc.image).is_ok_and(|lost| lost > 0), "a lost chain");
    let mut app = App::start(&mut sim, None);
    app.set_scan_cache(crate::stock::tests::cache());
    run(&mut sim, &mut app, u64::from(STOCK_SCAN_MS) + 2000);
    assert!(file(&sim.emmc.image, "Rustfw/Deleted/261020080000.fit").is_some());
    assert!(file(&sim.emmc.image, "Rustfw/Deleted/Full/261020080000.fit").is_some());
    assert_eq!(file(&sim.emmc.image, "Activities/261020080000.fit"), None);
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
    assert_eq!(app.stock.status(), crate::stock::Status::Done);
    assert!((0..app.stock.index.len()).all(|i| app.stock.index.get(i).unwrap().start.2 != 20), "not listed");
}

/// The review of 2026-10-29, S4: that finish failing at the scan (a card error at its first write) is not
/// forgotten: still to do (`trash_check`), and done when the drive is lent, before a computer gets the card.
#[test]
fn a_cut_delete_whose_finish_fails_is_tried_again() {
    use crate::storage::trash::tests::{cut_midway, file};
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = cut_midway();
    (sim.emmc.bad, sim.emmc.bad_left) = (std::vec![0..u32::MAX], Some(1));
    let mut app = App::start(&mut sim, None);
    app.set_scan_cache(crate::stock::tests::cache());
    while sim.emmc.bad_left != Some(0) && app.gps.ms < STOCK_SCAN_MS + 2000 {
        app.step(&mut sim);
    }
    assert_eq!(sim.emmc.bad_left, Some(0), "the finish's write failed");
    assert!(app.trash_check, "still to finish");
    app.set_usb(&mut sim, crate::ui::UsbDot::Ready);
    assert_eq!(app.drive_on(&mut sim), Ok(()));
    assert!(!app.trash_check, "finished before the drive was lent");
    assert!(file(&sim.emmc.image, "Rustfw/Deleted/261020080000.fit").is_some());
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
}

/// DELETE with more rides than the scan's cache holds (storage/stock.rs `CACHE_RIDES`): the scan starts again to
/// count the rides left out of the cache, and HISTORY loses the ride deleted at once, not when that scan ends.
#[test]
fn delete_past_the_cache() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::stock::tests::with_links(crate::stock::CACHE_RIDES + 2);
    let mut app = App::start(&mut sim, None);
    app.set_scan_cache(crate::stock::tests::cache());
    run(&mut sim, &mut app, u64::from(STOCK_SCAN_MS) + 1000);
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim, &mut app, u64::from(POLL_MS));
        assert!(sim.ms < 600_000);
    }
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::History));
    app.press(&mut sim, Press::Short(Key::Start));
    for _ in 0..5 {
        if app.screen() != Screen::Menu(Menu::History, 1) {
            app.press(&mut sim, Press::Short(Key::Page));
        }
    }
    assert_eq!(app.screen(), Screen::Menu(Menu::History, 1));
    let row = |app: &App| app.menu_row(Menu::History, 1).map(|(l, v, _)| std::format!("{} {}", l.as_str(), v.as_str()));
    let (first, all0) = (row(&app), app.stock.out.all);
    assert_eq!(first.as_deref(), Some("THU 22/10/2026 5.0"));
    app.press(&mut sim, Press::Short(Key::Start)); // row 1: the newest ride's card
    for _ in 0..100 {
        run(&mut sim, &mut app, u64::from(POLL_MS));
        if app.screen() == Screen::Past {
            break;
        }
    }
    assert_eq!(app.screen(), Screen::Past);
    app.press(&mut sim, Press::Long(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.notes.text.as_str(), "RIDE DELETED");
    assert_eq!(app.stock.status(), crate::stock::Status::Running, "rides past the cache: read again");
    assert_eq!(row(&app).as_deref(), Some("TUE 20/10/2026 14.4"), "gone from the list at once");
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim, &mut app, u64::from(POLL_MS));
        assert!(sim.ms < 1_200_000);
    }
    assert_eq!(row(&app).as_deref(), Some("TUE 20/10/2026 14.4"));
    assert_eq!((app.stock.out.all.rides, app.stock.out.all.dist_m), (all0.rides - 1, all0.dist_m - 5000));
}
