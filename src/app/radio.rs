//! The ANT+ sensors through the Nordic chip: each kind's channel (off, searching, paired), the rest after a search
//! timed out, and [`App::poll_nrf`], which reads the chip's events: broadcasts into the live values, the sensors
//! found while searching paired and kept in backup registers, BLE devices for the nearby list.

use crate::*;

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
    /// Opens or closes sensor `kind`'s channel to match `want`, and keeps the pairing.
    pub(crate) fn set_sensor(&mut self, b: &mut impl Board, kind: Kind, want: Sensor) {
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
        b.backup_write(kind.backup(), id.map_or(0, SensorId::to_word));
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
            ui::draw_nearby(&mut Panel(b, (0, self.bottom())), self.nearby.count(), self.scanning);
        }
    }
}
