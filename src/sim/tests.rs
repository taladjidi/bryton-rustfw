//! The simulator tests: the whole app on [`Sim`], driven by keys, a finger, sensors and fixes as functions of
//! time, checked on what the panel shows.

use super::*;
use crate::ui::tests::Frame;
use crate::ui::{Field, Rec, Strip, UsbDot};
/// A stand-in for the ANT+ network key (the real one is never in the repository).
const KEY: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
/// The owner's strap, as the unit paired it.
pub(super) const STRAP: SensorId = SensorId { device: 25126, trans: 0xf1 };

fn hr72(_: u64) -> [u8; 8] {
    [4, 0, 0, 0, 0, 0, 0, 72]
}
fn hr80(_: u64) -> [u8; 8] {
    [4, 0, 0, 0, 0, 0, 0, 80]
}
/// Speed and cadence: every second one crank turn and three wheel turns.
fn spd_cad(ms: u64) -> [u8; 8] {
    let (t, n) = (((ms / 1000) * 1024) as u16, (ms / 1000) as u16);
    let [t0, t1] = t.to_le_bytes();
    let ([c0, c1], [w0, w1]) = (n.to_le_bytes(), (3 * n).to_le_bytes());
    [t0, t1, c0, c1, t0, t1, w0, w1]
}
fn power300(_: u64) -> [u8; 8] {
    [0x10, 1, 0xff, 85, 0, 0, 0x2c, 0x01]
}

/// Passes of the main loop for `ms`: a step, then the sleep, in which (as on the unit) the touch panel is read
/// when INT falls (here: seen within a [`TOUCH_MS`] slice) and every [`TOUCH_MS`] while a finger is down.
fn run(sim: &mut Sim, app: &mut App, ms: u64) {
    let end = sim.ms + ms;
    while sim.ms < end && !sim.standby {
        app.step(sim);
        for _ in 0..POLL_MS / TOUCH_MS {
            sim.delay_ms(TOUCH_MS);
            if let Some(i) = app.touch(sim, TOUCH_MS) {
                app.input(sim, i);
            }
        }
    }
}

/// Back, after the banner up (if one is) is cleared: a Back with a banner up only clears it.
pub(super) fn back(sim: &mut Sim, app: &mut App) {
    if app.banner_ms.is_some() {
        app.press(sim, Press::Short(Key::Back));
    }
    app.press(sim, Press::Short(Key::Back));
}

/// The screen drawn step by step on the panel equals the screen drawn at once from the state (the screenshot).
pub(super) fn assert_consistent(sim: &Sim, app: &App) {
    for y in 0..H {
        let (drawn, panel) = (app.render_row(y).px, &sim.screen[(y * W) as usize..((y + 1) * W) as usize]);
        let bad: Vec<_> = (0..W as usize).filter(|&x| drawn[x] != panel[x]).collect();
        assert!(bad.is_empty(), "row {y}, x {bad:?}: screenshot {:04x}, panel {:04x}", drawn[bad[0]], panel[bad[0]]);
    }
}

/// Ride page `n` of `app` as the drawing code means it, with these values, under a strip at the sim's time
/// (`radio`: the Nordic chip answered).
fn ride(sim: &mut Sim, app: &App, n: usize, values: &[&str], radio: bool) -> Vec<u16> {
    let (h, m, _) = rtc_time_from_tr(sim.rtc().0);
    let mut f = Frame::new();
    crate::ui::draw_strip(
        &mut f,
        &Strip {
            time: (h, m),
            radio,
            gps: app.strip.gps,
            blink: app.strip.blink,
            usb: UsbDot::Off,
            battery: Some(63),
            charging: false,
            rec: app.strip.rec,
            locked: app.strip.locked,
        },
    );
    let page = app.pages()[n];
    for (i, v) in values.iter().enumerate() {
        let bx = page.cell_box(i).unwrap();
        // heart rate and power show their zone as a bar along the box's top, the number white (steady test
        // sensors: the 3 s average the power's box takes equals the live value)
        crate::ui::draw_field_frame_bar(&mut f, bx, page.fields[i], zone_bar(page.fields[i], v));
        crate::ui::draw_field_value(&mut f, bx, v, crate::ui::WHITE);
    }
    f.px
}

/// The zone bar of heart rate and power (default max HR and FTP), none (black) for the rest.
fn zone_bar(f: Field, v: &str) -> u16 {
    match (f, v.parse::<u16>()) {
        (Field::Hr, Ok(v)) => crate::ride::hr_color(crate::ride::hr_zone(v, crate::ride::MAX_HR)),
        (Field::Power, Ok(v)) => crate::ride::power_color(crate::ride::power_zone(v, crate::ride::FTP)),
        _ => crate::ui::BLACK,
    }
}

fn hm(sim: &mut Sim) -> std::string::String {
    let (h, m, _) = rtc_time_from_tr(sim.rtc().0);
    format!("{h:02}:{m:02}")
}

/// The main screen as the drawing code means it, at the sim's time, under a strip with `usb`.
fn main_screen(sim: &mut Sim, app: &App, usb: UsbDot) -> Vec<u16> {
    let (h, m, s) = rtc_time_from_tr(sim.rtc().0);
    let mut f = Frame::new();
    crate::ui::draw_strip(
        &mut f,
        &Strip {
            time: (h, m),
            radio: true,
            gps: app.strip.gps,
            blink: app.strip.blink,
            usb,
            battery: Some(63),
            charging: false,
            rec: app.strip.rec,
            locked: app.strip.locked,
        },
    );
    let _ = s;
    crate::ui::draw_main_time(&mut f, [h / 10, h % 10, m / 10, m % 10]);
    crate::ui::draw_date(&mut f, crate::date_text(rtc_date_from_dr(sim.rtc().1)).as_str());
    // the barometer is read a second after the start: the temperature is there by the time the tests look
    let temp = app.field_text(Field::Temp);
    let values = [(Field::WeekKm, "--"), (Field::WeekH, "--"), (Field::TotalKm, "--"), (Field::Temp, temp.as_str())];
    for (i, (field, v)) in values.into_iter().enumerate() {
        let bx = crate::ui::overview_box(i).unwrap();
        crate::ui::draw_field_frame(&mut f, bx, field);
        crate::ui::draw_field_value(&mut f, bx, v, crate::ui::WHITE);
    }
    f.px
}

/// The main screen, the overview: the strip, the time, the date, four fields (no notices yet, so nothing under them).
#[test]
fn boots_into_the_main_screen() {
    let mut sim = Sim::new(12, 34, 56);
    let mut app = App::start(&mut sim, None);
    app.step(&mut sim);
    app.set_usb(&mut sim, UsbDot::Ready);
    assert!(sim.visible());
    let pos = |c| sim.cmds.iter().position(|&x| x == c).unwrap();
    assert!(pos(SWRESET) < pos(SLPOUT) && pos(SLPOUT) < pos(DISPON));
    assert!(main_screen(&mut sim, &app, UsbDot::Ready) == sim.screen);
    assert_consistent(&sim, &app);
    // no notices: nothing under the fields, Ferris is not on the overview
    let (top, bottom) = (ui::NOTICES_Y as usize * W as usize, H as usize * W as usize);
    assert!(sim.screen[top..bottom].iter().all(|&p| p == ui::BLACK), "no Ferris on the overview");
    sim.save("preview");
}

/// Stock's order: reset, sd_ble_enable, then the version; no ANT without a key, no scan before the list; then the
/// phone link's GATT server (app/phone.rs), and no advertising without a bond.
#[test]
fn starts_the_nordic_chip() {
    let mut sim = Sim::new(12, 0, 0);
    App::start(&mut sim, None);
    assert_eq!(sim.nordic.got[..2], [vec![0, 0x60], vec![0, 0x65, 1]]);
    assert_eq!(sim.nordic.got[2], [&[0, 0x7c, 1, 0, 4, 0, 1][..], b"S510"].concat(), "the name first");
    assert!(sim.nordic.got.iter().all(|p| p[..2] != [0, 0x73] && p[..2] != [0, 0x8a]));
}

/// A chip that never answers costs one timeout per command, then the pages run and the label is red.
#[test]
fn a_silent_nordic_chip_does_not_stop_the_pages() {
    let mut sim = Sim::new(12, 0, 0);
    sim.nordic.dead = true;
    let t0 = sim.ms;
    sim.classic_pages();
    let mut app = App::start(&mut sim, Some(KEY));
    assert!(sim.ms - t0 < 3000, "start took {} ms", sim.ms - t0);
    assert_eq!(sim.nordic.got, [vec![0, 0x60]], "gave up after the first timeout");
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Page));
    assert!(ride(&mut sim, &app, 0, &["--", "--", "--"], false) == sim.screen, "BLE in red");
}

#[test]
fn ticks_through_midnight() {
    let mut sim = Sim::new(23, 59, 58);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 3000);
    assert_eq!(hm(&mut sim), "00:00");
    assert!(main_screen(&mut sim, &app, UsbDot::Off) == sim.screen);
}

/// Page goes from the main screen through the ride pages that are on (page 3 is the map by default), and back.
#[test]
fn page_turns_the_ride_pages() {
    let mut sim = Sim::new(9, 0, 0);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    app.step(&mut sim);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0));
    assert!(ride(&mut sim, &app, 0, &["--"; 3], true) == sim.screen, "full height, no Ferris");
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(1));
    assert_eq!(app.pages()[1].fields[..4], [Field::Speed, Field::Hr, Field::Cadence, Field::Power]);
    assert!(ride(&mut sim, &app, 1, &["--"; 4], true) == sim.screen);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(2), "the map");
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0), "round the ride pages, not back to the overview");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Main, "Back: the overview");
    assert_consistent(&sim, &app);
}

/// A finger swiping left on the main screen opens the first ride page, swiping right comes back; touching wakes
/// the backlight; without a finger, the chip is never read. Then a tap on the menu's second row opens page 1's
/// settings, as Start on that row does.
#[test]
fn touch_swipes_pages_and_taps_menu_rows() {
    let swipe = |t0: u64, x0: i64, dx: i64| {
        move |ms: u64| (t0..t0 + 250).contains(&ms).then(|| ((x0 + dx * (ms - t0) as i64 / 250) as u16, 160))
    };
    let (left, right) = (swipe(5000, 200, -120), swipe(6000, 40, 120)); // after the start (a few seconds)
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| left(ms).or(right(ms)));
    let mut app = App::start(&mut sim, None);
    let left_ms = 4900 - sim.ms;
    run(&mut sim, &mut app, left_ms);
    assert_eq!(sim.touch.reads, 0, "no finger, no read");
    run(&mut sim, &mut app, 500);
    assert_eq!(app.screen(), Screen::Ride(0));
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.screen(), Screen::Ride(2), "swipe right: the previous ride page, round");
    assert_consistent(&sim, &app);

    let row3 = ui::LIST_Y as u16 + 3 * ui::MENU_ROW as u16 + 5; // PAGES
    let taps = |ms: u64| (5000..5100).contains(&ms) || (5600..5700).contains(&ms);
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| taps(ms).then_some((120, row3)));
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    let left_ms = 5500 - sim.ms;
    run(&mut sim, &mut app, left_ms);
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 3), "the first tap selects row 3");
    assert_consistent(&sim, &app);
    run(&mut sim, &mut app, 500);
    assert_eq!(app.screen(), Screen::Menu(Menu::Pages, 0), "a second tap on it acts: PAGES");
    assert_consistent(&sim, &app);
}

/// The gear right of the overview's date: a tap opens the menu; a tap elsewhere on the overview does not.
#[test]
fn gear_on_the_overview_opens_the_menu() {
    let (gx, gy) = (ui::GEAR_AT.0 as u16 + 9, ui::GEAR_AT.1 as u16 + 9);
    let tap = move |t0: u64, p: (u16, u16)| move |ms: u64| (t0..t0 + 80).contains(&ms).then_some(p);
    let (a, b) = (tap(5000, (40, 160)), tap(6000, (gx, gy)));
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| a(ms).or(b(ms)));
    let mut app = App::start(&mut sim, None);
    sim.save("overview_gear");
    let left = 5500 - sim.ms;
    run(&mut sim, &mut app, left);
    assert_eq!(app.screen(), Screen::Main, "a tap on a field: nothing");
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 0), "a tap on the gear: the menu");
    assert_consistent(&sim, &app);
}

/// A USB drop the board recovered from is counted in its backup register (tag 0x0b, over stock's leftovers) and said.
#[test]
fn usb_drops_are_counted() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    sim.backup[USB_DROPS_BKP] = 0x3c5; // a value stock left there
    app.usb_dropped(&mut sim);
    app.usb_dropped(&mut sim);
    assert_eq!(sim.backup[USB_DROPS_BKP], 0x0b00_0002);
    assert_eq!(app.notices[0].as_str(), "09:00 USB RECONNECT");
}

/// The two diagnostics that share registers 0 and 31 (settings.rs) never read each other's words as their own: the
/// standby breadcrumbs' reset flags, whose top byte can be the drop count's tag 0x0b raw, are tagged 0x5c, so a drop
/// after them counts from 1 (the review of 2026-10-25).
#[test]
fn the_standby_flags_are_no_usb_drop_count() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    sim.backup[STANDBY_FLAGS_BKP] = standby_flags_word(0x0b00_0000, 0x40); // pin and power-on resets, standby
    app.usb_dropped(&mut sim);
    assert_eq!(sim.backup[USB_DROPS_BKP], 0x0b00_0001);
}

/// Quick touches count: a 60 ms tap and a 120 ms swipe (the panel is read on INT and every 10 ms while a finger
/// is down, not once per 50 ms pass when INT happens to be low).
#[test]
fn quick_taps_and_swipes() {
    let swipe = |ms: u64| (5000..5120).contains(&ms).then(|| ((200 - (ms - 5000) as i64 * 120 / 120) as u16, 160));
    let row1 = ui::LIST_Y as u16 + ui::MENU_ROW as u16 + 5;
    let tap = move |ms: u64| (7003..7063).contains(&ms).then_some((120, row1));
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| swipe(ms).or(tap(ms)));
    let mut app = App::start(&mut sim, None);
    let left = 5500 - sim.ms;
    run(&mut sim, &mut app, left);
    assert_eq!(app.screen(), Screen::Ride(0), "the fast swipe left");
    app.press(&mut sim, Press::Long(Key::Page));
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 1), "the short tap selects row 1");
}

/// A tap in a menu's bottom rows, under the last slot (y 316 to 319), selects the last row shown, not the first
/// of the next screen (2026-10-25 review: it turned the page).
#[test]
fn a_tap_at_the_bottom_stays_on_the_screen() {
    for y in [ui::H - 1, ui::LIST_Y + ui::MENU_ROWS as u32 * ui::MENU_ROW] {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        app.press(&mut sim, Press::Long(Key::Page));
        to_row(&mut sim, &mut app, menu::Row::Open(Menu::Sensors));
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(app.screen(), Screen::Menu(Menu::Sensors, 0));
        assert!(app.rows(Menu::Sensors) > ui::MENU_ROWS, "a second screen of rows");
        app.input(&mut sim, Input::Touch(Gesture::Tap(120, y as u16)));
        assert_eq!(app.screen(), Screen::Menu(Menu::Sensors, ui::MENU_ROWS - 1), "y {y}");
        assert_consistent(&sim, &app);
    }
}

/// Touch in a menu: a tap on MAX HR selects it, a second one edits it, a swipe up adds one, a tap keeps it, a swipe right goes back.
#[test]
fn touch_edits_a_number_and_goes_back() {
    let row = |r: u16| ui::LIST_Y as u16 + r * ui::MENU_ROW as u16 + 5;
    let max_hr = row(row_of(menu::Row::Set(Setting::MaxHr)) as u16);
    let tap = move |t0: u64, y: u16| move |ms: u64| (t0..t0 + 100).contains(&ms).then_some((120, y));
    let drag = |t0: u64, (x0, y0): (i64, i64), (dx, dy): (i64, i64)| {
        move |ms: u64| {
            let k = ms.checked_sub(t0).filter(|&k| k < 250)? as i64;
            Some(((x0 + dx * k / 250) as u16, (y0 + dy * k / 250) as u16))
        }
    };
    let (a, b, c, d) =
        (tap(5000, max_hr), drag(6000, (120, 250), (0, -100)), tap(7000, max_hr), drag(8000, (40, 160), (120, 0)));
    let first = tap(4500, max_hr); // a tap selects the row the menu opened on; the next one acts
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| first(ms).or(a(ms)).or(b(ms)).or(c(ms)).or(d(ms)));
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::MaxHr)); // in RIDER
    let left = 6900 - sim.ms;
    run(&mut sim, &mut app, left);
    assert!(app.editing, "the tap edits");
    assert_eq!(main_row(&app, row_of(menu::Row::Set(Setting::MaxHr))), "MAX HR < 191 >", "the swipe up adds one");
    run(&mut sim, &mut app, 1000);
    assert!(!app.editing, "the second tap keeps it");
    assert_eq!(Settings::from_word(sim.backup[11]).max_hr, 191);
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.screen(), Screen::Menu(Menu::Settings, row_of(menu::Row::Open(Menu::Me))), "swipe right: back");
    assert_consistent(&sim, &app);
}

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
    let mut sim = Sim::new(9, 0, 0).keys(|ms| if (5000..8300).contains(&ms) { Key::Back as u8 } else { 0 });
    sim.emmc.image = crate::stock::tests::with_rides(64);
    sim.work_ns_per_byte = 30_000;
    let mut app = App::start(&mut sim, None);
    app.set_scan_cache(crate::stock::tests::cache());
    app.stock.start(app.date);
    let left = 4000u64.saturating_sub(sim.ms);
    run(&mut sim, &mut app, left);
    assert_eq!(app.stock.status(), crate::stock::Status::Running);
    run(&mut sim, &mut app, 5000);
    assert!(sim.standby, "off, released at 8.3 s");
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
    assert_eq!([0, 4, 8].map(|i| value(&app, i)), ["4", "19", "37"].map(std::string::String::from));
    let cp = crate::power_curve::fit_cp(&app.stock.out.curve).unwrap();
    assert_eq!(value(&app, 1), std::format!("{} W", cp.cp));
    run(&mut sim, &mut app, 1000);
    assert_consistent(&sim, &app);
    sim.save("stock");

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
    assert!(app.notices[0].as_str().ends_with("IMPORTED 8"), "{}", app.notices[0].as_str());
    assert_eq!(Settings::from_word(sim.backup[WHEEL_BKP]).ftp, 265, "kept");
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
    assert!(app.notices[0].as_str().ends_with("IMPORTED 8"), "{}", app.notices[0].as_str());
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
    assert!(app.notices.iter().all(|n| !n.as_str().contains("IMPORTED")));
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
    let want = ["THU 22 OCT 5.0", "TUE 20 OCT 14.4", "THU 1 OCT 12.0", "SAT 15 MAR 6.0"];
    assert_eq!(rows, want.map(|r| Some(r.into())).into_iter().chain([None]).collect::<Vec<_>>(), "newest first");
    assert_consistent(&sim, &app);
    sim.save("history_list");

    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Page));
    let (cmds0, t0) = (sim.emmc.cmds.len(), sim.ms);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.banner.as_str(), "READING RIDE");
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
    run(&mut sim, &mut app, 500);
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
/// Start asks (any other key drops it), Start again moves the ride into `\Rustfw\Deleted` and its full copy into
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
    // the 20 Oct ride (row 2): asked, dropped by another key, asked again, deleted
    open_card(&mut sim, &mut app, 2);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.banner.as_str(), "DELETE RIDE?\nSTART: YES BACK: NO");
    assert_eq!(app.confirm, Some(PAST_DELETE));
    app.press(&mut sim, Press::Short(Key::Lap));
    app.press(&mut sim, Press::Short(Key::Start));
    assert!(file(&sim.emmc.image, "Activities/261020080000.fit").is_some(), "only asked: Lap dropped the first");
    let (clean, full) = (
        file(&sim.emmc.image, "Activities/261020080000.fit").unwrap(),
        file(&sim.emmc.image, "Rustfw/Activities/261020080000.fit").unwrap(),
    );
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::History, 2), "back to the list, in its place");
    assert_eq!(app.banner.as_str(), "RIDE DELETED");
    let img = &sim.emmc.image;
    assert_eq!(file(img, "Activities/261020080000.fit"), None);
    assert_eq!(file(img, "Rustfw/Activities/261020080000.fit"), None);
    assert_eq!(file(img, "Rustfw/Deleted/261020080000.fit"), Some(clean), "kept, under its name");
    assert_eq!(file(img, "Rustfw/Deleted/Full/261020080000.fit"), Some(full), "the full copy too");
    assert_eq!(crate::fat::write::tests::fsck(img), Ok(0));
    let rows: Vec<_> = (1..5).map(|i| row(&app, i)).collect();
    let want = ["THU 22 OCT 5.0", "THU 1 OCT 12.0", "SAT 15 MAR 6.0"];
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

    // the 22 Oct ride: its name taken in \Rustfw\Deleted, a suffix
    open_card(&mut sim, &mut app, 1);
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::History, 1));
    assert!(file(&sim.emmc.image, "Rustfw/Deleted/261022100000-1.fit").is_some_and(|f| !f.is_empty()));
    assert_eq!(file(&sim.emmc.image, "Rustfw/Deleted/261022100000.fit"), Some(Vec::new()), "the other kept");
    assert_eq!(row(&app, 1).unwrap(), "THU 1 OCT 12.0");

    // refused during a ride, and with a ride waiting to be resumed or saved
    open_card(&mut sim, &mut app, 1);
    let ev = app.ride.start_pause();
    app.recorder(ev);
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.banner.as_str(), "NOT DURING A RIDE");
    assert_eq!(app.screen(), Screen::Past);
    app.ride = crate::ride::Ride::default();
    app.resume_pending = true;
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.banner.as_str(), "NOT DURING A RIDE", "a ride waiting");
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

fn power210(_: u64) -> [u8; 8] {
    [0x10, 1, 0xff, 85, 0, 0, 210, 0]
}

