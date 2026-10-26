//! The phone link (app/phone.rs) against the simulated chip and a scripted phone (ble_sim.rs): pairing, the bond
//! on the card, reconnection, media keys (AMS and HID), notifications, a call, the time, and ANT+ alongside.

use super::tests::assert_consistent;
use super::*;
use crate::phone::Stored;

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

    // a message: its title and text in the banner and the notices (as much as two banner lines hold), at the phone's time
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.notices[0].as_str(), "09:41 Bob: Lunch at 1?");

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
    app.input(&mut sim, Input::Touch(Gesture::Swipe(Dir::Down))); // from the overview
    assert_eq!(app.screen, Screen::Media);
    app.input(&mut sim, Input::Touch(Gesture::Swipe(Dir::Up)));
    assert_eq!(app.screen, Screen::Main, "a swipe up closes it");
    app.activate(&mut sim, Menu::Phone, 2); // REMOTE
    assert_eq!(app.screen, Screen::Media);
    app.show(&mut sim);
    for i in [Input::Next, Input::Prev, Input::PrevHeld] {
        app.input(&mut sim, i);
    }
    run(&mut sim, &mut app, 200);
    app.input(&mut sim, Input::Touch(Gesture::Hold(120, 160))); // the shutter
    run(&mut sim, &mut app, 3000);
    app.media(&mut sim, crate::phone::Media::SkipFwd, false);
    assert_eq!(app.banner.as_str(), "NOT IN THIS APP", "not in the phone's list: not sent");
    let ph = sim.nordic.phone.as_ref().unwrap();
    assert_eq!(ph.commands, [2, 3, 4, 5], "toggle, next, previous, volume up: AMS");
    assert_eq!(ph.reports, [0x08, 0], "volume up pressed and released: HID");
    assert_eq!(app.phone.now.title.as_str(), "Song");
    assert_eq!(app.field_text(Field::Music).as_str(), "1:06", "64 s told, 2 s since");
    assert_consistent(&sim, &app);
    sim.save("phone_media");
}

/// A call: the overlay with the caller over the page, Start answers it (ANCS positive action); a second one declined
/// with Back; one ended on the phone takes the overlay away and the page is whole again.
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
    assert!(app.phone.ringing);
    assert_consistent(&sim, &app);
    sim.save("phone_call");
    // the date row's lower half is not left showing under the two-line banner
    let under = (crate::ui::BANNER_H * crate::ui::W) as usize..(crate::ui::TODAY_BOX.1 * crate::ui::W) as usize;
    assert!(sim.screen[under].iter().all(|&px| px == crate::ui::BLACK), "the date row is clear");
    app.input(&mut sim, Input::Select);
    run(&mut sim, &mut app, 3000);
    assert!(app.phone.ringing, "the second call");
    assert_consistent(&sim, &app);
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, 30))); // above the overlay: not an answer
    assert!(app.phone.ringing);
    let (x, y, w, h) = crate::ui::CALL_BUTTONS[0];
    app.input(&mut sim, Input::Touch(Gesture::Tap((x + w / 2) as u16, (y + h / 2) as u16))); // red: declined
    assert!(!app.phone.ringing);
    assert_consistent(&sim, &app);
    run(&mut sim, &mut app, 3000);
    assert!(app.phone.ringing, "the third");
    // a turn cue during the call: the caller stays in the banner's place, the cue waits for the call to end
    app.say_as("< LEFT 200 M", crate::Banner::Cue);
    run(&mut sim, &mut app, 1000);
    let mut call = crate::ui::tests::Frame::new();
    crate::ui::draw_call(&mut call, "Work");
    let top = (crate::ui::W * crate::ui::BANNER_H) as usize;
    assert!(sim.screen[..top] == call.px[..top], "the caller, not the cue");
    assert_consistent(&sim, &app);
    run(&mut sim, &mut app, 4000);
    assert!(!app.phone.ringing, "ended on the phone: closed");
    assert!(app.banner.as_str() == "< LEFT 200 M" && app.banner_drawn, "then the cue");
    assert_consistent(&sim, &app);
    assert_eq!(sim.nordic.phone.as_ref().unwrap().actions, [(9, 0), (10, 1)]);
}

