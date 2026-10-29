//! The home trainer over Bluetooth (FTMS, drivers/ftms.rs; or FE-C over BLE, drivers/fecble.rs, for trainers without
//! FTMS: the same link, discovery and reconnection, other service, no Request Control, the FE-C pages of the ANT+
//! path written as frames): the unit as a central, beside the phone's link where it
//! is a peripheral (app/phone.rs). Paired from SENSORS (BT TRAINER: a scan lists the trainers that advertise FTMS),
//! kept in `\Rustfw\sensors.bin` with the ANT+ one, and then:
//! - **connected** whenever it is paired, the chip took the two links' configuration (`nrf::start`) and no ANT+
//!   trainer is paired (ANT+ first when both are); a connection not made in [`CONNECT_MS`] is cancelled, and a lost
//!   or failed one is tried again after a wait that doubles from [`FIRST_WAIT_MS`] to [`MAX_WAIT_MS`], at most
//!   [`MAX_FAILS`] times in a row (a trainer out of range is not scanned for all day) until the rider opens TRAINER
//!   CONTROL, the BT TRAINER row or starts a workout; a trainer that was connected during the ride is kept looked
//!   for;
//! - **discovered** one GATT procedure at a time (the service, its characteristics, the CCCDs of Indoor Bike Data,
//!   Machine Status and the Control Point), **subscribed** (notifications, the Control Point's indications, before
//!   anything is written to it, FTMS 4.16.3), its Feature and supported ranges **read**, and **Request Control**
//!   written; a GATT request with no answer in [`GATT_MS`] drops the link (the next connection starts over);
//! - **driven** from trainer.rs, as the ANT+ one: the command due ([`crate::trainer::Trainer::due`]) is written to the
//!   Control Point once the response to the last one came (one procedure at a time); a write refused, or not answered
//!   in [`CP_MS`], is due again (and a write whose answer was lost no longer holds the next one up); a status
//!   "control lost" (or a reset, or a response "not permitted") asks for the control again, and a trainer that
//!   refused it is asked again [`REGAIN_MS`] later, then less often, while a workout, a course or the rider drives it.
//!
//! Its readings go where the ANT+ trainer's go (`heard_ms` and `Reading` of the trainer kind): the same fields,
//! screen and priority (a power meter or a speed sensor heard wins, app/trainer.rs `fold_trainer`).

use crate::ble::{self, Event};
use crate::fecble::{self, Proto};
use crate::ftms::{self, BIKE_DATA, CONTROL, FEATURE, POWER_RANGE, RESISTANCE_RANGE, SERVICE, STATUS};
use crate::trainer::Command;
use crate::*;

/// A connection not made in this long is cancelled, ms.
pub const CONNECT_MS: u32 = 10_000;
/// The first wait before connecting again after a loss or a failure, ms; doubled each time up to [`MAX_WAIT_MS`],
/// back to this once the trainer is under control.
pub const FIRST_WAIT_MS: u32 = 2_000;
/// The longest wait between two connections, ms.
pub const MAX_WAIT_MS: u32 = 60_000;
/// Connections failed in a row after which the unit stops trying (see `App::ftms_retry`), unless the trainer was
/// connected during the ride running.
pub const MAX_FAILS: u8 = 5;
/// A GATT request (a discovery, a read, a write, Request Control's response) not answered in this long drops the link.
pub const GATT_MS: u32 = 5_000;
/// A command's response not come in this long: the command is due again.
pub const CP_MS: u32 = 3_000;
/// No data from a connected trainer for this long: the link is dropped and made again (a trainer notifies its data
/// about once a second while connected, FTMS 4.9.1; its disconnection may have been among bytes a resync of the
/// Nordic link dropped, app/nrf_watch.rs).
// ponytail: a trainer that goes quiet when nobody pedals is reconnected every 30 s; a longer wait if one does
pub const QUIET_MS: u32 = 30_000;
/// The control refused (or never answered, three times): asked again this long after, doubled each time up to
/// [`MAX_WAIT_MS`], while the trainer is driven.
pub const REGAIN_MS: u32 = 10_000;
/// The chip's `NRF_ERROR_BUSY`: another GATT procedure is out.
const NRF_ERROR_BUSY: u32 = 17;
/// Times the subscription or Request Control is tried again on one link (a trainer that refuses for ever is then
/// read without control).
const TRIES: u8 = 3;