/// The synthetic card's planned workouts (tools/fat_image.py, today 2026-10-23): WORKOUTS lists today's and the
/// next days' (not the past one, not the broken one), Select shows the steps (repeats written out), Start
/// starts the ride and the workout. The step clock follows the ride timer: beep, beep, beeeep and a banner per step,
/// the target coloured from the 3 s power, a pause stops it, Lap skips a step (and ends the open cool-down),
/// the last step ends the workout.
#[test]
fn workout_from_the_calendar() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings(); // the defaults, not stock's
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power210, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    // the overview's field: today's planned time, the card read when the field is first shown
    app.pages[OVERVIEW].fields[3] = Workout;
    app.show(&mut sim);
    assert_eq!(text(&app, Workout), "0:01", "20 s + 2 x 45 s, the open cool-down not counted");
    app.pages[0] = PageCfg { layout: 6, fields: [StepTarget, StepTime, NextStep, Workout, Power3s, Timer] }.into();

    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Workouts, 0));
    let row = |app: &App, m, i| {
        app.menu_row(m, i).map(|(l, v, _)| format!("{} {}", l.as_str(), v.as_str())).unwrap_or_default()
    };
    assert_eq!([0, 1, 2].map(|i| row(&app, Menu::Workouts, i)), ["FRI 23 OCT 0:01", "SUN 25 OCT 1:30", ""]);
    run(&mut sim, &mut app, 1000);
    assert_consistent(&sim, &app);
    sim.save("workout_list");

    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.plan.name()), (Screen::Menu(Menu::Steps, 0), "SHORT INTERVALS"));
    let steps: Vec<_> = (0..8).map(|i| row(&app, Menu::Steps, i)).collect();
    let want = ["START 0:01", "0:20 100-120W", "0:30 200-220W", "0:15 150W", "0:30 200-220W", "0:15 150W"];
    assert_eq!(steps[..6], want, "FTP 200: Bryton's 50-60 %, then % FTP, then W");
    assert_eq!(steps[6..], ["LAP OPEN", ""]);
    run(&mut sim, &mut app, 1000);
    assert_consistent(&sim, &app);
    sim.save("workout_steps");

    let tones = sim.tones.len();
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.ride.state()), (Screen::Ride(0), ride::State::Running));
    let fields = |app: &App| [StepTarget, StepTime, NextStep, Workout].map(|f| text(app, f));
    assert_eq!(fields(&app), ["110|+-10", "0:20", "210|+-10", "1/6"]);
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.field_color(StepTarget), ui::RED, "210 W over 100-120");
    let (mut ms, start) = (0, sim.ms);
    while app.player.and_then(|p| p.step(app.plan.steps())) == Some(0) {
        run(&mut sim, &mut app, 50);
        ms += 50;
        assert!(ms < 30_000, "the warm-up ends");
    }
    assert!((14_000..16_500).contains(&(sim.ms - start)), "20 s from the start: {} ms after 5 s", sim.ms - start);
    assert_eq!(sim.tones[tones..], [2500, 0, 2000, 0, 2000, 0, 2500], "start, beep at 2 and 1 s left, the next step");
    assert_eq!(app.banner.as_str(), "2/6 200-220W");
    run(&mut sim, &mut app, 5000);
    assert_eq!(fields(&app), ["210|+-10", "0:25", "150", "2/6"]);
    assert_eq!(app.field_color(StepTarget), ui::GREEN, "210 W in 200-220");
    assert_consistent(&sim, &app);
    sim.save("workout_ride");

    // a pause stops the step clock with the ride's timer
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    assert_eq!(text(&app, StepTime), "0:25");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 2000);
    assert_eq!(text(&app, StepTime), "0:23");

    // Lap: the next step; the open cool-down waits for Lap; then done
    app.press(&mut sim, Press::Short(Key::Lap));
    assert_eq!((text(&app, Workout), app.banner.as_str()), ("3/6".into(), "3/6 150W"));
    assert_eq!(app.field_color(StepTarget), ui::RED, "210 W over 150 W");

    // one workout at a time: while it runs, another one's steps do not open, its own do
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.banner.as_str()), (Screen::Menu(Menu::Workouts, 1), "WORKOUT RUNNING"));
    app.press(&mut sim, Press::Short(Key::Lap));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), text(&app, Workout)), (Screen::Menu(Menu::Steps, 0), "3/6".into()));
    for _ in 0..4 {
        app.press(&mut sim, Press::Short(Key::Back)); // WORKOUTS, ALL SETTINGS, the ride menu, out
    }
    assert_eq!(app.screen(), Screen::Ride(0));
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Lap));
    }
    run(&mut sim, &mut app, 3000);
    assert_eq!(fields(&app), ["OPEN", "LAP", "END", "6/6"]);
    assert_eq!(app.banner.as_str(), "6/6 OPEN LAP");
    app.press(&mut sim, Press::Short(Key::Lap));
    assert_eq!((text(&app, Workout), app.banner.as_str()), ("DONE".into(), "WORKOUT DONE"));
    assert_eq!(fields(&app)[..3], ["--", "--", "--"]);
    run(&mut sim, &mut app, 1000);
    assert_consistent(&sim, &app);
    assert_eq!(app.ride.state(), ride::State::Running, "the ride goes on");
}

/// The workout graph field: the synthetic card's SHORT INTERVALS over the whole box, each step as high as its target
/// in its zone colour; once started, the rider's column moves right with the step clock, the part done dark, and
/// only the columns it leaves and reaches are sent (not the whole graph each second).
#[test]
fn workout_graph_field() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, Some(KEY));
    let fields = [Field::WorkoutGraph, Field::StepTarget, Field::StepTime, Field::Time, Field::Time, Field::Time];
    app.pages[0] = PageCfg { layout: 9, fields }.into(); // with the step's target and time left
    let rider = |app: &App| match app.drawn.first() {
        Some(Drawn::Workout(w)) => w.rider,
        _ => panic!("the graph's box first"),
    };
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start)); // today's: SHORT INTERVALS
    app.press(&mut sim, Press::Short(Key::Start)); // START: the ride and the workout
    assert_eq!(app.screen(), Screen::Ride(0));
    run(&mut sim, &mut app, 1000);
    let first = rider(&app).expect("the rider's column");
    assert_consistent(&sim, &app);

    run(&mut sim, &mut app, 25_000);
    let now = rider(&app).unwrap();
    assert!(now > first + 5, "the marker moves: column {first}, then {now}");
    assert_consistent(&sim, &app);
    sim.save("workout_graph");

    app.pages[0] = PageCfg { layout: 1, fields }.into(); // the graph alone: what it sends
    app.show(&mut sim);

    // Lap to the open cool-down (its 300 s, the last 73 % of the graph), the step's banner gone: the cost alone
    for _ in 0..4 {
        app.press(&mut sim, Press::Short(Key::Lap));
    }
    run(&mut sim, &mut app, 10_000); // the step's banner, then the one that waited for it
    let (at, px0, s0) = (rider(&app).unwrap(), sim.pixels, sim.ms);
    assert!(at > now + 30, "Lap jumps it: column {now}, then {at}");
    run(&mut sim, &mut app, 30_000);
    let px_s = (sim.pixels - px0) * 1000 / (sim.ms - s0);
    let end = rider(&app).unwrap();
    assert_consistent(&sim, &app);
    std::println!("workout graph: rider column {at} to {end} in 30 s, {px_s} panel px/s");
    // the strip and the rest send about 730 px/s with a still field in this box (measured); the graph adds two
    // columns of its height a move, about 0.6 moves a second (232 columns over the 410 s drawn): about 300 px/s,
    // against 60 000 for the whole graph
    assert!(px_s < 1200, "{px_s} px/s: only the columns that change");
}

/// The power the scripted meter sends ([`scripted_power`]); 0, 3, 6 or 9 W are added from one second to the next so
/// the number changes each second as a real meter's does.
static SCRIPTED_W: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);
fn scripted_power(ms: u64) -> [u8; 8] {
    let w = SCRIPTED_W.load(core::sync::atomic::Ordering::Relaxed) + (ms / 1000 % 4) as u16 * 3;
    let [lo, hi] = w.to_le_bytes();
    [0x10, 1, 0xff, 85, 0, 0, lo, hi]
}

/// The workout gauge field (WKT POWER) through the synthetic card's SHORT INTERVALS: the 3 s power big, the step's
/// band under a bar with a marker at the power, coloured by how far it is (red over the warm-up's 100-120 W, blue
/// under the 200-220 W interval, then green in it); the menu's window in its title; only the number's characters and
/// the marker's columns sent each second, and what is sent equals the gauge drawn whole.
#[test]
fn workout_gauge_field() {
    use core::sync::atomic::Ordering::Relaxed;
    let z = crate::ride::ZONE_COLORS;
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings(); // the defaults, not stock's
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: scripted_power, network: 0, ack: &[] });
    SCRIPTED_W.store(210, Relaxed);
    let mut app = App::start(&mut sim, Some(KEY));
    let fields = [Field::WktPower, Field::StepTime, Field::NextStep, Field::Time, Field::Time, Field::Time];
    app.pages[0] = PageCfg { layout: 3, fields }.into(); // the gauge over the step's time left and the next step
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start)); // today's: SHORT INTERVALS
    app.press(&mut sim, Press::Short(Key::Start)); // START: the ride and the workout
    let shown = |app: &App| match app.drawn.first() {
        Some(Drawn::Gauge(g)) => (std::string::String::from(g.value.as_str()), g.color),
        _ => panic!("the gauge's box first"),
    };
    run(&mut sim, &mut app, 6000);
    assert_eq!(shown(&app).1, z[5], "about 214 W over the warm-up's 100-120 W: red");
    assert_consistent(&sim, &app);
    sim.save("workout_gauge_over");

    let mut ms = 0;
    while app.player.and_then(|p| p.step(app.plan.steps())) == Some(0) {
        run(&mut sim, &mut app, 100);
        ms += 100;
        assert!(ms < 30_000, "the warm-up ends");
    }
    SCRIPTED_W.store(150, Relaxed);
    run(&mut sim, &mut app, 5000);
    assert_eq!(shown(&app).1, z[1], "about 154 W under 200-220 W: blue");
    assert_consistent(&sim, &app);
    sim.save("workout_gauge_under");

    SCRIPTED_W.store(206, Relaxed);
    run(&mut sim, &mut app, 5000);
    let (w, color) = shown(&app);
    assert_eq!(color, z[2], "{w} W in 200-220 W: green");
    assert_consistent(&sim, &app);
    sim.save("workout_gauge");

    // what a second costs: the strip, the step's time left, the number's changed characters, the marker
    let (px0, s0) = (sim.pixels, sim.ms);
    run(&mut sim, &mut app, 10_000);
    let px_s = (sim.pixels - px0) * 1000 / (sim.ms - s0);
    assert_consistent(&sim, &app);
    std::println!("workout gauge: {px_s} panel px/s, the number and the marker moving each second");
    // measured 6468 px/s: a digit of the number at scale 7 is 42 x 70 px and one or two change a second, the marker
    // two 3-column strips of 18 px, the strip and the time left the rest; the gauge's box whole would be 35 520 px
    assert!(px_s < 9000, "{px_s} px/s: only what changes");

    // the menu's window: 30 s, in the title
    app.settings.wkt_avg = 2;
    run(&mut sim, &mut app, 1000);
    let want = app.power.rolling(2).map(u32::from).unwrap_or_default();
    assert_eq!(app.field_text(Field::WktPower).as_str(), std::format!("{want}"));
    assert_consistent(&sim, &app);
}

/// Holding Page opens the menu; Page / Lap move, Start on NEARBY BLE opens the list and scans: two devices in
/// three reports, strongest first, "2 NEARBY..." until the timeout. Back goes out level by level.
#[test]
fn menu_lists_the_devices_nearby() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 0));
    assert_consistent(&sim, &app);
    sim.save("main_menu");
    app.press(&mut sim, Press::Short(Key::Lap));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 0), "Lap on the first row stays there: no wrap");
    let last = menu::MAIN.len() - 1;
    for _ in 0..last + 3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, last), "Page stops at the last row");
    for _ in 0..last {
        app.press(&mut sim, Press::Short(Key::Lap));
    }
    to_row(&mut sim, &mut app, menu::Row::Nearby);
    let nearby = row_of(menu::Row::Nearby);
    assert_eq!(app.screen(), Screen::Menu(Menu::System, nearby), "BLE NEARBY, in SYSTEM");
    assert_consistent(&sim, &app);
    sim.save("menu");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Nearby);
    assert_eq!(sim.nordic.got.last().unwrap()[..2], [0, 0x8a], "the scan starts with the list");
    run(&mut sim, &mut app, 1500);
    let names: Vec<_> = app.nearby.strongest().map(|d| (d.rssi, std::string::String::from(d.name()))).collect();
    assert_eq!(names, [(-42, "Bike".into()), (-70, "".into())], "last RSSI kept, the name from the first report");
    assert!(app.scanning);
    assert_consistent(&sim, &app);
    sim.save("nearby");
    run(&mut sim, &mut app, u64::from(SCAN_S) * 1000);
    assert!(!app.scanning);
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::System, nearby));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::Settings, row_of(menu::Row::Open(Menu::System))));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, row_of(menu::Row::Open(Menu::Settings))));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Main);
    assert_consistent(&sim, &app);
}

/// Back returns to where the menu was opened, row by row: a submenu to its row, the menu to the ride page.
#[test]
fn back_returns_where_it_came_from() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(1));
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Gps));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Gps, 0));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::System, row_of(menu::Row::Open(Menu::Gps))), "the submenu's row");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::Settings, row_of(menu::Row::Open(Menu::System))));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, row_of(menu::Row::Open(Menu::Settings))));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Ride(1), "the page the menu was opened from");
    assert_consistent(&sim, &app);
    // switching that page off from the menu: Back shows the main screen instead
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Page(1)));
    app.press(&mut sim, Press::Short(Key::Start)); // PAGE 2
    delete_page_by_keys(&mut sim, &mut app); // pages 2 and 3 deleted from the editor
    delete_page_by_keys(&mut sim, &mut app);
    assert_eq!(app.page_count(), 1);
    for _ in 0..3 {
        back(&mut sim, &mut app); // PAGES, the menu, out
    }
    assert_eq!(app.screen(), Screen::Main);
    assert_consistent(&sim, &app);
}

/// On a page's menu (its first row, EDIT ON PAGE): the editor, its actions, DELETE PAGE, sure; back on the menu.
fn delete_page_by_keys(sim: &mut Sim, app: &mut App) {
    let Screen::Menu(Menu::Page(_), 0) = app.screen() else { panic!("{:?}", app.screen()) };
    app.press(sim, Press::Short(Key::Start)); // EDIT ON PAGE
    app.press(sim, Press::Short(Key::Start)); // the actions
    for _ in 0..menu::EDIT_ROWS.len() - 1 {
        app.press(sim, Press::Short(Key::Page));
    }
    app.press(sim, Press::Short(Key::Start)); // DELETE PAGE
    app.press(sim, Press::Short(Key::Start)); // sure
    assert!(matches!(app.screen(), Screen::Menu(Menu::Page(_), _)), "{:?}", app.screen());
}

/// A key held in a menu moves the selection on each repeat, and its release does not move it once more.
#[test]
fn held_key_scrolls_a_menu() {
    let mut sim = Sim::new(9, 0, 0).keys(|ms| if (5000..5749).contains(&ms) { Key::Page as u8 } else { 0 });
    let mut app = App::start(&mut sim, None);
    assert!(sim.ms < 5000);
    app.press(&mut sim, Press::Long(Key::Page));
    let left = 7000 - sim.ms;
    run(&mut sim, &mut app, left);
    // held 750 ms: repeats at 500, 600, 700 ms, then the release
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 3));
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 4), "the next press acts");
}

/// Sensors menu: Start on a sensor searches; the first one heard is paired (kept in its backup register) and
/// its value reaches the ride page. With a key, ANT is started and every channel assigned, none opened yet.
#[test]
fn pairs_sensors_from_the_menu() {
    let mut sim = Sim::new(9, 0, 0);
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72, network: 0, ack: &[] });
    sim.nordic.sensors.push(AntSensor { device_type: 0x79, id: 555, data: spd_cad, network: 0, ack: &[] });
    sim.classic_pages();
    let mut app = App::start(&mut sim, Some(KEY));
    let ops: Vec<_> = sim.nordic.got.iter().filter(|p| p[0] == 6).map(|p| p[1]).collect();
    assert_eq!(ops, [0xfa, 0xcc, 0xc2, 0xc2, 0xc2, 0xc2, 0xc2, 0xc2], "enable, key, assign the 6 ANT+ channels");
    assert!(sim.nordic.got.contains(&[&[6, 0xcc, 0][..], &KEY].concat()), "the key on network 0");
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Sensors));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Sensors, 0));
    app.press(&mut sim, Press::Short(Key::Start)); // heart rate: search
    assert!(sim.nordic.got.contains(&vec![6, 0xd3, 0, 0, 0, 0x78, 0]), "any strap");
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start)); // speed + cadence: search
    run(&mut sim, &mut app, 3000);
    assert_eq!(sim.backup[Kind::Hr.backup()], STRAP.to_word(), "strap paired and kept");
    assert_eq!(sim.backup[Kind::SpeedCadence.backup()], SensorId { device: 555, trans: 0xf1 }.to_word());
    assert_consistent(&sim, &app);
    sim.save("sensors");
    back(&mut sim, &mut app);
    back(&mut sim, &mut app);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 200);
    assert!(ride(&mut sim, &app, 0, &["22.7", "72", "60"], true) == sim.screen, "22.7 km/h, HR 72, 60 rpm");
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 200);
    assert!(ride(&mut sim, &app, 1, &["22.7", "72", "60", "--"], true) == sim.screen);
    sim.save("ride");
}

/// After a restart the paired sensor's channel opens for it only: a friend's strap is not taken. Start on it
/// in the sensors menu, while it is silent, searches for it again; Start once more forgets it (channel closed,
/// register cleared).
#[test]
fn keeps_the_paired_sensor_until_forgotten() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: 999, data: hr80, network: 0, ack: &[] }); // a friend's strap
    sim.classic_pages();
    let mut app = App::start(&mut sim, Some(KEY));
    assert!(sim.nordic.got.contains(&vec![6, 0xd3, 0, 0x26, 0x62, 0x78, 0xf1]), "the paired strap only");
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Page));
    assert!(ride(&mut sim, &app, 0, &["--", "--", "--"], true) == sim.screen, "the friend's strap is not shown");
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Sensors));
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start)); // silent: searched for again, not forgotten
    let closes = |sim: &Sim| sim.nordic.got.iter().filter(|p| p[..2] == [6, 0xc5]).count();
    assert_eq!(closes(&sim), 1, "closed and opened for it again");
    assert_eq!(sim.backup[Kind::Hr.backup()], STRAP.to_word(), "still paired");
    assert_eq!(app.banner.as_str(), "SEARCHING AGAIN\nSTART: FORGET");
    assert_eq!(app.menu_row(Menu::Sensors, 0).unwrap().1.as_str(), "SURE?");
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
    sim.save("sensor_search_again");
    app.press(&mut sim, Press::Short(Key::Start)); // again: forgotten
    assert_eq!(closes(&sim), 2, "closed");
    assert_eq!(sim.backup[Kind::Hr.backup()], 0, "forgotten");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    assert_eq!(sim.backup[Kind::Hr.backup()], SensorId { device: 999, trans: 0xf1 }.to_word(), "paired anew");
}

/// A paired strap that is not sending: its channel searches 30 s, closes, rests a minute, searches again; the
/// strap is found when it comes back.
#[test]
fn a_silent_sensor_searches_then_rests() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    let mut app = App::start(&mut sim, Some(KEY));
    let opens = |sim: &Sim| sim.nordic.got.iter().filter(|p| p.starts_with(&[6, 0xc4, 0])).count();
    assert_eq!(opens(&sim), 1);
    assert!(sim.nordic.got.contains(&vec![6, 0xda, 0, crate::nrf::SEARCH_TIMEOUT]));
    run(&mut sim, &mut app, 30_000 + 1000);
    assert!(!sim.nordic.channels[0].open, "closed by its search time-out");
    run(&mut sim, &mut app, u64::from(SEARCH_REST_MS) - 2000);
    assert_eq!(opens(&sim), 1, "resting");
    run(&mut sim, &mut app, 2000);
    assert_eq!(opens(&sim), 2, "searching again");
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72, network: 0, ack: &[] });
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.live.get(Metric::Hr), Some(72), "found");
    run(&mut sim, &mut app, 60_000);
    assert!(sim.nordic.channels[0].open, "a sensor that sends keeps its channel");
}

/// The page menu: Start cycles the layout and the fields; kept in a backup register, read back at the next
/// start. A power meter's value shows in the new field.
#[test]
fn pages_are_configured_from_the_menu() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power300, network: 0, ack: &[] });
    sim.classic_pages();
    let mut app = App::start(&mut sim, Some(KEY));
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Page(0)));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Page(0), 0));
    app.press(&mut sim, Press::Short(Key::Page)); // LAYOUT
    while app.pages()[0].preset_id() != Some(4) {
        app.press(&mut sim, Press::Short(Key::Start)); // layouts round to 2 x 2
    }
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start)); // field 1 (speed): its group's row selected
    assert_eq!(app.screen(), Screen::Menu(Menu::Fields(0, 0), Field::Speed.group()));
    assert_consistent(&sim, &app);
    sim.save("field_groups");
    app.press(&mut sim, Press::Short(Key::Page)); // POWER
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Group(0, 0, 1), 0));
    assert_consistent(&sim, &app);
    sim.save("field_picker");
    app.press(&mut sim, Press::Short(Key::Start)); // WATTS: back to the page's menu, on the field's row
    assert_eq!(app.screen(), Screen::Menu(Menu::Page(0), 2));
    assert_consistent(&sim, &app);
    sim.save("page_menu");
    app.step(&mut sim); // the registers written at the next pass (after the file: no volume here)
    let want =
        PageCfg { layout: 4, fields: [Field::Power, Field::Hr, Field::Cadence, Field::Time, Field::Time, Field::Time] };
    assert_eq!(app.pages()[0], want.into());
    assert_eq!(PageCfg::from_words([sim.backup[20], sim.backup[21]]), Some(want));
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Back));
    }
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1000);
    let t = hm(&mut sim);
    assert_consistent(&sim, &app);
    assert!(ride(&mut sim, &app, 0, &["300", "--", "85", &t], true) == sim.screen);
    let app = App::start(&mut sim, Some(KEY));
    assert_eq!(app.pages()[0], want.into(), "read back after a restart");
}

/// When the simulated strap starts beating, ms.
const BEAT_START: u64 = 10_000;
/// The strap's interval before beat `k`, 1/1024 s: 820 plus or minus 16 (about 800 ms), scrambled (murmur3's
/// finaliser).
fn beat_ticks(k: u32) -> u64 {
    let mut x = k.wrapping_mul(0x9e37_79b9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 13;
    804 + u64::from(x >> 27)
}
/// A strap beating from [`BEAT_START`]: page 4, the last beat's time, the one before, the count.
fn strap_beats(ms: u64) -> [u8; 8] {
    let now = ms * 1024 / 1000;
    let (mut t, mut prev, mut k) = (BEAT_START * 1024 / 1000, 0, 0u32);
    while t + beat_ticks(k + 1) <= now {
        (prev, k) = (t, k + 1);
        t += beat_ticks(k);
    }
    let ([p0, p1], [t0, t1]) = ((prev as u16).to_le_bytes(), (t as u16).to_le_bytes());
    [4, 0, p0, p1, t0, t1, k as u8, 70]
}

/// A strap sends a known series of beats: the HRV fields show "--" until there are enough, then the RMSSD and
/// DFA alpha1 of the series, and "--" again once the strap has no heart rate.
#[test]
fn hrv_fields_from_the_strap() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: strap_beats, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 4, fields: [Dfa, Rmssd, Hr, Time, Time, Time] }.into();
    app.press(&mut sim, Press::Short(Key::Page));
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    run(&mut sim, &mut app, 20_000);
    assert_eq!((text(&app, Dfa), text(&app, Rmssd)), ("--".into(), "--".into()), "about 12 beats");
    run(&mut sim, &mut app, 20_000);
    assert!(text(&app, Dfa) == "--" && text(&app, Rmssd) != "--", "about 37 beats: RMSSD, not yet DFA");
    run(&mut sim, &mut app, 140_000);
    // stop where the app has the last beat or the one before: the same 5-beat update either way
    while strap_beats(sim.ms)[6] % hrv::EVERY != 2 {
        run(&mut sim, &mut app, 100);
    }
    let mut want = hrv::Hrv::default();
    for k in 1..=u32::from(strap_beats(sim.ms)[6]) {
        want.push(((beat_ticks(k) * 1000 + 512) >> 10) as u16);
    }
    let (a, ms) = (want.alpha1().unwrap(), want.rmssd().unwrap());
    assert!((0.3..0.7).contains(&a), "scrambled beats are close to white noise: {a}");
    let (a, ms) = (std::format!("{a:.2}"), std::format!("{ms}"));
    assert_eq!((text(&app, Dfa), text(&app, Rmssd)), (a.clone(), ms.clone()));
    let t = hm(&mut sim);
    assert!(ride(&mut sim, &app, 0, &[&a, &ms, "70", &t], true) == sim.screen);
    assert_consistent(&sim, &app);
    sim.save("hrv_fields");
    sim.nordic.sensors[0].data = |_| [4, 0, 0, 0, 0, 0, 0, 0]; // off the chest: no rate, no new beat
    run(&mut sim, &mut app, 7000);
    assert_eq!((text(&app, Dfa), text(&app, Rmssd)), ("--".into(), "--".into()), "no heart rate");
}

/// Start on the overview starts the ride and shows the first ride page; each key's effect on the ride is said in
/// the banner.
#[test]
fn start_from_the_overview() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Ride(0));
    assert_eq!(app.ride.state(), ride::State::Running);
    run(&mut sim, &mut app, 200);
    assert_eq!(app.banner.as_str(), "RIDE STARTED");
    assert_consistent(&sim, &app);
    sim.save("ride_started");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.banner.as_str(), "PAUSED: HOLD START TO END");
    save_ride(&mut sim, &mut app);
    assert_eq!((app.screen(), app.ride.state()), (Screen::Summary, ride::State::Idle), "the ride's card");
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
    back(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Main, "Back: the overview");
}

