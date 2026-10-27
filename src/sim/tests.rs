//! The simulator tests: the whole app on [`Sim`], driven by keys, a finger, sensors and fixes as functions of
//! time, checked on what the panel shows.

use super::*;
// Helpers that moved with their tests; the other test files still reach them as `tests::X`.
pub(super) use super::course_tests::hill;
use super::map_tests::ride_route;
pub(super) use super::map_tests::{drag, tiles_route};
use super::menu_tests::built_in_row;
use super::menu_tests::main_row;
use super::menu_tests::row_of;
pub(super) use super::menu_tests::{beat_ticks, row_in, strap_beats, to_row};
use super::ride_tests::power_noisy;
use crate::ui::tests::Frame;
use crate::ui::{Field, Rec, Strip, UsbDot};
/// A stand-in for the ANT+ network key (the real one is never in the repository).
pub(super) const KEY: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
/// The owner's strap, as the unit paired it.
pub(super) const STRAP: SensorId = SensorId { device: 25126, trans: 0xf1 };

pub(super) fn hr72(_: u64) -> [u8; 8] {
    [4, 0, 0, 0, 0, 0, 0, 72]
}
pub(super) fn hr80(_: u64) -> [u8; 8] {
    [4, 0, 0, 0, 0, 0, 0, 80]
}
/// Speed and cadence: every second one crank turn and three wheel turns.
pub(super) fn spd_cad(ms: u64) -> [u8; 8] {
    let (t, n) = (((ms / 1000) * 1024) as u16, (ms / 1000) as u16);
    let [t0, t1] = t.to_le_bytes();
    let ([c0, c1], [w0, w1]) = (n.to_le_bytes(), (3 * n).to_le_bytes());
    [t0, t1, c0, c1, t0, t1, w0, w1]
}
pub(super) fn power300(_: u64) -> [u8; 8] {
    [0x10, 1, 0xff, 85, 0, 0, 0x2c, 0x01]
}

/// Passes of the main loop for `ms`: a step, then the sleep, in which (as on the unit) the touch panel is read
/// when INT falls (here: seen within a [`TOUCH_MS`] slice) and every [`TOUCH_MS`] while a finger is down.
pub(super) fn run(sim: &mut Sim, app: &mut App, ms: u64) {
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
    if app.notes.ms.is_some() {
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
pub(super) fn ride(sim: &mut Sim, app: &App, n: usize, values: &[&str], radio: bool) -> Vec<u16> {
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
            more: app.strip.more,
        },
    );
    let page = app.pages()[n];
    for (i, v) in values.iter().enumerate() {
        let bx = <(u32, u32, u32, u32)>::from(page.cell_box(i).unwrap());
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

pub(super) fn hm(sim: &mut Sim) -> std::string::String {
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
            more: app.strip.more,
        },
    );
    let _ = s;
    crate::ui::draw_today(&mut f, crate::ui::Today::Pending);
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

/// Stock's order: reset, the two links' configuration (`sd_ble_cfg_set`), sd_ble_enable, then the version; no ANT
/// without a key, no scan before the list; then the phone link's GATT server (app/phone.rs), and no advertising
/// without a bond.
#[test]
fn starts_the_nordic_chip() {
    let mut sim = Sim::new(12, 0, 0);
    App::start(&mut sim, None);
    let cfg = crate::nrf::CFG.map(<[u8]>::to_vec);
    assert_eq!(sim.nordic.got[..4], [cfg[0].clone(), cfg[1].clone(), vec![0, 0x60], vec![0, 0x65, 1]]);
    assert_eq!(sim.nordic.got[4], [&[0, 0x7c, 1, 0, 4, 0, 1][..], b"S510"].concat(), "the name first");
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
    assert_eq!(sim.nordic.got, [crate::nrf::CFG[0].to_vec()], "gave up after the first timeout");
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
    let taps = |ms: u64| (5000..5100).contains(&ms);
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| taps(ms).then_some((120, row3)));
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    let left_ms = 5500 - sim.ms;
    run(&mut sim, &mut app, left_ms);
    assert_eq!(app.screen(), Screen::Menu(Menu::Pages, 0), "one tap acts: PAGES");
    assert_consistent(&sim, &app);
}

