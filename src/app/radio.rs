//! The ANT+ sensors through the Nordic chip: each kind's channel (off, searching, paired), the rest after a search
//! timed out, and [`App::poll_nrf`], which reads the chip's events: broadcasts into the live values, the sensors
//! found while searching paired and kept in backup registers, BLE devices for the nearby list. The shifting
//! system's gear changes (counted, recorded) and its buttons ([`App::remote`]) are here too.

use crate::*;
use phone::Media;

/// How long a sensor's channel rests, closed, after its search timed out ([`nrf::SEARCH_TIMEOUT`]) before it
/// searches again: so a strap not worn costs the radio 30 s of listening in 90 instead of all the time.
pub const SEARCH_REST_MS: u32 = 60_000;

/// Each search in a row that finds nothing doubles the rest after it, up to this: a sensor left at home (a power
/// meter on the other bike) then searches about 40 s in 280 instead of 40 in 100 (the chip's high-priority search,
/// 10 s by default, before the 30 s low-priority one), about 1 mA less per absent sensor (docs/power.md, estimated).
/// A sensor heard, the rider's own action in SENSORS, or a ride's Start (the channels resting search again at once)
/// start from [`SEARCH_REST_MS`] again.
pub const SEARCH_REST_MAX_MS: u32 = 4 * SEARCH_REST_MS;

/// The rest after `misses` searches in a row that found nothing (the first: [`SEARCH_REST_MS`]).
///
/// ```
/// use bryton_rustfw::{search_rest_ms, SEARCH_REST_MAX_MS, SEARCH_REST_MS};
/// assert_eq!([0, 1, 2, 3, 9].map(search_rest_ms), [SEARCH_REST_MS, SEARCH_REST_MS, 120_000, 240_000, SEARCH_REST_MAX_MS]);
/// ```
pub fn search_rest_ms(misses: u8) -> u32 {
    (SEARCH_REST_MS << misses.saturating_sub(1).min(2)).min(SEARCH_REST_MAX_MS)
}

/// A sensor's channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sensor {
    /// Closed.
    Off,
    /// Open for any sensor of its kind: the first one heard is paired.
    Search,
    /// Open for this sensor.
    Paired(SensorId),
}

impl App {
    /// The channels resting after a search timed out ([`SEARCH_REST_MS`]) search again, `pass_ms` later each.
    pub(crate) fn reopen_rested(&mut self, b: &mut impl Board, pass_ms: u32) {
        for kind in Kind::ALL {
            let i = kind as usize;
            if self.radio.resting_ms[i] == u32::MAX {
                continue;
            }
            self.radio.resting_ms[i] = self.radio.resting_ms[i].saturating_add(pass_ms).min(u32::MAX - 1);
            if self.radio.resting_ms[i] >= search_rest_ms(self.radio.misses[i]) {
                let id = match self.radio.sensors[i] {
                    Sensor::Paired(id) => Some(id),
                    _ => None, // still pairing: search for any
                };
                let ok = nrf::ant_open(b, &mut self.rx, kind.channel(), kind.device_type(), kind.period(), id).is_ok();
                self.radio.resting_ms[i] = if ok { u32::MAX } else { 0 }; // failed: another rest
            }
        }
    }

    /// Opens or closes sensor `kind`'s channel to match `want`, and keeps the pairing. One shifting system at a
    /// time: opening one closes and forgets the other (review of 571e613: both open shared the gear, so a packet
    /// of each counted as a shift, and register 14, so pairing one overwrote the other while the menu showed both;
    /// no register is free for a second one).
    pub(crate) fn set_sensor(&mut self, b: &mut impl Board, kind: Kind, want: Sensor) {
        if kind.network() == 1 && !self.di2 {
            return; // no channel without Shimano's key
        }
        let other = match kind {
            Kind::Shifting => Some(Kind::Di2),
            Kind::Di2 => Some(Kind::Shifting),
            _ => None,
        };
        if let Some(other) = other.filter(|&o| want != Sensor::Off && self.radio.sensors[o as usize] != Sensor::Off) {
            self.set_sensor(b, other, Sensor::Off);
        }
        if matches!(kind, Kind::Trainer | Kind::Power) {
            self.tlink.pm = power_match::PowerMatch::new(); // another sensor: its offset is not this one's
        }
        let i = kind as usize;
        self.watch.owed[i] = Sensor::Off; // what the rider asks for now is what is wanted
        let resting = self.radio.resting_ms[i] != u32::MAX;
        if self.radio.sensors[i] != Sensor::Off && !resting && nrf::ant_close(b, &mut self.rx, kind.channel()).is_err()
        {
            return; // still open: leave it as it was
        }
        self.radio.resting_ms[i] = u32::MAX;
        self.radio.misses[i] = 0; // the rider's action: from the first rest again
        self.radio.sensors[i] = Sensor::Off;
        self.radio.heard_ms[i] = u32::MAX;
        let id = match want {
            Sensor::Off => None,
            Sensor::Search => None,
            Sensor::Paired(id) => Some(id),
        };
        // the two shifting kinds share a register: forgetting one leaves the other's pairing; the trainer is in
        // the sensors file (a word an earlier build left in the register goes with it, once that is written)
        match id {
            Some(id) => self.keep_pairing(b, kind, id),
            None if kind == Kind::Trainer => self.forget_trainer(),
            None if kind.from_word(b.backup_read(kind.backup())).is_some() => b.backup_write(kind.backup(), 0),
            None => {}
        }
        if want != Sensor::Off
            && nrf::ant_open(b, &mut self.rx, kind.channel(), kind.device_type(), kind.period(), id).is_ok()
        {
            self.radio.sensors[i] = want;
        }
    }