/// The card at the end of a ride: its track, its elevation profile, its numbers.
#[test]
fn ride_card_at_the_end() {
    fn hills(ms: u64) -> i32 {
        let t = (ms / 1000) as i32 % 1200;
        200_000 + if t < 600 { t * 200 } else { (1200 - t) * 200 } // up 120 m and down, in mm
    }
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    sim.gnss.alt = Some(hills);
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_noisy, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 2000);
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 40 * 60_000);
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Summary);
    let card = app.ride_card();
    assert_eq!((card.stats[0].0, card.stats[0].1.as_str()), ("TIME", "0:40"));
    assert_eq!(card.title.as_str(), "23 OCT 09:00");
    assert_consistent(&sim, &app);
    sim.save("ride_card");
}

/// Pages kept by the builds of 2026-10-23 (from register 19, which stock reads at boot) move to 20..27, and 19 is
/// given back (stock's first-boot value, 0).
#[test]
fn pages_leave_stocks_register() {
    let mut sim = Sim::new(9, 0, 0);
    let p = PageCfg { layout: 5, fields: [Field::Map, Field::Hr, Field::Power, Field::Np, Field::Wbal, Field::Dfa] };
    let w = p.to_words().map(|w| w & 0x0fff_ffff | 0xb << 28); // as those builds wrote it
    (sim.backup[19], sim.backup[20]) = (w[0], w[1]);
    let app = App::start(&mut sim, None);
    assert_eq!(app.pages()[0], p.into());
    assert_eq!(sim.backup[19], 0, "given back to stock");
    assert_eq!(PageCfg::from_words([sim.backup[20], sim.backup[21]]), Some(p));
    assert_eq!(App::start(&mut sim, None).pages()[0], p.into(), "read from the new place after a restart");
}

/// Every layout draws on the unit as in the screenshot (layout_<id>.bmp, to look at).
#[test]
fn every_layout_draws() {
    use Field::*;
    let fields = [Map, Speed, Hr, Power, Cadence, Timer];
    for id in 1..ui::LAYOUTS.len() as u8 {
        let mut sim = Sim::new(9, 0, 0);
        let words = PageCfg { layout: id, fields }.to_words();
        (sim.backup[20], sim.backup[21]) = (words[0], words[1]);
        let mut app = App::start(&mut sim, None);
        app.press(&mut sim, Press::Short(Key::Page));
        run(&mut sim, &mut app, 1000);
        assert_eq!(app.screen(), Screen::Ride(0));
        assert_consistent(&sim, &app);
        sim.save(&std::format!("layout_{id}"));
    }
}

/// The battery level in the strip (green while charging); an empty battery switches the unit off, but not
/// while charging.
#[test]
fn battery_in_the_strip_and_off_when_empty() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.step(&mut sim);
    assert_eq!(app.strip.battery, Some(63), "3.9 V");
    sim.charger = true;
    sim.battery_mv = Some(3100);
    run(&mut sim, &mut app, 3 * u64::from(BATTERY_MS) + 1000);
    assert!(!sim.standby && app.strip.charging, "charging: stays on");
    sim.charger = false;
    run(&mut sim, &mut app, 2 * u64::from(BATTERY_MS));
    assert!(!sim.standby, "two low readings: not yet");
    run(&mut sim, &mut app, u64::from(BATTERY_MS));
    assert!(sim.standby && !sim.display_on, "empty: switched off");
}

/// Away from the overview, a notice is a banner for a while: the page above it keeps updating; when it goes,
/// what it covered is drawn again. A key dismisses it and acts (Start on a ride must start it).
#[test]
fn notice_banner_over_a_ride_page() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 3000);
    app.notice("STRAP PAIRED");
    run(&mut sim, &mut app, 1500);
    assert!(app.banner_ms.is_some());
    assert_consistent(&sim, &app);
    sim.save("banner");
    run(&mut sim, &mut app, u64::from(BANNER_MS));
    assert!(app.banner_ms.is_none(), "gone by itself");
    assert_consistent(&sim, &app);
    app.notice("TEST");
    run(&mut sim, &mut app, 100);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(1), "the key dismissed it and acted");
    assert!(app.banner_ms.is_none());
    assert_consistent(&sim, &app);
}

/// The overview: notices in place of Ferris, every row's text the same size (the time small in front, a long text
/// cut); its fields are chosen in the menu (OVERVIEW: four rows, no field count) and kept. The first GPS fix is no
/// notice (the strip says it).
#[test]
fn overview_notices_and_fields() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 3000);
    sim.gnss.fix = Some(7000);
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.notices[0].as_str(), "", "no notice for the fix");
    app.notice("RIDE SAVED");
    app.notice("A Long Caller Name Here");
    run(&mut sim, &mut app, 100);
    assert_consistent(&sim, &app);
    sim.save("overview_notice");
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Page(OVERVIEW)));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.rows(Menu::Page(OVERVIEW)), 4);
    app.press(&mut sim, Press::Short(Key::Start)); // WEEK KM: TOTALS
    app.press(&mut sim, Press::Short(Key::Start)); // on WEEK KM
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start)); // WEEK H
    assert_consistent(&sim, &app);
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Back));
    }
    assert_eq!(app.screen(), Screen::Main);
    assert_consistent(&sim, &app);
    assert_eq!(App::start(&mut sim, None).pages()[OVERVIEW].fields[0], Field::WeekH, "kept");
}

/// The receiver is powered at start and gets its configuration (RAM layer) once; with a fix, "GPS" turns green
/// and the speed comes from it when no speed sensor is paired; it is powered off with the unit.
#[test]
fn gps_fix_and_speed() {
    let mut sim = Sim::new(9, 0, 0);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    assert!(!sim.gnss.powered, "powered later, after its line has been low long enough");
    run(&mut sim, &mut app, 3000);
    assert!(sim.gnss.powered);
    let sent = |sim: &Sim, f: [u8; 32]| sim.gnss.got.windows(32).any(|w| w == f);
    assert!(sent(&sim, crate::gnss::config()), "configuration");
    let frames = sim.gnss.got.iter().filter(|&&b| b == 0xb5).count();
    assert_eq!(frames, 2, "the RTC time (14 h accuracy, no offset learnt yet), no position yet");
    assert_eq!(app.strip.gps, Some(0), "searching");
    sim.gnss.fix = Some(7000); // 25.2 km/h
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.strip.gps, Some(3), "9 satellites: 3 bars");
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 100);
    let (h, m, _) = rtc_time_from_tr(sim.rtc().0);
    let mut want = Frame::new();
    want.px = ride(&mut sim, &app, 0, &["25.2", "--", "--"], true);
    let strip = Strip {
        time: (h, m),
        radio: true,
        gps: Some(3),
        blink: true,
        usb: UsbDot::Off,
        battery: Some(63),
        charging: false,
        rec: Rec::Idle,
        locked: false,
    };
    crate::ui::draw_strip(&mut want, &strip);
    assert!(want.px == sim.screen, "GPS speed, green GPS");
    sim.save("gps");
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Gps));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Gps, 0));
    run(&mut sim, &mut app, 1100);
    assert_consistent(&sim, &app);
    sim.save("gps_menu");
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Back));
    }
    // the fix taught the RTC's offset (+2 h) and the position (to 1e-4 degrees): kept for the next power-up
    let aid = crate::gnss::Aid::from_words([sim.backup[17], sim.backup[18]]);
    assert_eq!(aid, crate::gnss::Aid { offset_15min: Some(8), pos: Some((5_123_000, 4_234_000)) });
    sim.gnss.powered = false; // a power cycle: the board drives the line low at boot
    sim.gnss.off_since = sim.ms;
    let mut again = App::start(&mut sim, None);
    sim.gnss.got.clear();
    run(&mut sim, &mut again, 2500);
    let (tr, dr) = sim.rtc();
    let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), crate::rtc_date_from_dr(dr));
    let utc = crate::from_epoch_s(crate::epoch_s((y, mo, d, h, m, s)) - 2 * 3600);
    let sent_time = (0..3).any(|back| {
        sim.gnss.got.windows(32).any(|w| w == crate::gnss::mga_time(crate::from_epoch_s(crate::epoch_s(utc) - back), 2))
    });
    assert!(sent_time, "UTC from the RTC at the next power-up");
    assert!(sent(&sim, crate::gnss::mga_pos(5_123_000, 4_234_000, 10_000)), "and the last position");
    sim.gnss.powered = false; // silent receiver: the fix ages out
    run(&mut sim, &mut app, u64::from(PVT_STALE_MS) + 200);
    assert_eq!(app.strip.gps, None);
    app.press(&mut sim, Press::Long(Key::Back));
    assert!(!sim.gnss.powered, "off with the unit");
}

/// A key stuck down since the start does not keep the light on (2026-10-25 review: the time-out never came).
#[test]
fn a_stuck_key_lets_the_light_time_out() {
    let mut sim = Sim::new(9, 0, 0).keys(|_| Key::Lap as u8);
    let mut app = App::start(&mut sim, None);
    app.settings.light_timeout = 1; // 10 s
    run(&mut sim, &mut app, 11_000);
    assert_eq!(sim.light_level, 0, "timed out");
}

/// The backlight: level and time-out from the menu (kept with the wheel size); after the time-out any key
/// brings it back (and acts), Back only brings it back.
#[test]
fn backlight_level_and_timeout() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    assert_eq!(sim.light_level, 100, "100 % and never off by default");
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::Light));
    app.press(&mut sim, Press::Short(Key::Start)); // LIGHT %: 60
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start)); // LIGHT OFF: 10 s
    assert_eq!(sim.light_level, 60);
    assert_consistent(&sim, &app);
    sim.save("light_menu");
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Back)); // SETTINGS, MENU, out
    }
    run(&mut sim, &mut app, 11_000);
    assert_eq!(sim.light_level, 0, "timed out");
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!((sim.light_level, app.screen()), (60, Screen::Ride(0)), "a key brings it back and acts");
    app.press(&mut sim, Press::Short(Key::Back)); // a ride page's Back: the overview
    run(&mut sim, &mut app, 11_000);
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(sim.light_level, 60, "Back after the time-out: on");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!((sim.light_level, app.screen()), (60, Screen::Main), "then Back on the overview: nothing");
    let _again = App::start(&mut sim, None);
    assert_eq!(sim.light_level, 60, "kept: {:x}", sim.backup[11]);
}

/// The text of row `i` of SYSTEM > BUILT IN: label and value.
fn built_in_row(app: &App, i: usize) -> std::string::String {
    let (label, value, _) = app.menu_row(Menu::BuiltIn, i).unwrap();
    format!("{} {}", label.as_str(), value.as_str())
}

/// The way from menu `from` to `row`, by the rows the app shows (the first found, at most 5 menus down; the main
/// menu walked into only from the ride menu, as ALL SETTINGS): each menu on the way and the row chosen in it.
pub(crate) fn path_to(app: &App, from: Menu, row: menu::Row) -> Option<Vec<(Menu, usize)>> {
    fn walk(app: &App, from: Menu, row: menu::Row, depth: usize) -> Option<Vec<(Menu, usize)>> {
        for i in 0..app.rows(from) {
            let r = app.row(from, i)?;
            if r == row {
                return Some(std::vec![(from, i)]);
            }
            if let menu::Row::Open(m) = r
                && (m != Menu::Main || matches!(from, Menu::Ride(_)))
                && depth < 5
                && let Some(mut p) = walk(app, m, row, depth + 1)
            {
                p.insert(0, (from, i));
                return Some(p);
            }
        }
        None
    }
    walk(app, from, row, 1)
}

/// From the first row of the menu shown, down to `row`, each menu on the way opened by Page and Start.
pub(crate) fn to_row(sim: &mut Sim, app: &mut App, row: menu::Row) {
    let Screen::Menu(from, _) = app.screen() else { panic!("not a menu: {:?}", app.screen()) };
    let path = path_to(app, from, row).unwrap_or_else(|| panic!("{row:?} not in {from:?}"));
    for (k, &(_, i)) in path.iter().enumerate() {
        for _ in 0..i {
            app.press(sim, Press::Short(Key::Page));
        }
        if k + 1 < path.len() {
            app.press(sim, Press::Short(Key::Start));
        }
    }
}

/// Every menu below `from` and how deep (the main menu is 1), through the rows the app shows, the full menu not
/// walked again from the ride menu.
fn menus_under(app: &App, from: Menu, depth: usize, out: &mut Vec<(Menu, usize)>) {
    out.push((from, depth));
    for i in 0..app.rows(from) {
        if let Some(menu::Row::Open(m)) = app.row(from, i)
            && m != Menu::Main
            && out.iter().all(|&(seen, _)| seen != m)
        {
            menus_under(app, m, depth + 1, out);
        }
    }
}

/// The menu tree (UI review 4.3): every setting and every row of the old tree is in it, a setting at most three
/// levels down (MENU > SETTINGS > ME > FTP), the diagnostics under SYSTEM, every row's label and value whole on its
/// row (a number at the end of its range too, and while edited). Each menu, reached by the keys: Lap on the first
/// row and Page on the last stay there (no wrap), Back goes up one level at a time to the screen it was opened
/// from. A screenshot of each (`target/sim/menu_<title>.bmp`).
#[test]
fn menu_tree_reaches_everything() {
    use menu::{PhoneRow, Reading, Row, StockRow};
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.fix = Some(8000);
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 3000);
    assert!(app.pvt.is_some(), "the GPS rows all shown");
    let mut menus = Vec::new();
    menus_under(&app, Menu::Main, 1, &mut menus);
    // every row of every menu, with its level
    let mut rows = Vec::new();
    for &(m, depth) in &menus {
        for i in 0..app.rows(m) {
            rows.push((app.row(m, i).unwrap(), depth, m, i));
        }
    }
    let level = |row: Row| rows.iter().find(|r| r.0 == row).map(|r| r.1);
    let settings = [
        Setting::Wheel,
        Setting::Light,
        Setting::LightOff,
        Setting::Touch,
        Setting::MaxHr,
        Setting::Ftp,
        Setting::Weight,
        Setting::WPrime,
        Setting::Eat,
        Setting::Drink,
        Setting::HrAlert,
        Setting::WktAvg,
    ];
    let mut wanted: Vec<Row> = settings.iter().map(|&s| Row::Set(s)).collect();
    wanted.extend((0..4).map(|n| Row::Set(Setting::Remote(n))));
    wanted.extend((0..Kind::ALL.len()).map(Row::Sensor));
    wanted.extend(menu::PHONE.map(Row::Phone));
    wanted.extend(menu::FITNESS.map(Row::Stock));
    wanted.extend([Row::Stock(StockRow::Import), Row::Drive, Row::Nearby, Row::About]);
    wanted.extend((0..app.page_count()).map(Row::Layout));
    wanted.extend((0..app.page_count()).map(Row::EditPage));
    wanted.push(Row::AddPage);
    wanted.extend((0..4).map(|f| Row::Field(OVERVIEW, f)));
    wanted.extend((0..app.page_count()).map(|n| Row::Field(n, 0)));
    for &row in &wanted {
        let d = level(row).unwrap_or_else(|| panic!("{row:?} not in the tree"));
        assert!(d <= 3, "{row:?}: {d} levels down");
    }
    assert_eq!(level(Row::Phone(PhoneRow::Forget)), Some(3), "SENSORS > PHONE");
    // the readings (diagnostics, nothing to set): under SYSTEM
    for r in menu::BUILT_IN.iter().chain(&menu::GPS) {
        assert_eq!(level(Row::Read(*r)), Some(4), "{r:?}: MENU > SETTINGS > SYSTEM > ...");
    }
    assert_eq!(level(Row::Read(Reading::Lat)), Some(4));
    for m in [Menu::Workouts, Menu::Courses, Menu::History, Menu::Stock] {
        assert!(menus.iter().any(|&(x, _)| x == m), "{m:?} reached");
    }
    assert_eq!(menu::MAIN.len(), ui::MENU_ROWS, "the main menu: one screen");
    // whole on the row: label and value side by side, numbers at the end of their range, and edited
    let whole = |label: &str, value: &str| {
        assert!(ui::LABEL.width(label) <= ui::menu_label_w(label, value), "{label} {value}: the label cut");
    };
    for &(row, _, m, i) in &rows {
        let (label, value, _) = app.menu_row(m, i).unwrap();
        whole(label.as_str(), value.as_str());
        if let Row::Set(s) = row
            && s.number().is_some()
        {
            for _ in 0..200 {
                app.settings.step(s, true);
            }
            app.screen = Screen::Menu(m, i);
            for editing in [false, true] {
                app.editing = editing;
                let (label, value, _) = app.menu_row(m, i).unwrap();
                whole(label.as_str(), value.as_str());
            }
            (app.editing, app.screen) = (false, Screen::Main);
        }
    }

    // by the keys, from the overview: each menu, its ends, Back level by level; a screenshot
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.fix = Some(8000);
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 3000);
    for &(m, depth) in &menus {
        app.press(&mut sim, Press::Long(Key::Page));
        if m != Menu::Main {
            to_row(&mut sim, &mut app, Row::Open(m));
            app.press(&mut sim, Press::Short(Key::Start));
        }
        if app.screen() != Screen::Menu(m, 0) {
            // COURSES without a card: said, not opened
            assert_eq!((m, app.banner.as_str()), (Menu::Courses, "NO COURSE"));
            while app.screen() != Screen::Main {
                app.press(&mut sim, Press::Short(Key::Back));
            }
            continue;
        }
        run(&mut sim, &mut app, u64::from(LIST_REDRAW_MS) + 100); // a list that changes: redrawn once a second
        assert_consistent(&sim, &app);
        let mut title = Line::<16>::new();
        title.push_str(match m {
            Menu::Page(OVERVIEW) => "OVERVIEW",
            Menu::Main => "MAIN",
            _ => menu_title(m),
        });
        if let Menu::Page(n @ 0..crate::PAGES) = m {
            title.push_u32(n as u32 + 1);
        }
        let name: std::string::String = title
            .as_str()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
            .collect();
        sim.save(&std::format!("menu_{}", name.trim_end_matches('_')));
        app.press(&mut sim, Press::Short(Key::Lap));
        assert_eq!(app.screen(), Screen::Menu(m, 0), "{m:?}: no wrap at the top");
        let last = app.rows(m).max(1) - 1;
        for _ in 0..=last {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        assert_eq!(app.screen(), Screen::Menu(m, last), "{m:?}: no wrap at the bottom");
        assert_consistent(&sim, &app);
        for up in 1..depth {
            app.press(&mut sim, Press::Short(Key::Back));
            assert!(matches!(app.screen(), Screen::Menu(..)), "{m:?}: Back {up} of {depth}");
        }
        assert!(matches!(app.screen(), Screen::Menu(Menu::Main, _)), "{m:?}: up to the main menu");
        app.press(&mut sim, Press::Short(Key::Back));
        assert_eq!(app.screen(), Screen::Main, "{m:?}: {depth} Backs to the overview");
    }
}

/// The labels of the menu shown, one per row.
fn labels(app: &App) -> Vec<std::string::String> {
    let Screen::Menu(m, _) = app.screen() else { panic!("not a menu: {:?}", app.screen()) };
    (0..app.rows(m)).map(|i| app.menu_row(m, i).unwrap().0.as_str().into()).collect()
}

/// The ride menu (UI review step 8): Page held while a ride records or is paused opens RIDE instead of the full
/// menu, with only the rows that apply: WORKOUT while a step runs, COURSE while one is followed, EDIT THIS PAGE but
/// on the course page. Each row acts: PAUSE stays (RESUME then, and END RIDE under it), RESUME goes back to the page,
/// BRIGHTNESS changes in place, NEXT STEP, STOP WORKOUT and STOP FOLLOWING act and go back to riding, END RIDE
/// pauses and opens the end-ride screen (RESUME back to the page, SAVE the ride's card). SENSORS, EDIT THIS PAGE and
/// ALL SETTINGS open their menus; Back goes up one level at a time; no wrap. Screenshots: ride_menu,
/// ride_menu_paused, ride_menu_workout.
#[test]
fn ride_menu() {
    use menu::{RideRow, Row};
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.set_course_memory(crate::storage::course::tests::memory());
    let hold_page = |sim: &mut Sim, app: &mut App| app.press(sim, Press::Long(Key::Page));
    let back = |sim: &mut Sim, app: &mut App, n| (0..n).for_each(|_| app.press(sim, Press::Short(Key::Back)));
    hold_page(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 0), "no ride: the full menu");
    back(&mut sim, &mut app, 1);
    app.press(&mut sim, Press::Short(Key::Start)); // the ride
    assert_eq!(app.screen(), Screen::Ride(0));
    hold_page(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Menu(Menu::Ride(0), 0));
    let base = ["PAUSE", "END RIDE", "BRIGHTNESS", "SENSORS", "ALL SETTINGS"];
    assert_eq!(labels(&app), base, "no workout, no course: their rows not shown; no page edited while recording");
    run(&mut sim, &mut app, 1100);
    assert_consistent(&sim, &app);
    sim.save("ride_menu");
    app.press(&mut sim, Press::Short(Key::Lap));
    assert_eq!(app.screen(), Screen::Menu(Menu::Ride(0), 0), "no wrap");
    // BRIGHTNESS: in place
    to_row(&mut sim, &mut app, Row::Set(Setting::Light));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((sim.light_level, app.screen()), (60, Screen::Menu(Menu::Ride(0), 2)));
    // SENSORS, ALL SETTINGS: their menus, Back to the ride menu's row, then to the page
    for (row, m, at) in [(Row::Open(Menu::Sensors), Menu::Sensors, 3), (Row::Open(Menu::Main), Menu::Main, 4)] {
        back(&mut sim, &mut app, 1);
        hold_page(&mut sim, &mut app);
        to_row(&mut sim, &mut app, row);
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(app.screen(), Screen::Menu(m, 0), "{row:?}");
        back(&mut sim, &mut app, 1);
        assert_eq!(app.screen(), Screen::Menu(Menu::Ride(0), at));
    }
    back(&mut sim, &mut app, 1);
    assert_eq!(app.screen(), Screen::Ride(0), "Back: the page");
    assert_eq!(app.ride.state(), ride::State::Running, "nothing changed the ride");

    // PAUSE stays, the row says RESUME; RESUME goes back to the page
    hold_page(&mut sim, &mut app);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.ride.state(), app.screen()), (ride::State::Paused, Screen::Menu(Menu::Ride(0), 0)));
    assert_eq!(labels(&app)[0], "RESUME");
    run(&mut sim, &mut app, 1100);
    assert_consistent(&sim, &app);
    sim.save("ride_menu_paused");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.ride.state(), app.screen()), (ride::State::Running, Screen::Ride(0)));

    // a workout from ALL SETTINGS > WORKOUTS: WORKOUT > NEXT STEP, STOP WORKOUT
    hold_page(&mut sim, &mut app);
    to_row(&mut sim, &mut app, Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start)); // the first one's steps
    app.press(&mut sim, Press::Short(Key::Start)); // START
    assert_eq!(app.screen(), Screen::Ride(0));
    let step = |app: &App| app.player.as_ref().and_then(|p| p.step(app.plan.steps()));
    assert_eq!(step(&app), Some(0));
    hold_page(&mut sim, &mut app);
    assert_eq!(labels(&app)[2], "WORKOUT");
    run(&mut sim, &mut app, 1100);
    assert_consistent(&sim, &app);
    sim.save("ride_menu_workout");
    to_row(&mut sim, &mut app, Row::Ride(RideRow::NextStep));
    assert_eq!(app.screen(), Screen::Menu(Menu::WorkoutNow, 0));
    back(&mut sim, &mut app, 1);
    assert_eq!(app.screen(), Screen::Menu(Menu::Ride(0), 2), "Back: the ride menu, on WORKOUT");
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start)); // NEXT STEP
    assert_eq!((step(&app), app.screen()), (Some(1), Screen::Ride(0)));
    assert!(app.banner.as_str().starts_with("2/"), "the step said, no lap: {:?}", app.banner.as_str());
    hold_page(&mut sim, &mut app);
    to_row(&mut sim, &mut app, Row::Ride(RideRow::StopWorkout));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.player.is_none(), app.screen(), app.banner.as_str()), (true, Screen::Ride(0), "WORKOUT STOPPED"));
    assert_eq!(app.ride.state(), ride::State::Running, "the ride goes on");

    // a course from ALL SETTINGS > COURSES: on its page, COURSE > STOP FOLLOWING, and no EDIT THIS PAGE
    hold_page(&mut sim, &mut app);
    to_row(&mut sim, &mut app, Row::Open(Menu::Courses));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    for _ in 0..hill(&app) {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    assert_eq!((app.screen(), app.banner.as_str()), (Screen::Ride(COURSE), "COURSE ON"));
    hold_page(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Menu(Menu::Ride(COURSE), 0));
    assert_eq!(labels(&app), ["PAUSE", "END RIDE", "COURSE", "BRIGHTNESS", "SENSORS", "ALL SETTINGS"]);
    to_row(&mut sim, &mut app, Row::Ride(RideRow::StopCourse));
    app.press(&mut sim, Press::Short(Key::Start));
    assert!(app.courses.as_deref().unwrap().on.is_none(), "not followed");
    assert_eq!((app.screen(), app.banner.as_str()), (Screen::Ride(0), "COURSE OFF"), "its page gone: the first");
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);

    // END RIDE: the end-ride screen (the ride paused), RESUME back to the page as it was, then SAVE: the card
    hold_page(&mut sim, &mut app);
    assert_eq!(labels(&app), base);
    to_row(&mut sim, &mut app, Row::Ride(RideRow::End));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.ride.state(), app.screen()), (ride::State::Paused, Screen::End(0)));
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Start)); // RESUME: the page, the ride paused
    assert_eq!((app.ride.state(), app.screen()), (ride::State::Paused, Screen::Ride(0)));
    hold_page(&mut sim, &mut app);
    to_row(&mut sim, &mut app, Row::Ride(RideRow::End));
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.ride.state(), app.screen()), (ride::State::Idle, Screen::Summary));
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
    back(&mut sim, &mut app, 2); // the banner, then the card
    assert_eq!(app.screen(), Screen::Main, "the card, then the overview");
    hold_page(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 0), "no ride: the full menu again");
}

