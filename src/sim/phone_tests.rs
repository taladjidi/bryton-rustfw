//! The phone link (app/phone.rs) against the simulated chip and a scripted phone (ble_sim.rs): pairing, the bond
//! on the card, reconnection, media keys (AMS and HID), notifications, a call, the time, and ANT+ alongside.

use super::*;
use crate::phone::Stored;

fn run(sim: &mut Sim, app: &mut App, ms: u64) {
    let end = sim.ms + ms;
    while sim.ms < end {
        app.step(sim);
        sim.delay_ms(POLL_MS);
    }
}

/// `\Rustfw\ble.bin` as the card holds it.
fn stored(sim: &Sim) -> Option<Stored> {
    let mut d = crate::fat::tests::Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let f = fat.open(&mut d, "Rustfw/ble.bin").ok()?;
    let mut s = [0u8; 512];
    assert_eq!(fat.read_at(&mut d, &f, 0, &mut s), Ok(512));
    Stored::from_sector(&s)
}

/// An iPhone that connects a second after the unit advertises.
fn iphone(script: Vec<(u64, Act)>) -> PhoneSim {
    let mut p = PhoneSim::new(true);
    (p.connect_at, p.script) = (1000, script);
    p
}

/// A unit with the synthetic volume, and a phone in range.
fn unit(phone: PhoneSim) -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.nordic.phone = Some(phone);
    let app = App::start(&mut sim, None);
    (sim, app)
}

/// Not paired: nothing advertised (the radio stays quiet) until PAIR; the phone then pairs (Just Works), the bond
/// goes on the card with the HID report's CCCD, the phone's services are found and subscribed, and the clock is set
/// from its time.
#[test]
fn pairing_from_the_menu() {
    let script = std::vec![(4000, Act::Notify(4, 7, "Bob", "Lunch at 1?")), (8000, Act::Drop)];
    let (mut sim, mut app) = unit(iphone(script));
    let server = &sim.nordic.server;
    assert!(server.attrs.iter().any(|a| a.1 == 0x2a4d), "the HID report in the table");
    assert_eq!(server.adv_data, crate::phone::ADV);
    assert_eq!(server.vs.len(), 8, "ANCS and AMS UUIDs registered");
    run(&mut sim, &mut app, 2000);
    assert_eq!(sim.nordic.server.adv, None, "no bond, not asked: not advertising");
    assert_eq!(app.phone.link(), Link::Off);

    app.activate(&mut sim, Menu::Phone, 1); // PAIR
    let [i0, i1] = ADV_FAST.0.to_le_bytes();
    assert!(sim.nordic.got.iter().any(|p| p[..2] == [0, 0x73] && p[6..8] == [i0, i1]), "advertising to pair");
    run(&mut sim, &mut app, 1500);
    assert_eq!(app.phone.link(), Link::Secure);
    assert!(app.phone.bonded());
    let ph = sim.nordic.phone.as_ref().unwrap();
    assert_eq!(ph.subscribed, [0x18, 0x13, 0x23, 0x26, 0x33], "Data Source first, then NS, AMS, the time");
    assert_eq!(sim.rtc_sets, 1, "the clock set from the phone");
    assert_eq!(rtc_date_from_dr(sim.rtc().1), (2026, 10, 25));
    assert_eq!(app.aid.offset_15min, Some(4), "UTC + 1 h from Local Time Information");

    let s = stored(&sim).expect("the bond on the card");
    let bond = s.bond.unwrap();
    assert_eq!(bond.key.ltk, ble_sim::LTK);
    assert!(bond.attrs_len > 0, "the HID report's CCCD kept");
    assert!(s.notify && s.keys);

    // a message: its title and text in the banner and the notices (as much as a line holds), at the phone's time
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.notices[0].as_str(), "09:41 Bob: Lunch at ");

    // the phone walks away: the unit advertises slowly again, for it to come back
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.phone.link(), Link::Waiting);
    assert_eq!(sim.nordic.server.adv.map(|a| (a.0, a.1)), Some(ADV_SLOW));
}

/// Media keys: AMS remote commands to an iPhone (Lap held on a page, the remote's keys and gestures), HID for the
/// camera's shutter (a hold on the remote) and to a phone without AMS; what plays shown on the remote and in its
/// field.
#[test]
fn media_keys_and_now_playing() {
    let script = std::vec![(5000, Act::Track("Song", "Band"))];
    let (mut sim, mut app) = unit(iphone(script));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 3000);
    app.input(&mut sim, Input::Media); // Lap held on the overview
    app.activate(&mut sim, Menu::Phone, 2); // REMOTE
    assert_eq!(app.screen, Screen::Media);
    app.show(&mut sim);
    for i in [Input::Next, Input::Prev, Input::PrevHeld] {
        app.input(&mut sim, i);
    }
    run(&mut sim, &mut app, 200);
    app.input(&mut sim, Input::Touch(Gesture::Hold(120, 160))); // the shutter
    run(&mut sim, &mut app, 3000);
    let ph = sim.nordic.phone.as_ref().unwrap();
    assert_eq!(ph.commands, [2, 3, 4, 5], "toggle, next, previous, volume up: AMS");
    assert_eq!(ph.reports, [0x08, 0], "volume up pressed and released: HID");
    assert_eq!(app.phone.now.title.as_str(), "Song");
    assert_eq!(app.field_text(Field::Music).as_str(), "0:02");
    assert_consistent(&sim, &app);
    sim.save("phone_media");
}

