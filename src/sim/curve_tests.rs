//! The POWER CURVE screen on the Sim (app/curve.rs, ui/curve.rs): opened from FITNESS and from the ride menu, the
//! 90-day curve from stock's rides and this ride's from a power meter, the cursor, CP and W'. The screens are saved
//! as `target/sim/curve_*.bmp`.

use super::menu_tests::to_row;
use super::tests::run;
use super::tests::*;
use super::*;
use crate::menu::{RideRow, Row, StockRow};
use crate::ui::{self, curve::PLOT};

const ID: SensorId = SensorId { device: 7, trans: 5 };

fn power210(_: u64) -> [u8; 8] {
    [0x10, 1, 0xff, 85, 0, 0, 210, 0]
}

/// The unit with stock's rides read and a power meter at 210 W.
fn unit() -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.backup[Kind::Power.backup()] = Kind::Power.to_word(ID);
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power210, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8]));
    run(&mut sim, &mut app, u64::from(STOCK_SCAN_MS) + 20_000);
    assert!(app.stock.ready() && app.stock.out.curve.watts[7] > 0, "stock's curve is read");
    (sim, app)
}

/// Pixels of `color` in the plot (the readout's text has the colours too).
fn count(sim: &Sim, color: u16) -> usize {
    let (x0, y0, w, h) = (PLOT.0 as usize, PLOT.1 as usize, PLOT.2 as usize, PLOT.3 as usize);
    (y0..y0 + h).flat_map(|y| &sim.screen[y * 240 + x0..y * 240 + x0 + w]).filter(|&&p| p == color).count()
}

/// FITNESS > POWER CURVE: the 90-day curve and CP draw, the cursor reads a duration; this ride's curve shows once a
/// ride has gone a while, and grows with it; Page steps the cursor, Back leaves.
#[test]
fn the_curve_with_this_ride_over_it() {
    let (mut sim, mut app) = unit();
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, Row::Open(Menu::History));
    app.press(&mut sim, Press::Short(Key::Start));
    to_row(&mut sim, &mut app, Row::Open(Menu::Stock));
    app.press(&mut sim, Press::Short(Key::Start));
    to_row(&mut sim, &mut app, Row::Stock(StockRow::Curve));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Curve(7), "on a 5-minute best");
    assert!(
        count(&sim, ui::ACCENT) > 100 && count(&sim, ui::GREEN) == 0,
        "the 90 days, no ride yet: {} {}",
        count(&sim, ui::ACCENT),
        count(&sim, ui::GREEN)
    );
    sim.save("curve_idle");
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Curve(8));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_ne!(app.screen(), Screen::Curve(8), "Back leaves");
    // a ride: the meter's 210 W, held for 6 minutes
    while app.screen() != Screen::Main {
        app.press(&mut sim, Press::Short(Key::Back));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), ride::State::Running);
    run(&mut sim, &mut app, 400_000);
    let best = app.ride_curve.best;
    assert!(best[..8].iter().all(|&w| (205..=215).contains(&w)), "210 W for every duration up to 5 min: {best:?}");
    assert_eq!(best[9], 0, "not held 12 minutes");
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, Row::Ride(RideRow::Curve));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Curve(7));
    run(&mut sim, &mut app, 1000);
    assert!(count(&sim, ui::GREEN) > 100 && count(&sim, ui::ACCENT) > 100, "both curves");
    sim.save("curve_ride");
    // a tap on the plot's left edge picks the 1 s end; Lap is the ride's while riding
    app.input(&mut sim, Input::Touch(Gesture::Tap(PLOT.0 as u16, PLOT.1 as u16 + 20)));
    assert_eq!(app.screen(), Screen::Curve(0));
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Curve(1));
    assert_consistent(&sim, &app);
}

/// LIGHT: the same screen with the light theme, for the gallery.
#[test]
fn the_curve_in_light() {
    let (mut sim, mut app) = unit();
    app.open_curve();
    app.theme = ui::theme::Theme::LIGHT;
    app.repaint(&mut sim);
    sim.save("curve_light");
    assert_ne!(count(&sim, ui::theme::Theme::LIGHT.ink(ui::ACCENT)), 0, "the curve in LIGHT's orange");
}

/// Fuzz: random keys and taps on the curve screen, riding and not, never panic, and the cursor stays on a duration;
/// the fuzz must have reached the screen (a count over the seeds).
#[test]
fn fuzz_the_curve_inputs() {
    let (mut sim, mut app) = unit();
    let mut reached = 0;
    for seed in 1..=200u32 {
        let mut x = seed.wrapping_mul(2_654_435_761);
        let mut next = move || {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            x >> 8
        };
        app.open_curve();
        for _ in 0..6 {
            let i = match next() % 5 {
                0 => Input::Next,
                1 => Input::Prev,
                2 => Input::Select,
                3 => Input::Touch(Gesture::Tap((next() % 240) as u16, (next() % 320) as u16)),
                _ => Input::Touch(Gesture::Swipe(Dir::Up)),
            };
            app.input(&mut sim, i);
            if let Screen::Curve(k) = app.screen() {
                assert!(k < crate::power_curve::N);
                reached += 1;
            } else {
                break;
            }
        }
        if seed == 100 {
            app.press(&mut sim, Press::Short(Key::Start)); // the rest of the seeds ride
        }
        run(&mut sim, &mut app, 1500);
    }
    assert!(reached > 400, "{reached}");
}