/// The row index of `row` in the menu that holds it (off a ride, from the main menu).
fn row_of(row: menu::Row) -> usize {
    let mut sim = Sim::new(9, 0, 0);
    let app = App::start(&mut sim, None);
    path_to(&app, Menu::Main, row).and_then(|p| p.last().copied()).unwrap_or_else(|| panic!("{row:?}")).1
}

/// The text of row `i` of the menu shown: label and value.
fn main_row(app: &App, i: usize) -> std::string::String {
    let Screen::Menu(m, _) = app.screen() else { panic!("not a menu: {:?}", app.screen()) };
    let (label, value, _) = app.menu_row(m, i).unwrap();
    format!("{} {}", label.as_str(), value.as_str())
}

/// AUTO: the backlight follows the ambient light, smoothed, without flicker on a band edge, and the light
/// sensor measures only now and then, and not at all while the backlight is off.
#[test]
fn auto_backlight_follows_the_ambient_light() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    assert_eq!(sim.bus.accel_ctrl1, 0x07, "the accelerometer switched off");
    run(&mut sim, &mut app, 30_000);
    assert_eq!(sim.bus.light_reads, 0, "a fixed level and no menu: the light sensor is never used");
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::Light));
    for _ in 0..4 {
        app.press(&mut sim, Press::Short(Key::Start)); // 60, 30, 10, AUTO
    }
    assert_eq!(main_row(&app, row_of(menu::Row::Set(Setting::Light))), "BRIGHTNESS AUTO");
    assert_eq!(Settings::from_word(sim.backup[11]).light_level, LIGHT_AUTO, "kept");
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Back)); // SETTINGS, MENU, out
    }
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::BuiltIn));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 4000);
    assert_eq!(built_in_row(&app, 0), "LUX 300");
    assert_eq!(built_in_row(&app, 1), "TEMP C 21.5");
    assert_eq!(built_in_row(&app, 2), "HPA 1013");
    assert_consistent(&sim, &app);
    sim.save("sensor_rows");
    for _ in 0..4 {
        app.press(&mut sim, Press::Short(Key::Back)); // SYSTEM, SETTINGS, MENU, out
    }
    assert_eq!(sim.light_level, 1, "300 lux: past 120 lux, the minimum (stock)");

    sim.bus.lux10 = Some(5); // 0.5 lux: dark
    run(&mut sim, &mut app, 60_000);
    assert_eq!(sim.light_level, 8);
    sim.bus.lux10 = Some(300); // 30 lux
    run(&mut sim, &mut app, 1000);
    assert_eq!(sim.light_level, 50, "within a second");

    // a light hovering on the 50 lux edge: one level throughout
    let mut levels = std::collections::BTreeSet::new();
    for i in 0..40 {
        sim.bus.lux10 = Some(if i % 2 == 0 { 450 } else { 550 });
        run(&mut sim, &mut app, 3000);
        levels.insert(sim.light_level);
    }
    assert_eq!(levels.into_iter().collect::<Vec<_>>(), [50]);

    // one reading per LIGHT_MS
    let reads0 = sim.bus.light_reads;
    run(&mut sim, &mut app, 60_000);
    let reads = sim.bus.light_reads - reads0;
    assert!((230..=245).contains(&reads), "{reads} readings in 60 s");

    app.settings.light_timeout = 1; // 10 s: the light is off, so no readings
    run(&mut sim, &mut app, 12_000);
    let reads = sim.bus.light_reads;
    run(&mut sim, &mut app, 30_000);
    assert_eq!(sim.bus.light_reads, reads);
    assert_eq!(sim.light_level, 0);
}

/// Without the chips the app runs as before: the rows say so, AUTO keeps a fixed level.
#[test]
fn missing_sensor_chips() {
    let mut sim = Sim::new(9, 0, 0);
    sim.bus.lux10 = None;
    sim.bus.baro = None;
    sim.backup[11] = 0xa5 << 24 | (LIGHT_AUTO as u32) << 16 | 2105;
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 10_000);
    assert_eq!(sim.light_level, LIGHT_LEVELS[1]);
    assert_eq!(built_in_row(&app, 0), "LUX NONE");
    assert_eq!(built_in_row(&app, 1), "TEMP C NONE");
}

/// The barometer converts once per reading (one-shot), every 10 s.
#[test]
fn barometer_one_shot() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 60_000);
    assert!((6..=8).contains(&sim.bus.baro_conversions), "{}", sim.bus.baro_conversions);
    assert_eq!(app.baro, Some((10_132, 2150)));
    sim.bus.baro = Some((9_500, -512));
    run(&mut sim, &mut app, 20_000);
    assert_eq!(built_in_row(&app, 1), "TEMP C -5.1");
    assert_eq!(built_in_row(&app, 2), "HPA 950");
}

/// Back on the overview does nothing (it used to switch the light off, a black screen that looked like a crash).
#[test]
fn back_on_the_overview_does_nothing() {
    // Presses after the start (which takes about 1 s with the Nordic chip): at 3 s and at 4 s.
    let back = |ms| (3000..3200).contains(&ms) || (4000..4100).contains(&ms);
    let mut sim = Sim::new(8, 0, 0).keys(move |ms| if back(ms) { Key::Back as u8 } else { 0 });
    let mut app = App::start(&mut sim, None);
    let t0 = sim.ms;
    assert!(t0 < 3000);
    run(&mut sim, &mut app, 3500 - t0);
    assert!(sim.backlight && app.screen() == Screen::Main);
    run(&mut sim, &mut app, 1000);
    assert!(sim.backlight && app.screen() == Screen::Main);
    assert_consistent(&sim, &app);
}

#[test]
fn back_held_3s_powers_off_after_release() {
    // pressed after the start (a key already down at the start is ignored until released)
    let mut sim = Sim::new(8, 0, 0).keys(|ms| if (4000..9000).contains(&ms) { Key::Back as u8 } else { 0 });
    let mut app = App::start(&mut sim, None);
    assert!(sim.ms < 4000);
    run(&mut sim, &mut app, 13_000);
    assert!(sim.standby);
    assert!(sim.ms >= 9000, "standby while the key is still down would wake the unit at once");
    assert!(sim.asleep && !sim.display_on && !sim.backlight);
    assert!(sim.touch.run_since.is_none(), "the touch chip in reset: no INT to keep the core out of standby");
    assert_eq!(sim.nordic.got.last(), Some(&vec![0, 0x32]), "the radio chip switched off too");
    assert_eq!(sim.tones, [1500, 2500, 0, 2500, 1500, 0], "rising at power-on, falling at power-off");
}

/// No zone colour is near the menu's orange (`ui::ACCENT`: selection, info banners) or the alarm red (`ui::RED`):
/// a zone must never look like a banner (UI review 2, P3).
#[test]
fn zone_colours_are_not_accent_or_alarm() {
    let rgb = |c: u16| [(c >> 11) as i32 * 255 / 31, (c >> 5 & 63) as i32 * 255 / 63, (c & 31) as i32 * 255 / 31];
    for (i, z) in crate::ride::ZONE_COLORS.iter().enumerate() {
        for (name, other) in [("ACCENT", ui::ACCENT), ("RED", ui::RED), ("the banner red", 0xf985)] {
            let d: i32 = rgb(*z).iter().zip(rgb(other)).map(|(a, b)| (a - b).abs()).sum();
            assert!(*z != other && d > 80, "zone {} is {z:04x}, near {name}: distance {d}", i + 1);
        }
    }
    let mut all = crate::ride::ZONE_COLORS.to_vec();
    all.dedup();
    assert_eq!(all.len(), 7, "seven distinct colours");
}

/// Heart rate steps over a zone's edge: 125 bpm (zone 2 of 190), 135 (zone 3) for 2 s only, 125 again, then 135 for
/// good from 20 s.
fn hr_edge(ms: u64) -> [u8; 8] {
    let s = ms / 1000;
    let bpm = if (14..16).contains(&s) || s >= 24 { 135 } else { 125 };
    [4, 0, 0, 0, 0, 0, 0, bpm]
}

/// The heart rate's zone bar changes only after three seconds in the new zone (a value on an edge does not flip it
/// every second), and the bar is all that is redrawn when it does: the number, white, has not changed.
#[test]
fn the_zone_bar_holds_three_seconds_and_is_all_that_is_redrawn() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr_edge, network: 0, ack: &[] });
    sim.gnss.fix = Some(8000); // a fix: the strip's "GPS" does not blink (720 px a second) under the count
    let mut app = App::start(&mut sim, Some(KEY));
    app.pages[0] = crate::page::Page::preset(
        1,
        [Field::Hr, Field::Time, Field::Time, Field::Time, Field::Time, Field::Time, Field::Time, Field::Time],
    )
    .unwrap();
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0));
    let bx = app.pages()[0].cell_box(0).unwrap();
    let bar_at = |sim: &Sim| sim.screen[((bx.1 + 1) * W + bx.0 + 10) as usize];
    let (z2, z3) = (crate::ride::hr_color(2), crate::ride::hr_color(3));
    run(&mut sim, &mut app, 6000);
    let mut flips = std::vec::Vec::new();
    let mut last = bar_at(&sim);
    assert_eq!(last, z2);
    let t0 = sim.ms;
    for _ in 0..(30_000 / 250) {
        let px = sim.pixels;
        run(&mut sim, &mut app, 250);
        let now = bar_at(&sim);
        if now != last {
            flips.push((sim.ms - t0, now, sim.pixels - px));
            last = now;
        }
        assert_consistent(&sim, &app);
    }
    assert_eq!(flips.len(), 1, "one change, the 2 s of 135 bpm did not flip it: {flips:?}");
    let (at, colour, px) = flips[0];
    assert_eq!(colour, z3);
    // 135 bpm from 24 s on the sensor's clock, the run began about 6 s in: shown 3 s later
    assert!((18_000..22_000).contains(&at), "{at} ms");
    assert_eq!(px, u64::from(bx.2) * u64::from(ui::ZONE_BAR_H), "only the bar: {px} px");
    sim.save("zone_bar");
}

/// The live readings on the pages (power, speed, cadence, heart rate) change once a second, in the second's tick,
/// though the sensors broadcast at 4 Hz and the value in hand changes at each broadcast (UI review 2).
#[test]
fn live_readings_show_at_one_hertz() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_fast, network: 0, ack: &[] });
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr_fast, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 3000);
    let (mut changes, mut raw, mut seconds) = (0, 0, 0);
    let text = |app: &App| [Field::Power, Field::Hr].map(|f| std::string::String::from(app.field_text(f).as_str()));
    let (mut last, mut last_raw, mut last_sec) = (text(&app), app.live.get(Metric::Power), app.seconds);
    for _ in 0..80 {
        run(&mut sim, &mut app, 250);
        let (now, now_raw) = (text(&app), app.live.get(Metric::Power));
        raw += u32::from(now_raw != last_raw);
        if now != last {
            changes += 1;
            assert_ne!(app.seconds, last_sec, "a reading changed between two seconds: {last:?} to {now:?}");
        }
        seconds += u32::from(app.seconds != last_sec);
        (last, last_raw, last_sec) = (now, now_raw, app.seconds);
    }
    std::println!("20 s: {changes} changes shown, {raw} in the live value, {seconds} seconds");
    assert!(changes >= 10 && changes <= seconds, "shown: {changes} of {seconds} seconds");
    assert!(raw > changes + 15, "the sensors really change at each broadcast: {raw}");
    assert!(sim.nordic.sensors.len() == 2);
}

/// An empty list says so in a row, not in a banner that goes after 4 s: WORKOUTS with nothing planned.
#[test]
fn empty_workouts_say_so_in_a_row() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Workouts, 0));
    assert_eq!(app.rows(Menu::Workouts), 1);
    let (label, _, color) = app.menu_row(Menu::Workouts, 0).unwrap();
    assert_eq!((label.as_str(), color), ("NONE PLANNED", ui::GREY));
    run(&mut sim, &mut app, 6000);
    assert!(app.banner_ms.is_none(), "no banner");
    assert_consistent(&sim, &app);
    sim.save("menu_workouts_empty");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Workouts, 0), "the row does nothing");
    app.press(&mut sim, Press::Short(Key::Back));
    assert!(matches!(app.screen(), Screen::Menu(Menu::Main, _)));
}

/// The ride's card stays until Back or Start: another key, a swipe, a tap or a hold does not dismiss it (2026-10-25
/// review 2, P4: a raindrop threw the summary away).
#[test]
fn the_ride_card_stays_until_back_or_start() {
    use crate::touch::Dir;
    let ended = |sim: &mut Sim| {
        let mut app = App::start(sim, None);
        sim.emmc.image = crate::fat::tests::image();
        app.press(sim, Press::Short(Key::Start));
        run(sim, &mut app, 5000);
        app.press(sim, Press::Short(Key::Start));
        save_ride(sim, &mut app);
        run(sim, &mut app, 5000); // the banners go
        assert_eq!(app.screen(), Screen::Summary);
        app
    };
    let mut sim = Sim::new(9, 0, 0);
    let mut app = ended(&mut sim);
    for i in [
        Input::Next,
        Input::Prev,
        Input::Menu,
        Input::Media,
        Input::Touch(Gesture::Swipe(Dir::Left)),
        Input::Touch(Gesture::Swipe(Dir::Right)),
        Input::Touch(Gesture::Tap(120, 160)),
        Input::Touch(Gesture::Hold(120, 160)),
    ] {
        app.input(&mut sim, i);
        assert_eq!(app.screen(), Screen::Summary, "{i:?}");
    }
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Main, "Back");
    let mut sim = Sim::new(9, 0, 0);
    let mut app = ended(&mut sim);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.ride.state()), (Screen::Main, ride::State::Idle), "Start: no new ride");
}

/// Back answers a confirmation "no": the asking row goes back to its label, nothing is done, and the menu stays
/// (the Back only cancels).
#[test]
fn back_says_no_to_a_confirmation() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    let import = menu::SYSTEM.iter().position(|&r| r == menu::Row::Stock(menu::StockRow::Import)).unwrap();
    app.open(Screen::Menu(Menu::System, import));
    app.show(&mut sim);
    app.settings.ftp = 55;
    app.input(&mut sim, Input::Select);
    assert_eq!(app.menu_row(Menu::System, import).unwrap().1.as_str(), "SURE?");
    app.input(&mut sim, Input::Back);
    assert_eq!(app.screen(), Screen::Menu(Menu::System, import), "the menu stays");
    assert_eq!(app.menu_row(Menu::System, import).unwrap().1.as_str(), "FROM STOCK");
    run(&mut sim, &mut app, 200);
    assert!(app.banner_ms.is_none());
    assert_consistent(&sim, &app);
    app.input(&mut sim, Input::Select);
    app.input(&mut sim, Input::Back);
    app.input(&mut sim, Input::Select);
    assert_eq!(app.settings.ftp, 55, "Start, Back, Start: asked again, not done");
    app.input(&mut sim, Input::Back);
    assert!(matches!(app.screen(), Screen::Menu(Menu::System, _)), "Back cancelled the second asking too");
}

/// Lap on the overview during a ride marks a lap, as on a ride page (and as a shifter's lap does); off a ride it does
/// nothing.
#[test]
fn lap_on_the_overview_marks_a_lap() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Lap));
    run(&mut sim, &mut app, 200);
    assert_ne!(app.banner.as_str(), "LAP", "no ride: no lap");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Main);
    app.press(&mut sim, Press::Short(Key::Lap));
    assert_eq!(app.banner.as_str(), "LAP");
    run(&mut sim, &mut app, 200);
    assert_eq!(app.screen(), Screen::Main);
    assert_consistent(&sim, &app);
}

/// Back pressed with a banner up only clears it (it used to leave the page as well); Page, Lap and Start still act at
/// once, and a ringing call's Back still declines.
#[test]
fn back_clearing_a_banner_only_clears_it() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0));
    app.say("EAT REMINDER");
    run(&mut sim, &mut app, 300);
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Ride(0), "the banner cleared, the page kept");
    run(&mut sim, &mut app, 300);
    assert!(app.banner_ms.is_none());
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Main, "no banner: Back goes back");
    app.say("EAT REMINDER");
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0), "Page acts at once");
}

/// Back and Lap held together for 1 s lock the touch: a padlock in the strip, every tap and swipe ignored, the keys
/// still work; the same hold again unlocks. Neither key acts on its own while they are down together.
#[test]
fn back_and_lap_together_lock_the_touch() {
    let both = |ms: u64| (5000..6300).contains(&ms) || (14_000..15_300).contains(&ms);
    let (gx, gy) = (ui::GEAR_AT.0 as u16 + 9, ui::GEAR_AT.1 as u16 + 9);
    let tap = move |t0: u64| move |ms: u64| (t0..t0 + 80).contains(&ms).then_some((gx, gy));
    let (a, b) = (tap(8000), tap(17_000));
    let mut sim = Sim::new(9, 0, 0)
        .keys(move |ms| if both(ms) { Key::Back as u8 | Key::Lap as u8 } else { 0 })
        .finger(move |ms| a(ms).or(b(ms)));
    let mut app = App::start(&mut sim, None);
    let left = 5500 - sim.ms;
    run(&mut sim, &mut app, left);
    assert!(!app.strip.locked, "not yet: held 0.5 s");
    run(&mut sim, &mut app, 1000);
    assert!(app.strip.locked && app.banner.as_str() == "TOUCH LOCKED");
    run(&mut sim, &mut app, 500);
    assert_eq!(app.screen(), Screen::Main, "neither key acted at its release");
    assert!(sim.backlight, "Back did not toggle the light");
    // the padlock is in the strip, and the screen equals the panel
    let lit = |sim: &Sim| (81..88).any(|x| sim.screen[(12 * W + x) as usize] == ui::WHITE);
    run(&mut sim, &mut app, 4500); // the banner goes, the gear tapped at 8 s
    assert!(lit(&sim), "padlock");
    assert_consistent(&sim, &app);
    sim.save("touch_lock");
    run(&mut sim, &mut app, 100);
    assert_eq!(app.screen(), Screen::Main, "a tap on the gear: ignored while locked");
    // keys still work, on every screen
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0));
    assert!(app.strip.locked);
    app.press(&mut sim, Press::Short(Key::Back));
    let left = 15_800u64.saturating_sub(sim.ms);
    run(&mut sim, &mut app, left);
    assert!(!app.strip.locked && app.banner.as_str() == "TOUCH UNLOCKED");
    run(&mut sim, &mut app, 2500);
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 0), "unlocked: the gear's tap at 17 s works");
    assert_consistent(&sim, &app);
}

/// Back and Lap pressed one after the other (not together) still act on their own.
#[test]
fn chord_keys_alone_still_act() {
    let mut k = Keys::default();
    assert_eq!(k.poll(Key::Back as u8, 50), None);
    assert_eq!(k.poll(0, 50), Some(Press::Short(Key::Back)));
    assert_eq!(k.poll(Key::Lap as u8, 50), None);
    assert_eq!(k.poll(0, 50), Some(Press::Short(Key::Lap)));
    // together: neither acts; the chord fires once, at 1 s
    let both = Key::Back as u8 | Key::Lap as u8;
    let held: Vec<_> = (0..24).map(|_| k.poll(both, 50)).collect();
    assert_eq!(held.iter().filter(|p| **p == Some(Press::Chord)).count(), 1);
    assert_eq!(held[19], Some(Press::Chord), "at 1 s");
    assert!(held.iter().all(|p| matches!(p, None | Some(Press::Chord))), "no repeat or long of either: {held:?}");
    assert_eq!(k.poll(Key::Back as u8, 50), None, "one released, one down");
    assert_eq!(k.poll(0, 50), None, "and the release of the other: nothing");
    // a quick overlap (under 1 s) acts as neither
    for _ in 0..4 {
        assert_eq!(k.poll(both, 50), None);
    }
    assert_eq!(k.poll(0, 50), None);
    // and keys work again after
    assert_eq!(k.poll(Key::Lap as u8, 50), None);
    assert_eq!(k.poll(0, 50), Some(Press::Short(Key::Lap)));
}

/// A synthetic ride at 8 m/s (28.8 km/h): 240 s east with a gentle wiggle, then a loop of 600 m radius
/// counterclockwise, starting heading east. Latitude, longitude (1e-7 degrees) and heading (1e-5 degrees).
fn ride_route(ms: u64) -> (i32, i32, i32) {
    route_from(ms, (FIX_LAT, FIX_LON), 1.0)
}

/// The same ride turned half a circle (west, then a loop south), from `origin`: `sign` -1.
fn route_from(ms: u64, (lat0, lon0): (i32, i32), sign: f64) -> (i32, i32, i32) {
    let at = |t: f64| {
        if t < 240.0 {
            (8.0 * t, 30.0 * (t * core::f64::consts::TAU / 60.0).sin())
        } else {
            let a = (t - 240.0) * 8.0 / 600.0;
            (1920.0 + 600.0 * a.sin(), 600.0 - 600.0 * a.cos())
        }
    };
    let t = ms as f64 / 1000.0;
    let ((x, y), (x1, y1)) = (at(t), at(t + 0.5));
    let (x, y, x1, y1) = (sign * x, sign * y, sign * x1, sign * y1);
    let heading = (x1 - x).atan2(y1 - y).to_degrees().rem_euclid(360.0);
    let m_per_unit = 0.011_132; // metres per 1e-7 degree of latitude, as map.rs
    let cos = (f64::from(lat0) * 1e-7).to_radians().cos();
    (lat0 + (y / m_per_unit) as i32, lon0 + (x / m_per_unit / cos) as i32, (heading * 1e5) as i32)
}