    /// Takes what the Nordic chip sent since the last pass: advertisers, sensor data, the scan's end, and the phone
    /// link's events (app/phone.rs), those kept while a command waited first.
    pub(crate) fn poll_nrf(&mut self, b: &mut impl Board) {
        let mut title = false;
        let mut buf = [0; 64];
        for _ in 0..64 {
            let n = b.nrf_recv(&mut buf);
            for &byte in buf.get(..n).unwrap_or(&[]) {
                let Some(p) = self.rx.push(byte) else { continue };
                let mut phone = false;
                if let Some(r) = nrf::adv_report(p) {
                    title |= self.radio.nearby.add(&r);
                    self.list_dirty = true;
                    self.list_dirty |= self.ftms.found(&r);
                } else if let Some((ch, data)) = nrf::ant_broadcast(p)
                    && let Some(kind) = Kind::from_channel(ch)
                {
                    if self.radio.heard_ms[kind as usize] >= sensors::STALE_MS {
                        // back after a drop-out: its counters may have moved on by any amount; the beats before it
                        // are not part of this series (and the beat time may have wrapped, a false interval)
                        self.decoders.restart(kind);
                        if kind == Kind::Hr {
                            self.hrv = Default::default();
                        }
                        if kind == Kind::Trainer {
                            self.trainer.lost();
                        }
                    }
                    self.decoders.decode(kind, data, self.settings.wheel_mm, &mut self.live);
                    if let (Kind::Power, Some(r)) = (kind, powercal::reply(data)) {
                        self.powercal.heard(r); // page 0x01: the calibration's answer (decode ignores it)
                    }
                    if kind == Kind::Trainer {
                        fec::decode(data, &mut self.trainer.reading);
                        self.fold_trainer();
                    }
                    for &rr in self.decoders.beats.rr() {
                        self.hrv.push(rr);
                        self.rec.full.beat(rr); // raw, into the full copy's hrv messages (RAM only: storage/recorder/mod.rs)
                    }
                    if matches!(kind, Kind::Shifting | Kind::Di2) {
                        self.shifted(); // its presses wait in the decoders until the stream is read (below)
                    }
                    self.radio.heard_ms[kind as usize] = 0;
                    self.radio.misses[kind as usize] = 0;
                    if self.radio.sensors[kind as usize] == Sensor::Search {
                        self.radio.ask |= 1 << ch;
                    }
                } else if let Some(kind) = nrf::ant_closed(p).and_then(Kind::from_channel) {
                    // its search timed out: reopened after a rest, longer for each search in a row that found nothing
                    self.radio.resting_ms[kind as usize] = 0;
                    let m = &mut self.radio.misses[kind as usize];
                    *m = m.saturating_add(1);
                } else if nrf::ant_tx_failed(p) == Some(Kind::Power.channel()) {
                    self.powercal.unsent(); // a calibration request the meter did not take: sent again
                } else if nrf::ant_tx_failed(p) == Some(Kind::Trainer.channel()) {
                    self.trainer.unsent(); // an acknowledged message the trainer did not take: sent again
                    self.tlink.fails = self.tlink.fails.saturating_add(1);
                } else if nrf::ant_tx_completed(p) == Some(Kind::Trainer.channel()) {
                    self.trainer.acked(); // a release taken is not sent again
                } else if nrf::event_id(p) == Some(nrf::EVT_TIMEOUT) && p.get(5) == Some(&1) {
                    self.radio.scanning = false; // the scan's (source 1)
                    title = true;
                } else {
                    phone = nrf::event_id(p).is_some();
                }
                if phone {
                    self.rx.keep_last(); // acted on below, the stream read
                }
            }
            if n == 0 {
                break;
            }
        }
        // the phone's events: those kept while a command waited, and those just read; acting on them may send
        // commands, whose waits keep more
        let mut ev = [0; nrf::KEPT];
        for _ in 0..32 {
            let Some(n) = self.rx.pop(&mut ev) else { break };
            self.phone_event(b, ev.get(..n).unwrap_or(&[]));
        }
        if title && self.screen == Screen::Nearby {
            ui::draw_nearby(&mut Panel(b, self.area(), self.look()), self.radio.nearby.count(), self.radio.scanning);
        }
        // a shifter's presses, once the stream is read (a media key sends a command): one per side
        for p in core::mem::take(&mut self.decoders.press).into_iter().flatten() {
            self.remote(b, p);
        }
    }