/// The characteristics subscribed to, with the CCCD value: notifications, the Control Point's indications.
const NOTIFYING: [(u16, u8); 3] = [(BIKE_DATA, 1), (STATUS, 1), (CONTROL, 2)];
/// FE-C over BLE: the trainer's messages.
const NOTIFYING_FEC: [(u16, u8); 1] = [(fecble::NOTIFY, 1)];
/// The characteristics read once.
const READS: [u16; 3] = [FEATURE, RESISTANCE_RANGE, POWER_RANGE];

/// What is subscribed to on a link of the protocol (`fec`: FE-C over BLE).
fn notifying(fec: bool) -> &'static [(u16, u8)] {
    if fec { &NOTIFYING_FEC } else { &NOTIFYING }
}

/// Where the link is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// No link.
    Idle,
    /// Looking for the service.
    Service,
    /// Its characteristics, from a handle on.
    Chars(u16),
    /// The CCCD of [`NOTIFYING`]'s `n`th.
    Descs(u8),
    /// Writing it.
    Cccd(u8),
    /// Reading [`READS`]'s `n`th.
    Read(u8),
    /// Request Control written: its response awaited.
    Control,
    /// Discovered: commands go out (with the control).
    Ready,
}

/// A trainer heard while searching.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Found {
    /// Its address byte and address, as `ble::connect` takes them.
    pub(crate) peer: [u8; 7],
    /// The last report's signal strength, dBm.
    pub(crate) rssi: i8,
    name: [u8; 14],
    name_len: u8,
    /// What it speaks (FTMS when it offers both).
    pub(crate) proto: Proto,
}

impl Found {
    /// Its advertised name (printable ASCII), or empty.
    pub(crate) fn name(&self) -> &str {
        core::str::from_utf8(self.name.get(..usize::from(self.name_len)).unwrap_or(&[])).unwrap_or("")
    }
}

/// The Bluetooth trainer's link.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ftms {
    pub(crate) conn: Option<u16>,
    /// A connection being made: for so long.
    connecting: Option<u32>,
    /// Until the next connection may be tried, and the wait after the next failure.
    wait_ms: u32,
    backoff_ms: u32,
    step: Step,
    /// The service's last handle; its characteristics: (16-bit UUID, declaration handle, value handle).
    end: u16,
    chars: [(u16, u16, u16); 12],
    n_chars: usize,
    /// The link speaks FE-C over BLE (set when the connection is asked for), the vendor UUID type its base got
    /// (0: not registered with the chip yet; kept across links, not across a chip reset), and whether the trainer's
    /// frames carry the sum checksum (fecble.rs).
    fec: bool,
    vs: u8,
    fec_sum: bool,
    /// The CCCDs of [`NOTIFYING`].
    cccd: [u16; 3],
    /// A GATT request is out, for so long.
    waiting: bool,
    busy_ms: u32,
    /// A Control Point write of a command is out (its Written answer not come): Request Control waits for it, and the
    /// answer is not Request Control's.
    cp_inflight: bool,
    /// Connections failed in a row; the trainer was connected while a ride ran (kept looked for).
    fails: u8,
    kept: bool,
    /// Since its last data (or the connection).
    quiet_ms: u32,
    tries: u8,
    /// The trainer granted the control.
    pub(crate) control: bool,
    /// The control refused: since then, and the wait before it is asked again.
    regain_ms: u32,
    regain_wait: u32,
    /// A command's op code whose response is awaited, for so long.
    cp_wait: Option<u8>,
    cp_ms: u32,
    /// Its supported resistance (0.1 units) and power ranges, and the modes its Feature says.
    resistance: (i16, i16),
    max_w: u16,
    modes: Option<u8>,
    /// The search: on, and the trainers heard.
    pub(crate) searching: bool,
    pub(crate) found: [Found; 4],
    pub(crate) n_found: usize,
}

impl Default for Ftms {
    fn default() -> Self {
        Ftms {
            conn: None,
            connecting: None,
            wait_ms: 0,
            backoff_ms: FIRST_WAIT_MS,
            step: Step::Idle,
            end: 0,
            chars: [(0, 0, 0); 12],
            n_chars: 0,
            fec: false,
            vs: 0,
            fec_sum: false,
            cccd: [0; 3],
            waiting: false,
            busy_ms: 0,
            cp_inflight: false,
            fails: 0,
            kept: false,
            quiet_ms: 0,
            tries: 0,
            control: false,
            regain_ms: 0,
            regain_wait: REGAIN_MS,
            cp_wait: None,
            cp_ms: 0,
            resistance: (0, 100), // without the range read: 0 to 10.0
            max_w: trainer::MAX_W,
            modes: None,
            searching: false,
            found: [Found { peer: [0; 7], rssi: 0, name: [0; 14], name_len: 0, proto: Proto::Ftms }; 4],
            n_found: 0,
        }
    }
}