/// The ride west and south of 0 N 0 E, where the synthetic tiles are (tools/fat_image.py).
pub(super) fn tiles_route(ms: u64) -> (i32, i32, i32) {
    route_from(ms, crate::tiles::tests::ORIGIN, -1.0)
}

/// The basemap from the synthetic tiles (on the FAT volume macOS made, through the eMMC driver): drawn at
/// the last known position before a fix, kept at the first fix, then under the ride. Between view changes the
/// arrow and the track restore it in small boxes and the eMMC is not read; the screenshot equals the panel.
#[test]
fn basemap_under_the_map() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let aid = crate::gnss::Aid { offset_15min: Some(8), pos: Some(crate::tiles::tests::ORIGIN) };
    [sim.backup[17], sim.backup[18]] = aid.to_words();
    sim.gnss.route = Some(tiles_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    run(&mut sim, &mut app, 500);
    let bg = |sim: &Sim| sim.screen.iter().filter(|&&p| p == crate::tiles::BACKGROUND).count();
    sim.save("basemap_last_known");
    assert!(bg(&sim) > 30_000, "the basemap: {}", bg(&sim));
    assert_consistent(&sim, &app);
    std::println!("basemap at the last known position: {:?}", app.basemap_stats());
    // stock's rides are read by themselves after STOCK_SCAN_MS: read first, so the reads counted below are the
    // basemap's
    app.stock.start(app.date);
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim, &mut app, u64::from(POLL_MS));
    }
    let reads = |sim: &Sim| sim.emmc.cmds.iter().filter(|c| c.0 == 17 || c.0 == 18).count();
    sim.gnss.fix = Some(8000);
    let view = app.map_view(2);
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.map_view(2), view, "the first fix, inside the view: the same view, no new basemap");
    assert_consistent(&sim, &app);
    let t0 = sim.ms;
    let (mut refits, mut refit_px, mut small_px, mut small_reads, mut drawn) = (0, 0, 0, 0, 0);
    let mut seconds = 0;
    while sim.ms < t0 + 840_000 {
        let (view, px, r) = (app.map_view(0), sim.pixels, reads(&sim));
        app.step(&mut sim);
        sim.delay_ms(POLL_MS);
        if app.map_view(0) != view {
            refits += 1;
            refit_px += sim.pixels - px;
            drawn += u64::from(app.basemap_stats().px);
            std::println!("view change at {} s: {:?}", (sim.ms - t0) / 1000, app.basemap_stats());
        } else {
            small_px += sim.pixels - px;
            small_reads += reads(&sim) - r;
        }
        if (sim.ms - t0) / 1000 > seconds {
            seconds = (sim.ms - t0) / 1000;
            if seconds % 60 == 0 {
                assert_consistent(&sim, &app);
            }
            if seconds == 120 {
                sim.save("basemap_ride");
            }
        }
    }
    assert_consistent(&sim, &app);
    sim.save("basemap_loop");
    let s = (sim.ms - t0) / 1000;
    std::println!(
        "basemap ride: {refits} view changes ({} panel px, {} buffer px each), {} panel px/s between them",
        refit_px / refits.max(1),
        drawn / refits.max(1),
        small_px / s
    );
    assert_eq!(small_reads, 0, "the eMMC is read on view changes only");
    assert!(small_px / s < 2000, "{} px/s between view changes", small_px / s);
    assert!(bg(&sim) > 20_000, "the basemap under the loop");
}

/// IGN map packs on the volume (the synthetic ones of test-data/ign, through the eMMC driver): the map page draws
/// them under the track, with "(c) IGN" in the map's bottom right corner; between view changes the eMMC is not read.
#[test]
fn ign_map_under_the_map() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::tiles::tests::image_with_packs();
    let aid = crate::gnss::Aid { offset_15min: Some(8), pos: Some(crate::tiles::tests::ORIGIN) };
    [sim.backup[17], sim.backup[18]] = aid.to_words();
    sim.gnss.route = Some(tiles_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    sim.gnss.fix = Some(500);
    run(&mut sim, &mut app, 60_000);
    let stats = app.basemap_stats();
    std::println!("IGN map: {stats:?}");
    assert!(stats.ign, "{stats:?}");
    sim.save("ign_map");
    assert_consistent(&sim, &app);
    // the attribution: the map's bottom right corner (the page's last 28 x 11 pixels) black and white
    let screen = &sim.screen;
    let corner = (H - 11..H).flat_map(|y| (W - 28..W).map(move |x| screen[(y * W + x) as usize]));
    let (black, white) =
        corner.fold((0, 0), |(b, w), p| (b + usize::from(p == ui::BLACK), w + usize::from(p == ui::WHITE)));
    assert!(black > 20 && white > 100, "(c) IGN: {black} black, {white} white");

    // a ride's card (the one just ended; a past ride's goes the same way) has the map under its track too
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 20_000);
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.screen(), Screen::Summary);
    assert!(app.basemap_stats().ign, "the card's basemap: {:?}", app.basemap_stats());
    sim.save("card_ign");
    assert_consistent(&sim, &app);
}

/// The map page (page 3 by default) through a ride: "NO FIX" until the first fix, then the track and the arrow;
/// fitted again a few times while the ride goes out, never once the loop is closed; between those, only small
/// boxes are redrawn. The panel always equals the screenshot rendered row by row.
/// Before any fix, the map shows the last known position (kept from an earlier power-up) as a grey dot instead
/// of "NO FIX"; the first fix replaces it with the arrow.
#[test]
fn map_shows_the_last_known_position_until_a_fix() {
    let mut sim = Sim::new(9, 0, 0);
    let aid = crate::gnss::Aid { offset_15min: Some(8), pos: Some((FIX_LAT / 1000 * 1000, FIX_LON / 1000 * 1000)) };
    [sim.backup[17], sim.backup[18]] = aid.to_words();
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    assert_eq!(app.screen(), Screen::Ride(2), "page 3: the map");
    run(&mut sim, &mut app, 500);
    // the middle of the map, where the dot or the arrow is (the strip and the scale bar are white too)
    let count = |sim: &Sim, c: u16| {
        let (cx, cy) = (W / 2, crate::ui::RIDE_Y + crate::ui::RIDE_H / 2);
        let mut n = 0;
        for y in cy - 20..cy + 20 {
            n += (cx - 20..cx + 20).filter(|&x| sim.screen[(y * W + x) as usize] == c).count();
        }
        n
    };
    sim.save("map_last_known");
    assert_eq!(count(&sim, crate::ui::GREY), 49, "the grey dot, 7 x 7");
    assert_eq!(count(&sim, crate::ui::WHITE), 0, "no arrow, no NO FIX text");
    assert_consistent(&sim, &app);
    sim.gnss.fix = Some(5000);
    run(&mut sim, &mut app, 3000);
    assert!(!app.track.stale() && count(&sim, crate::ui::WHITE) > 20, "the fix: the arrow");
    assert_consistent(&sim, &app);
}

#[test]
fn map_page_follows_a_ride() {
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    assert_eq!(app.screen(), Screen::Ride(2));
    run(&mut sim, &mut app, 3000);
    assert_consistent(&sim, &app);
    sim.save("map_nofix");
    sim.gnss.fix = Some(8000);
    let area = crate::map::area(crate::ui::field_box(1, crate::ui::RIDE_H, 0).unwrap());
    let t0 = sim.ms;
    let (mut refits, mut refit_px, mut small_px, mut late_refits) = (0, 0, 0, 0);
    let mut seconds = 0;
    while sim.ms < t0 + 840_000 {
        let (view, px) = (app.map_view(0), sim.pixels);
        app.step(&mut sim);
        sim.delay_ms(POLL_MS);
        let cost = sim.pixels - px;
        if app.map_view(0) != view {
            refits += 1;
            refit_px += cost;
            late_refits += u32::from(sim.ms > t0 + 720_000);
        } else {
            small_px += cost;
        }
        if (sim.ms - t0) / 1000 > seconds {
            seconds = (sim.ms - t0) / 1000;
            if seconds % 60 == 0 {
                assert_consistent(&sim, &app);
            }
            match seconds {
                30 => sim.save("map_start"),
                240 => sim.save("map_out"),
                _ => {}
            }
        }
    }
    assert_consistent(&sim, &app);
    sim.save("map_loop");
    let s = (sim.ms - t0) / 1000;
    std::println!(
        "map: {refits} full redraws ({} px each), {} px/s otherwise, {} px/s in all, over {s} s",
        refit_px / refits.max(1),
        small_px / s,
        (refit_px + small_px) / s
    );
    assert!((2..=12).contains(&refits), "fitted again as the ride grows: {refits}");
    assert_eq!(late_refits, 0, "the loop closed: the view stays");
    assert!(small_px / s < 2000, "{} px/s between full redraws", small_px / s);
    let v = app.map_view(0).unwrap();
    let (x, y) = v.px(area, app.track.pos().unwrap());
    assert_eq!(sim.screen[(y as u32 * W + x as u32) as usize], crate::ui::WHITE, "the arrow at the position");
    let track = sim.screen.iter().filter(|&&p| p == crate::ui::ACCENT).count();
    assert!(track > 500, "the track drawn: {track} px");
    // the map as one field of three, over speed and heart rate
    app.pages[0] =
        PageCfg { layout: 3, fields: [Field::Map, Field::Speed, Field::Hr, Field::Time, Field::Time, Field::Time] }
            .into();
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0), "from the map, round to the first page");
    run(&mut sim, &mut app, 10_000);
    assert_consistent(&sim, &app);
    sim.save("map_field");
}

/// The map's "+" and "-" (map.rs, manual zoom) on the full-page map over the basemap: each tap one level, the view
/// following the rider across re-centres (the arrow always on the map), the panel equal to the screenshot at each
/// step, back to the fit at its level; a hold on the map still opens the menu; no buttons in a map of half a page.
#[test]
fn map_zoom_buttons() {
    use crate::map::{self, MIN_LEVEL, mpp16};
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.gnss.route = Some(tiles_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    assert_eq!(app.screen(), Screen::Ride(2), "the map page");
    sim.gnss.fix = Some(500);
    run(&mut sim, &mut app, 200_000); // out and onto the loop: a fit coarser than a few levels
    let area = map::area(ui::field_box(1, ui::RIDE_H, 0).unwrap());
    let [plus, minus] = map::buttons(area).unwrap();
    let tap = |sim: &mut Sim, app: &mut App, b: map::Rect| {
        app.input(sim, Input::Touch(Gesture::Tap((b.0 + b.2 / 2) as u16, (b.1 + b.3 / 2) as u16)));
        assert_consistent(sim, app);
    };
    let fit = app.map_view(0).unwrap();
    assert_eq!(app.map_zoom, None);
    sim.save("map_zoom_fit");
    tap(&mut sim, &mut app, plus);
    let first = app.map_zoom.expect("zoomed in");
    assert_eq!(mpp16(first + 1), fit.mpp16, "one level finer than the fit");
    assert_eq!(app.map_view(0).unwrap().mpp16, mpp16(first));
    for _ in 0..20 {
        tap(&mut sim, &mut app, plus);
    }
    assert_eq!(app.map_zoom, Some(MIN_LEVEL), "no finer than 1.5 m a pixel");
    assert_eq!(app.map_view(0).unwrap().mpp16, 24);
    sim.save("map_zoom_in");
    // riding on, zoomed in: the view follows, centred again only when the arrow leaves the middle part
    let (t0, px0) = (sim.ms, sim.pixels);
    let mut recentres = 0;
    while sim.ms < t0 + 120_000 {
        let view = app.map_view(0);
        run(&mut sim, &mut app, u64::from(POLL_MS));
        let v = app.map_view(0).unwrap();
        recentres += u32::from(Some(v) != view);
        assert_eq!(v.mpp16, 24, "the zoom kept");
        let (x, y) = v.px(area, app.track.pos().unwrap());
        assert!(x >= area.2 / 6 && x < area.2 * 5 / 6, "the arrow on the map: {x}");
        assert!(y >= area.1 + area.3 / 6 && y < area.1 + area.3 * 5 / 6, "the arrow on the map: {y}");
        if (sim.ms - t0) % 30_000 < u64::from(POLL_MS) {
            assert_consistent(&sim, &app);
        }
    }
    let s = (sim.ms - t0) / 1000;
    std::println!("map zoomed in at 1.5 m a pixel: {recentres} re-centres in {s} s, {} px/s", (sim.pixels - px0) / s);
    assert!((2..=12).contains(&recentres), "followed, not redrawn every second: {recentres}");
    sim.save("map_zoom_follow");
    // out again, a level at a time, past the fit: still by hand, coarser than the fit
    let fit_level = (MIN_LEVEL..map::MAX_LEVEL).find(|&l| mpp16(l) == fit.mpp16).unwrap();
    for _ in MIN_LEVEL..fit_level + 2 {
        tap(&mut sim, &mut app, minus);
    }
    assert_eq!(app.map_zoom, Some(fit_level + 2), "coarser than the fit, by hand");
    assert_eq!(app.map_view(0).unwrap().mpp16, mpp16(fit_level + 2));
    // the crosshair: back to the fit
    let centre = (minus.0, minus.1 + minus.3 + 4, minus.2, minus.3);
    sim.save("map_zoom_out");
    tap(&mut sim, &mut app, centre);
    assert_eq!(app.map_zoom, None, "back to the fit");
    let shown = map::Shown::of(&app.track, area, None, None);
    assert_eq!(app.map_view(0), shown.view, "the fitted view");
    // a hold on the map opens the page editor (no ride recording), even on a button
    app.input(&mut sim, Input::Touch(Gesture::Hold((plus.0 + 4) as u16, (plus.1 + 4) as u16)));
    assert_eq!(app.screen(), Screen::Edit(2, 0));
    app.input(&mut sim, Input::Back);
    assert_eq!(app.screen(), Screen::Ride(2));
    // half a page: no buttons, a tap there does not zoom
    app.pages[2] =
        PageCfg { layout: 2, fields: [Field::Map, Field::Speed, Field::Time, Field::Time, Field::Time, Field::Time] }
            .into();
    app.show(&mut sim);
    let half = map::area(ui::field_box(2, ui::RIDE_H, 0).unwrap());
    assert_eq!(map::buttons(half), None);
    app.map_zoom = Some(MIN_LEVEL + 1); // as if zoomed in on the full page: "+" or "-" there would change it
    for b in [plus, minus] {
        tap(&mut sim, &mut app, b);
        assert_eq!(app.map_zoom, Some(MIN_LEVEL + 1), "no buttons, no zoom");
    }
    sim.save("map_zoom_half");
}

/// Indoors (the owner's test on the unit, 2026-10-25: "the map's gestures don't work"): a fix that does not move,
/// so the fit is already the finest it takes (2 m a pixel). "-" zooms out by hand, a drag pans that view, the
/// crosshair goes back to the fit, "+" goes finer than the fit (1.5 m); before, "+" and "-" both left the fit and
/// nothing could pan.
#[test]
fn map_zoom_indoors() {
    use crate::map::{self, FIT_LEVEL, MIN_LEVEL, mpp16};
    let mut sim = Sim::new(9, 0, 0);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    sim.gnss.fix = Some(500);
    run(&mut sim, &mut app, 30_000);
    let area = map::area(ui::field_box(1, ui::RIDE_H, 0).unwrap());
    let [plus, minus] = map::buttons(area).unwrap();
    let centre = (minus.0, minus.1 + minus.3 + 4, minus.2, minus.3);
    let tap = |sim: &mut Sim, app: &mut App, b: map::Rect| {
        app.input(sim, Input::Touch(Gesture::Tap((b.0 + b.2 / 2) as u16, (b.1 + b.3 / 2) as u16)));
        assert_consistent(sim, app);
    };
    let fit = app.map_view(0).unwrap();
    assert_eq!(fit.mpp16, mpp16(FIT_LEVEL), "indoors: the fit at its finest");
    let count = |sim: &Sim, c: u16| {
        let r = centre;
        (r.1..r.1 + r.3)
            .flat_map(|y| (r.0..r.0 + r.2).map(move |x| (x, y)))
            .filter(|&(x, y)| sim.screen[(y as u32 * ui::W + x as u32) as usize] == c)
            .count()
    };
    let no_crosshair = count(&sim, ui::WHITE);
    tap(&mut sim, &mut app, minus);
    assert_eq!(app.map_zoom, Some(FIT_LEVEL + 1), "out by hand from the fit");
    assert_eq!(app.map_view(0).unwrap().mpp16, mpp16(FIT_LEVEL + 1));
    assert!(count(&sim, ui::WHITE) > no_crosshair + 100, "the crosshair drawn");
    sim.save("map_zoom_indoors_out");
    // a drag pans it (3 m a pixel: left and down, the view goes east and north)
    let before = app.map_view(0).unwrap();
    drag(&mut sim, &mut app, (100, 200), (-60, 30));
    let v = app.map_view(0).unwrap();
    assert_eq!((v.cx, v.cy), (before.cx + 180, before.cy + 90), "the view moved with the finger: {v:?}");
    assert_eq!(app.screen(), Screen::Ride(2));
    assert_consistent(&sim, &app);
    // the crosshair: the fit again, the crosshair gone
    tap(&mut sim, &mut app, centre);
    assert_eq!((app.map_zoom, app.map_centre), (None, None));
    assert_eq!(app.map_view(0), Some(fit));
    assert_eq!(count(&sim, ui::WHITE), no_crosshair, "no crosshair at the fit");
    // finer than the fit
    tap(&mut sim, &mut app, plus);
    assert_eq!(app.map_zoom, Some(MIN_LEVEL));
    assert_eq!(app.map_view(0).unwrap().mpp16, 24, "1.5 m a pixel");
    sim.save("map_zoom_indoors_in");
}

/// A zoom set in one ride is not the next ride's (review: unused while the new fit was finer, it came back by
/// itself as the fit grew); a course followed or dropped fits the map again too.
#[test]
fn map_zoom_ends_with_the_ride() {
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    sim.emmc.image = crate::fat::tests::image();
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    sim.gnss.fix = Some(500);
    app.press(&mut sim, Press::Short(Key::Start));
    app.screen = Screen::Ride(2);
    app.show(&mut sim);
    run(&mut sim, &mut app, 200_000);
    let area = crate::map::area(ui::field_box(1, ui::RIDE_H, 0).unwrap());
    let [plus, _] = crate::map::buttons(area).unwrap();
    app.input(&mut sim, Input::Touch(Gesture::Tap((plus.0 + 20) as u16, (plus.1 + 20) as u16)));
    app.input(&mut sim, Input::Touch(Gesture::Drag(100, 200, 30, 0)));
    assert!(app.map_zoom.is_some() && app.map_centre.is_some(), "zoomed and panned");
    // the ride ended, a new one started
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    back(&mut sim, &mut app);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.map_zoom, app.map_centre), (None, None), "the new ride's map fitted");
    app.screen = Screen::Ride(2);
    app.show(&mut sim);
    run(&mut sim, &mut app, 200_000);
    assert!(matches!(app.drawn[0], crate::Drawn::Map(m) if m.view.is_some() && !m.zoomed()), "still fitted");
    assert_consistent(&sim, &app);
    // zoomed again, then a course followed: fitted to it
    app.input(&mut sim, Input::Touch(Gesture::Tap((plus.0 + 20) as u16, (plus.1 + 20) as u16)));
    assert!(app.map_zoom.is_some());
    app.set_course_memory(crate::storage::course::tests::memory());
    app.open_courses(&mut sim);
    app.follow_course(hill(&app));
    assert_eq!((app.map_zoom, app.map_centre), (None, None), "a course: the map fitted");
}

/// A course followed, the map zoomed in: the course drawn under the track as at the fit, the view following the
/// rider along it.
#[test]
fn map_zoom_with_a_course() {
    use crate::map;
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.gnss.route = Some(course_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    app.set_course_memory(crate::storage::course::tests::memory());
    app.open_courses(&mut sim);
    app.follow_course(hill(&app));
    for _ in 0..100 {
        app.step(&mut sim); // read over a few passes
    }
    sim.gnss.fix = Some(500);
    app.screen = Screen::Ride(2);
    app.show(&mut sim);
    run(&mut sim, &mut app, 20_000);
    let area = map::area(ui::field_box(1, ui::RIDE_H, 0).unwrap());
    let [plus, _] = map::buttons(area).unwrap();
    for _ in 0..3 {
        app.input(&mut sim, Input::Touch(Gesture::Tap((plus.0 + 20) as u16, (plus.1 + 20) as u16)));
    }
    let level = app.map_zoom.expect("zoomed in");
    let mut views = 0;
    for _ in 0..12 {
        let view = app.map_view(0);
        run(&mut sim, &mut app, 10_000);
        let v = app.map_view(0).unwrap();
        views += u32::from(Some(v) != view);
        assert_eq!(v.mpp16, map::mpp16(level));
        let (x, y) = v.px(area, app.track.pos().unwrap());
        assert!(x >= area.0 && x < area.0 + area.2 && y >= area.1 && y < area.1 + area.3, "the arrow on the map");
        let course = sim.screen.iter().filter(|&&p| p == map::COURSE).count();
        assert!(course > 50, "the course drawn: {course} px");
        assert_consistent(&sim, &app);
    }
    assert!(views > 0, "the view follows the rider");
    sim.save("map_zoom_course");
}

/// A finger dragged from `from` by `by` pixels on the touch panel, through the touch path as on the unit (sampled every
/// [`TOUCH_MS`]), then lifted; the passes run until it is handled.
pub(super) fn drag(sim: &mut Sim, app: &mut App, from: (i32, i32), by: (i32, i32)) {
    let t0 = sim.ms;
    sim.touch.finger = Box::new(move |ms| {
        let t = ms.checked_sub(t0)? as i32;
        let k = (t - 50).clamp(0, 200);
        (t < 400).then_some(((from.0 + by.0 * k / 200) as u16, (from.1 + by.1 * k / 200) as u16))
    });
    run(sim, app, 800);
    sim.touch.finger = Box::new(|_| None);
}

/// Panning the map (map.rs): fitted, a swipe still changes the page; zoomed in, a drag on the map moves the map with
/// the finger at the release, the view stops following, "+" keeps the centre, "re-centre" follows again; the pan is
/// kept near the track; the keys still change pages and a hold still opens the menu. The panel equals the screenshot
/// at each step.
#[test]
fn map_pan() {
    use crate::map;
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.gnss.route = Some(tiles_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    sim.gnss.fix = Some(500);
    run(&mut sim, &mut app, 200_000);
    let area = map::area(ui::field_box(1, ui::RIDE_H, 0).unwrap());
    let [plus, minus] = map::buttons(area).unwrap();
    let centre_button = (minus.0, minus.1 + minus.3 + 4, minus.2, minus.3);
    let tap = |sim: &mut Sim, app: &mut App, b: map::Rect| {
        app.input(sim, Input::Touch(Gesture::Tap((b.0 + b.2 / 2) as u16, (b.1 + b.3 / 2) as u16)));
        assert_consistent(sim, app);
    };
    // fitted: a swipe on the map changes the page as before, and back
    drag(&mut sim, &mut app, (160, 200), (-80, 0));
    assert_eq!(app.screen(), Screen::Ride(0), "swipe left: the next page");
    drag(&mut sim, &mut app, (80, 200), (80, 0));
    assert_eq!(app.screen(), Screen::Ride(2), "swipe right: the map again");
    assert_eq!(app.map_centre, None);
    tap(&mut sim, &mut app, plus);
    tap(&mut sim, &mut app, plus);
    let v0 = app.map_view(0).unwrap();
    let m = |px: i32| px * v0.mpp16 as i32 / 16;
    // zoomed: a drag pans, the map moving with the finger (right and up: the view goes west and south)
    let (px, t0) = (sim.pixels, sim.ms);
    drag(&mut sim, &mut app, (100, 200), (60, -40));
    assert_eq!(app.screen(), Screen::Ride(2), "the drag panned, no page change");
    let v = app.map_view(0).unwrap();
    assert_eq!((v.cx, v.cy), (v0.cx - m(60), v0.cy - m(40)), "moved by the finger's vector");
    assert_eq!(app.map_centre, Some((v.cx, v.cy)));
    assert_consistent(&sim, &app);
    sim.save("map_pan");
    std::println!("a pan: {} px drawn in {} ms", sim.pixels - px, sim.ms - t0);
    // a short drag (not a swipe) pans too
    drag(&mut sim, &mut app, (100, 200), (-20, 0));
    assert_eq!(app.map_view(0).unwrap().cx, v.cx + m(20));
    // the view stays where it was put while the rider goes on
    let (kept, px, t0) = (app.map_view(0), sim.pixels, sim.ms);
    run(&mut sim, &mut app, 60_000);
    assert_eq!(app.map_view(0), kept, "panned: no following");
    std::println!("panned, riding: {} px/s", (sim.pixels - px) / ((sim.ms - t0) / 1000));
    assert_consistent(&sim, &app);
    // "+" while panned: finer around the same centre
    tap(&mut sim, &mut app, plus);
    let z = app.map_view(0).unwrap();
    assert!(z.mpp16 < v0.mpp16 && (z.cx, z.cy) == (kept.unwrap().cx, kept.unwrap().cy), "{z:?}");
    // a hold on the zoomed map opens the page editor (no ride recording)
    app.input(&mut sim, Input::Touch(Gesture::Hold(100, 200)));
    assert!(matches!(app.screen(), Screen::Edit(..)));
    app.input(&mut sim, Input::Back);
    // the pan is kept within PAN_SCREENS map widths of the track
    for _ in 0..30 {
        drag(&mut sim, &mut app, (200, 150), (-180, 0));
    }
    let far = app.map_view(0).unwrap();
    let east = app.track.points().iter().map(|p| p.0).max().unwrap();
    assert!(far.cx <= east + area.2 * map::PAN_SCREENS * far.mpp16 as i32 / 16 + 1, "{far:?}");
    assert!(far.cx > east, "far east: {far:?}");
    assert_consistent(&sim, &app);
    sim.save("map_pan_far");
    // re-centre: following the rider again, the button gone
    tap(&mut sim, &mut app, centre_button);
    assert_eq!(app.map_centre, None);
    let v = app.map_view(0).unwrap();
    let (x, y) = v.px(area, app.track.pos().unwrap());
    assert!(x >= area.2 / 5 && x < area.2 * 4 / 5 && y >= area.1 + area.3 / 5, "the rider in the middle: {x},{y}");
    assert!(app.zoom_at(centre_button.0 as u16 + 20, centre_button.1 as u16 + 20).is_none(), "no re-centre button");
    sim.save("map_pan_recentred");
    run(&mut sim, &mut app, 30_000);
    assert_consistent(&sim, &app);
    // zoomed, the keys still change pages
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0), "Page: the next page");
}

