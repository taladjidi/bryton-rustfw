//! The ANT+ sensors through the Nordic chip: each kind's channel (off, searching, paired), the rest after a search
//! timed out, and [`App::poll_nrf`], which reads the chip's events: broadcasts into the live values, the sensors
//! found while searching paired and kept in backup registers, BLE devices for the nearby list. The shifting
//! system's gear changes (counted, recorded) and its buttons ([`App::remote`]) are here too.

use crate::*;
use phone::Media;

/// How long a sensor's channel rests, closed, after its search timed out ([`nrf::SEARCH_TIMEOUT`]) before it
/// searches again: so a strap not worn costs the radio 30 s of listening in 90 instead of all the time.
// ponytail: fixed; during a ride a lost sensor should come back sooner (search again at once on the first drop-out)
pub const SEARCH_REST_MS: u32 = 60_000;

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
        if let Some(other) = other.filter(|&o| want != Sensor::Off && self.sensors[o as usize] != Sensor::Off) {
            self.set_sensor(b, other, Sensor::Off);
        }
        let i = kind as usize;
        let resting = self.resting_ms[i] != u32::MAX;
        if self.sensors[i] != Sensor::Off && !resting && nrf::ant_close(b, &mut self.rx, kind.channel()).is_err() {
            return; // still open: leave it as it was
        }
        self.resting_ms[i] = u32::MAX;
        self.sensors[i] = Sensor::Off;
        self.heard_ms[i] = u32::MAX;
        let id = match want {
            Sensor::Off => None,
            Sensor::Search => None,
            Sensor::Paired(id) => Some(id),
        };
        // the two shifting kinds share a register: forgetting one leaves the other's pairing
        if id.is_some() || kind.from_word(b.backup_read(kind.backup())).is_some() {
            b.backup_write(kind.backup(), id.map_or(0, |id| kind.to_word(id)));
        }
        if want != Sensor::Off
            && nrf::ant_open(b, &mut self.rx, kind.channel(), kind.device_type(), kind.period(), id).is_ok()
        {
            self.sensors[i] = want;
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
                    title |= self.nearby.add(&r);
                    self.list_dirty = true;
                } else if let Some((ch, data)) = nrf::ant_broadcast(p)
                    && let Some(kind) = Kind::from_channel(ch)
                {
                    if self.heard_ms[kind as usize] >= sensors::STALE_MS {
                        // back after a drop-out: its counters may have moved on by any amount; the beats before it
                        // are not part of this series (and the beat time may have wrapped, a false interval)
                        self.decoders.restart(kind);
                        if kind == Kind::Hr {
                            self.hrv = Default::default();
                        }
                    }
                    self.decoders.decode(kind, data, self.settings.wheel_mm, &mut self.live);
                    for &rr in self.decoders.beats.rr() {
                        self.hrv.push(rr);
                    }
                    if matches!(kind, Kind::Shifting | Kind::Di2) {
                        self.shifted(); // its presses wait in the decoders until the stream is read (below)
                    }
                    self.heard_ms[kind as usize] = 0;
                    if self.sensors[kind as usize] == Sensor::Search {
                        self.ask |= 1 << ch;
                    }
                } else if let Some(kind) = nrf::ant_closed(p).and_then(Kind::from_channel) {
                    self.resting_ms[kind as usize] = 0; // its search timed out: reopened after a rest
                } else if nrf::event_id(p) == Some(nrf::EVT_TIMEOUT) && p.get(5) == Some(&1) {
                    self.scanning = false; // the scan's (source 1)
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
            ui::draw_nearby(&mut Panel(b, self.area()), self.nearby.count(), self.scanning);
        }
        // a shifter's presses, once the stream is read (a media key sends a command): one per side
        for p in core::mem::take(&mut self.decoders.press).into_iter().flatten() {
            self.remote(b, p);
        }
    }

    /// The gear just decoded: a change while riding is a shift, counted and recorded (a gear change event, the
    /// front or the rear one; recorder.rs takes it into RAM, never waiting on the card).
    fn shifted(&mut self) {
        let Some(now) = self.live.get(Metric::Gear) else { return };
        let Some(was) = self.gear.replace(now).filter(|&g| g != now) else { return };
        if self.ride.state() != ride::State::Running {
            return;
        }
        self.shifts = self.shifts.saturating_add(1);
        let (_, t) = self.rec_time();
        let front = now >> 8 != was >> 8;
        let [rear_n, front_n] = now.to_le_bytes();
        self.recorder.gear(t, front, front_n, rear_n);
        self.full.gear(t, front, front_n, rear_n);
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
        self.idle_ms = 0;
        if self.light_shown == 0 && self.backlight {
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