/// The gear right of the overview's date: a tap opens the menu; a tap elsewhere on the overview does not.
#[test]
fn gear_on_the_overview_opens_the_menu() {
    let (gx, gy) = (ui::GEAR_C.0 as u16, ui::GEAR_C.1 as u16);
    let tap = move |t0: u64, p: (u16, u16)| move |ms: u64| (t0..t0 + 80).contains(&ms).then_some(p);
    let field = ui::overview_box(0).map_or((40, 200), |b| ((b.0 + b.2 / 2) as u16, (b.1 + b.3 / 2) as u16));
    let (a, b) = (tap(5000, field), tap(6000, (gx, gy)));
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| a(ms).or(b(ms)));
    let mut app = App::start(&mut sim, None);
    sim.save("overview_gear");
    app.settings.smooth = true;
    app.show(&mut sim);
    sim.save("overview_gear_smooth");
    app.settings.smooth = false;
    app.show(&mut sim);
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
    assert_eq!(app.notes.list[0].as_str(), "09:00 USB RECONNECT");
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

/// Touch in a menu: a tap on MAX HR edits it, a swipe up adds one, a tap keeps it, a swipe right goes back.
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
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| a(ms).or(b(ms)).or(c(ms)).or(d(ms)));
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
    assert_eq!(app.notes.text.as_str(), "RIDE STARTED");
    assert_consistent(&sim, &app);
    sim.save("ride_started");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.notes.text.as_str(), "PAUSED: HOLD START TO END");
    save_ride(&mut sim, &mut app);
    assert_eq!((app.screen(), app.ride.state()), (Screen::Summary, ride::State::Idle), "the ride's card");
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
    back(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Main, "Back: the overview");
}

/// "RIDE SAVED" is said without covering the ride card's title (the date under the strip): the banner's 44 px are the
/// strip and the title row, so over the card the notice goes to the notices list (the overview shows it) only.
#[test]
fn ride_saved_leaves_the_cards_title() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 3000);
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Summary);
    run(&mut sim, &mut app, 1500);
    assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{}", app.notes.list[0].as_str());
    assert_consistent(&sim, &app);
    sim.save("ride_saved_card");
    // the title row (under the strip's line) still has the card's grey date, not the banner's orange fill
    let y = (ui::PAGE_Y + 12) as usize;
    let orange = sim.screen[y * W as usize + 2];
    assert_ne!(orange, ui::ACCENT, "the title row is not the banner's");
    assert!(app.notes.ms.is_none(), "no banner over the card");
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
    assert!(app.notes.ms.is_some());
    assert_consistent(&sim, &app);
    sim.save("banner");
    run(&mut sim, &mut app, u64::from(BANNER_MS));
    assert!(app.notes.ms.is_none(), "gone by itself");
    assert_consistent(&sim, &app);
    app.notice("TEST");
    run(&mut sim, &mut app, 100);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(1), "the key dismissed it and acted");
    assert!(app.notes.ms.is_none());
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
    assert_eq!(app.notes.list[0].as_str(), "", "no notice for the fix");
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
    for _ in 0..4 {
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
        more: false,
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
    assert!(app.notes.ms.is_none());
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
    assert_ne!(app.notes.text.as_str(), "LAP", "no ride: no lap");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Main);
    app.press(&mut sim, Press::Short(Key::Lap));
    assert_eq!(app.notes.text.as_str(), "LAP");
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
    assert!(app.notes.ms.is_none());
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
    let (gx, gy) = (ui::GEAR_C.0 as u16, ui::GEAR_C.1 as u16);
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
    assert!(app.strip.locked && app.notes.text.as_str() == "TOUCH LOCKED");
    run(&mut sim, &mut app, 500);
    assert_eq!(app.screen(), Screen::Main, "neither key acted at its release");
    assert!(sim.backlight, "Back did not toggle the light");
    // the padlock is in the strip, and the screen equals the panel
    let lit = |sim: &Sim| (79..86).any(|x| sim.screen[(12 * W + x) as usize] == ui::WHITE);
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
    assert!(!app.strip.locked && app.notes.text.as_str() == "TOUCH UNLOCKED");
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