/// A call: the call screen with the caller, Start answers it (ANCS positive action); a second one declined with
/// Back; one ended on the phone closes the screen.
#[test]
fn incoming_calls() {
    let script = std::vec![
        (3000, Act::Notify(1, 9, "Mum", "")),
        (6000, Act::Notify(1, 10, "Unknown", "")),
        (9000, Act::Notify(1, 11, "Work", "")),
        (14_000, Act::Remove(11)),
    ];
    let (mut sim, mut app) = unit(iphone(script));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 2500);
    assert_eq!(app.screen, Screen::Call);
    assert_consistent(&sim, &app);
    sim.save("phone_call");
    app.input(&mut sim, Input::Select);
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.screen, Screen::Call, "the second call");
    app.input(&mut sim, Input::Back);
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.screen, Screen::Call, "the third");
    run(&mut sim, &mut app, 5000);
    assert_ne!(app.screen, Screen::Call, "ended on the phone: closed");
    assert_eq!(sim.nordic.phone.as_ref().unwrap().actions, [(9, 0), (10, 1)]);
}

/// After a restart the bond is read from the card, the unit advertises slowly, the phone encrypts with its key and
/// the HID report works at once (its CCCD given back). FORGET erases it; a phone with another key is refused.
#[test]
fn the_bonded_phone_comes_back() {
    let (mut sim, mut app) = unit(iphone(Vec::new()));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 3000);
    assert!(stored(&sim).and_then(|s| s.bond).is_some());

    let mut sim2 = Sim::new(9, 5, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    let mut phone = PhoneSim::new(false); // no AMS: media keys go as HID
    (phone.key, phone.connect_at) = (Some(ble_sim::LTK), 0);
    sim2.nordic.phone = Some(phone);
    let mut app = App::start(&mut sim2, None);
    run(&mut sim2, &mut app, BOND_LOAD_MS as u64 + 1000);
    assert_eq!(sim2.nordic.got.iter().filter(|p| p[..2] == [0, 0x73]).count(), 1, "slow advertising, once");
    assert_eq!(app.phone.link(), Link::Secure);
    app.input(&mut sim2, Input::Media);
    run(&mut sim2, &mut app, 200);
    assert_eq!(sim2.nordic.phone.as_ref().unwrap().reports, [0x01, 0], "play / pause over HID");

    app.activate(&mut sim2, Menu::Phone, 5); // FORGET
    run(&mut sim2, &mut app, 500);
    assert_eq!(app.phone.link(), Link::Off);
    assert_eq!(sim2.nordic.server.adv, None);
    assert_eq!(stored(&sim2).map(|s| s.bond.is_none()), Some(true), "the bond erased on the card");

    // a phone with a key the unit does not have: no encryption, the link dropped
    let mut sim3 = Sim::new(9, 9, 0);
    sim3.emmc.image = crate::fat::tests::image();
    let mut phone = PhoneSim::new(true);
    (phone.key, phone.connect_at) = (Some([1; 16]), 0);
    sim3.nordic.phone = Some(phone);
    let mut app = App::start(&mut sim3, None);
    app.activate(&mut sim3, Menu::Phone, 1);
    run(&mut sim3, &mut app, 1000);
    assert_ne!(app.phone.link(), Link::Secure);
}

/// During a ride the phone's time is not taken: the ride's record times must not jump.
#[test]
fn no_clock_change_during_a_ride() {
    let (mut sim, mut app) = unit(iphone(Vec::new()));
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.phone.link(), Link::Secure);
    assert_eq!((sim.rtc_sets, app.aid.offset_15min), (0, None));
}

/// A chip that refuses a set-up command: the link is off, and the PHONE menu names the command and its error (for
/// the first runs on the unit, which has no log).
#[test]
fn a_refused_setup_is_named() {
    let mut sim = Sim::new(9, 0, 0);
    sim.nordic.refuse = Some(0xa2); // characteristic add
    let mut app = App::start(&mut sim, None);
    app.activate(&mut sim, Menu::Phone, 1);
    assert_eq!(app.phone.link(), Link::NoBle);
    assert_eq!(app.menu_row(Menu::Phone, 0).unwrap().1.as_str(), "ERR A2/0004");
}

/// ANT+ goes on while the phone pairs and talks: the strap's heart rate keeps coming.
#[test]
fn ant_alongside_the_phone() {
    let (mut sim, _) = unit(iphone(std::vec![(3000, Act::Notify(6, 3, "Mail", "Hi"))]));
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: 25126, data: |_| [4, 0, 0, 0, 0, 0, 0, 72] });
    sim.backup[crate::sensors::Kind::Hr.backup()] = SensorId { device: 25126, trans: 0xf1 }.to_word();
    let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8]));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.phone.link(), Link::Secure);
    assert_eq!(app.live.get(crate::sensors::Metric::Hr), Some(72));
    assert!(app.notices[0].as_str().ends_with("Mail: Hi"), "{}", app.notices[0].as_str());
}

/// The same screen drawn step by step and at once (tests.rs has the original).
fn assert_consistent(sim: &Sim, app: &App) {
    for y in 0..H {
        let (drawn, panel) = (app.render_row(y).px, &sim.screen[(y * W) as usize..((y + 1) * W) as usize]);
        assert!(drawn[..] == panel[..], "row {y}");
    }
}