    /// The gear just decoded: a change while riding is a shift, counted and recorded (a gear change event, the
    /// front or the rear one; storage/recorder/mod.rs takes it into RAM, never waiting on the card).
    fn shifted(&mut self) {
        let Some(now) = self.live.get(Metric::Gear) else { return };
        let Some(was) = self.radio.gear.replace(now).filter(|&g| g != now) else { return };
        if self.ride.state() != ride::State::Running {
            return;
        }
        self.radio.shifts = self.radio.shifts.saturating_add(1);
        let (_, t) = self.rec_time();
        let front = now >> 8 != was >> 8;
        let [rear_n, front_n] = now.to_le_bytes();
        self.rec.clean.gear(t, front, front_n, rear_n);
        self.rec.full.gear(t, front, front_n, rear_n);
    }

    /// A shifter button pressed: the action the remote menu set for it (menu.rs [`menu::Remote`]). The ride's
    /// actions (pages, Lap, Start) act on the ride pages and the overview only, as P12's: never as keys, which
    /// in a menu would activate a row or step a number, on the media screen change the track, during a call answer
    /// it (review of 571e613). Lap on the overview marks a lap too, if a ride runs. The media actions only reach
    /// the phone, so they work everywhere.
    pub(crate) fn remote(&mut self, b: &mut impl Board, p: sensors::Press) {
        let action = self.settings.remote.get(p as usize).copied().unwrap_or(menu::Remote::None);
        let page = matches!(self.screen, Screen::Main | Screen::Ride(_));
        match action {
            menu::Remote::Play => return self.media(b, Media::PlayPause, false),
            menu::Remote::NextTrack => return self.media(b, Media::Next, false),
            menu::Remote::PrevTrack => return self.media(b, Media::Prev, false),
            menu::Remote::None => return,
            _ if !page => return,
            _ => {}
        }
        // a press is as a key for the light: it comes back after the time-out
        self.light.idle_ms = 0;
        if self.light.shown == 0 {
            self.update_light(b);
        }
        match action {
            menu::Remote::NextPage => self.page_by(b, true),
            menu::Remote::PrevPage => self.page_by(b, false),
            menu::Remote::Lap => self.lap(),
            menu::Remote::Start => self.start_key(b),
            _ => {}
        }
    }
}

/// What starting the Nordic chip came to ([`start_chip`]).
pub(crate) struct ChipStart {
    /// The BLE stack's version; None: the chip did not start.
    pub(crate) ble: Option<nrf::Version>,
    /// ANT started (needs BLE and a built-in ANT+ key).
    pub(crate) ant: bool,
    /// Shimano's network is up (its key built in): the Di2 channel can open.
    pub(crate) di2: bool,
    /// The chip answered ANT's start with a refusal (app/nrf_watch.rs).
    pub(crate) refusal: Option<(u8, u32)>,
    /// The sensors paired in the backup registers, their channels opened.
    pub(crate) sensors: [Sensor; Kind::ALL.len()],
}

/// Starts BLE, then ANT with `ant_key` (and Shimano's network with `di2_key`), and opens the channel of each sensor
/// kept in the backup registers.
pub(crate) fn start_chip(
    b: &mut impl Board,
    rx: &mut nrf::Rx,
    ant_key: Option<[u8; 8]>,
    di2_key: Option<[u8; 8]>,
) -> ChipStart {
    let ble = nrf::start(b).ok();
    let nets = Kind::ALL.map(Kind::network);
    let mut refusal = None;
    let (ant, di2) = match (ble, ant_key) {
        (Some(_), Some(key)) => match nrf::ant_start(b, rx, key, di2_key, &nets) {
            Ok(di2) => (true, di2),
            Err(e) => {
                refusal = rx.failed.filter(|_| matches!(e, nrf::Error::Code(_))); // an answer (app/nrf_watch.rs)
                (false, false)
            }
        },
        _ => (false, false),
    };
    let mut sensors = [Sensor::Off; Kind::ALL.len()];
    for (kind, s) in Kind::ALL.into_iter().zip(&mut sensors) {
        if let Some(id) = kind.from_word(b.backup_read(kind.backup()))
            && ant
            && (kind.network() == 0 || di2)
            && nrf::ant_open(b, rx, kind.channel(), kind.device_type(), kind.period(), Some(id)).is_ok()
        {
            *s = Sensor::Paired(id);
        }
    }
    ChipStart { ble, ant, di2, refusal, sensors }
}