/// After a fault (hw.rs marks it in backup register 29 and resets): a notice at start, and the count cleared after
/// a minute of running.
#[test]
fn restart_after_a_fault() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[FAULT_BKP] = 0xfa00_0001;
    let mut app = App::start(&mut sim, None);
    assert_eq!(app.notes.list[0].as_str(), "09:00 FAULT RESET 1");
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
    assert!(!sim.standby && app.notes.text.as_str() == "HOLD BACK AGAIN TO SWITCH OFF", "warned");
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
    run(&mut sim, &mut app, 500); // its start's beep
    let tones0 = sim.tones.len();
    let steps0 = sim.ms;
    run(&mut sim, &mut app, 298_500);
    assert_eq!(sim.tones.len(), tones0, "not yet");
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.notes.text.as_str(), "EAT REMINDER");
    let beeps: std::vec::Vec<u32> = sim.tones[tones0..].to_vec();
    assert_eq!(beeps, [2500, 0, 2500, 0, 2500, 0], "three beeps, then off");
    assert!(sim.ms - steps0 >= 300_000, "{}", sim.ms - steps0);
    assert_consistent(&sim, &app);
    sim.save("eat_reminder");
    // paused on the minute: nothing more
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 500); // the pause's beep
    let n = sim.tones.len();
    run(&mut sim, &mut app, 120_000);
    assert_eq!(sim.tones.len(), n, "paused: no reminder");
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
        app.notes.ms.map(|_| std::string::String::from(app.notes.text.as_str()))
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
        app.notes.ms.map(|_| std::string::String::from(app.notes.text.as_str()))
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
    let (gx, gy) = (ui::GEAR_C.0 as u16, ui::GEAR_C.1 as u16);
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
    assert_eq!(app.notes.text.as_str(), "IMPORT SETTINGS?\nSTART: YES BACK: NO");
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
    app.alert.push(Some(alerts::Alert::Eat));
    run(&mut sim, &mut app, 2000);
    assert_eq!(sim.tones[t0..], [2500, 0, 2500, 0, 2500, 0, 2500, 0], "the step's tone, then the alert's three");
}

/// Two alerts due in the same second are both said, one after the other (the second was thrown away).
#[test]
fn two_alerts_in_one_second_are_both_said() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.alert.push(Some(alerts::Alert::CadenceLow));
    app.alert.push(Some(alerts::Alert::HrHigh));
    run(&mut sim, &mut app, 200);
    assert_eq!(app.notes.text.as_str(), "HEART RATE HIGH");
    let mut said = std::vec::Vec::new();
    for _ in 0..200 {
        run(&mut sim, &mut app, 50);
        if said.last().is_none_or(|l| l != app.notes.text.as_str()) {
            said.push(std::string::String::from(app.notes.text.as_str()));
        }
    }
    assert!(said.iter().any(|b| b == "CADENCE LOW"), "{said:?}");
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

/// Passes of `pass` ms each for `ms` (a slow card, or a deeper sleep): the step is told the pass was that long
/// ([`Board::last_sleep`]), as main.rs does.
pub(super) fn run_passes(sim: &mut Sim, app: &mut App, ms: u64, pass: u32) {
    let end = sim.ms + ms;
    while sim.ms < end && !sim.standby {
        app.step(sim);
        sim.sleep = (pass, 0);
        sim.delay_ms(pass);
    }
}