/// A call over the media remote: Start answers, Back declines, the remote stays.
#[test]
fn a_call_beats_the_media_remote() {
    let script = std::vec![(3000, Act::Notify(1, 9, "Mum", "")), (6000, Act::Notify(1, 10, "Unknown", ""))];
    let (mut sim, mut app) = unit(iphone(script));
    app.activate(&mut sim, Menu::Phone, 1);
    app.open(Screen::Media);
    run(&mut sim, &mut app, 2500);
    assert!(app.phone.ringing);
    app.input(&mut sim, Input::Select);
    assert!(!app.phone.ringing, "answered, not play / pause");
    assert!(app.screen == Screen::Media);
    run(&mut sim, &mut app, 3500);
    assert!(app.phone.ringing);
    app.input(&mut sim, Input::Back);
    assert!(!app.phone.ringing, "declined");
    assert!(app.screen == Screen::Media, "the remote stays");
    assert_eq!(sim.nordic.phone.as_ref().unwrap().actions, [(9, 0), (10, 1)]);
}

/// Back on a MORE page declines a call instead of only hiding MORE.
#[test]
fn a_call_on_a_more_page() {
    let script = std::vec![(3000, Act::Notify(1, 9, "Mum", ""))];
    let (mut sim, mut app) = unit(iphone(script));
    app.activate(&mut sim, Menu::Phone, 1);
    app.open(Screen::Ride(0));
    app.input(&mut sim, Input::Touch(Gesture::Swipe(crate::Dir::Up)));
    assert!(app.more_shown().is_some());
    run(&mut sim, &mut app, 2500);
    assert!(app.phone.ringing);
    app.input(&mut sim, Input::Back);
    assert!(!app.phone.ringing, "declined");
    assert!(app.more_shown().is_some(), "MORE untouched");
    assert_eq!(sim.nordic.phone.as_ref().unwrap().actions, [(9, 1)]);
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

    app.activate(&mut sim2, Menu::Phone, 5); // FORGET: asked first
    assert!(app.banner.as_str().starts_with("FORGET THE PHONE?") && app.phone.bonded());
    app.activate(&mut sim2, Menu::Phone, 5); // confirmed
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

/// The bond is written only once the ride has settled, never while one runs: here the card is full, so the
/// recorder has stopped (done) though the ride goes on, and FORGET's erasing waits for the ride's end (the review of
/// 2026-10-25: the write was gated on the recorder only).
#[test]
fn the_bond_waits_for_the_ride_to_settle() {
    let (mut sim, mut app) = unit(iphone(Vec::new()));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 3000);
    let mut sim2 = Sim::new(9, 5, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    crate::fat::write::tests::fill(&mut sim2.emmc.image, 0, true);
    let mut phone = PhoneSim::new(true);
    (phone.key, phone.connect_at) = (Some(ble_sim::LTK), 0);
    sim2.nordic.phone = Some(phone);
    let mut app = App::start(&mut sim2, None);
    run(&mut sim2, &mut app, BOND_LOAD_MS as u64 + 1000);
    app.press(&mut sim2, Press::Short(Key::Start));
    run(&mut sim2, &mut app, 2000);
    assert_eq!(app.recorder.phase(), crate::recorder::Phase::Done, "the card full: not recording");
    app.activate(&mut sim2, Menu::Phone, 5);
    app.activate(&mut sim2, Menu::Phone, 5); // FORGET, confirmed
    run(&mut sim2, &mut app, 2000);
    assert!(stored(&sim2).and_then(|s| s.bond).is_some(), "not written during the ride");
    app.press(&mut sim2, Press::Short(Key::Start));
    save_ride(&mut sim2, &mut app);
    run(&mut sim2, &mut app, 1000);
    assert_eq!(stored(&sim2).map(|s| s.bond.is_none()), Some(true), "written once it ended");
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
    sim.nordic.sensors.push(AntSensor {
        device_type: 0x78,
        id: 25126,
        data: |_| [4, 0, 0, 0, 0, 0, 0, 72],
        network: 0,
        ack: &[],
    });
    sim.backup[crate::sensors::Kind::Hr.backup()] = SensorId { device: 25126, trans: 0xf1 }.to_word();
    let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8]));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.phone.link(), Link::Secure);
    assert_eq!(app.live.get(crate::sensors::Metric::Hr), Some(72));
    assert!(app.notices[0].as_str().ends_with("Mail: Hi"), "{}", app.notices[0].as_str());
}

/// Paired with one phone, PAIR lets another pair (or the same one after it forgot the unit): the window is the
/// menu's, not the advertising's, which stops at the connection (review, 2026-10-24).
#[test]
fn pair_another_phone_while_bonded() {
    let (mut sim, mut app) = unit(iphone(std::vec![(4000, Act::Drop)]));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 5000);
    assert!(app.phone.bonded() && app.phone.link() == Link::Waiting);
    let mut other = PhoneSim::new(true);
    other.connect_at = sim.ms + 500;
    sim.nordic.phone = Some(other);
    app.activate(&mut sim, Menu::Phone, 1); // PAIR
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.phone.link(), Link::Secure, "the new phone paired");
}