/// Heart rate rising from 90 to 209 over two minutes, then again: every zone at the default max of 190.
fn hr_ramp(ms: u64) -> [u8; 8] {
    [4, 0, 0, 0, 0, 0, 0, 90 + (ms / 1000 % 120) as u8]
}
/// Power rising from 100 to 395 W over a minute, then again: every zone at the default FTP of 200 W.
fn power_ramp(ms: u64) -> [u8; 8] {
    let [p0, p1] = (100 + (ms / 1000 % 60) as u16 * 5).to_le_bytes();
    [0x10, 1, 0xff, 85, 0, 0, p0, p1]
}
/// Climbing 0.2 m/s from 100 m.
fn climb(ms: u64) -> i32 {
    100_000 + (ms / 5) as i32
}

/// The ride keys and fields: a ride on the GPS route (28.8 km/h) with a strap and a power meter. Start, a lap,
/// a stop (auto-pause), Start to pause and resume, a long Start to end. The fields' values follow, heart rate and
/// power in their zone colours, the graph's bars too; the strip's time is green while recording, orange when
/// paused. Few pixels a second while riding.
#[test]
fn a_ride_with_laps_and_pauses() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image(); // a volume to record on (else the strip says not saving)
    sim.own_settings(); // the defaults, not stock's
    sim.gnss.route = Some(ride_route);
    sim.gnss.alt = Some(climb);
    sim.bus.baro_alt = Some(climb); // the barometer climbs with the receiver
    sim.gnss.fix = Some(8000);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr_ramp, network: 0, ack: &[] });
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_ramp, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 6, fields: [Timer, Dist, AvgSpeed, AvgHr, AvgPower, Graph] }.into();
    app.pages[1] = PageCfg { layout: 6, fields: [Hr, Power, MaxSpeed, LapTime, LapDist, Ascent] }.into();
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 5000);
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    assert_eq!((text(&app, Timer), text(&app, Dist), app.strip.rec), ("00:00".into(), "0.00".into(), Rec::Idle));
    app.press(&mut sim, Press::Short(Key::Start));
    let (t0, mut px0) = (sim.ms, sim.pixels);
    let mut seconds = 0;
    while sim.ms < t0 + 720_000 {
        run(&mut sim, &mut app, 1000);
        seconds += 1;
        if seconds % 30 == 0 {
            assert_consistent(&sim, &app);
        }
        if seconds == 60 {
            let px_s = (sim.pixels - px0) / 60;
            std::println!("ride page (timer, km, avg km/h, avg HR, avg W, graph), first minute: {px_s} px/s");
        }
        if seconds == 600 {
            px0 = sim.pixels; // the averages have settled
        }
    }
    // the averages of ramps settle as a long ride's do: from then on, mostly the timer, the distance, the graph (1250 px/s
    // before the UI review's step 3: the timer's seconds at the box's 4x, 560 px of ink instead of a 240 px 2x cell, and
    // the graph without its 1x axes in a half box, 112 columns instead of 85)
    let px_s = (sim.pixels - px0) / 120;
    std::println!("ride page (timer, km, avg km/h, avg HR, avg W, graph), minutes 10 to 12: {px_s} px/s");
    assert!(px_s < 1800, "{px_s} px/s while riding");
    assert_eq!(app.strip.rec, Rec::Running);
    let timer = app.ride.timer_s();
    assert!((719..=721).contains(&timer), "timer {timer}");
    assert_eq!(text(&app, Dist), "5.76", "8 m/s for 720 s");
    assert_eq!(text(&app, AvgSpeed), "28.8");
    let bx = |i| crate::ui::field_box(6, crate::ui::RIDE_H, i).unwrap();
    let colors_in = |sim: &Sim, (x, y, w, h): (u32, u32, u32, u32)| {
        let mut c: Vec<u16> = (y..y + h)
            .flat_map(|y| (x..x + w).map(move |x| (x, y)))
            .map(|(x, y)| sim.screen[(y * W + x) as usize])
            .collect();
        c.sort();
        c.dedup();
        c
    };
    let graph = colors_in(&sim, bx(5));
    let zones = crate::ride::ZONE_COLORS.iter().filter(|c| graph.contains(c)).count();
    assert!(zones >= 5, "the power bars in their zones' colours: {zones} colours");
    sim.save("ride_page");
    app.press(&mut sim, Press::Short(Key::Lap));
    app.press(&mut sim, Press::Short(Key::Page));
    let (t0, px0) = (sim.ms, sim.pixels);
    run(&mut sim, &mut app, 60_000);
    let px_s = (sim.pixels - px0) * 1000 / (sim.ms - t0);
    std::println!("ride page (HR, power changing every second, max km/h, lap time, lap km, ascent): {px_s} px/s");
    // the zones: a bar along the top of the heart rate and power boxes in the zone shown (held 3 s), the number white
    let (hr_bar, w_bar) = (crate::ride::hr_color(app.zones[3].shown()), crate::ride::power_color(app.zones[0].shown()));
    let (hr_box, w_box) = (colors_in(&sim, bx(0)), colors_in(&sim, bx(1)));
    assert!(hr_box.contains(&hr_bar) && w_box.contains(&w_bar), "zone bars");
    assert!(hr_box.contains(&crate::ui::WHITE) && w_box.contains(&crate::ui::WHITE), "white numbers");
    let zone_colors = |c: &[u16]| crate::ride::ZONE_COLORS.iter().filter(|z| c.contains(z)).count();
    assert!(zone_colors(&hr_box) == 1 && zone_colors(&w_box) == 1, "the bar is the only zone colour in the box");
    assert_eq!((text(&app, LapTime), text(&app, LapDist)), ("01:00".into(), "0.48".into()));
    assert_eq!(text(&app, MaxSpeed), "28.8");
    let ascent = app.ride.ascent_m();
    assert!((150..=157).contains(&ascent), "0.2 m/s for 780 s: {ascent} m");
    assert_consistent(&sim, &app);
    sim.save("ride_page2");
    // stopped: auto-pause, the timer stops
    sim.gnss.fix = Some(0);
    run(&mut sim, &mut app, 3000);
    let timer = app.ride.timer_s();
    run(&mut sim, &mut app, 30_000);
    assert_eq!((app.ride.timer_s(), app.strip.rec), (timer, Rec::Paused), "auto-paused");
    assert_consistent(&sim, &app);
    sim.save("ride_autopause");
    sim.gnss.fix = Some(8000);
    run(&mut sim, &mut app, 10_000);
    assert!(app.ride.timer_s() >= timer + 8 && app.strip.rec == Rec::Running, "moving again");
    app.press(&mut sim, Press::Short(Key::Start));
    let (timer, dist) = (app.ride.timer_s(), app.ride.dist_m());
    run(&mut sim, &mut app, 10_000);
    assert_eq!((app.ride.timer_s(), app.ride.dist_m(), app.strip.rec), (timer, dist, Rec::Paused), "paused");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    assert!(app.ride.timer_s() > timer, "resumed");
    app.press(&mut sim, Press::Long(Key::Start));
    assert_eq!(app.ride.state(), crate::ride::State::Running, "a long Start does not end a running ride");
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    assert_eq!((app.ride.state(), app.strip.rec), (crate::ride::State::Idle, Rec::Idle), "ended");
    assert!(app.ride.dist_m() > 2000, "the totals stay on screen");
    assert_consistent(&sim, &app);
    sim.save("ride_ended");
}

/// A dual-sided power meter: 300 W, 85 rpm, the right pedal 48 % of the power.
fn power_lr(_: u64) -> [u8; 8] {
    [0x10, 1, 0x80 | 48, 85, 0, 0, 0x2c, 0x01]
}

/// The derived fields (power.rs, ride.rs): a ride at 300 W on the GPS route (8 m/s, climbing 0.2 m/s) with a
/// strap and a dual-sided power meter, at the default FTP (200 W), weight (75 kg) and W' (20 kJ). After two
/// minutes the fields show the values worked out by hand, W' balance in its draining colour, and the screen
/// agrees with the screenshot.
#[test]
fn derived_power_and_climb_fields() {
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    sim.gnss.alt = Some(climb);
    sim.bus.baro_alt = Some(climb); // the barometer climbs with the receiver
    sim.gnss.fix = Some(8000);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr_ramp, network: 0, ack: &[] });
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_lr, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 6, fields: [Power3s, Power10s, Power30s, Np, If, Tss] }.into();
    app.pages[1] = PageCfg { layout: 6, fields: [Kj, Kcal, Wkg, Wbal, MaxPower, MaxHr] }.into();
    app.pages[2] = PageCfg { layout: 6, fields: [AvgCadence, Balance, Vam, Grade, Timer, Time] }.into();
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 5000);
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    assert_eq!((text(&app, Np), text(&app, Wbal), text(&app, Kj)), ("--".into(), "--".into(), "0".into()));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 120_000);
    let t = app.ride.timer_s();
    assert!((119..=121).contains(&t), "timer {t}");
    // 300 W steady: every average 300, NP 300, IF 300 / 200, TSS t x 300^2 x 100 / (200^2 x 3600) = 7.5 at 120 s
    for f in [Power3s, Power10s, Power30s, Np, MaxPower] {
        assert_eq!(text(&app, f), "300", "{f:?}");
    }
    assert_eq!((text(&app, If), text(&app, Tss)), ("1.50".into(), "7".into()));
    // 300 W for t s: 0.3 t kJ, and kcal = kJ / (4.184 x 0.24); 300 / 75 = 4.0 W/kg
    let kj = 3 * t / 10;
    assert_eq!((text(&app, Kj), text(&app, Kcal)), (format!("{kj}"), format!("{}", kj * 1000 * 100 / 100_416)));
    assert_eq!(text(&app, Wkg), "4.0");
    // 100 W over CP for every second since Start (the first seconds count before the GPS speed moves the timer)
    let wbal = app.power.wbal_j().unwrap();
    assert!((7900..=8100).contains(&wbal), "W' bal {wbal} J: 20 kJ - 120 s x 100 W");
    assert_eq!(text(&app, Wbal), format!("{}.{}", wbal / 1000, wbal % 1000 / 100));
    assert_eq!(app.field_color(Wbal), crate::ride::ZONE_COLORS[4], "40 %: orange");
    assert_eq!(app.field_color(Np), crate::ride::power_color(6), "150 % of FTP");
    assert_eq!((text(&app, AvgCadence), text(&app, Balance)), ("85".into(), "52/48".into()));
    let max_hr = app.ride.max_hr().unwrap();
    assert_eq!(text(&app, MaxHr), format!("{max_hr}"));
    assert!(max_hr >= 200, "the strap's ramp, 90 bpm up");
    // 0.2 m/s up at 8 m/s: 720 m/h, 2.5 % (GPS altitude in whole fixes: a little either way)
    let (vam, grade) = (app.ride.vam().unwrap(), app.ride.grade_x10().unwrap());
    assert!((700..=740).contains(&vam) && (24..=26).contains(&grade), "VAM {vam}, grade {grade}");
    assert_eq!((text(&app, Vam), text(&app, Grade)), (format!("{vam}"), format!("2.{}", grade % 10)));
    assert_consistent(&sim, &app);
    sim.save("power_fields");
    for name in ["power_fields2", "power_fields3"] {
        app.press(&mut sim, Press::Short(Key::Page));
        run(&mut sim, &mut app, 3000);
        assert_consistent(&sim, &app);
        sim.save(name);
    }
    // a new ride: the totals and W' start again
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    app.press(&mut sim, Press::Short(Key::Back)); // the card, then the overview
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    assert!(app.power.wbal_j().unwrap() > 19_000 && app.power.kj() < 2, "a new ride");
}

/// Weight and W' from the menu, kept in RTC backup register 16 through a restart; the other settings stay in
/// register 11.
#[test]
fn weight_and_wprime_from_the_menu() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::Weight));
    app.press(&mut sim, Press::Short(Key::Start));
    for _ in 0..7 {
        app.press(&mut sim, Press::Short(Key::Page)); // 75 -> 68 kg
    }
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Page)); // W' row
    app.press(&mut sim, Press::Short(Key::Start));
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Lap)); // 20.0 -> 21.5 kJ
    }
    assert_eq!(main_row(&app, row_of(menu::Row::Set(Setting::WPrime))), "W' < 21.5 >");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(main_row(&app, row_of(menu::Row::Set(Setting::Weight))), "WEIGHT 68 KG");
    assert_consistent(&sim, &app);
    sim.save("power_settings");
    assert_eq!(sim.backup[16] >> 24, 0xc1, "tagged");
    let again = App::start(&mut sim, None);
    assert_eq!((again.settings.weight_kg, again.settings.wprime_j, again.settings.ftp), (68, 21_500, 200));
}

/// Power as ridden: a two-minute swell from 140 to 320 W, +-30 W of noise from one second to the next.
fn power_noisy(ms: u64) -> [u8; 8] {
    let s = ms / 1000;
    let swell = (s % 120).min(120 - s % 120) as u16;
    let noise = (s.wrapping_mul(2_654_435_761) >> 7) % 61;
    let [p0, p1] = (110 + 3 * swell + noise as u16).to_le_bytes();
    [0x10, 1, 0xff, 85, 0, 0, p0, p1]
}

/// Power and heart rate that change at every broadcast (4 Hz, the sensors' own rate): a power meter's and a strap's
/// real readings move all the time, the test sensors above only once a second.
fn power_fast(ms: u64) -> [u8; 8] {
    let k = ms / 250;
    let swell = (k / 4 % 120).min(120 - k / 4 % 120) as u16;
    let noise = (k.wrapping_mul(2_654_435_761) >> 7) % 61;
    let [p0, p1] = (110 + 3 * swell + noise as u16).to_le_bytes();
    [0x10, 1, 0xff, 85, 0, 0, p0, p1]
}
fn hr_fast(ms: u64) -> [u8; 8] {
    [4, 0, 0, 0, 0, 0, 0, 120 + ((ms / 250).wrapping_mul(2_654_435_761) >> 9) as u8 % 9]
}

/// The graph field: power in its zones' colours, one pixel column per sample, drawn by differences as it
/// scrolls (the screenshot agrees). Prints the pixels a second it costs.
#[test]
fn graph_field() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_noisy, network: 0, ack: &[] });
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 3, fields: [Graph, MaxSpeed, HrGraph, Timer, Timer, Timer] }.into();
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 600_000);
    assert_consistent(&sim, &app);
    let (px0, win0) = (sim.pixels, sim.windows);
    run(&mut sim, &mut app, 120_000);
    let (px_s, win_s) = ((sim.pixels - px0) / 120, (sim.windows - win0) / 120);
    std::println!("graph field (240 x 135), noisy power: {px_s} px/s in {win_s} windows/s");
    assert_consistent(&sim, &app);
    let (x, y, cols, h) = crate::ui::graph_area(crate::ui::field_box(3, crate::ui::RIDE_H, 0).unwrap());
    let bottom = &sim.screen[((y + h - 1) * W + x) as usize..][..cols as usize];
    assert!(bottom.iter().all(|&c| crate::ride::ZONE_COLORS.contains(&c)), "a filled area, no gaps");
    sim.save("graph");
}

/// A tap on a graph shows the next plot (power, heart rate, W' balance, DFA alpha1, elevation) until the unit is
/// switched off, the page as kept unchanged; a hold switches every graph between the last minutes and the whole
/// ride. Only while the ride is paused (or none runs): while it records, a tap or a hold on a page does nothing (UI
/// review step 7: a wet glove changed the page for good, or opened the menu over the ride).
#[test]
fn graph_plots_by_tap_and_span_by_hold() {
    let taps = [400_000u64, 401_000, 402_000, 403_000];
    let (hold, riding) = (405_000u64, [420_000u64, 422_000]);
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| {
        let tap = taps.iter().chain(&riding[..1]).any(|&t| (t..t + 80).contains(&ms));
        let held = (hold..hold + 900).contains(&ms) || (riding[1]..riding[1] + 900).contains(&ms);
        (tap || held).then_some((120, 100))
    });
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_noisy, network: 0, ack: &[] });
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 2, fields: [Graph, HrGraph, Timer, Timer, Timer, Timer] }.into();
    save_page(&mut sim, 0, &app.pages[0]);
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start)); // a ride: W' balance runs
    sim.gnss.fix = Some(7000);
    let until = taps[0] - 2000 - sim.ms;
    run(&mut sim, &mut app, until);
    app.press(&mut sim, Press::Short(Key::Start)); // paused: the taps act
    let mut shots = std::vec::Vec::new();
    let shown = |app: &App| app.widgets().next().map(|w| w.1);
    for (k, want) in [HrGraph, WbalGraph, DfaGraph, AltGraph].into_iter().enumerate() {
        let until = taps[k] + 500 - sim.ms;
        run(&mut sim, &mut app, until);
        assert_eq!((shown(&app), app.pages()[0].fields[0]), (Some(Widget::Field(want)), Graph), "tap {k}");
        assert_consistent(&sim, &app);
        shots.push(std::format!("graph_{k}"));
        sim.save(&shots[k]);
    }
    assert_eq!(PageCfg::from_words([sim.backup[20], sim.backup[21]]).unwrap().fields[0], Graph, "not kept");
    let until = hold + 1500 - sim.ms;
    run(&mut sim, &mut app, until);
    assert!(app.graph_whole, "the hold");
    assert_eq!(app.screen(), Screen::Ride(0), "on a graph, a hold is not the menu");
    assert_consistent(&sim, &app);
    sim.save("graph_whole");
    app.press(&mut sim, Press::Short(Key::Start)); // recording again: touch only turns pages
    let until = riding[1] + 1500 - sim.ms;
    run(&mut sim, &mut app, until);
    assert_eq!((shown(&app), app.graph_whole), (Some(Widget::Field(AltGraph)), true), "the tap and the hold ignored");
    assert_eq!(app.screen(), Screen::Ride(0), "no menu by a hold while recording");
    run(&mut sim, &mut app, 60_000);
    assert_consistent(&sim, &app);
}

/// After a fault (hw.rs marks it in backup register 29 and resets): a notice at start, and the count cleared after
/// a minute of running.
#[test]
fn restart_after_a_fault() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[FAULT_BKP] = 0xfa00_0001;
    let mut app = App::start(&mut sim, None);
    assert_eq!(app.notices[0].as_str(), "09:00 FAULT RESET 1");
    run(&mut sim, &mut app, u64::from(FAULT_CLEAR_MS) - 2000);
    assert_eq!(sim.backup[FAULT_BKP], 0xfa00_0001, "kept a while");
    run(&mut sim, &mut app, 4000);
    assert_eq!(sim.backup[FAULT_BKP], 0, "a minute of running: cleared");
}

/// A speed sensor paired but silent (a dead battery) does not leave the ride without speed: the GPS's is used.
#[test]
fn silent_speed_sensor_falls_back_to_gps() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Speed.backup()] = SensorId { device: 99, trans: 1 }.to_word(); // paired, never heard
    sim.gnss.route = Some(ride_route);
    let mut app = App::start(&mut sim, Some(KEY));
    sim.gnss.fix = Some(8000);
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.live.get(Metric::Speed), Some(288), "8 m/s from the GPS");
}

/// During a ride, holding Back once only warns (switching off would lose it); a second hold within 10 s
/// switches off. With no ride, the first hold does.
#[test]
fn off_during_a_ride_asks_twice() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Long(Key::Back));
    assert!(!sim.standby && app.banner.as_str() == "HOLD BACK AGAIN TO SWITCH OFF", "warned");
    run(&mut sim, &mut app, 12_000);
    app.press(&mut sim, Press::Long(Key::Back));
    assert!(!sim.standby, "the warning ran out: warned again");
    app.press(&mut sim, Press::Long(Key::Back));
    assert!(sim.standby, "the second hold");
    assert!(app.busy() || app.ride.state() != crate::ride::State::Idle);
}

/// A pass that takes longer than a second (an eMMC or radio wait) loses no ride time: the ride counts every
/// second gone, up to MAX_CATCH_UP (more is the clock being set).
#[test]
fn long_pass_loses_no_ride_time() {
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    sim.gnss.fix = Some(8000);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 3000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 10_000);
    let t = app.ride.timer_s();
    sim.delay_ms(3000); // a 3 s pass
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.ride.timer_s(), t + 4, "the 3 s of the long pass and the next second");
    sim.delay_ms(30_000); // the clock jumps (set from the GPS): counted up to MAX_CATCH_UP only
    run(&mut sim, &mut app, 100);
    assert!(app.ride.timer_s() <= t + 4 + u32::from(MAX_CATCH_UP) + 1);
}

/// Idle is free, and no pass stalls the loop: on the overview with nothing changing, nearly every pass sends
/// nothing to the panel (the clock's minute aside), and on a ride page while riding no pass costs the panel more
/// than PASS_BUS_MS at the panel's speed (about 2 us a pixel, 20 us a window at 8 MHz: a model, measured on the
/// unit as one full map of 72 K pixels in about 146 ms).
#[test]
fn idle_is_free_and_no_pass_stalls() {
    const PASS_BUS_MS: u64 = 60;
    let bus_us = |px: u64, win: u64| px * 2 + win * 20;
    // the overview, idle
    let mut sim = Sim::new(9, 0, 30);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 5000);
    let (mut busy, mut passes) = (0, 0);
    for _ in 0..1200 {
        let (px, win) = (sim.pixels, sim.windows);
        run(&mut sim, &mut app, u64::from(POLL_MS));
        passes += 1;
        busy += u32::from(sim.pixels != px || sim.windows != win);
    }
    // the strip's GPS label blinks while it searches (once a second), nothing else: a pass in 20 at most, plus
    // a few for the minute's digits and the temperature
    assert!(busy <= passes / 20 + 3, "{busy} of {passes} passes drew on an idle overview");
    // a ride page while riding (values change every second): no pass over PASS_BUS_MS of panel time
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_noisy, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 10_000);
    let mut worst = 0;
    for _ in 0..1200 {
        let (px, win) = (sim.pixels, sim.windows);
        run(&mut sim, &mut app, u64::from(POLL_MS));
        worst = worst.max(bus_us(sim.pixels - px, sim.windows - win));
    }
    std::println!("idle overview: {busy} of {passes} passes drew; ride page: worst pass {} ms of panel", worst / 1000);
    assert!(worst / 1000 <= PASS_BUS_MS, "a pass took {} ms of panel time", worst / 1000);
}