/// Every timer counts the pass's real length, not a pass: with passes of 400 ms (8 times POLL_MS) a banner lasts
/// BANNER_MS, a tone its length, the backlight times out on time, a silent sensor rests its minute, and the radio
/// watchdog counts the quiet in ms (2026-10-25 review: about 23 timers added a fixed POLL_MS a pass).
#[test]
fn timers_count_real_time_in_long_passes() {
    const PASS: u32 = 400;
    // a banner
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Page));
    run_passes(&mut sim, &mut app, 3000, PASS);
    app.notice("STRAP PAIRED");
    let t0 = sim.ms;
    run_passes(&mut sim, &mut app, u64::from(BANNER_MS) - 2 * u64::from(PASS), PASS);
    assert!(app.notes.ms.is_some(), "still there before its time");
    while app.notes.ms.is_some() {
        run_passes(&mut sim, &mut app, 1, PASS);
    }
    let shown = sim.ms - t0;
    assert!((u64::from(BANNER_MS)..=u64::from(BANNER_MS + 2 * PASS)).contains(&shown), "banner {shown} ms");
    // a tone
    static MELODY: [(u32, u32); 1] = [(1000, 1000)];
    app.tones.play(&MELODY);
    let n0 = sim.tones.len();
    while sim.tones.len() == n0 {
        run_passes(&mut sim, &mut app, 1, PASS);
    }
    let t1 = sim.ms;
    while sim.tones.last() != Some(&0) {
        run_passes(&mut sim, &mut app, 1, PASS);
    }
    let played = sim.ms - t1;
    assert!((1000..=1000 + 2 * u64::from(PASS)).contains(&played), "tone {played} ms");
    // the backlight: 10 s of no key
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.settings.light_level = 2;
    app.settings.light_timeout = 1;
    run_passes(&mut sim, &mut app, 1, PASS);
    assert_ne!(app.light.shown, 0);
    let t0 = sim.ms;
    while app.light.shown != 0 && sim.ms < t0 + 30_000 {
        run_passes(&mut sim, &mut app, 1, PASS);
    }
    let on = sim.ms - t0;
    assert!((9000..=10_000 + 2 * u64::from(PASS)).contains(&on), "light on {on} ms");
    // a silent strap: searches 30 s, closes, rests SEARCH_REST_MS, searches again
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    let mut app = App::start(&mut sim, Some(KEY));
    let opens = |sim: &Sim| sim.nordic.got.iter().filter(|p| p.starts_with(&[6, 0xc4, 0])).count();
    run_passes(&mut sim, &mut app, 31_000, PASS);
    assert!(!sim.nordic.channels[0].open, "closed by its search time-out");
    run_passes(&mut sim, &mut app, u64::from(SEARCH_REST_MS) - 3000, PASS);
    assert_eq!(opens(&sim), 1, "resting");
    run_passes(&mut sim, &mut app, 4000, PASS);
    assert_eq!(opens(&sim), 2, "searching again");
    // the watchdog's quiet, in ms: the chip heard from, then silent
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, Some(KEY));
    run_passes(&mut sim, &mut app, 10_000, PASS);
    let quiet = app.watch.quiet_ms;
    assert!((8000..=10_400).contains(&quiet), "quiet {quiet} ms after 10 s of passes of {PASS} ms");
}

/// Start, pause and resume each beep: rising for a start or a resume, falling for a pause (the owner, 2026-10-25).
#[test]
fn start_and_pause_beep() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    for want in [[1800, 2600, 0], [2600, 1800, 0], [1800, 2600, 0]] {
        let t0 = sim.tones.len();
        app.start_pause();
        run(&mut sim, &mut app, 1000);
        assert_eq!(sim.tones[t0..], want);
    }
}

/// The bike's weight (BIKE > WEIGHT, 5.0 to 25.0 kg in 0.1 kg, 9.0 by default): kept in register 16 with the
/// rider's, the single presses' actions moved to register 28 for it; earlier builds' words read; the pacer's mass and
/// the trainer's.
#[test]
fn bike_weight() {
    use menu::Remote;
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    let row = |app: &App| app.menu_row(Menu::Bike, 1).map(|(l, v, _)| format!("{} {}", l.as_str(), v.as_str()));
    assert_eq!(row(&app).as_deref(), Some("WEIGHT 9.0 KG"));
    assert_eq!(app.pace_rider().mass, f32::from(app.settings.weight_kg) + 9.0);
    app.settings.bike_hg = 50;
    app.settings.step(menu::Setting::BikeWeight, false);
    assert_eq!(app.settings.bike_hg, 50, "held at the bottom of its range");
    for _ in 0..250 {
        app.settings.step(menu::Setting::BikeWeight, true);
    }
    assert_eq!(app.settings.bike_hg, 250);
    app.settings.bike_hg = 85;
    assert_eq!(row(&app).as_deref(), Some("WEIGHT 8.5 KG"));
    assert_eq!(app.pace_rider().mass, f32::from(app.settings.weight_kg) + 8.5);
    // every action and the alerts kept across the two registers
    app.settings.remote = [Remote::Lap, Remote::Play, Remote::PrevTrack, Remote::Start];
    (app.settings.eat_min, app.settings.drink_min, app.settings.hr_alert) = (45, 20, 230);
    let s = app.settings;
    let kept = Settings::from_word(s.to_word()).with_power_word(s.power_word()).with_alerts_word(s.alerts_word());
    assert_eq!(kept, s);
    // a stored 0 in the bike's bits is the default
    assert_eq!(Settings::from_word(0).with_power_word(s.power_word() & !(0xff << 16)).bike_hg, 90);
    // earlier builds: 0xc1 (no bike), 0xb (whole kg, single presses in 21..26) with 0xc2 alerts (bpm)
    assert_eq!(Settings::from_word(0).with_power_word(0xc1_00_28_46).bike_hg, 90);
    let old_b = 0xb << 28 | (Remote::Play as u32) << 24 | (Remote::Lap as u32) << 21 | 7 << 16 | 40;
    let old_alerts = 0xc2 << 24 | 172 << 16 | 9;
    let old = Settings::from_word(0).with_power_word(old_b).with_alerts_word(old_alerts);
    assert_eq!((old.bike_hg, old.weight_kg, old.hr_alert, old.eat_min), (70, 70, 172, 45));
    assert_eq!(old.remote[..2], [Remote::Lap, Remote::Play]);
}