impl Ftms {
    /// The value handle of characteristic `uuid` (0: not found).
    fn handle(&self, uuid: u16) -> u16 {
        self.chars[..self.n_chars].iter().find(|c| c.0 == uuid).map_or(0, |c| c.2)
    }

    /// The link is gone (or given up): what belonged to it, and the wait before the next.
    fn unlink(&mut self) {
        let (found, n_found, searching, backoff_ms) = (self.found, self.n_found, self.searching, self.backoff_ms);
        *self = Ftms {
            found,
            n_found,
            searching,
            backoff_ms,
            fails: self.fails,
            kept: self.kept,
            vs: self.vs,
            ..Ftms::default()
        };
        self.retry_later();
    }

    fn retry_later(&mut self) {
        self.wait_ms = self.backoff_ms;
        self.backoff_ms = (self.backoff_ms * 2).min(MAX_WAIT_MS);
    }

    /// An advertising report heard while searching: a trainer that advertises FTMS or the FE-C over BLE service is
    /// listed with the protocol it speaks (its name, and the service itself, may come in the scan response, a report
    /// of its own; FTMS wins when both are heard). True when the list changed.
    pub(crate) fn found(&mut self, r: &nrf::AdvReport) -> bool {
        if !self.searching {
            return false;
        }
        let mut peer = [r.addr_type; 7];
        peer[1..].copy_from_slice(&r.addr);
        let known = self.found[..self.n_found].iter().position(|d| d.peer == peer);
        let proto = fecble::advertised(r.data);
        let i = match known {
            Some(i) => i,
            None if proto.is_some() && self.n_found < self.found.len() => {
                self.n_found += 1;
                self.n_found - 1
            }
            None => return false,
        };
        let Some(d) = self.found.get_mut(i) else { return false };
        (d.peer, d.rssi) = (peer, r.rssi);
        if let Some(p) = proto
            && (p == Proto::Ftms || known.is_none())
        {
            d.proto = p;
        }
        if known.is_none() {
            d.name_len = 0;
        }
        if let Some(name) = nrf::ad_name(r.data) {
            let n = name.len().min(d.name.len());
            for (slot, &c) in d.name.iter_mut().zip(&name[..n]) {
                *slot = if c.is_ascii_graphic() || c == b' ' { c } else { b'?' };
            }
            d.name_len = n as u8;
        }
        true
    }

    /// Commands may go out.
    pub(crate) fn ready(&self) -> bool {
        self.conn.is_some()
            && self.step == Step::Ready
            && self.control
            && self.cp_wait.is_none()
            && !(self.fec && self.cp_inflight)
    }
}

impl App {
    /// The Bluetooth trainer is the one used: paired, and no ANT+ trainer is (ANT+ first).
    pub(crate) fn ftms_on(&self) -> bool {
        self.sensors_store.bt.is_some() && self.radio.sensors.get(Kind::Trainer as usize) == Some(&Sensor::Off)
    }

    /// A BLE event that belongs to the trainer's link: on its connection, a connection made as a central, or the
    /// time-out of a connection being made (source 2).
    pub(crate) fn ftms_owns(&self, e: &Event) -> bool {
        // a connection by its role only: a handle freed by a lost DISCONNECTED may be the phone's next
        matches!(e, Event::Connected { role: 2, .. } | Event::Timeout { src: 2 })
            || (!matches!(e, Event::Connected { .. }) && e.conn().is_some() && e.conn() == self.ftms.conn)
    }

    /// The chip is about to be reset: the link is gone with it, tried again at once once it is back.
    pub(crate) fn ftms_down(&mut self) {
        let fails = self.ftms.fails;
        self.ftms.unlink();
        (self.ftms.wait_ms, self.ftms.fails, self.ftms.vs) = (0, fails, 0); // the chip forgets the UUID base too
        self.radio.heard_ms[Kind::Trainer as usize] = u32::MAX;
    }

    /// After a resync of the Nordic link, whose dropped bytes may have held the trainer's DISCONNECTED: a link the
    /// chip knows nothing of is over (a time-out is no answer: the link is left as it is).
    pub(crate) fn ftms_recheck(&mut self, b: &mut impl Board) {
        if let Some(conn) = self.ftms.conn
            && matches!(ble::sys_attr_get(b, &mut self.rx, conn, &mut [0; 8]), Err(nrf::Error::Code(_)))
        {
            self.ftms_lost();
        }
        self.ftms.cp_inflight = false; // its answer may have been among the bytes dropped
    }