/// Reminders: EAT after 5 minutes of riding, said in a banner with three beeps that do not stop the loop;
/// nothing while paused; set from the ALERTS menu and kept.
#[test]
fn eat_reminder_on_a_ride() {
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Alerts));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Alerts, 0));
    app.press(&mut sim, Press::Short(Key::Start)); // EAT: edit
    app.press(&mut sim, Press::Short(Key::Lap)); // 5
    app.press(&mut sim, Press::Short(Key::Start)); // kept
    assert_eq!(Settings::from_word(0).with_alerts_word(sim.backup[28]).eat_min, 5);
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Back)); // SETTINGS, MENU, out
    }
    sim.gnss.fix = Some(8000);
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start)); // the ride
    let tones0 = sim.tones.len();
    let steps0 = sim.ms;
    run(&mut sim, &mut app, 299_000);
    assert_eq!(sim.tones.len(), tones0, "not yet");
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.banner.as_str(), "EAT REMINDER");
    let beeps: std::vec::Vec<u32> = sim.tones[tones0..].to_vec();
    assert_eq!(beeps, [2500, 0, 2500, 0, 2500, 0], "three beeps, then off");
    assert!(sim.ms - steps0 >= 300_000, "{}", sim.ms - steps0);
    assert_consistent(&sim, &app);
    sim.save("eat_reminder");
    // paused on the minute: nothing more
    app.press(&mut sim, Press::Short(Key::Start));
    let n = sim.tones.len();
    run(&mut sim, &mut app, 120_000);
    assert_eq!(sim.tones.len(), n, "paused: no reminder");
}

/// The synthetic card plus big files: a 2 MB GPX course, a ride of about 1.5 MB, and 120 days of planned workouts
/// of 9 KB each (too long to be workouts, but a reader that does not look first reads 8 KB of each).
fn big_card() -> Vec<u8> {
    use crate::fat::{Fat, tests::Ram, write::dos_time};
    let mut d = Ram(crate::fat::tests::image(), 0);
    let mut fat = Fat::mount(&mut d).unwrap();
    let t = dos_time((2026, 10, 20, 8, 0, 0));
    let put = |fat: &mut Fat, d: &mut Ram, dir: crate::fat::File, name: &str, bytes: &[u8]| {
        let mut padded = bytes.to_vec();
        padded.resize(bytes.len().div_ceil(512) * 512, 0);
        let mut h = fat.create(d, dir, name, t).unwrap();
        fat.write_sectors(d, &mut h, 0, &padded).unwrap();
        fat.set_size(d, &mut h, bytes.len() as u32, t).unwrap();
    };
    let mut gpx = b"<gpx><trk><trkseg>\n".to_vec();
    for k in 0.. {
        if gpx.len() > 2 << 20 {
            break;
        }
        let line = format!("<trkpt lat=\"{:.6}\" lon=\"0.001\"><ele>{}</ele></trkpt>\n", f64::from(k) * 1e-4, k % 50);
        gpx.extend_from_slice(line.as_bytes());
    }
    gpx.extend_from_slice(b"</trkseg></trk></gpx>\n");
    let dir = fat.open(&mut d, "Courses").unwrap();
    put(&mut fat, &mut d, dir, "Big.gpx", &gpx);
    let t0 = crate::fit::fit_time((2026, 10, 21, 8, 0, 0));
    let mut ride = Vec::new();
    let mut w = crate::fit::Writer::begin(&mut ride, t0).unwrap();
    for k in 0..50_000u32 {
        let s = crate::fit::Sample {
            time: t0 + k,
            pos: Some((k as i32 * 10, 1000)),
            alt_mm: Some(100_000),
            speed_mm_s: Some(8000),
            hr: Some(140),
            cadence: Some(90),
            power: Some(200),
            temp_c: Some(20),
            ..Default::default()
        };
        w.record(&mut ride, &s).unwrap();
    }
    w.finish(&mut ride, t0 + 50_000).unwrap();
    let dir = fat.open(&mut d, "Activities").unwrap();
    put(&mut fat, &mut d, dir, "261021080000.fit", &ride);
    let cal = fat.open(&mut d, "Workout/calendar").unwrap();
    for k in 0..120u32 {
        let name = format!("2027{:02}{:02}", 1 + k / 28, 1 + k % 28);
        let day = fat.mkdir(&mut d, cal, &name, t).unwrap();
        put(&mut fat, &mut d, day, "plan.fit", &[0x55; 9 * 1024]);
    }
    d.0
}

/// The bytes read by `f`, the most of a pass so far kept in `worst`.
fn pass(
    sim: &mut Sim,
    app: &mut App,
    worst: &mut (u64, &'static str),
    what: &'static str,
    f: impl FnOnce(&mut Sim, &mut App),
) {
    let before = sim.emmc.bytes_read;
    f(sim, app);
    let n = sim.emmc.bytes_read - before;
    if n > worst.0 {
        *worst = (n, what);
    }
}

/// Passes of the main loop for `ms`, each measured as [`pass`].
fn passes(sim: &mut Sim, app: &mut App, worst: &mut (u64, &'static str), what: &'static str, ms: u64) {
    for _ in 0..ms / u64::from(POLL_MS) {
        pass(sim, app, worst, what, |sim, app| app.step(sim));
        sim.delay_ms(POLL_MS);
    }
}

/// Opening COURSES, following a course, a ride's card in HISTORY and WORKOUTS on a card with big files: no pass of
/// the main loop reads more than 300 ms of the unit's measured 2.5 MB/s (read and parsed, [`App::SCAN_IDLE`]); the
/// long reads go on over the passes, and their results come (review 2: each was one pass, up to 4 MB).
#[test]
fn long_reads_spread_over_passes() {
    const MAX: u64 = 2_500_000 * 300 / 1000;
    let card = big_card();
    let mut worst = (0, "");
    let w = &mut worst;
    let fresh = || {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = card.clone();
        let mut app = App::start(&mut sim, None);
        app.set_course_memory(crate::storage::course::tests::memory());
        app.press(&mut sim, Press::Long(Key::Page));
        (sim, app)
    };
    // COURSES: the list at once, the lengths over the passes
    let (mut sim, mut app) = fresh();
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Courses));
    pass(&mut sim, &mut app, w, "courses", |sim, app| app.press(sim, Press::Short(Key::Start)));
    passes(&mut sim, &mut app, w, "course lengths", 5000);
    let list = &app.courses.as_deref().unwrap().list;
    let big = (0..list.len()).find(|&i| list.get(i).unwrap().name() == "BIG").unwrap();
    let (_, value, _) = app.menu_row(Menu::Courses, big).unwrap();
    let listed = std::string::String::from(value.as_str());
    // following the big course
    for _ in 0..big {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    pass(&mut sim, &mut app, w, "course load", |sim, app| app.press(sim, Press::Short(Key::Start)));
    passes(&mut sim, &mut app, w, "course load", 5000);
    assert_eq!(app.screen(), Screen::Ride(crate::COURSE), "followed: the course page");
    let len = app.courses.as_deref().unwrap().course.len_m();
    std::println!("big course: {listed} listed, {len} m followed");
    let km: f64 = listed.trim_end_matches(" KM").parse().unwrap();
    assert!(len > 300_000 && (km * 1000.0 - f64::from(len)).abs() < 100.0, "{listed}: {len} m");
    // HISTORY: the scan, then the big ride's card
    let (mut sim, mut app) = fresh();
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::History));
    app.press(&mut sim, Press::Short(Key::Start));
    while app.stock.status() == crate::stock::Status::Running {
        passes(&mut sim, &mut app, w, "scan", 1000);
        assert!(sim.ms < 300_000);
    }
    let n = (0..app.stock.index.len()).find(|&i| app.stock.index.get(i).unwrap().start.2 == 21).unwrap();
    for _ in 0..n + 1 {
        // FITNESS first
        app.press(&mut sim, Press::Short(Key::Page));
    }
    pass(&mut sim, &mut app, w, "ride card", |sim, app| app.press(sim, Press::Short(Key::Start)));
    passes(&mut sim, &mut app, w, "ride card", 5000);
    assert_eq!((app.screen(), app.past.card.title.as_str()), (Screen::Past, "21 OCT 08:00"));
    assert!(app.past.bytes > 1 << 20, "{}", app.past.bytes);
    // WORKOUTS
    let (mut sim, mut app) = fresh();
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Workouts));
    pass(&mut sim, &mut app, w, "workouts", |sim, app| app.press(sim, Press::Short(Key::Start)));
    assert_eq!(app.screen(), Screen::Menu(Menu::Workouts, 0));
    assert_eq!(app.menu_row(Menu::Workouts, 0).unwrap().0.as_str(), "FRI 23 OCT", "today's still first");
    std::println!("the most read in a pass: {} KB ({})", worst.0 / 1024, worst.1);
    assert!(worst.0 <= MAX, "{} B in one pass ({})", worst.0, worst.1);
}

/// Banners said in the same second both show (review of the turn cues: a workout step's banner was lost under a
/// cue): the higher kind first (a turn now over a step, an alarm over a turn), the other waits and follows; a lower
/// one said meanwhile does not push out the one waiting; then the page is whole again.
#[test]
fn banners_wait_their_turn() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1000);
    let shown = |sim: &mut Sim, app: &mut App, ms| {
        run(sim, app, ms);
        assert_consistent(sim, app);
        app.banner_ms.map(|_| std::string::String::from(app.banner.as_str()))
    };
    app.say_as("2/6 200-220W", Banner::Ride); // the workout's step and a turn, in one second
    app.say_as("< LEFT NOW", Banner::Now);
    assert_eq!(shown(&mut sim, &mut app, 200).as_deref(), Some("< LEFT NOW"), "the turn first");
    app.say("LAP");
    assert_eq!(shown(&mut sim, &mut app, 200).as_deref(), Some("< LEFT NOW"), "not covered by LAP");
    assert_eq!(shown(&mut sim, &mut app, BANNER_MS.into()).as_deref(), Some("2/6 200-220W"), "then the step");
    assert_eq!(shown(&mut sim, &mut app, BANNER_MS.into()), None, "LAP gave way to the step: nothing after");
    app.say_as("< RIGHT 200 M", Banner::Cue);
    app.alarm("OFF COURSE");
    assert_eq!(shown(&mut sim, &mut app, 200).as_deref(), Some("OFF COURSE"), "an alarm first");
    assert_eq!(shown(&mut sim, &mut app, BANNER_MS.into()).as_deref(), Some("< RIGHT 200 M"), "then the cue");
}

/// A banner replaced by a newer one of its kind does not come back, and a cue is only good for its moment
/// (2026-10-25 review: LEFT NOW, then RIGHT 50 M, then RIGHT NOW showed the stale LEFT NOW again after the right
/// turn; LAP said twice showed 8 s).
#[test]
fn banners_replaced_do_not_come_back() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1000);
    let shown = |sim: &mut Sim, app: &mut App, ms| {
        run(sim, app, ms);
        assert_consistent(sim, app);
        app.banner_ms.map(|_| std::string::String::from(app.banner.as_str()))
    };
    app.say_as("< LEFT NOW", Banner::Now);
    assert_eq!(shown(&mut sim, &mut app, 1000).as_deref(), Some("< LEFT NOW"));
    app.say_as("> RIGHT 50 M", Banner::Cue); // waits under the turn now
    assert_eq!(shown(&mut sim, &mut app, 1000).as_deref(), Some("< LEFT NOW"));
    app.say_as("> RIGHT NOW", Banner::Now); // replaces LEFT NOW; the cue ahead waiting is past
    assert_eq!(shown(&mut sim, &mut app, 200).as_deref(), Some("> RIGHT NOW"));
    assert_eq!(shown(&mut sim, &mut app, BANNER_MS.into()), None, "nothing stale after the turn");
    // a cue ahead pushed off by the turn now does not come back either
    app.say_as("> RIGHT 200 M", Banner::Cue);
    app.say_as("> RIGHT NOW", Banner::Now);
    assert_eq!(shown(&mut sim, &mut app, BANNER_MS.into()), None, "not RIGHT 200 M after RIGHT NOW");
    // the same kind: the newer replaces, the older does not follow
    app.say("LAP");
    app.say("LAP");
    assert_eq!(shown(&mut sim, &mut app, u64::from(BANNER_MS) - 200).as_deref(), Some("LAP"));
    assert_eq!(shown(&mut sim, &mut app, 400), None, "LAP twice is up 4 s, not 8");
    app.say_as("2/6 200-220W", Banner::Ride);
    app.say_as("3/6 150W", Banner::Ride);
    assert_eq!(shown(&mut sim, &mut app, 200).as_deref(), Some("3/6 150W"));
    assert_eq!(shown(&mut sim, &mut app, BANNER_MS.into()), None, "the older step does not follow");
}

/// TOUCH in DEVICE: Start turns the touch panel off (kept in register 11's bit 23), and then no gesture acts, a tap
/// on the gear included; the keys turn it on again.
#[test]
fn touch_can_be_switched_off() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::Touch));
    assert_eq!(main_row(&app, row_of(menu::Row::Set(Setting::Touch))), "TOUCH ON");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(main_row(&app, row_of(menu::Row::Set(Setting::Touch))), "TOUCH OFF");
    assert!(!Settings::from_word(sim.backup[WHEEL_BKP]).touch, "kept");
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
    sim.save("touch_off");
    let shown = app.screen();
    app.input(&mut sim, Input::Touch(Gesture::Tap(10, (ui::PAGE_Y + 8) as u16))); // the "<"
    app.input(&mut sim, Input::Touch(Gesture::Swipe(crate::touch::Dir::Right)));
    assert_eq!(app.screen(), shown, "touch does nothing");
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Back));
    }
    let (gx, gy) = (ui::GEAR_AT.0 as u16 + 9, ui::GEAR_AT.1 as u16 + 9);
    app.input(&mut sim, Input::Touch(Gesture::Tap(gx, gy)));
    assert_eq!(app.screen(), Screen::Main, "not the gear either");
    app.press(&mut sim, Press::Long(Key::Page)); // the keys: back to TOUCH, on again
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::Touch));
    app.press(&mut sim, Press::Short(Key::Start));
    assert!(Settings::from_word(sim.backup[WHEEL_BKP]).touch, "on again");
}

/// IMPORT asks first: Start shows "SURE?" in the row and asks in the banner; any other input drops it (Page and
/// back); Start twice imports, and a tap on the row asking does too.
#[test]
fn import_asks_first() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    let import = row_of(menu::Row::Stock(menu::StockRow::Import));
    app.open(Screen::Menu(Menu::System, import));
    app.show(&mut sim);
    app.settings.ftp = 55; // stock's is another: an import shows
    app.input(&mut sim, Input::Select);
    assert_eq!((app.menu_row(Menu::System, import).unwrap().1.as_str(), app.settings.ftp), ("SURE?", 55));
    assert_eq!(app.banner.as_str(), "IMPORT SETTINGS?\nSTART: YES BACK: NO");
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
    sim.save("import_sure");
    app.input(&mut sim, Input::Prev);
    app.input(&mut sim, Input::Next);
    assert_eq!(app.menu_row(Menu::System, import).unwrap().1.as_str(), "FROM STOCK", "dropped by Page");
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
    app.input(&mut sim, Input::Select);
    app.input(&mut sim, Input::Select);
    assert_ne!(app.settings.ftp, 55, "Start twice: imported");
    app.settings.ftp = 55;
    let y = (ui::LIST_Y + (import % ui::MENU_ROWS) as u32 * ui::MENU_ROW + 20) as u16;
    app.input(&mut sim, Input::Select); // asks
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, y))); // the row chosen and asking: a tap confirms
    assert_ne!(app.settings.ftp, 55, "imported again");
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
}

/// A workout's step tone and an alert in the same pass: one tone player, so both play whole, one after the other
/// (review 2: two players cut each other off).
#[test]
fn tones_do_not_cut_each_other() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    let t0 = sim.tones.len();
    app.workout_event(workout::Event::Done);
    app.alert = Some(alerts::Alert::Eat);
    run(&mut sim, &mut app, 2000);
    assert_eq!(sim.tones[t0..], [2500, 0, 2500, 0, 2500, 0, 2500, 0], "the step's tone, then the alert's three");
}

/// Performance budgets: in steady state, each kind of screen sends at most so many pixels and panel windows a
/// second (every pixel is a SPI transfer and CPU time, AGENTS.md "redraw only what changes"). A change that
/// redraws more than it should fails here before it costs battery on the unit. Measured 2026-10-24, budgets with
/// a margin; the numbers are printed (`cargo test-host perf_budgets -- --nocapture`).
#[test]
fn perf_budgets() {
    use Field::*;
    // a ride on the GPS route, with a power meter and a strap, from start-up
    let ride = |pages: [crate::page::Page; 3], screen: usize, zoom: Option<u32>, fast: bool| {
        let mut sim = Sim::new(9, 0, 0);
        sim.gnss.route = Some(ride_route);
        let course = screen == crate::COURSE || pages.iter().any(|p| p.fields.contains(&Profile));
        if course {
            sim.emmc.image = crate::fat::tests::image();
            sim.gnss.route = Some(course_route);
        }
        sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
        type Broadcast = fn(u64) -> [u8; 8]; // a sensor's page at a time
        let (power_fn, hr_fn): (Broadcast, Broadcast) = if fast { (power_fast, hr_fast) } else { (power_noisy, hr72) };
        sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_fn, network: 0, ack: &[] });
        sim.backup[Kind::Hr.backup()] = STRAP.to_word();
        sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr_fn, network: 0, ack: &[] });
        let mut app = App::start(&mut sim, Some(KEY));
        app.pages[..3].copy_from_slice(&pages);
        if course {
            app.set_course_memory(crate::storage::course::tests::memory());
            app.open_courses(&mut sim);
            app.follow_course(hill(&app));
            for _ in 0..100 {
                app.step(&mut sim); // read over a few passes
            }
            assert!(app.courses.as_deref().unwrap().on.is_some());
        }
        sim.gnss.fix = Some(8000);
        if screen != PAGES {
            app.press(&mut sim, Press::Short(Key::Start));
            app.screen = Screen::Ride(screen); // the page measured (a course loaded opens the course page)
            app.map_zoom = zoom;
            app.show(&mut sim);
        }
        run(&mut sim, &mut app, 120_000); // settled: the map fitted, the graph full
        let (px, win, t0) = (sim.pixels, sim.windows, sim.ms);
        run(&mut sim, &mut app, 300_000);
        let s = (sim.ms - t0) / 1000;
        assert_consistent(&sim, &app);
        ((sim.pixels - px) / s, (sim.windows - win) / s)
    };
    let p = |layout, f: [Field; 6]| crate::page::Page::from(PageCfg { layout, fields: f });
    // pages made in the editor: eight values in four rows of two; a graph over two rows of two values
    let eight = crate::page::Page::preset(14, [Speed, Hr, Power, Cadence, Dist, Timer, Power3s, Np]).unwrap();
    let mut made = crate::page::Page::preset(1, [Graph, Time, Time, Time, Time, Time, Time, Time]).unwrap();
    made = made.split_down(0, Power3s).and_then(|p| p.resize(0, true)).and_then(|p| p.split_across(1, Hr)).unwrap();
    made = made.split_down(1, Cadence).and_then(|p| p.split_across(3, Timer)).and_then(|p| p.resize(3, true)).unwrap();
    let values = p(6, [Speed, Hr, Power, Power3s, Dist, Timer]);
    let graph = p(2, [Graph, HrGraph, Time, Time, Time, Time]);
    let map = p(1, [Map, Time, Time, Time, Time, Time]);
    let profile = p(4, [Profile, ToGo, Climb, Time, Time, Time]);
    type Case = (&'static str, [crate::page::Page; 3], usize, Option<u32>, bool, (u64, u64));
    let cases: [Case; 11] = [
        // measured 53 px/s, 0 windows/s (the clock's digits once a minute)
        ("overview", [values, graph, map], PAGES, None, false, (150, 5)),
        // made in the editor: 2162, 6 (7101, 10 with the zone fills, 8959 at VALUE_MARGIN 2): eight values at 3x in
        // 120 x 74 boxes, the readings once a second, the zone a bar
        ("eight values", [eight, graph, map], 0, None, false, (3_500, 12)),
        // 1537, 20 (3581, 22 with the zone fills): a graph over four values (the second row of values taller)
        ("graph over four", [made, values, map], 0, None, false, (2_500, 30)),
        // 2037, 6: the changed digits only, once a second, and a zone bar (about 1,000 px) at a zone change, held
        // three seconds. 8070, 10 with the zone fills (a box of ~12,000 px at each change, the noisy power's 3 s
        // average crossing an edge about every 4 s), 6538 before them
        ("six values", [values, graph, map], 0, None, false, (3_000, 12)),
        // the same page with a power meter and a strap that change at every broadcast (4 Hz, like the real ones):
        // 2198, 6 with the readings shown once a second (taken in the second's tick); 11339, 20 when each broadcast
        // was drawn and the zones filled the boxes
        ("six values, 4 Hz sensors", [values, graph, map], 0, None, true, (3_000, 12)),
        // 646, 17: the changed columns of two graphs, a step of 5 bars every 10 s
        ("two graphs", [graph, values, map], 0, None, false, (1_000, 30)),
        // 1494, 33: the track's new segments and the arrow, refits included (1260 before the fit kept the arrow
        // clear of the zoom buttons, 1211 without them)
        ("map", [map, values, graph], 0, None, false, (1_800, 50)),
        // 584, 3: on the course, the profile's rider column (a whole profile at each eighth), two values, and the
        // hill's left turn said twice (2026-10-25: 456 before the cues; a banner and the page under it again, about
        // 19 000 px each)
        ("course profile", [profile, values, map], 0, None, false, (800, 10)),
        // 1999, 9: the course page (COURSE_PAGE): the profile's rider column, the distances, the timer's seconds,
        // NEXT TURN's last digit (1548 before it), the turn's two banners
        ("course page", [values, graph, map], crate::COURSE, None, false, (2_600, 20)),
        // 4563, 44: zoomed in at 1.5 m a pixel (map.rs MIN_LEVEL), following the rider: a whole map at each
        // re-centre (about every 26 s at 8 m/s)
        ("map zoomed 1.5 m", [map, values, graph], 0, Some(crate::map::MIN_LEVEL), false, (6_000, 60)),
        // 3470, 42: at 2 m a pixel, a re-centre about every 35 s
        ("map zoomed 2 m", [map, values, graph], 0, Some(2), false, (4_500, 60)),
    ];
    let mut over = std::vec::Vec::new();
    for (name, pages, screen, zoom, fast, (px_max, win_max)) in cases {
        let (px, win) = ride(pages, screen, zoom, fast);
        std::println!("{name}: {px} px/s, {win} windows/s (budget {px_max}, {win_max})");
        if px > px_max || win > win_max {
            over.push(std::format!("{name}: {px} px/s, {win} windows/s over {px_max}, {win_max}"));
        }
    }
    assert!(over.is_empty(), "{over:?}");
}