/// A stranger's phone near a bonded unit connects to its slow advertising but cannot pair: its CCCD writes do not
/// replace the bond's, and it is dropped once [`UNSECURED_MS`] pass without encryption, so it cannot hold the one
/// link the owner's phone needs.
#[test]
fn a_strangers_phone_cannot_hold_the_link() {
    let (mut sim, mut app) = unit(iphone(std::vec![(4000, Act::Drop)]));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 5000);
    let bond = app.phone.stored.bond.unwrap();
    let mut stranger = PhoneSim::new(false);
    stranger.connect_at = sim.ms;
    sim.nordic.phone = Some(stranger);
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.phone.link(), Link::Connected);
    // its view of the table: the HID report off, the battery on; it writes the battery's CCCD
    let server = &mut sim.nordic.server;
    let cccds: Vec<u16> = server.attrs.iter().filter(|a| a.2).map(|a| a.0).collect();
    for a in server.attrs.iter_mut().filter(|a| a.2) {
        a.3 = if a.0 == cccds[1] { std::vec![1, 0] } else { std::vec![0, 0] };
    }
    let mut w = std::vec![0, 0];
    w.extend(cccds[1].to_le_bytes());
    w.extend([0x02, 0x29, 1, 1, 0, 0, 0, 2, 0, 1, 0]);
    sim.nordic.evt(0x50, &w);
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.phone.stored.bond, Some(bond), "the bond's CCCDs kept");
    run(&mut sim, &mut app, u64::from(UNSECURED_MS));
    assert_eq!(app.phone.link(), Link::Waiting, "dropped: the owner's phone can come back");
}

/// A GATT time-out leaves the link unusable (no more ATT on it): the unit drops it, as Nordic's examples do, and
/// waits for the phone again.
#[test]
fn a_gatt_timeout_drops_the_link() {
    let (mut sim, mut app) = unit(iphone(Vec::new()));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.phone.link(), Link::Secure);
    sim.nordic.evt(0x3b, &[0, 0, 0, 0, 0, 0, 0]);
    run(&mut sim, &mut app, 500);
    assert_eq!(app.phone.link(), Link::Waiting);
}

/// The phone gone, what played is no longer counted on (the MUSIC field would tick on forever).
#[test]
fn now_playing_ends_with_the_link() {
    let (mut sim, mut app) = unit(iphone(std::vec![(4000, Act::Track("Song", "Band")), (9000, Act::Drop)]));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 4500);
    assert_eq!(app.phone.now.title.as_str(), "Song");
    run(&mut sim, &mut app, 5000);
    assert_eq!((app.phone.now.title.as_str(), app.field_text(Field::Music).as_str()), ("", "--"), "the field empty");
}

/// A burst of messages with a call among them: the call's name is still fetched (the call screen opens) and its
/// answer still sent; messages only take the queue's first slots.
#[test]
fn a_call_in_a_burst_of_messages() {
    let mut script: Vec<(u64, Act)> = (1..=5).map(|uid| (3000, Act::Notify(4, uid, "Group", "Hi"))).collect();
    script.push((3000, Act::Notify(1, 9, "Mum", "")));
    let (mut sim, mut app) = unit(iphone(script));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 4000);
    assert!(app.phone.ringing);
    app.input(&mut sim, Input::Select);
    run(&mut sim, &mut app, 2000);
    assert_eq!(sim.nordic.phone.as_ref().unwrap().actions, [(9, 0)]);
}