    /// The rider asks for the trainer (TRAINER CONTROL, the BT TRAINER row, a workout started): the connection is
    /// tried at once, and [`MAX_FAILS`] more times if it fails.
    pub(crate) fn ftms_retry(&mut self) {
        (self.ftms.fails, self.ftms.wait_ms) = (0, 0);
    }

    /// A scan of the rider's is about to start (BLE NEARBY, the trainer search): the connection being made is given
    /// up, since the chip refuses a scan during one. The next attempt waits for the scan to end (`step_ftms`).
    pub(crate) fn ftms_hold(&mut self, b: &mut impl Board) {
        if self.ftms.connecting.take().is_some() {
            let _ = ble::connect_cancel(b, &mut self.rx);
        }
    }

    /// SENSORS' BT TRAINER: a scan, and the list of the trainers heard.
    pub(crate) fn ftms_search(&mut self, b: &mut impl Board) {
        if !self.ble.is_some_and(|v| v.central) {
            return self.say("NO BT TRAINER\nRADIO: ONE LINK");
        }
        (self.ftms.searching, self.ftms.n_found) = (true, 0);
        self.ftms_retry();
        self.ftms_hold(b);
        if !self.radio.scanning {
            self.radio.scanning = nrf::scan_start(b, &mut self.rx, SCAN_S).is_ok();
        }
        self.open(Screen::Menu(Menu::BtTrainers, 0));
    }

    /// The trainer `i` of the list paired: kept in the sensors file, the ANT+ trainer forgotten (one trainer), the
    /// scan stopped, the connection made at once.
    pub(crate) fn ftms_pair(&mut self, b: &mut impl Board, i: usize) {
        let Some(peer) = self.ftms.found.get(i).filter(|_| i < self.ftms.n_found).map(|d| d.peer) else { return };
        let fec = self.ftms.found.get(i).is_some_and(|d| d.proto == Proto::Fec);
        if self.radio.sensors[Kind::Trainer as usize] != Sensor::Off {
            self.set_sensor(b, Kind::Trainer, Sensor::Off);
        }
        let changed = self.sensors_store.bt != Some(peer) || self.sensors_store.bt_fec != fec;
        if changed {
            self.ftms_drop(b); // a link to the trainer paired before is not this one's
        }
        self.sensors_store.dirty |= changed;
        (self.sensors_store.bt, self.sensors_store.bt_fec) = (Some(peer), fec);
        if self.radio.scanning {
            let _ = ble::scan_stop(b, &mut self.rx);
            self.radio.scanning = false;
        }
        self.ftms.searching = false;
        self.ftms_retry();
        self.ftms.backoff_ms = FIRST_WAIT_MS;
        self.close();
        self.notice("BT TRAINER PAIRED");
    }

    /// The Bluetooth trainer forgotten (SENSORS: BT TRAINER, confirmed).
    pub(crate) fn ftms_forget(&mut self, b: &mut impl Board) {
        self.sensors_store.dirty |= self.sensors_store.bt.is_some();
        self.sensors_store.bt = None;
        self.ftms_drop(b);
        self.radio.heard_ms[Kind::Trainer as usize] = u32::MAX;
    }

    /// Ends the link (or the connection being made) from this side: forgotten at once, its last events going to
    /// nobody (the phone's handler checks its own connection).
    fn ftms_drop(&mut self, b: &mut impl Board) {
        if let Some(conn) = self.ftms.conn {
            let _ = ble::disconnect(b, &mut self.rx, conn);
        } else if self.ftms.connecting.is_some() {
            let _ = ble::connect_cancel(b, &mut self.rx);
        } else {
            return;
        }
        self.ftms_lost();
    }

    pub(crate) fn ftms_lost(&mut self) {
        let had = self.ftms.conn.is_some();
        self.ftms.fails = self.ftms.fails.saturating_add(u8::from(!had)); // a connection not made
        self.ftms.unlink();
        if had && self.ftms_on() {
            self.radio.heard_ms[Kind::Trainer as usize] = u32::MAX; // its readings stop now, not 5 s later
        }
    }