/// The synthetic hill (tools/fat_image.py: 2 km east on the flat at 100 m, 2 km north up to 220 m, 1 km east)
/// ridden at 8 m/s from its start, with a detour 150 m south from 60 s to 95 s.
fn course_route(ms: u64) -> (i32, i32, i32) {
    let (s, t) = (ms as f64 * 0.008, ms / 1000);
    let (x, mut y, heading) = match s {
        s if s < 2000.0 => (-2000.0 + s, -1500.0, 90.0),
        s if s < 4000.0 => (0.0, -3500.0 + s, 0.0),
        s => (s - 4000.0, 500.0, 90.0),
    };
    if (60..95).contains(&t) {
        y -= 150.0;
    }
    let m_per_unit = 0.011_132; // metres per 1e-7 degree, as map.rs (at 0 N the same east and north)
    ((y / m_per_unit) as i32, (x / m_per_unit) as i32, (heading * 1e5) as i32)
}

/// The list row of stock's hill (in \PlanTrip: the first HILL listed).
fn hill(app: &App) -> usize {
    let list = &app.courses.as_deref().unwrap().list;
    (0..list.len()).find(|&i| list.get(i).unwrap().name() == "HILL").unwrap()
}

/// A course is read over several passes: left before it is read (Back), the screen the rider went to stays, with
/// COURSE ON said and the course followed; it opens the first ride page only from the list (stock's route details
/// lead to the ride pages too: its manual, "Route Guidance", press to start riding).
#[test]
fn course_read_after_leaving_the_list() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.set_course_memory(crate::storage::course::tests::memory());
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Courses));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    for _ in 0..hill(&app) {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    back(&mut sim, &mut app);
    let left = app.screen();
    assert!(!matches!(left, Screen::Menu(Menu::Courses, _) | Screen::Ride(_)), "{left:?}");
    run(&mut sim, &mut app, 200);
    assert_eq!((app.screen(), app.banner.as_str()), (left, "COURSE ON"));
    assert!(app.courses.as_deref().unwrap().on.is_some());
    assert_consistent(&sim, &app);
}

/// Following a course (tools/fat_image.py's hill, in stock's \PlanTrip): COURSES lists the card's courses
/// with their lengths, Select follows one (the first ride page). On the ride the fields say what is left, the
/// climb ahead and the distance off the course; a detour gives "OFF COURSE" (once, with the alert's beeps) and
/// "BACK ON COURSE"; the climb's start a banner and a beep; then the fields follow the climb. The map draws the
/// course under the track; the profile moves by a column at a time. Following costs a few dozen segments a
/// second.
#[test]
fn course_ride() {
    use Field::*;
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let aid = crate::gnss::Aid { offset_15min: Some(8), pos: Some((0, 0)) };
    [sim.backup[17], sim.backup[18]] = aid.to_words();
    sim.gnss.route = Some(course_route);
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    app.set_course_memory(crate::storage::course::tests::memory());
    app.pages[0] = PageCfg { layout: 6, fields: [ToGo, OffCourse, Climb, ClimbGain, UpToGo, Timer] }.into();
    app.pages[1] = PageCfg { layout: 3, fields: [Profile, Climb, ClimbGain, Time, Time, Time] }.into();
    // the map whole, set by hand (not the default page, which shows the course's fields while one is followed)
    app.pages[2] = PageCfg { layout: 1, fields: [Map, Speed, Time, Time, Time, Time] }.into();
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    let fields = |app: &App| [ToGo, OffCourse, Climb, ClimbGain, UpToGo].map(|f| text(app, f));

    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Courses));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Courses, 0));
    let row = |app: &App, i| {
        app.menu_row(Menu::Courses, i).map(|(l, v, _)| format!("{} {}", l.as_str(), v.as_str())).unwrap_or_default()
    };
    assert_eq!(row(&app, 0).split_once(' ').map(|r| r.1), Some("..."), "listed at once, measured next");
    run(&mut sim, &mut app, 1000);
    let mut rows: Vec<_> = (0..7).map(|i| row(&app, i)).collect();
    rows.sort();
    assert_eq!(
        rows,
        ["", "BROKEN --", "HILL 5.0 KM", "HILL 5.0 KM", "HILL GARMIN 5.0 KM", "TURNS 6.7 KM", "ZIGZAG 6.7 KM"]
    );
    assert_consistent(&sim, &app);
    sim.save("courses_menu");

    for _ in 0..hill(&app) {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.banner.as_str(), "READING COURSE");
    run(&mut sim, &mut app, 200);
    assert_eq!((app.screen(), app.banner.as_str()), (Screen::Ride(crate::COURSE), "COURSE ON"), "the course page");
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0), "then round to page 1");
    assert_eq!(fields(&app), ["--", "--", "--", "--", "--"], "no fix yet");
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start)); // the ride
    run(&mut sim, &mut app, 30_000);
    let s = sim.ms as f64 * 0.008;
    let to_go: f64 = text(&app, ToGo).parse().unwrap();
    assert!((to_go - (5000.0 - s) / 1000.0).abs() <= 0.15, "{to_go} km to go after {s} m");
    assert!(text(&app, OffCourse).parse::<u32>().unwrap() <= 2, "on the course");
    assert_eq!(fields(&app)[2..], ["2.0K|6%", "120", "120"], "the climb ahead: 2 km at 6 %, 120 m");
    assert_consistent(&sim, &app);

    // the detour: off course after 5 s beyond 50 m, said once; back
    let tones = sim.tones.len();
    while sim.ms < 63_500 {
        run(&mut sim, &mut app, 500);
        assert_ne!(app.banner.as_str(), "OFF COURSE", "not before 5 s off: {}", sim.ms);
    }
    run(&mut sim, &mut app, 1500);
    assert_eq!(app.banner.as_str(), "OFF COURSE");
    run(&mut sim, &mut app, 1000);
    assert_eq!(sim.tones[tones..tones + 3], [2500, 0, 2500], "the alert's beeps");
    assert!(text(&app, OffCourse).parse::<u32>().unwrap().abs_diff(150) <= 2 && app.field_color(OffCourse) == ui::RED);
    assert_consistent(&sim, &app);
    while sim.ms < 95_500 {
        run(&mut sim, &mut app, 500);
    }
    run(&mut sim, &mut app, 1500);
    assert_eq!(app.banner.as_str(), "BACK ON COURSE");
    assert!(text(&app, OffCourse).parse::<u32>().unwrap() <= 2 && app.field_color(OffCourse) == ui::WHITE);

    // the climb: a banner at its start, then what is left of it
    while sim.ms < 249_000 {
        run(&mut sim, &mut app, 1000);
        assert!(!app.banner.as_str().starts_with("CLIMB"), "not before the climb: {}", sim.ms);
    }
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.banner.as_str(), "CLIMB 2.0 KM 6%");
    run(&mut sim, &mut app, 47_000); // 400 m up
    let f = fields(&app);
    assert_eq!(f[2], "1.6K|6%", "1.6 km left: {f:?}");
    let m = |i: usize| f[i].parse::<u32>().unwrap();
    assert!(m(3).abs_diff(96) <= 2 && m(4).abs_diff(96) <= 6, "96 m left to climb: {f:?}");
    assert_eq!(app.field_color(Climb), ui::YELLOW, "6 %");
    assert_consistent(&sim, &app);
    sim.save("course_fields");

    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 3000);
    assert_consistent(&sim, &app);
    sim.save("course_profile");
    let (px, t0) = (sim.pixels, sim.ms);
    run(&mut sim, &mut app, 20_000);
    assert_consistent(&sim, &app);
    std::println!("profile page: {} px/s", (sim.pixels - px) * 1000 / (sim.ms - t0));
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 3000);
    assert_consistent(&sim, &app);
    sim.save("course_map");
    let course = sim.screen.iter().filter(|&&p| p == crate::map::COURSE).count();
    assert!(course > 150, "the course on the map (under the track where ridden): {course} px");
    // the course page: in the round after the ride pages while the course is on
    let shown = |app: &App| app.widgets().filter_map(|(_, w)| if let Widget::Field(f) = w { Some(f) } else { None });
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(crate::COURSE), "after the map page");
    run(&mut sim, &mut app, 3000);
    assert_eq!(shown(&app).collect::<Vec<_>>(), crate::COURSE_PAGE.fields);
    assert_consistent(&sim, &app);
    sim.save("course_page");
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0), "then round to page 1");

    // the cost: segments and boxes looked at a second, detour included
    let at = app.courses.as_deref().unwrap().at;
    let per_s = at.examined / (sim.ms / 1000) as u32;
    std::println!("course following: {per_s} segments a second ({} in all)", at.examined);
    assert!(per_s <= 60, "{per_s} segments a second");

    // Select on the course followed stops following it
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Courses));
    app.press(&mut sim, Press::Short(Key::Start));
    for _ in 0..hill(&app) {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.banner.as_str(), "COURSE OFF");
    assert_eq!(fields(&app), ["--", "--", "--", "--", "--"]);
}

/// The zigzag (course/cues.rs `zigzag`, tools/fat_image.py's `Zigzag.gpx`) ridden at 8 m/s from its start.
fn zigzag_route(ms: u64) -> (i32, i32, i32) {
    let pts = crate::course::cues::tests::zigzag();
    let mut s = ms as f64 * 0.008;
    let mut at = (pts[0].0, pts[0].1, 90.0);
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let heading = (b.0 - a.0).atan2(b.1 - a.1).to_degrees().rem_euclid(360.0);
        if s <= len {
            at = (a.0 + (b.0 - a.0) * s / len, a.1 + (b.1 - a.1) * s / len, heading);
            break;
        }
        (s, at) = (s - len, (b.0, b.1, heading));
    }
    let m_per_unit = 0.011_132;
    ((at.1 / m_per_unit) as i32, (at.0 / m_per_unit) as i32, (at.2 * 1e5) as i32)
}

/// Turn-by-turn on the zigzag's GPX track (its turns found on its shape): each turn said in the banner with its
/// arrow 200 m before ("LEFT 200 M", a beep) and at it ("LEFT NOW", two beeps), once each, in order; the course
/// page's NEXT TURN field shows the arrow and the distance, redrawn by its changed characters, and END past the last
/// one. The banner costs its own pixels and no more.
#[test]
fn course_cues_ride() {
    use crate::course::Turn;
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.gnss.route = Some(zigzag_route);
    let mut app = App::start(&mut sim, None);
    app.set_course_memory(crate::storage::course::tests::memory());
    app.open_courses(&mut sim);
    run(&mut sim, &mut app, 1000);
    let list = &app.courses.as_deref().unwrap().list;
    let k = (0..list.len()).find(|&i| list.get(i).unwrap().name() == "ZIGZAG").unwrap();
    app.follow_course(k);
    run(&mut sim, &mut app, 200);
    assert_eq!(app.screen(), Screen::Ride(crate::COURSE));
    let text = |app: &App| std::string::String::from(app.field_text(Field::NextTurn).as_str());
    assert_eq!(text(&app), "--", "no fix yet");
    let cues: Vec<_> = app.courses.as_deref().unwrap().course.cues().to_vec();
    let kinds: Vec<_> = cues.iter().map(|c| c.turn).collect();
    use Turn::*;
    assert_eq!(kinds, [Left, Right, SharpRight, SlightLeft, Right, UTurn]);

    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    // to 400 m: the left turn at 1 km is 600 m ahead
    while sim.ms < 50_000 {
        run(&mut sim, &mut app, 1000);
    }
    let t = text(&app);
    let m: u32 = t.trim_start_matches('\x01').trim_end_matches('M').parse().unwrap();
    assert!(t.starts_with('\x01') && (570..=610).contains(&m), "{t:?}");
    assert_consistent(&sim, &app);
    sim.save("course_next_turn");

    // the ride to the end: every banner as it shows, where the rider is then (m), the tones
    let (tones, mut said, mut last) = (sim.tones.len(), Vec::new(), std::string::String::new());
    let mut banner_px = Vec::new();
    let end = (app.courses.as_deref().unwrap().course.len_m() as u64 + 40) * 125;
    while sim.ms < end {
        let px = sim.pixels;
        run(&mut sim, &mut app, 500);
        let b = std::string::String::from(app.banner.as_str());
        if app.banner_ms.is_some() && b != last {
            said.push((sim.ms as f64 * 0.008, b.clone()));
            banner_px.push(sim.pixels - px);
            if b.ends_with("NOW") && said.len() == 2 {
                assert_consistent(&sim, &app);
                sim.save("course_cue_banner");
            }
        }
        last = b;
    }
    // past the last turn, at the end: no turn left, said as such (not "--" as off course)
    assert_eq!(text(&app), "END");
    let mut want = Vec::new();
    for c in &cues {
        let words = std::format!("{} {}", c.turn.glyph(), c.turn.word());
        want.push((f64::from(c.at_m) - 200.0, std::format!("{words} 200 M")));
        want.push((f64::from(c.at_m) - 32.0, std::format!("{words} NOW")));
    }
    let texts: Vec<_> = said.iter().map(|s| s.1.as_str()).collect();
    assert_eq!(texts, want.iter().map(|w| w.1.as_str()).collect::<Vec<_>>(), "{said:?}");
    for (s, w) in said.iter().zip(&want) {
        assert!((s.0 - w.0).abs() <= 24.0, "{s:?} where {w:?}"); // a fix a second, looked at every 500 ms
    }
    let beeps = sim.tones[tones..].iter().filter(|&&hz| hz == 1800).count();
    assert_eq!(beeps, 3 * cues.len(), "a beep at 200 m, two at the turn");
    // the half second a banner shows: the banner (240 x 44 = 10560 px, at the top) and the page's own changes; once in a while
    // the profile moves on by an eighth then too (a whole profile)
    banner_px.sort();
    let median = banner_px[banner_px.len() / 2];
    std::println!("a cue's banner: {median} px in its half second (median; {banner_px:?})");
    assert!(median < 10560 + 2000, "{banner_px:?}");
    assert_consistent(&sim, &app);
}

/// Max HR and FTP from the menu: Start edits the value in place, Lap steps it up (1 bpm, 5 W), Page down,
/// holding a key repeats, Start keeps it; kept in the wheel's register beside the other settings.
#[test]
fn max_hr_and_ftp_from_the_menu() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::Wheel));
    app.press(&mut sim, Press::Short(Key::Start)); // wheel: next size
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::MaxHr));
    app.press(&mut sim, Press::Short(Key::Start)); // edit MAX HR
    for _ in 0..5 {
        app.press(&mut sim, Press::Short(Key::Lap)); // 190 -> 195
    }
    app.press(&mut sim, Press::Short(Key::Page)); // 194
    sim.save("max_hr_edit");
    app.press(&mut sim, Press::Short(Key::Start)); // kept
    app.press(&mut sim, Press::Short(Key::Page)); // FTP row
    app.press(&mut sim, Press::Short(Key::Start));
    for _ in 0..200 {
        app.press(&mut sim, Press::Repeat(Key::Page)); // held: 200 W down to the floor
    }
    app.press(&mut sim, Press::Short(Key::Page)); // the release does not step again
    app.press(&mut sim, Press::Short(Key::Lap)); // 55
    app.press(&mut sim, Press::Short(Key::Back)); // kept, still in the menu
    assert!(matches!(app.screen, Screen::Menu(Menu::Me, r) if r == row_of(menu::Row::Set(Setting::Ftp))));
    assert_consistent(&sim, &app);
    sim.save("zones_menu");
    let again = App::start(&mut sim, None);
    assert_eq!((again.settings.max_hr, again.settings.ftp, again.settings.wheel_mm), (194, 55, WHEELS[2]));
    assert_eq!((again.settings.light_level, again.settings.light_timeout), (0, 0), "the backlight settings untouched");
}

/// Continuous capture: 4 s with a Page press, one BMP per changed frame in target/sim/run/.
#[test]
fn record_a_run() {
    let mut sim = Sim::new(9, 59, 57).keys(|ms| if ms / 100 == 25 { Key::Page as u8 } else { 0 }).record("run");
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 4000);
    let frames = std::fs::read_dir(out_dir().join("run")).unwrap().count();
    assert!(frames >= 2, "the first page, then the second, got {frames}");
}

/// The course page stays in the round while a course is followed, even when the map cannot draw it yet: a ride
/// started before any fix leaves the track without an origin (review: Page skipped the course page then).
#[test]
fn course_page_before_a_fix() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let aid = crate::gnss::Aid { offset_15min: Some(8), pos: Some((0, 0)) };
    [sim.backup[17], sim.backup[18]] = aid.to_words();
    sim.gnss.route = Some(course_route);
    let mut app = App::start(&mut sim, None);
    app.set_course_memory(crate::storage::course::tests::memory());
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Courses));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    for _ in 0..hill(&app) {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 200);
    assert_eq!(app.screen(), Screen::Ride(crate::COURSE));
    app.press(&mut sim, Press::Short(Key::Start)); // the ride, no fix yet
    run(&mut sim, &mut app, 2000);
    let round: Vec<_> = (0..4)
        .map(|_| {
            app.press(&mut sim, Press::Short(Key::Page));
            app.screen()
        })
        .collect();
    assert_eq!(round[3], Screen::Ride(crate::COURSE), "{round:?}");
    app.press(&mut sim, Press::Long(Key::Page));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Ride(crate::COURSE), "Back from the menu returns to it");
}

/// After "+" the pan's range is smaller: a centre panned to the old edge is outside it, and a small drag must still
/// move the map with the finger, not jump back across (review of 6de15ed: 960 m the wrong way). A drag that starts
/// on a button does not pan.
#[test]
fn pan_after_zoom_in_moves_with_the_finger() {
    use crate::map;
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.gnss.route = Some(tiles_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    sim.gnss.fix = Some(500);
    run(&mut sim, &mut app, 200_000);
    let area = map::area(ui::field_box(1, ui::RIDE_H, 0).unwrap());
    let [plus, _] = map::buttons(area).unwrap();
    let (px, py) = ((plus.0 + 20) as u16, (plus.1 + 20) as u16);
    app.input(&mut sim, Input::Touch(Gesture::Tap(px, py)));
    for _ in 0..30 {
        drag(&mut sim, &mut app, (200, 150), (-180, 0));
    }
    app.input(&mut sim, Input::Touch(Gesture::Tap(px, py)));
    let before = app.map_view(0).unwrap();
    drag(&mut sim, &mut app, (150, 150), (-15, 0)); // the finger left: the view goes east, or stays at the edge
    let after = app.map_view(0).unwrap();
    assert!(after.cx >= before.cx, "a drag left moved the view west by {} m", before.cx - after.cx);
    assert_consistent(&sim, &app);
    // a gloved tap sliding 12 px on "-" is "-": a level out, the panned centre kept, not a pan
    let [_, minus] = map::buttons(area).unwrap();
    let (zoom, centre) = (app.map_zoom, app.map_centre);
    drag(&mut sim, &mut app, (minus.0 + 14, minus.1 + 20), (12, 0));
    assert_eq!(app.map_zoom.map(|z| z + 1), zoom.map(|z| z + 2), "one level out");
    assert_eq!(app.map_centre, centre, "not panned");
    assert_consistent(&sim, &app);
}

/// A cue comes up whatever is on screen while navigating: over a menu, over the media remote, and with the light
/// timed out or switched off with Back (it comes back on, as for a key). The panel equals a full redraw each time (a
/// screen updating its own parts does not paint over the banner).
#[test]
fn course_cues_over_any_screen() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.gnss.route = Some(zigzag_route);
    let mut app = App::start(&mut sim, None);
    app.set_course_memory(crate::storage::course::tests::memory());
    app.open_courses(&mut sim);
    run(&mut sim, &mut app, 1000);
    let list = &app.courses.as_deref().unwrap().list;
    let k = (0..list.len()).find(|&i| list.get(i).unwrap().name() == "ZIGZAG").unwrap();
    app.follow_course(k);
    run(&mut sim, &mut app, 200);
    app.settings.light_timeout = 1; // 10 s
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    let cues: Vec<_> = app.courses.as_deref().unwrap().course.cues().to_vec();
    // each cue's first banner (200 m before) found on a different screen
    let screens = [Some(Screen::Menu(Menu::Main, 0)), Some(Screen::Media), None, None];
    for (i, (cue, screen)) in cues.iter().zip(screens).enumerate() {
        let at_ms = (u64::from(cue.at_m).saturating_sub(260)) * 125;
        while sim.ms < at_ms {
            run(&mut sim, &mut app, 500);
        }
        match screen {
            Some(s) => {
                app.open(s);
                app.show(&mut sim);
            }
            None if i == 2 => {}          // the light timed out on its own (10 s, no key)
            None => app.idle_ms = 60_000, // the light timed out (well past the 10 s)
        }
        run(&mut sim, &mut app, 3000);
        if screen.is_none() {
            assert!(!sim.backlight, "cue {i}: the light off before the cue");
        }
        let until = sim.ms + 12_000;
        while sim.ms < until && !app.banner.as_str().ends_with("200 M") {
            run(&mut sim, &mut app, 250);
        }
        assert!(app.banner.as_str().ends_with("200 M"), "cue {i} said: {:?}", app.banner.as_str());
        run(&mut sim, &mut app, 250);
        assert!(sim.backlight, "cue {i}: the light on for it");
        assert_consistent(&sim, &app);
        if let Some(s) = screen {
            assert_eq!(app.screen(), s, "cue {i}: the screen stays");
            app.close();
            app.show(&mut sim);
        }
    }
}

/// ABOUT (DEVICE menu, last row): Ferris large, the version, the commit the build is made from; Back returns to the
/// menu. The panel equals a full redraw.
#[test]
fn about_screen() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::About);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::About);
    let rust = sim.screen.iter().filter(|&&p| p == ui::RUST).count();
    assert!(rust > 64 * 64, "Ferris, large: {rust} px");
    assert_consistent(&sim, &app);
    sim.save("about");
    app.press(&mut sim, Press::Short(Key::Back));
    assert!(matches!(app.screen(), Screen::Menu(Menu::System, _)));
}

/// A key held while USB keeps the CPU busy in the sleep (a copy to the drive) counts the time it was held, not the
/// USB work again (2026-10-25 review: a pass counted its sleep and that work): Back held 2 s does not switch off;
/// held 3.5 s it does.
#[test]
fn a_hold_while_usb_is_busy_counts_real_time() {
    for (hold, off) in [(2000, false), (3500, true)] {
        let mut sim =
            Sim::new(9, 0, 0).keys(move |ms| if (2000..2000 + hold).contains(&ms) { Key::Back as u8 } else { 0 });
        let mut app = App::start(&mut sim, None);
        while sim.ms < 8000 && !sim.standby {
            app.step(&mut sim);
            // the sleep: 50 ms, 45 of them serving USB
            sim.delay_ms(POLL_MS);
            sim.awake_ns += 45_000_000;
            sim.sleep = (POLL_MS, 45_000);
        }
        assert_eq!(sim.standby, off, "Back held {hold} ms");
    }
}

/// Woken by the power button (Back's line) held on through the start: the hold is not Back held 3 s, so the unit
/// stays on; released and held again for 3 s, it switches off as usual.
#[test]
fn the_power_button_held_through_the_start_does_not_switch_off() {
    let mut sim =
        Sim::new(9, 0, 0).keys(|ms| if ms < 5000 || (8000..12_000).contains(&ms) { Key::Back as u8 } else { 0 });
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 7000);
    assert!(!sim.standby, "the wake-up press held 5 s is not a switch-off");
    run(&mut sim, &mut app, 6000);
    assert!(sim.standby, "a new 3 s hold switches off");
}

/// HISTORY lists every ride the index holds, the oldest of a full index included (the row count once stopped at
/// 128, one short of FITNESS and 128 rides: code review).
#[test]
fn history_reaches_the_oldest_of_a_full_index() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.stock.start(app.date);
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim, &mut app, u64::from(POLL_MS));
        assert!(sim.ms < 120_000, "the scan ends");
    }
    let ride = *app.stock.index.get(0).expect("a synthetic ride");
    for _ in 0..crate::history::RIDES {
        app.stock.index.add(ride);
    }
    assert_eq!(app.stock.index.len(), crate::history::RIDES);
    assert_eq!(app.rows(Menu::History), crate::history::RIDES + 1, "FITNESS and every ride");
    assert_eq!(app.row(Menu::History, crate::history::RIDES), Some(menu::Row::Past(crate::history::RIDES - 1)));
}