/// The whole screen as the screenshot path draws it, in the font the settings choose.
fn render_all(app: &App) -> Vec<u16> {
    (0..H).flat_map(|y| app.render_row(y).px).collect()
}

/// FONT in DISPLAY: Start switches to SMOOTH at once (the screen drawn whole, the panel equal to the screenshot),
/// the choice is kept in register 16's bit 24, and back to PIXEL brings the old screen back exactly. On a
/// ride page the smooth text is inside the same boxes: nothing differs outside the fields and the strip.
#[test]
fn font_smooth_keeps_layouts() {
    let toggle = |sim: &mut Sim, app: &mut App| {
        app.press(sim, Press::Long(Key::Page));
        to_row(sim, app, menu::Row::Set(Setting::Font));
        app.press(sim, Press::Short(Key::Start));
        run(sim, app, 100);
        assert_consistent(sim, app);
        let shot = sim.screen.clone();
        while matches!(app.screen(), Screen::Menu(..)) {
            back(sim, app);
        }
        run(sim, app, 100);
        assert_consistent(sim, app);
        shot
    };
    for ride_page in [false, true] {
        let mut sim = Sim::new(12, 34, 56);
        let mut app = App::start(&mut sim, None);
        if ride_page {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        run(&mut sim, &mut app, 200);
        assert!(!app.settings.smooth, "PIXEL by default");
        let pixel = render_all(&app);
        let menu_shot = toggle(&mut sim, &mut app);
        assert!(app.settings.smooth);
        assert_ne!(sim.backup[POWER_BKP] & 1 << 24, 0, "kept");
        assert!(Settings::from_word(0).with_power_word(sim.backup[POWER_BKP]).smooth);
        let smooth = render_all(&app);
        let diff: Vec<usize> = (0..pixel.len()).filter(|&i| pixel[i] != smooth[i]).collect();
        assert!(diff.len() > 1000, "the text changed: {} pixels", diff.len());
        if ride_page {
            let page = app.pages()[0];
            let boxes: Vec<_> = (0..6).map(|i| page.cell_box(i).unwrap()).collect();
            for i in diff {
                let (x, y) = ((i % W as usize) as u32, (i / W as usize) as u32);
                assert!(
                    y < ui::STRIP_H + 2 || boxes.iter().any(|b| x >= b.0 && x < b.0 + b.2 && y >= b.1 && y < b.1 + b.3),
                    "({x}, {y}) is outside the strip and the fields"
                );
            }
            sim.save("font_smooth_ride");
        } else {
            sim.save("font_smooth_overview");
        }
        if !ride_page {
            std::fs::write(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/sim/font_smooth_menu.bmp"),
                crate::sim::bmp(&menu_shot),
            )
            .unwrap();
        }
        toggle(&mut sim, &mut app);
        assert!(!app.settings.smooth && sim.backup[POWER_BKP] & 1 << 24 == 0);
        let again = render_all(&app);
        assert!(again.iter().zip(&pixel).filter(|(a, b)| a != b).count() < 200, "PIXEL back (only the clock moved)");
    }
}

/// The font choice in register 16: bit 24 of the tag 0xa word; words written before have it clear (PIXEL).
#[test]
fn font_choice_register_round_trip() {
    let s = Settings { smooth: true, ..Settings::from_word(0) };
    assert_eq!(s.power_word() >> 24, 0xa1);
    assert!(Settings::from_word(0).with_power_word(s.power_word()).smooth);
    assert!(!Settings::from_word(0).with_power_word(Settings::from_word(0).power_word()).smooth);
    assert!(!Settings::from_word(0).with_power_word(0xa0_00_00_00).smooth, "an earlier word: PIXEL");
    let mut a = Settings::from_word(0);
    a.cycle(Setting::Font);
    let mut l = crate::text::Line::<12>::new();
    a.text(Setting::Font, &mut l);
    assert_eq!((a.smooth, l.as_str()), (true, "SMOOTH"));
}