    /// Once a pass (from `step_trainer`, before the command): the connection made, cancelled or dropped, the
    /// answers that do not come, Request Control asked again.
    pub(crate) fn step_ftms(&mut self, b: &mut impl Board) {
        let dt = self.dt_ms;
        let want = self.ftms_on() && self.ble.is_some_and(|v| v.central);
        let riding = self.ride.state() != ride::State::Idle;
        // refused (a trainer without a Control Point or its CCCD has nothing to ask with), and driven: asked again
        let f = &self.ftms;
        let regain = f.step == Step::Ready
            && !f.control
            && !f.fec
            && f.handle(CONTROL) != 0
            && f.cccd[2] != 0
            && self.trainer.engaged(&self.trainer_drive());
        // the list left without pairing: the reports of a scan still running (BLE NEARBY) are not listed
        self.ftms.searching &= matches!(self.screen, Screen::Menu(Menu::BtTrainers, _));
        let f = &mut self.ftms;
        f.kept = riding && (f.kept || f.conn.is_some()); // connected while a ride ran: looked for while it runs
        if f.conn.is_some() || f.connecting.is_some() {
            f.busy_ms = f.busy_ms.saturating_add(dt);
            f.cp_ms = f.cp_ms.saturating_add(dt);
            f.quiet_ms = if f.conn.is_some() { f.quiet_ms.saturating_add(dt) } else { 0 };
            let connecting = f.connecting.map(|ms| ms.saturating_add(dt));
            f.connecting = connecting;
            let stuck = (f.waiting && f.busy_ms >= GATT_MS) || f.quiet_ms >= QUIET_MS;
            if !want || stuck || connecting.is_some_and(|ms| ms >= CONNECT_MS) {
                return self.ftms_drop(b);
            }
            if (f.cp_wait.is_some() || f.cp_inflight) && f.cp_ms >= CP_MS {
                // no answer: due again, and a write answer lost does not hold the next write (or Request Control) up
                if f.cp_wait.take().is_some() {
                    self.trainer.unsent();
                }
                f.cp_inflight = false;
            }
            if regain {
                // later, then less often: a trainer that says no is not asked every pass
                f.regain_ms = f.regain_ms.saturating_add(dt);
                if f.regain_ms >= f.regain_wait {
                    (f.regain_ms, f.regain_wait) = (0, (f.regain_wait * 2).min(MAX_WAIT_MS));
                    (f.step, f.tries) = (Step::Control, 0);
                }
            }
            if !f.waiting && !f.cp_inflight && f.step == Step::Control {
                self.ftms_next(b); // Request Control again (refused, or the control lost)
            }
            return;
        }
        f.wait_ms = f.wait_ms.saturating_sub(dt);
        if !want || self.radio.scanning || f.wait_ms > 0 || (f.fails >= MAX_FAILS && !f.kept) {
            return; // a scan running (a search, BLE NEARBY) would hold the connection up
        }
        let peer = self.sensors_store.bt.unwrap_or_default();
        let fec = self.sensors_store.bt_fec;
        if fec && self.ftms.vs == 0 {
            // the service's 128-bit base, once per chip start (the phone link took 8 of the 10 the SoftDevice holds)
            match ble::uuid_vs_add(b, &mut self.rx, &fecble::BASE) {
                Ok(t) => self.ftms.vs = t,
                Err(_) => {
                    self.ftms.fails = self.ftms.fails.saturating_add(1);
                    self.ftms.retry_later();
                    return;
                }
            }
        }
        match ble::connect(b, &mut self.rx, &peer) {
            Ok(()) => (self.ftms.connecting, self.ftms.busy_ms, self.ftms.fec) = (Some(0), 0, fec),
            Err(_) => {
                self.ftms.fails = self.ftms.fails.saturating_add(1);
                self.ftms.retry_later();
            }
        }
    }

    /// The command due goes out on the Control Point (app/trainer.rs `send_trainer` for the Bluetooth trainer): the
    /// rider's power, resistance or grade; a write the chip refuses is due again.
    pub(crate) fn ftms_command(&mut self, b: &mut impl Board, c: Command) {
        let f = &mut self.ftms;
        let c = match c {
            Command::Power(q) => Command::Power(q.min(f.max_w.saturating_mul(4))),
            c => c,
        };
        let (Some((bytes, n)), Some(conn)) = (ftms::encode(c, f.resistance), f.conn) else { return };
        let data = bytes.get(..n).unwrap_or(&[]);
        if ble::write(b, &mut self.rx, conn, f.handle(CONTROL), data).is_ok() {
            (f.cp_wait, f.cp_ms, f.cp_inflight, f.busy_ms) = (data.first().copied(), 0, true, 0);
            self.tlink.cmds |= fit::cmd_bit(fec::encode(c)[0]); // the ride file names it by its FE-C page
        } else {
            self.trainer.unsent();
        }
    }

    /// FE-C over BLE: `page` written to the trainer as a frame (app/trainer.rs `send_trainer`), one write in flight.
    /// False when it could not go: the caller has it due again.
    pub(crate) fn ftms_fec_send(&mut self, b: &mut impl Board, page: [u8; 8]) -> bool {
        let f = &mut self.ftms;
        let (Some(conn), false) = (f.conn, f.cp_inflight) else { return false };
        let handle = f.handle(fecble::WRITE);
        let frame = fecble::frame(page, f.fec_sum);
        if handle == 0 || ble::write(b, &mut self.rx, conn, handle, &frame).is_err() {
            return false;
        }
        (f.cp_wait, f.cp_ms, f.cp_inflight) = (Some(page[0]), 0, true);
        true
    }