/// Three messages, then the overview's notices tapped: the NOTIFICATIONS list, newest first with the sender and the
/// time, a row opens the message wrapped over lines, Back returns up the stack; an empty list says so.
#[test]
fn notifications_list_and_detail() {
    const LONG: &str =
        "Meet at the old bridge by the market at noon, then we ride up to the lake and back along the river";
    let script = std::vec![
        (4000, Act::Notify(4, 7, "Ana", "Running late")),
        (4500, Act::Notify(6, 8, "Club", LONG)),
        (5000, Act::Notify(4, 9, "Bob", "Lunch at 1?")),
    ];
    let (mut sim, mut app) = unit(iphone(script));
    // nothing yet: the list says so
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, (ui::NOTICES_Y + 10) as u16)));
    assert_eq!(app.screen(), Screen::Menu(Menu::Notifs, 0));
    assert_eq!(app.menu_row(Menu::Notifs, 0).unwrap().0.as_str(), "NO NOTIFICATIONS");
    sim.save("notifs_empty");
    app.input(&mut sim, Input::Back);
    assert_eq!(app.screen(), Screen::Main);

    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 8000);
    assert_eq!(app.phone.inbox.len(), 3);

    app.input(&mut sim, Input::Touch(Gesture::Tap(120, (ui::NOTICES_Y + 10) as u16)));
    assert_eq!(app.screen(), Screen::Menu(Menu::Notifs, 0));
    let rows: Vec<_> = (0..4).map(|i| app.menu_row(Menu::Notifs, i).map(|r| (r.0, r.1))).collect();
    use std::string::ToString;
    let text = |i: usize| rows[i].map(|(a, b)| (a.as_str().to_string(), b.as_str().to_string()));
    assert_eq!(text(0), Some(("Bob".into(), "09:41".into())), "newest first, at the phone's time");
    assert_eq!(text(1).map(|r| r.0), Some("Club".into()));
    assert_eq!(text(2).map(|r| r.0), Some("Ana".into()));
    assert_eq!(text(3), None);
    sim.save("notifs_list");

    // a tap selects the row, a second one opens it
    let y = (ui::LIST_Y + ui::MENU_ROW + 10) as u16;
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, y)));
    assert_eq!(app.screen(), Screen::Menu(Menu::Notifs, 1));
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, y)));
    assert_eq!(app.screen(), Screen::Notif(1, 0));
    let lines: Vec<_> = app.notif_text(1).collect();
    assert_eq!(lines[0], ("Club", true));
    assert!(lines.len() > 3 && lines[1..].iter().all(|l| !l.1 && l.0.len() <= ui::notif_cols()));
    let joined: Vec<&str> = lines[1..].iter().map(|l| l.0).collect();
    assert_eq!(joined.join(" "), LONG, "the message whole, wrapped at spaces");
    sim.save("notifs_detail");
    // Page / Lap at the end of a short text change nothing
    app.input(&mut sim, Input::Next);
    assert_eq!(app.screen(), Screen::Notif(1, 0));

    // Back to the list, Back to the overview
    app.input(&mut sim, Input::Back);
    assert_eq!(app.screen(), Screen::Menu(Menu::Notifs, 1));
    app.input(&mut sim, Input::Back);
    assert_eq!(app.screen(), Screen::Main);
}

/// A ride keeps recording while the list and a detail are open, and a message that comes meanwhile does not move the
/// detail to another one.
#[test]
fn notifications_during_a_ride() {
    let script = std::vec![(4000, Act::Notify(4, 7, "Ana", "First")), (9000, Act::Notify(4, 8, "Bob", "Second"))];
    let (mut sim, mut app) = unit(iphone(script));
    app.activate(&mut sim, Menu::Phone, 1);
    app.leave_menus();
    app.input(&mut sim, Input::Select); // Start
    run(&mut sim, &mut app, 6000);
    assert_eq!(app.ride.state(), ride::State::Running);
    app.input(&mut sim, Input::Back); // clears the message's banner
    app.input(&mut sim, Input::Back); // to the overview
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, (ui::NOTICES_Y + 10) as u16)));
    assert_eq!(app.screen(), Screen::Menu(Menu::Notifs, 0), "reachable by a tap while recording");
    app.input(&mut sim, Input::Back);
    app.strip.locked = true;
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, (ui::NOTICES_Y + 10) as u16)));
    assert_eq!(app.screen(), Screen::Main, "the touch lock stops the tap");
    app.strip.locked = false;
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, (ui::NOTICES_Y + 10) as u16)));
    app.input(&mut sim, Input::Select);
    assert_eq!(app.screen(), Screen::Notif(0, 0));
    assert_eq!(app.notif_text(0).next(), Some(("Ana", true)));
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.phone.inbox.len(), 2);
    assert_eq!(app.screen(), Screen::Notif(1, 0));
    assert_eq!(app.notif_text(1).next(), Some(("Ana", true)), "still the same one");
    assert_eq!(app.ride.state(), ride::State::Running, "recording all along");
}

/// The row in the PHONE menu opens the list for the keys.
#[test]
fn notifications_row_in_the_phone_menu() {
    let (mut sim, mut app) = unit(iphone(std::vec![]));
    app.open(Screen::Menu(Menu::Phone, menu::PHONE.len()));
    app.activate(&mut sim, Menu::Phone, menu::PHONE.len());
    assert_eq!(app.screen(), Screen::Menu(Menu::Notifs, 0));
}