    /// A BLE event of the trainer's link ([`App::ftms_owns`]).
    pub(crate) fn ftms_event(&mut self, b: &mut impl Board, e: Event) {
        let rx = &mut self.rx;
        match e {
            Event::Connected { conn, .. } => {
                (self.ftms.conn, self.ftms.connecting, self.ftms.step) = (Some(conn), None, Step::Service);
                self.ftms.fails = 0;
                self.ftms_next(b);
            }
            Event::Disconnected { .. } => self.ftms_lost(),
            Event::Timeout { .. } => {
                self.ftms.fails = self.ftms.fails.saturating_add(1);
                self.ftms.connecting = None;
                self.ftms.retry_later();
            }
            Event::ConnParamRequest { conn, params } => {
                let _ = ble::conn_param_update(b, rx, conn, params);
            }
            Event::ParamRequest { conn, id: 0x21 } => {
                let _ = ble::phy_update(b, rx, conn);
            }
            Event::ParamRequest { conn, .. } => {
                let _ = ble::data_length_update(b, rx, conn);
            }
            Event::MtuRequest { conn } => {
                let _ = ble::exchange_mtu_reply(b, rx, conn);
            }
            Event::SysAttrMissing { conn } => {
                let _ = ble::sys_attr_set(b, rx, conn, None); // our server, read by the trainer: nothing kept
            }
            Event::SecParamsRequest { conn } => {
                let _ = ble::sec_params_reply(b, rx, conn, false); // no pairing as a central (no security role)
            }
            Event::UserMemRequest { conn } => {
                let _ = ble::user_mem_reply(b, rx, conn);
            }
            Event::GattTimeout { .. } => self.ftms_drop(b),
            Event::Hvx { conn, handle, kind, data } => {
                if kind == 2 {
                    let _ = ble::hv_confirm(b, rx, conn, handle);
                }
                self.ftms_hvx(b, handle, data);
            }
            Event::Services { .. } | Event::Chars { .. } | Event::Descs { .. } | Event::Read { .. } => {
                self.ftms_answer(b, e)
            }
            Event::Written { .. } => self.ftms_answer(b, e),
            _ => {}
        }
    }