/// A message that arrives while the list is open, or a detail is open over it, moves the selection with it: Back
/// from the detail lands on the same message, and the rows on screen are the new ones.
#[test]
fn a_message_while_the_list_is_open() {
    let script = std::vec![
        (4000, Act::Notify(4, 7, "Ana", "First")),
        (5000, Act::Notify(4, 8, "Bob", "Second")),
        (9000, Act::Notify(4, 9, "Cy", "Third")),
    ];
    let (mut sim, mut app) = unit(iphone(script));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 7000);
    assert_eq!(app.phone.inbox.len(), 2);
    app.open(Screen::Menu(Menu::Notifs, 1)); // Ana, the older
    app.show(&mut sim);
    run(&mut sim, &mut app, 4000);
    assert_eq!(app.screen(), Screen::Menu(Menu::Notifs, 2), "still on Ana");
    assert_eq!(app.menu_row(Menu::Notifs, 2).unwrap().0.as_str(), "Ana");
    // and under a detail (the third arrives at 9 s: do it again with a fourth through keep_notif)
    app.input(&mut sim, Input::Select);
    assert_eq!(app.screen(), Screen::Notif(2, 0));
    let a = crate::phone::Attributes { uid: 99, ..Default::default() };
    app.keep_notif(&mut sim, &a);
    assert_eq!(app.screen(), Screen::Notif(3, 0));
    app.input(&mut sim, Input::Back);
    assert_eq!(app.screen(), Screen::Menu(Menu::Notifs, 3), "Back to the same message");
    for _ in 0..12 {
        app.keep_notif(&mut sim, &a);
    }
    assert_eq!(app.screen(), Screen::Menu(Menu::Notifs, crate::phone::INBOX - 1), "clamped");
}

/// The longest message the unit asks for (120 characters, in words that waste the most room) still fits the lines
/// of the detail: Page / Lap change nothing and no MORE is drawn. (Scrolling itself is tested in ui.rs.)
#[test]
fn the_longest_notification_fits() {
    let (mut sim, mut app) = unit(iphone(std::vec![]));
    let mut a = crate::phone::Attributes { uid: 7, ..Default::default() };
    crate::phone::fold(&mut a.title, b"A sender with a long name");
    crate::phone::fold(&mut a.message, "abcdefghijklmnopqrs ".repeat(6).as_bytes());
    app.keep_notif(&mut sim, &a);
    app.open(Screen::Menu(Menu::Notifs, 0));
    app.show(&mut sim);
    app.input(&mut sim, Input::Select);
    assert_eq!(app.screen(), Screen::Notif(0, 0));
    assert!(app.notif_lines(0) > 6 && app.notif_lines(0) <= ui::NOTIF_LINES, "{} lines", app.notif_lines(0));
    app.input(&mut sim, Input::Next);
    assert_eq!(app.screen(), Screen::Notif(0, 0));
    let mut f = crate::ui::tests::Frame::new();
    app.draw_notif(&mut f, 0, 0);
    assert!((ui::W - 60..ui::W).all(|x| (ui::LIST_Y..ui::LIST_Y + 20).all(|y| f.at(x, y) == ui::BLACK)), "no MORE");
}

/// The longest message the inbox keeps (120 bytes) with a long title arrives whole through the phone: its answer is
/// more fragments than one main loop pass used to keep (eight).
#[test]
fn the_longest_message_arrives_whole() {
    let long: &'static str = std::boxed::Box::leak("abcdefghijklmnopqrs ".repeat(6).into_boxed_str());
    let script = std::vec![(4000, Act::Notify(4, 7, "A sender with a long name", long))];
    let (mut sim, mut app) = unit(iphone(script));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 8000);
    assert_eq!(app.phone.inbox.len(), 1, "the answer was dropped");
}

/// A discovery answer lost (here the first service's, ANCS): the discovery goes on after its deadline, so AMS and
/// the time still come (review: a dropped answer left them off for the link).
#[test]
fn a_lost_discovery_answer_does_not_stop_it() {
    let (mut sim, mut app) = unit(iphone(Vec::new()));
    sim.nordic.drop_gattc = Some((0, 0x30));
    app.activate(&mut sim, Menu::Phone, 1); // PAIR
    run(&mut sim, &mut app, 10_000);
    assert_eq!(sim.nordic.drop_gattc, None, "the answer was lost");
    assert_eq!(app.phone.link(), Link::Secure);
    let ph = sim.nordic.phone.as_ref().unwrap();
    assert!(ph.subscribed.contains(&0x23) && ph.subscribed.contains(&0x33), "AMS and the time: {:?}", ph.subscribed);
    assert_eq!(sim.rtc_sets, 1, "the clock set from the phone");
}