    /// A GATT client answer: the discovery's next step, or a command's write refused.
    fn ftms_answer(&mut self, b: &mut impl Board, e: Event) {
        let f = &mut self.ftms;
        let step = f.step;
        // the answer to a command's write, not to the discovery's (the control may have been lost while it was out)
        let cp_answer = matches!(e, Event::Written { .. }) && core::mem::take(&mut f.cp_inflight);
        f.step = match (step, e) {
            (Step::Service, Event::Services { status: 0, list, .. }) => {
                let service = if f.fec { (fecble::SERVICE, f.vs) } else { (SERVICE, ble::UUID_BLE) };
                match ble::services(list).find(|(u, ..)| *u == service) {
                    Some((_, start, end)) => {
                        f.end = end;
                        Step::Chars(start)
                    }
                    None => {
                        self.notice(if self.ftms.fec { "NOT A FE-C TRAINER" } else { "NOT AN FTMS TRAINER" });
                        return self.ftms_drop(b);
                    }
                }
            }
            (Step::Service, Event::Services { .. }) => return self.ftms_drop(b),
            (Step::Chars(from), Event::Chars { status: 0, list, .. }) if !list.is_empty() => {
                let mut last = 0;
                for ((uuid, t), _, decl, value) in ble::chars(list) {
                    if let Some(c) = f.chars.get_mut(f.n_chars) {
                        // the 16-bit part counts for the service's own base only
                        *c = (if t == (if f.fec { f.vs } else { ble::UUID_BLE }) { uuid } else { 0 }, decl, value);
                        f.n_chars += 1;
                    }
                    last = value;
                }
                // handles that do not move forward end it (a peer must not keep us asking)
                if last >= from && last < f.end { Step::Chars(last + 1) } else { Step::Descs(0) }
            }
            (Step::Chars(_), Event::Chars { .. }) => Step::Descs(0),
            (Step::Descs(i), Event::Descs { status: 0, list, .. }) => {
                if let (Some((h, _)), Some(slot)) =
                    (ble::descs(list).find(|(_, u)| *u == (ble::CCCD, ble::UUID_BLE)), f.cccd.get_mut(usize::from(i)))
                {
                    *slot = h;
                }
                Step::Descs(i + 1)
            }
            (Step::Descs(i), Event::Descs { .. }) => Step::Descs(i + 1),
            (Step::Cccd(i), Event::Written { .. }) if !cp_answer => Step::Cccd(i + 1),
            (Step::Read(i), Event::Read { status, handle, data, .. }) => {
                if status == 0 && handle == f.handle(FEATURE) {
                    f.modes = ftms::modes(data);
                } else if status == 0 && handle == f.handle(RESISTANCE_RANGE) {
                    f.resistance = ftms::range(data).unwrap_or(f.resistance);
                } else if status == 0 && handle == f.handle(POWER_RANGE) {
                    f.max_w = ftms::range(data).map_or(f.max_w, |(_, hi)| hi.clamp(0, trainer::MAX_W as i16) as u16);
                }
                Step::Read(i + 1)
            }
            (Step::Control, Event::Written { .. }) if cp_answer => return, // a command's, the control lost since
            (Step::Control, Event::Written { status: 0, .. }) => {
                (f.waiting, f.busy_ms) = (true, 0); // its response is an indication
                return;
            }
            // the trainer forgot the subscription (FTMS 4.16.3): subscribed again, a few times
            (Step::Control | Step::Ready, Event::Written { status: ftms::CCCD_NOT_SET, .. }) if f.tries < TRIES => {
                f.tries += 1;
                if step == Step::Ready && f.cp_wait.take().is_some() {
                    self.trainer.unsent();
                }
                Step::Cccd(0)
            }
            (Step::Control, Event::Written { .. }) => {
                f.tries += 1; // refused (a procedure in progress): asked again at the next pass, a few times
                f.waiting = false;
                return;
            }
            (Step::Ready, Event::Written { status, .. }) => {
                if status != 0 && f.cp_wait.take().is_some() {
                    self.trainer.unsent(); // refused: due again
                } else if f.fec && f.cp_wait.take().is_some() {
                    self.trainer.acked(); // written: the answer, if any, is a page 71 among the notifications
                }
                return;
            }
            _ => return, // not the answer awaited: the request stays out
        };
        self.ftms.waiting = false;
        self.ftms_next(b);
    }

    /// Sends the discovery's next request (skipping what is not there), or ends it.
    fn ftms_next(&mut self, b: &mut impl Board) {
        let Some(conn) = self.ftms.conn else { return };
        for _ in 0..16 {
            let rx = &mut self.rx;
            let f = &mut self.ftms;
            let sent = match f.step {
                Step::Service => {
                    let service = if f.fec { (fecble::SERVICE, f.vs) } else { (SERVICE, ble::UUID_BLE) };
                    ble::discover_service(b, rx, conn, service).is_ok()
                }
                Step::Chars(from) => ble::discover_chars(b, rx, conn, from, f.end).is_ok(),
                Step::Descs(i) => match notifying(f.fec).get(usize::from(i)) {
                    Some(&(uuid, _)) => {
                        // the descriptors lie between the value and the next declaration (or the service's end)
                        let value = f.handle(uuid);
                        let next = f.chars[..f.n_chars].iter().map(|c| c.1).filter(|&d| d > value).min();
                        let end = next.map_or(f.end, |d| d - 1);
                        value != 0 && value < end && ble::discover_descs(b, rx, conn, value + 1, end).is_ok()
                    }
                    None => {
                        f.step = Step::Cccd(0);
                        continue;
                    }
                },
                Step::Cccd(i) => match (notifying(f.fec).get(usize::from(i)), f.cccd.get(usize::from(i))) {
                    (Some(&(_, v)), Some(&h)) => h != 0 && ble::write(b, rx, conn, h, &[v, 0]).is_ok(),
                    _ => {
                        f.step = Step::Read(0);
                        continue;
                    }
                },
                Step::Read(i) => match READS.get(usize::from(i)).filter(|_| !f.fec) {
                    Some(&u) => f.handle(u) != 0 && ble::read(b, rx, conn, f.handle(u)).is_ok(),
                    None => {
                        f.step = Step::Control;
                        continue;
                    }
                },
                Step::Control if f.fec => {
                    // no control to ask for: the trainer takes the pages as they come
                    let write = f.handle(fecble::WRITE) != 0;
                    (f.step, f.control, f.tries, f.backoff_ms) = (Step::Ready, write, 0, FIRST_WAIT_MS);
                    self.trainer.resend();
                    if !write {
                        self.notice("BT TRAINER: NO CONTROL");
                    }
                    return;
                }
                Step::Control => {
                    let cp = f.handle(CONTROL);
                    if cp == 0 || f.cccd[2] == 0 || f.tries >= TRIES {
                        f.step = Step::Ready; // read only: no Control Point, or no control given
                        if f.regain_wait != REGAIN_MS {
                            return; // said when it was first refused
                        }
                        return self.notice("BT TRAINER: NO CONTROL");
                    }
                    if f.cp_inflight {
                        return; // a command's write is out: one GATT procedure at a time
                    }
                    match ble::write(b, rx, conn, cp, &ftms::REQUEST_CONTROL) {
                        Ok(()) => true,
                        Err(nrf::Error::Code(NRF_ERROR_BUSY)) => return, // not a refusal: again at the next pass
                        Err(_) => {
                            f.tries += 1; // the chip refused: again at the next pass
                            return;
                        }
                    }
                }
                Step::Idle | Step::Ready => return,
            };
            if sent {
                (f.waiting, f.busy_ms) = (true, 0);
                return;
            }
            // nothing to ask (or the chip refused): the next step
            f.step = match f.step {
                Step::Service => return self.ftms_drop(b),
                Step::Chars(_) => Step::Descs(0),
                Step::Descs(i) => Step::Descs(i + 1),
                Step::Cccd(i) => Step::Cccd(i + 1),
                Step::Read(i) => Step::Read(i + 1),
                s => s,
            };
        }
    }

    /// A notification or indication from the trainer: its data, its status, the Control Point's responses.
    fn ftms_hvx(&mut self, b: &mut impl Board, handle: u16, data: &[u8]) {
        let f = &mut self.ftms;
        if f.fec {
            if handle != 0 && handle == f.handle(fecble::NOTIFY) {
                self.fec_message(data);
            }
        } else if handle == 0 {
        } else if handle == f.handle(BIKE_DATA) {
            if self.radio.heard_ms[Kind::Trainer as usize] >= sensors::STALE_MS {
                self.trainer.lost(); // back after a drop-out: what was sent is not known to stand
            }
            ftms::bike_data(data, &mut self.trainer.reading);
            self.trainer.reading.modes = self.ftms.modes;
            self.radio.heard_ms[Kind::Trainer as usize] = 0;
            self.ftms.quiet_ms = 0;
            self.fold_trainer();
        } else if handle == f.handle(STATUS) {
            if ftms::control_lost(data) && f.control {
                f.control = false;
                if f.cp_wait.take().is_some() {
                    self.trainer.unsent();
                }
                self.trainer.resend();
                if self.ftms.step == Step::Ready {
                    (self.ftms.step, self.ftms.tries) = (Step::Control, 0);
                    self.ftms_next(b); // waits for a command's write still out (step_ftms asks again)
                }
            }
        } else if handle == f.handle(CONTROL) {
            let Some((op, result)) = ftms::response(data) else { return };
            if op == ftms::REQUEST_CONTROL[0] && f.step == Step::Control {
                (f.waiting, f.step, f.control) = (false, Step::Ready, result == 1);
                if result == 1 {
                    (f.tries, f.backoff_ms, f.regain_wait) = (0, FIRST_WAIT_MS, REGAIN_MS);
                    self.trainer.resend(); // what stood before is sent again, under this control
                } else if f.regain_wait == REGAIN_MS {
                    self.notice("BT TRAINER: NO CONTROL"); // the first refusal: asked again later, quietly
                }
            } else if f.cp_wait == Some(op) {
                f.cp_wait = None;
                if result == 1 {
                    self.trainer.acked();
                }
                self.trainer.reading.last_cmd = ftms::as_fec((op, result));
                if result == 5 && f.step == Step::Ready {
                    // not permitted: the control is not ours any more
                    (f.control, f.step, f.tries) = (false, Step::Control, 0);
                    self.trainer.resend();
                    self.ftms_next(b);
                }
            }
        }
    }

    /// A notification of an FE-C over BLE trainer: a frame, its page decoded as the ANT+ path does (a frame of
    /// another shape or with a wrong checksum is dropped).
    fn fec_message(&mut self, data: &[u8]) {
        let Some((page, sum)) = fecble::parse(data) else { return };
        if self.radio.heard_ms[Kind::Trainer as usize] >= sensors::STALE_MS {
            self.trainer.lost(); // back after a drop-out: what was sent is not known to stand
        }
        (self.ftms.fec_sum, self.ftms.quiet_ms) = (sum, 0);
        fec::decode(page, &mut self.trainer.reading);
        self.radio.heard_ms[Kind::Trainer as usize] = 0;
        self.fold_trainer();
    }
}
