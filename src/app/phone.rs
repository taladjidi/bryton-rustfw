//! The phone link over BLE, with no companion app: this firmware is a peripheral the phone pairs with (Just Works,
//! one bond kept in `\Rustfw\ble.bin`), a GATT server for media keys (HID over GATT), the battery and the device
//! information, and a GATT client of the phone's own services: Apple's notifications (ANCS) and media (AMS), and
//! the Current Time Service. The formats are in phone.rs, the chip's calls in drivers/ble.rs.
//!
//! **Order after a link** (the usual one for a bonding peripheral, as in Nordic's SoftDevice message sequence
//! charts and its peer manager: recalled, inferred): connected; not bonded, a Security Request (the phone asks the rider to pair); the keys at
//! BLE_GAP_EVT_AUTH_STATUS; bonded, the phone encrypts with the key kept (BLE_GAP_EVT_SEC_INFO_REQUEST); once
//! encrypted, the CCCDs kept are given back (`sd_ble_gatts_sys_attr_set`) and the phone's services discovered one
//! GATT procedure at a time: ANCS, AMS, CTS; then their notifications enabled, AMS told what to report, the time read.
//!
//! **Power:** advertising only while pairing (the PHONE menu: every 152.5 ms for 60 s) or bonded and not connected
//! (every 1022.5 ms, so the phone finds the unit again), two of the intervals Apple's Accessory Design Guidelines
//! recommend (recalled, inferred). An advertising event every second costs the Nordic chip about 10 to 15 uA on
//! average (Nordic's online power profiler for the nRF52832, 0 dBm, 31-byte payload: inferred, not measured here),
//! next to the unit's milliamperes; connected, the preferred parameters (15 to 30 ms, slave latency 6: the chip
//! answers every 7th event when it has nothing to send, about 200 ms) keep the media keys quick without listening
//! every 15 ms.

use crate::ble::{self, Event};
use crate::phone::{self as ph, Media, Stored};
use crate::*;

/// How many passes a media key's release is tried (a pass is 50 ms).
const RELEASE_TRIES: u8 = 10;

/// Advertising while pairing: interval (0.625 ms units) and time-out (s).
pub const ADV_FAST: (u16, u16) = (244, 60);
/// Advertising while bonded and not connected: until a connection.
pub const ADV_SLOW: (u16, u16) = (1636, 0);
/// The preferred connection parameters: 15 to 30 ms, slave latency 6, supervision time-out 4 s (within Apple's
/// guidelines: max interval x (latency + 1) x 3 under the time-out, recalled, inferred).
pub const PPCP: ble::ConnParams = (12, 24, 6, 400);
/// The bond is read from the card this long after the start (the card is not touched at boot); the bonded phone
/// can come back from then on.
pub const BOND_LOAD_MS: u32 = 3000;
/// A link not encrypted after this long, outside a pairing, is dropped: a stranger's phone (which cannot pair
/// without PAIR) must not hold the one link the owner's phone needs.
pub const UNSECURED_MS: u32 = 30_000;

/// A discovery request (a service, its characteristics or descriptors, a subscription, a read) not answered in this
/// long is given up and the discovery goes on with the next step: an answer lost (among bytes a resync of the Nordic
/// link dropped) must not leave ANCS, AMS and the time off for the whole link.
pub const DISC_MS: u32 = 5_000;
/// The file the bond is kept in, in [`recorder::DIR`].
pub const BOND_FILE: &str = "ble.bin";

/// What the radio advertises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adv {
    /// Nothing.
    Off,
    /// Pairing ([`ADV_FAST`]).
    Fast,
    /// Bonded, waiting for the phone ([`ADV_SLOW`]).
    Slow,
}

// The phone's attributes the client uses (`Phone::h`).
const NS: usize = 0;
const NS_CCCD: usize = 1;
const CP: usize = 2;
const DS: usize = 3;
const DS_CCCD: usize = 4;
const RC: usize = 5;
const RC_CCCD: usize = 6;
const EU: usize = 7;
const EU_CCCD: usize = 8;
const CT: usize = 9;
const CT_CCCD: usize = 10;
const LTI: usize = 11;

/// The services discovered, in order: ANCS, AMS, CTS; and each one's notifying characteristics with their CCCDs.
const NOTIFYING: [&[(usize, usize)]; 3] =
    [&[(NS, NS_CCCD), (DS, DS_CCCD)], &[(RC, RC_CCCD), (EU, EU_CCCD)], &[(CT, CT_CCCD)]];

/// The client's progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Disc {
    /// Nothing yet (not encrypted).
    Idle,
    /// The Exchange MTU Request sent, its response awaited.
    Mtu,
    /// Looking for service `n`.
    Service(u8),
    /// Its characteristics, from a handle on.
    Chars(u8, u16),
    /// The CCCD of one of its notifying characteristics.
    Descs(u8),
    /// Notifications on, the `n`th CCCD of [`Phone::subscriptions`].
    Subscribe(u8),
    /// AMS told what to report.
    Register(u8),
    /// The time read: Current Time, then Local Time Information.
    Read(u8),
    /// Ready: operations run from the queue.
    Done,
}

impl Disc {
    /// The step after this one when it has nothing to ask, or its answer did not come (the CCCDs: the next one is
    /// tried, its bit being set).
    fn skipped(self) -> Disc {
        match self {
            Disc::Mtu => Disc::Service(0),
            Disc::Service(n) => Disc::Service(n + 1),
            Disc::Chars(n, _) => Disc::Descs(n),
            Disc::Subscribe(i) => Disc::Subscribe(i + 1),
            Disc::Register(i) => Disc::Register(i + 1),
            Disc::Read(i) => Disc::Read(i + 1),
            d => d,
        }
    }
}

/// A GATT write queued until the client is free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    /// Get a notification's attributes.
    Fetch(u32),
    /// Accept (true) or decline a call.
    Act(u32, bool),
    /// An AMS remote command.
    Ams(u8),
    /// Get App Attributes for a bundle id.
    App(Line<24>),
}

/// The phone link's state.
pub struct Phone {
    /// The settings and the bond, as in `ble.bin`.
    pub(crate) stored: Stored,
    /// `ble.bin` read (or tried); to be written.
    loaded: bool,
    save: bool,
    /// The GATT server was set up.
    ready: bool,
    adv: Adv,
    /// The PHONE menu's PAIR window: pairing accepted until a bond, the window's end or the link's (the advertising
    /// stops at a connection, so it cannot tell).
    pairing: bool,
    pub(crate) conn: Option<u16>,
    /// How long the link has been up (a link left unencrypted is dropped: [`UNSECURED_MS`]).
    link_ms: u32,
    secure: bool,
    /// Our attributes: the HID report and its CCCD, the battery level and its CCCD.
    report: (u16, u16),
    battery: (u16, u16),
    /// The vendor UUID types the stack gave [`ph::VS`] (0: not registered).
    types: [u8; 8],
    disc: Disc,
    /// Since the discovery's last request ([`DISC_MS`]).
    disc_ms: u32,
    /// The service being discovered: its last handle, its characteristics' declaration handles.
    svc_end: u16,
    decls: [u16; 8],
    n_decls: usize,
    /// The phone's attributes found ([`NS`]...): 0 none.
    h: [u16; 12],
    /// CCCDs looked for already (one bit per slot), and the one being looked for.
    tried: u16,
    looking: usize,
    /// The peer's address at the link.
    peer: [u8; 7],
    ops: [Op; 4],
    n_ops: usize,
    /// A GATT write is out: its answer awaited (for a fetch, the Data Source's too), for so long at most.
    busy: bool,
    busy_ms: u32,
    fetching: bool,
    ds: ph::DataSource,
    /// The names of the last apps seen, and a notification held until its app's name came (or did not).
    pub(crate) apps: ph::AppCache,
    pub(crate) held: Option<ph::Attributes>,
    /// The app being asked about (queued or out), and whether its request is the write awaiting an answer.
    asking: Option<Line<24>>,
    app_out: bool,
    /// The ATT MTU of the link: 23 until the exchange.
    pub(crate) mtu: u16,
    /// An incoming call: its id and who calls.
    pub(crate) call: Option<(u32, Line<24>)>,
    /// The call's overlay is up (its caller known): buttons and banner over the bottom of the screen.
    pub(crate) ringing: bool,
    /// The last notifications, for the NOTIFICATIONS list (app/notifs.rs).
    pub(crate) inbox: ph::Inbox,
    /// What plays.
    pub(crate) now: ph::NowPlaying,
    /// The AMS commands the playing app accepts ([`ph::ams_commands`]; 0: no list yet, all tried).
    commands: u16,
    /// A media key's report was sent: the release goes at the next pass.
    release: bool,
    /// Passes the release failed in a row.
    release_tries: u8,
    /// The battery level last sent.
    battery_sent: Option<u8>,
    /// The media screen's rows as drawn (fingerprints).
    pub(crate) shown: ui::media::Shown,
}

impl Default for Phone {
    fn default() -> Self {
        Phone {
            stored: Stored::default(),
            loaded: false,
            save: false,
            ready: false,
            adv: Adv::Off,
            pairing: false,
            conn: None,
            link_ms: 0,
            secure: false,
            report: (0, 0),
            battery: (0, 0),
            types: [0; 8],
            disc: Disc::Idle,
            disc_ms: 0,
            svc_end: 0,
            decls: [0; 8],
            n_decls: 0,
            h: [0; 12],
            tried: 0,
            looking: 0,
            peer: [0; 7],
            ops: [Op::Ams(0); 4],
            n_ops: 0,
            busy: false,
            busy_ms: 0,
            fetching: false,
            apps: ph::AppCache::default(),
            held: None,
            asking: None,
            app_out: false,
            mtu: 23,
            ds: ph::DataSource::default(),
            call: None,
            ringing: false,
            inbox: ph::Inbox::default(),
            now: ph::NowPlaying::default(),
            commands: 0,
            release: false,
            release_tries: 0,
            battery_sent: None,
            shown: ui::media::Shown { text: 0, playing: None, bar: 0, times: 0 },
        }
    }
}

/// What the PHONE menu's status row says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// No BLE (the chip did not start, or the server could not be set up).
    NoBle,
    /// Not paired, not advertising.
    Off,
    /// Advertising to pair.
    Pairing,
    /// Bonded, waiting for the phone.
    Waiting,
    /// Connected, not encrypted yet.
    Connected,
    /// Connected and encrypted.
    Secure,
}

impl Phone {
    /// The PAIR window is open.
    pub(crate) fn pairing(&self) -> bool {
        self.pairing
    }

    /// The link as the menu shows it.
    pub fn link(&self) -> Link {
        match (self.ready, self.conn, self.secure, self.adv) {
            (false, ..) => Link::NoBle,
            (_, Some(_), true, _) => Link::Secure,
            (_, Some(_), false, _) => Link::Connected,
            (_, None, _, Adv::Fast) => Link::Pairing,
            (_, None, _, Adv::Slow) => Link::Waiting,
            _ => Link::Off,
        }
    }

    /// Bonded with a phone.
    pub fn bonded(&self) -> bool {
        self.stored.bond.is_some()
    }

    /// The CCCDs to enable, in order: the Data Source before the Notification Source (so an answer has somewhere
    /// to go), AMS, the time.
    fn subscriptions(&self) -> [usize; 5] {
        [DS_CCCD, NS_CCCD, RC_CCCD, EU_CCCD, CT_CCCD]
    }

    /// The UUID of service `n` ([`NOTIFYING`]'s order).
    fn service(&self, n: u8) -> ble::Uuid {
        match n {
            0 => (ph::uuid16(&ph::VS[ph::VS_ANCS]), self.types[ph::VS_ANCS]),
            1 => (ph::uuid16(&ph::VS[ph::VS_AMS]), self.types[ph::VS_AMS]),
            _ => (0x1805, ble::UUID_BLE),
        }
    }

    /// The slot of a characteristic of the phone's, by its UUID.
    fn slot(&self, uuid: ble::Uuid) -> Option<usize> {
        let vs = [(1, NS), (2, CP), (3, DS), (5, RC), (6, EU)];
        vs.into_iter()
            .find(|&(k, _)| self.types[k] != 0 && uuid == (ph::uuid16(&ph::VS[k]), self.types[k]))
            .map(|(_, s)| s)
            .or(match uuid {
                (0x2a2b, ble::UUID_BLE) => Some(CT),
                (0x2a0f, ble::UUID_BLE) => Some(LTI),
                _ => None,
            })
    }

    /// Queues `op`. A message's fetch takes the first slots only: the last two stay for a call's name, its answer and
    /// the media keys, so a burst of messages cannot crowd them out.
    fn queue(&mut self, op: Op) -> bool {
        let urgent = match op {
            Op::Fetch(uid) => self.call.is_some_and(|(c, _)| c == uid),
            _ => true,
        };
        if !urgent && self.n_ops + 2 >= self.ops.len() {
            return false;
        }
        let Some(slot) = self.ops.get_mut(self.n_ops) else { return false };
        *slot = op;
        self.n_ops += 1;
        true
    }

    /// The link is gone: what belonged to it.
    fn unlink(&mut self) {
        (self.conn, self.secure, self.disc, self.busy, self.n_ops) = (None, false, Disc::Idle, false, 0);
        (self.h, self.tried, self.call, self.release) = ([0; 12], 0, None, false);
        (self.held, self.mtu) = (None, 23);
        (self.asking, self.app_out) = (None, false);
        self.battery_sent = None;
        (self.link_ms, self.now) = (0, ph::NowPlaying::default()); // what played is not known without the phone
        self.commands = 0; // the next link's app sends its own list (review: a podcast app's kept blocked Next)
    }
}

impl App {
    /// The chip is about to be reset: the link and the server are gone with it (the bond and settings stay).
    pub(crate) fn phone_down(&mut self, b: &mut impl Board) {
        let had_call = self.phone.call.is_some();
        self.phone.unlink();
        (self.phone.ready, self.phone.adv, self.phone.pairing) = (false, Adv::Off, false);
        if had_call {
            self.end_ring(b);
        }
    }

    /// The link ended: what belonged to it is dropped, and the wait for the bonded phone starts again.
    fn link_lost(&mut self, b: &mut impl Board) {
        let had_call = self.phone.call.is_some();
        self.phone.unlink();
        self.phone.pairing = false;
        if had_call {
            self.end_ring(b);
        }
        self.idle_advertising(b);
    }

    /// After a resync of the Nordic link, whose dropped bytes may have held the link's end (or the advertising's):
    /// a link the chip knows nothing of is over, and the wait for the bonded phone is asserted again (review: a
    /// lost DISCONNECTED left the unit connected for ever, advertising nothing, and the bonded phone could not
    /// come back).
    pub(crate) fn phone_recheck(&mut self, b: &mut impl Board) {
        if !self.phone.ready {
            return;
        }
        if let Some(conn) = self.phone.conn {
            // the chip refuses a link it has not (a time-out is no answer: the link is left as it is)
            if matches!(ble::sys_attr_get(b, &mut self.rx, conn, &mut [0; ph::ATTRS]), Err(nrf::Error::Code(_))) {
                self.link_lost(b);
            }
        } else if !self.phone.pairing {
            let _ = ble::adv_stop(b, &mut self.rx);
            self.phone.adv = Adv::Off;
            self.idle_advertising(b);
        }
    }

    /// The chip was reset: the server set up again, and the advertising of a bonded phone's wait back (the bond
    /// is read from the card at the start, [`App::phone_storage`], and starts it itself when not yet read).
    pub(crate) fn phone_up(&mut self, b: &mut impl Board) {
        self.phone_setup(b);
        if self.phone.ready && self.phone.loaded {
            self.idle_advertising(b);
        }
    }

    /// Sets up the GATT server (HID, battery, device information), the name and appearance, the advertising
    /// data, and registers the phone's vendor UUIDs: once, after the chip started.
    #[inline(never)] // its command buffers in its own frame, once at the start
    pub(crate) fn phone_setup(&mut self, b: &mut impl Board) {
        let rx = &mut self.rx;
        let p = &mut self.phone;
        let r = (|| -> Result<(), nrf::Error> {
            use ble::{Char, ENCRYPTED, NO_ACCESS, NOTIFY, OPEN, READ, WRITE_CMD};
            ble::device_name_set(b, rx, b"S510")?;
            ble::appearance_set(b, rx, 0x03c0)?; // generic HID
            ble::ppcp_set(b, rx, PPCP)?;
            // HID over GATT: encrypted, as the profile asks
            let hid = ble::service_add(b, rx, 0x1812)?;
            let info =
                Char { uuid: 0x2a4a, props: READ, value: &ph::HID_INFO, max: 4, read: ENCRYPTED, write: NO_ACCESS };
            ble::characteristic_add(b, rx, hid, &info)?;
            let map = &ph::REPORT_MAP;
            let map = Char {
                uuid: 0x2a4b,
                props: READ,
                value: map,
                max: map.len() as u16,
                read: ENCRYPTED,
                write: NO_ACCESS,
            };
            ble::characteristic_add(b, rx, hid, &map)?;
            let report =
                Char { uuid: 0x2a4d, props: READ | NOTIFY, value: &[0], max: 1, read: ENCRYPTED, write: NO_ACCESS };
            p.report = ble::characteristic_add(b, rx, hid, &report)?;
            ble::descriptor_add(b, rx, p.report.0, 0x2908, &ph::REPORT_REF, ENCRYPTED)?;
            let point = Char { uuid: 0x2a4c, props: WRITE_CMD, value: &[0], max: 1, read: NO_ACCESS, write: ENCRYPTED };
            ble::characteristic_add(b, rx, hid, &point)?;
            let mode =
                Char { uuid: 0x2a4e, props: READ | WRITE_CMD, value: &[1], max: 1, read: ENCRYPTED, write: ENCRYPTED };
            ble::characteristic_add(b, rx, hid, &mode)?;
            let bas = ble::service_add(b, rx, 0x180f)?;
            let level =
                Char { uuid: 0x2a19, props: READ | NOTIFY, value: &[100], max: 1, read: OPEN, write: NO_ACCESS };
            p.battery = ble::characteristic_add(b, rx, bas, &level)?;
            let dis = ble::service_add(b, rx, 0x180a)?;
            let maker = Char { uuid: 0x2a29, props: READ, value: b"rustfw", max: 6, read: OPEN, write: NO_ACCESS };
            ble::characteristic_add(b, rx, dis, &maker)?;
            let pnp = Char { uuid: 0x2a50, props: READ, value: &ph::PNP_ID, max: 7, read: OPEN, write: NO_ACCESS };
            ble::characteristic_add(b, rx, dis, &pnp)?;
            ble::adv_data_set(b, rx, &ph::ADV, &ph::SCAN_RSP)?;
            for (t, uuid) in p.types.iter_mut().zip(&ph::VS) {
                *t = ble::uuid_vs_add(b, rx, uuid)?;
            }
            Ok(())
        })();
        p.ready = r.is_ok();
    }

    /// Advertising as `want` (a no-op when it already is).
    fn advertise(&mut self, b: &mut impl Board, want: Adv) {
        if self.phone.adv == want || !self.phone.ready || (want != Adv::Off && self.phone.conn.is_some()) {
            return;
        }
        if self.phone.adv != Adv::Off {
            let _ = ble::adv_stop(b, &mut self.rx); // already stopped by a connection or a time-out: harmless
        }
        let (interval, timeout) = match want {
            Adv::Off => return self.phone.adv = Adv::Off,
            Adv::Fast => ADV_FAST,
            Adv::Slow => ADV_SLOW,
        };
        let tag = self.conn_tag();
        self.phone.adv = if ble::adv_start(b, &mut self.rx, interval, timeout, tag).is_ok() { want } else { Adv::Off };
    }

    /// What advertising should be while not connected: slow when bonded, else none.
    fn idle_advertising(&mut self, b: &mut impl Board) {
        let want = if self.phone.bonded() { Adv::Slow } else { Adv::Off };
        self.advertise(b, want);
    }

    /// The PHONE menu's PAIR: advertise to be found for [`ADV_FAST`]'s time.
    pub(crate) fn phone_pair(&mut self, b: &mut impl Board) {
        if self.phone.conn.is_some() {
            return self.say("CONNECTED ALREADY");
        }
        self.phone.adv = Adv::Off; // restart even if advertising slowly
        self.phone.pairing = true;
        let _ = ble::adv_stop(b, &mut self.rx);
        self.advertise(b, Adv::Fast);
        self.say(if self.phone.adv == Adv::Fast { "PAIR FROM THE PHONE" } else { "NO BLE" });
    }

    /// A phone setting changed: written with the bond.
    pub(crate) fn phone_changed(&mut self) {
        self.phone.save = true;
    }

    /// The PHONE menu's FORGET: the link dropped, the bond erased.
    pub(crate) fn phone_forget(&mut self, b: &mut impl Board) {
        if let Some(conn) = self.phone.conn {
            let _ = ble::disconnect(b, &mut self.rx, conn);
        }
        self.phone.stored.bond = None;
        self.phone.save = true;
        self.advertise(b, Adv::Off);
        self.say("PHONE FORGOTTEN");
    }

    /// The bond read from the card [`BOND_LOAD_MS`] after the start; written back when it changed, between rides
    /// only (the recorder is the card's one writer while a ride is open). A build that may not write the card
    /// ([`Board::emmc_writable`]) reads it but never writes it: a bond made there lasts until the unit is switched off.
    fn phone_storage(&mut self, b: &mut impl Board) {
        if self.emmc.lent() || !self.phone.ready || self.gps.ms < BOND_LOAD_MS {
            return;
        }
        if !self.phone.loaded {
            self.phone.loaded = true;
            let mut d = emmc::Volume(&mut self.emmc, b);
            let mut s = [0u8; 512];
            let read = fat::Fat::mount(&mut d).and_then(|mut fat| {
                let f = fat.open(&mut d, "Rustfw/ble.bin")?;
                fat.read_at(&mut d, &f, 0, &mut s)
            });
            // a phone paired before the read (its bond not written yet) is the one kept
            if let Ok(512) = read
                && let Some(stored) = Stored::from_sector(&s)
                && !self.phone.save
            {
                self.phone.stored = stored;
            }
            if self.phone.conn.is_none() && self.phone.adv == Adv::Off {
                self.idle_advertising(b);
            }
        }
        if self.card_writable && self.phone.save && self.ride_settled() {
            self.phone.save = false;
            let (local, _) = self.rec_time();
            let sector = self.phone.stored.to_sector();
            let mut d = emmc::Volume(&mut self.emmc, b);
            let r = fat::Fat::mount(&mut d).and_then(|mut fat| {
                let t = fat::write::dos_time(local);
                let dir = fat.ensure_dir(&mut d, fat.root(), recorder::DIR, t)?;
                let mut h = match fat.locate(&mut d, dir, BOND_FILE) {
                    Err(fat::Error::NotFound) => fat.create(&mut d, dir, BOND_FILE, t)?,
                    other => other?,
                };
                fat.write_sectors(&mut d, &mut h, 0, &sector)?;
                if h.size != 512 {
                    fat.set_size(&mut d, &mut h, 512, t)?;
                }
                Ok(())
            });
            if r.is_err() {
                self.notice("BOND NOT SAVED");
            }
        }
    }

    /// The phone link's work once a pass: the card, a key's release, the battery level, the queued operations.
    pub(crate) fn step_phone(&mut self, b: &mut impl Board) {
        if !self.phone.ready {
            return;
        }
        self.phone_storage(b);
        self.phone.now.tick(self.dt_ms);
        let Some(conn) = self.phone.conn else { return };
        self.phone.busy_ms = self.phone.busy_ms.saturating_add(self.dt_ms);
        self.phone.link_ms = self.phone.link_ms.saturating_add(self.dt_ms);
        if !self.phone.secure && !self.phone.pairing && self.phone.link_ms >= UNSECURED_MS {
            self.phone.link_ms = 0; // once: the disconnection's event unlinks
            let _ = ble::disconnect(b, &mut self.rx, conn);
            return;
        }
        if self.phone.busy && self.phone.busy_ms >= 3000 {
            self.phone.busy = false; // an answer that never came: the queue goes on
            if core::mem::take(&mut self.phone.app_out) {
                self.app_unknown(b);
            }
        }
        if !self.phone.busy && self.phone.n_ops == 0 && self.phone.held.is_some() {
            self.show_held(b); // its app's name never came
        }
        if !matches!(self.phone.disc, Disc::Idle | Disc::Done) {
            self.phone.disc_ms = self.phone.disc_ms.saturating_add(self.dt_ms);
            if self.phone.disc_ms >= DISC_MS {
                if self.phone.disc == Disc::Mtu {
                    // no answer: the stack's MTU procedure stays pending and refuses later GATTC calls (BUSY), so the
                    // link is dropped; the phone connects again, as after any lost link
                    self.phone.disc = Disc::Idle;
                    let _ = ble::disconnect(b, &mut self.rx, conn);
                    return;
                }
                // the discovery's answer never came: on with the next step (a late one is taken for its answer, or
                // not the one awaited, and the deadline moves it on again)
                (self.phone.disc, self.phone.disc_ms) = (self.phone.disc.skipped(), 0);
                self.discover(b, conn);
            }
        }
        if self.phone.release {
            // the key's release: one notification queued at a time (the stack's default), so a pass after the press;
            // tried for at most RELEASE_TRIES passes (review finding 9: not every pass until the link drops)
            let failed = ble::notify(b, &mut self.rx, conn, self.phone.report.0, &[0]).is_err();
            self.phone.release_tries = if failed { self.phone.release_tries + 1 } else { 0 };
            self.phone.release = failed && self.phone.release_tries < RELEASE_TRIES;
        }
        let level = self.strip.battery;
        if self.phone.secure && level.is_some() && level != self.phone.battery_sent {
            // once per change: refused while the phone has not enabled it, and the value set for its reads
            let v = level.unwrap_or(0).min(100);
            if ble::notify(b, &mut self.rx, conn, self.phone.battery.0, &[v]).is_err() {
                let _ = ble::value_set(b, &mut self.rx, self.phone.battery.0, &[v]);
            }
            self.phone.battery_sent = level;
        }
        self.next_op(b, conn);
    }

    /// A media key, from a page (Lap held), the media screen or a touch: AMS when the phone has it (an iPhone),
    /// else HID; the camera's shutter is HID's volume up.
    pub(crate) fn media(&mut self, b: &mut impl Board, key: Media, shutter: bool) {
        let p = &mut self.phone;
        let Some(conn) = p.conn.filter(|_| p.secure && p.stored.keys) else {
            return self.say(if self.phone.stored.keys { "NO PHONE" } else { "MEDIA KEYS OFF" });
        };
        if !shutter
            && p.h[RC] != 0
            && let Some(cmd) = key.ams()
        {
            // only the skips refused by the list: an app may list Play and Pause and not the toggle, or leave out
            // the volume, which the phone takes anyway (review)
            if matches!(key, Media::SkipFwd | Media::SkipBack) && p.commands != 0 && p.commands & 1 << cmd == 0 {
                return self.say("NOT IN THIS APP"); // a skip in a music app: the phone would refuse it
            }
            p.queue(Op::Ams(cmd));
            return self.next_op(b, conn);
        }
        let k = if shutter { Media::VolUp } else { key };
        let Some(report) = k.report() else {
            return self.say("NEEDS AN IPHONE"); // a skip: AMS only
        };
        match ble::notify(b, &mut self.rx, conn, p.report.0, &[report]) {
            Ok(()) => p.release = true,
            Err(_) => self.say("KEYS NOT ENABLED"), // the phone did not enable the report: not used as HID
        }
    }

    /// The call's overlay goes (answered, declined, missed, the link lost): what was under it drawn again.
    pub(crate) fn end_ring(&mut self, b: &mut impl Board) {
        if core::mem::take(&mut self.phone.ringing) {
            let mut p = Panel(b, (ui::CALL_Y, ui::H), self.look());
            p.fill(0, ui::CALL_Y, ui::W, ui::H - ui::CALL_Y, ui::BLACK);
            self.draw_screen(&mut p);
            if self.screen == Screen::Main {
                // the date row left out under the call's banner, drawn again (its top rows come with redraw_top)
                ui::draw_date(&mut Panel(b, (ui::BANNER_H, ui::CALL_Y), self.look()), date_text(self.date).as_str());
            }
            self.redraw_top(b); // the strip and the page's top, or a banner said meanwhile
        }
    }

    /// Answers the incoming call (true) or declines it.
    pub(crate) fn answer(&mut self, b: &mut impl Board, accept: bool) {
        if let (Some((uid, _)), Some(conn)) = (self.phone.call.take(), self.phone.conn) {
            self.phone.queue(Op::Act(uid, accept));
            self.next_op(b, conn);
        }
    }

    /// Runs the next queued operation when the client is free.
    fn next_op(&mut self, b: &mut impl Board, conn: u16) {
        let p = &mut self.phone;
        if p.busy || p.disc != Disc::Done || p.n_ops == 0 {
            return;
        }
        let op = p.ops[0];
        p.ops.rotate_left(1);
        p.n_ops -= 1;
        let mut app = [0; 28];
        let (handle, data): (u16, &[u8]) = match op {
            Op::App(id) => {
                let n = ph::get_app(id.as_str(), &mut app);
                (p.h[CP], &app[..n])
            }
            Op::Fetch(uid) => {
                p.ds = Default::default();
                (p.h[CP], &ph::get_attributes(uid))
            }
            Op::Act(uid, yes) => (p.h[CP], &ph::action(uid, yes)),
            Op::Ams(cmd) => (p.h[RC], &[cmd]),
        };
        p.busy = handle != 0 && ble::write(b, &mut self.rx, conn, handle, data).is_ok();
        (p.busy_ms, p.fetching) = (0, p.busy && matches!(op, Op::Fetch(_) | Op::App(_)));
        p.app_out = p.busy && matches!(op, Op::App(_));
    }

    /// The connection configuration tag links use: [`ble::TAG`] when the chip took the two links' configuration, else
    /// the default one.
    pub(crate) fn conn_tag(&self) -> u8 {
        if self.ble.is_some_and(|v| v.central) { ble::TAG } else { 0 }
    }

    /// One BLE event from the chip (not an advertising report: radio.rs has those): the trainer's link's to
    /// app/ftms.rs, the rest the phone's.
    pub(crate) fn phone_event(&mut self, b: &mut impl Board, p: &[u8]) {
        let Some(e) = ble::event(p) else { return };
        if self.ftms_owns(&e) {
            return self.ftms_event(b, e);
        }
        if let Event::Connected { conn, role: 1, .. } = e
            && self.ftms.conn == Some(conn)
        {
            self.ftms_lost(); // the trainer's DISCONNECTED was lost: its handle is the phone's now
        }
        let rx = &mut self.rx;
        match e {
            Event::Connected { conn, role: 1, peer } => {
                self.phone.unlink();
                (self.phone.conn, self.phone.adv) = (Some(conn), Adv::Off); // the stack stopped advertising
                self.phone.peer = peer;
                if !self.phone.bonded() || self.phone.pairing {
                    let _ = ble::authenticate(b, rx, conn); // the phone asks the rider to pair
                }
            }
            Event::Disconnected { conn, .. } if Some(conn) == self.phone.conn => self.link_lost(b),
            Event::SecParamsRequest { conn } => {
                // pairing: while asked for (PAIR), or with no bond yet (then the unit advertises only after PAIR)
                let ok = self.phone.pairing || !self.phone.bonded();
                let _ = ble::sec_params_reply(b, rx, conn, ok);
            }
            Event::SecInfoRequest { conn, .. } => {
                let key = self.phone.stored.bond.map(|d| (d.key.ltk, d.key.flags));
                let _ = ble::sec_info_reply(b, rx, conn, key);
            }
            Event::AuthStatus { status: 0, bonded: true, own, peer, .. } => {
                // our LTK (legacy pairing), or with LESC the one key both sides share
                let key = own.filter(|k| !k.is_empty()).or(peer.filter(|k| !k.is_empty()));
                if let Some(key) = key {
                    let mut bond = ph::Bond { key, peer: self.phone.peer, attrs: [0; ph::ATTRS], attrs_len: 0 };
                    if let Some(conn) = self.phone.conn
                        && let Ok(n) = ble::sys_attr_get(b, rx, conn, &mut bond.attrs)
                    {
                        bond.attrs_len = n as u8; // CCCDs the phone set before the keys came
                    }
                    self.phone.stored.bond = Some(bond);
                    (self.phone.save, self.phone.pairing) = (true, false);
                    self.notice("PHONE PAIRED");
                }
            }
            Event::AuthStatus { .. } => self.say("PAIRING FAILED"),
            Event::ConnSecUpdate { conn, level } if level >= 2 && Some(conn) == self.phone.conn => {
                self.phone.secure = true;
                if let Some(d) = self.phone.stored.bond {
                    let _ = ble::sys_attr_set(b, rx, conn, Some(&d.attrs[..usize::from(d.attrs_len)]));
                }
                if self.phone.disc == Disc::Idle {
                    self.phone.disc = Disc::Mtu;
                    self.discover(b, conn);
                }
            }
            Event::SysAttrMissing { conn } => {
                let attrs = self.phone.stored.bond.filter(|_| self.phone.secure);
                let _ = ble::sys_attr_set(b, rx, conn, attrs.as_ref().map(|d| &d.attrs[..usize::from(d.attrs_len)]));
            }
            Event::Write { conn, uuid: ble::CCCD, .. } => {
                self.phone.battery_sent = None; // the level again, in case it was its CCCD
                // the CCCDs kept with the bond, for the next link: the bonded phone's only (an unencrypted peer is
                // a stranger, whose writes would switch the owner's notifications off and wear the card)
                if self.phone.secure
                    && let Some(d) = &mut self.phone.stored.bond
                {
                    let mut attrs = [0; ph::ATTRS];
                    if let Ok(n) = ble::sys_attr_get(b, rx, conn, &mut attrs) {
                        (d.attrs, d.attrs_len) = (attrs, n as u8);
                        self.phone.save = true;
                    }
                }
            }
            Event::MtuRequest { conn } => {
                let _ = ble::exchange_mtu_reply(b, rx, conn, self.ble.map_or(23, |v| v.mtu));
            }
            Event::ParamRequest { conn, id } => {
                let _ = match id {
                    0x21 => ble::phy_update(b, rx, conn),
                    _ => ble::data_length_update(b, rx, conn),
                };
            }
            Event::ConnParamRequest { conn, params } => {
                let _ = ble::conn_param_update(b, rx, conn, params); // for a central only: not the phone's link
            }
            Event::UserMemRequest { conn } => {
                let _ = ble::user_mem_reply(b, rx, conn);
            }
            Event::Timeout { src: 0 } => {
                // the pairing window closed: back to waiting for the bonded phone, or nothing
                (self.phone.adv, self.phone.pairing) = (Adv::Off, false);
                if self.phone.conn.is_none() {
                    self.idle_advertising(b);
                }
            }
            Event::GattTimeout { conn } if Some(conn) == self.phone.conn => {
                // no more ATT on this link (Core spec, ATT transaction time-out): dropped, as Nordic's examples do
                self.phone.disc = Disc::Idle;
                let _ = ble::disconnect(b, rx, conn);
            }
            Event::Hvx { conn, handle, kind, data } if Some(conn) == self.phone.conn => {
                self.phone_hvx(b, handle, kind, data)
            }
            Event::MtuRsp { conn, .. }
            | Event::Services { conn, .. }
            | Event::Chars { conn, .. }
            | Event::Descs { conn, .. }
            | Event::Read { conn, .. }
            | Event::Written { conn, .. }
                if Some(conn) == self.phone.conn =>
            {
                self.client_answer(b, conn, e);
            }
            _ => {}
        }
    }

    /// A GATT client answer: the discovery's next step, or a queued operation done.
    fn client_answer(&mut self, b: &mut impl Board, conn: u16, e: Event) {
        let p = &mut self.phone;
        match (p.disc, e) {
            (Disc::Mtu, Event::MtuRsp { status, mtu, .. }) => {
                // the link uses the smaller of the two Rx MTUs (Core spec Vol 3 Part F 3.4.2.2); a refusal stays 23
                p.mtu = if status == 0 { mtu.clamp(23, self.ble.map_or(23, |v| v.mtu)) } else { 23 };
                p.disc = Disc::Service(0);
            }
            (Disc::Service(n), Event::Services { status: 0, list, .. }) => {
                match ble::services(list).find(|(u, ..)| *u == p.service(n)) {
                    Some((_, start, end)) => {
                        (p.svc_end, p.n_decls) = (end, 0);
                        p.disc = Disc::Chars(n, start);
                    }
                    None => p.disc = Disc::Service(n + 1),
                }
            }
            (Disc::Service(n), _) => p.disc = Disc::Service(n + 1), // not there
            (Disc::Chars(n, from), Event::Chars { status: 0, list, .. }) if !list.is_empty() => {
                let mut last = 0;
                for (uuid, _, decl, value) in ble::chars(list) {
                    if let Some(slot) = p.slot(uuid) {
                        p.h[slot] = value;
                    }
                    if let Some(d) = p.decls.get_mut(p.n_decls) {
                        *d = decl;
                        p.n_decls += 1;
                    }
                    last = value;
                }
                // a phone whose handles do not move forward would be asked forever (review 3, finding 8): on to
                // the descriptors instead
                p.disc = if last >= from && last < p.svc_end { Disc::Chars(n, last + 1) } else { Disc::Descs(n) };
            }
            (Disc::Chars(n, _), _) => p.disc = Disc::Descs(n), // the end of the characteristics
            (Disc::Descs(_), Event::Descs { status: 0, list, .. }) => {
                let cccd = ble::descs(list).find(|(_, u)| *u == (ble::CCCD, ble::UUID_BLE));
                if let (Some((h, _)), Some(slot)) = (cccd, p.h.get_mut(p.looking)) {
                    *slot = h;
                }
            }
            (Disc::Descs(_), Event::Descs { .. }) => {}
            (Disc::Subscribe(i), Event::Written { .. }) => p.disc = Disc::Subscribe(i + 1),
            (Disc::Register(i), Event::Written { .. }) => p.disc = Disc::Register(i + 1),
            (Disc::Read(i), Event::Read { status, handle, data, .. }) => {
                if status == 0 && handle == p.h[CT] {
                    self.phone_time(b, Some(data), None);
                } else if status == 0 && handle == p.h[LTI] {
                    self.phone_time(b, None, Some(data));
                }
                self.phone.disc = Disc::Read(i + 1);
            }
            (Disc::Done, Event::Written { status, .. }) => {
                // a fetch waits for its answer on the Data Source as well, which a new one would cut
                p.busy = p.fetching && status == 0;
                if status != 0 && core::mem::take(&mut p.app_out) {
                    self.app_unknown(b); // refused: no name to wait for
                }
                return self.next_op(b, conn);
            }
            _ => return,
        }
        self.discover(b, conn);
    }

    /// Sends the discovery's next request (skipping what is not there), or ends it.
    fn discover(&mut self, b: &mut impl Board, conn: u16) {
        let mtu = self.ble.map_or(23, |v| v.mtu);
        for _ in 0..32 {
            let rx = &mut self.rx;
            let p = &mut self.phone;
            let sent = match p.disc {
                Disc::Mtu => mtu > 23 && ble::exchange_mtu_request(b, rx, conn, mtu).is_ok(),
                Disc::Service(n) if n < 3 => {
                    let uuid = p.service(n);
                    uuid.1 != 0 && ble::discover_service(b, rx, conn, uuid).is_ok()
                }
                Disc::Service(_) => {
                    p.disc = Disc::Subscribe(0);
                    continue;
                }
                Disc::Chars(_, from) => ble::discover_chars(b, rx, conn, from, p.svc_end).is_ok(),
                Disc::Descs(n) => {
                    let next = NOTIFYING
                        .get(usize::from(n))
                        .and_then(|l| l.iter().find(|&&(v, c)| p.h[v] != 0 && p.tried & 1 << c == 0));
                    match next {
                        Some(&(v, c)) => {
                            (p.tried, p.looking) = (p.tried | 1 << c, c);
                            // the descriptors lie between the value and the next declaration (or the service's end)
                            let value = p.h[v];
                            let next_decl = p.decls[..p.n_decls].iter().filter(|&&d| d > value).min().copied();
                            let end = next_decl.map_or(p.svc_end, |d| d - 1);
                            value < end && ble::discover_descs(b, rx, conn, value + 1, end).is_ok()
                        }
                        None => {
                            p.disc = Disc::Service(n + 1);
                            continue;
                        }
                    }
                }
                Disc::Subscribe(i) => match p.subscriptions().get(usize::from(i)) {
                    Some(&c) => {
                        let wanted = p.stored.notify || !matches!(c, NS_CCCD | DS_CCCD);
                        wanted && p.h[c] != 0 && ble::write(b, rx, conn, p.h[c], &[1, 0]).is_ok()
                    }
                    None => {
                        p.disc = Disc::Register(0);
                        continue;
                    }
                },
                Disc::Register(i) => match ph::AMS_REGISTER.get(usize::from(i)) {
                    Some(cmd) => p.h[EU_CCCD] != 0 && ble::write(b, rx, conn, p.h[EU], cmd).is_ok(),
                    None => {
                        p.disc = Disc::Read(0);
                        continue;
                    }
                },
                Disc::Read(i) => match [CT, LTI].get(usize::from(i)) {
                    Some(&s) => p.h[s] != 0 && ble::read(b, rx, conn, p.h[s]).is_ok(),
                    None => {
                        p.disc = Disc::Done;
                        return self.next_op(b, conn);
                    }
                },
                Disc::Idle | Disc::Done => return,
            };
            if sent {
                p.disc_ms = 0;
                return;
            }
            p.disc = p.disc.skipped(); // nothing to ask (or the chip refused): the next step
        }
    }

    /// A notification's attributes arrived. A call is shown at once; another waits for its app's name when that is
    /// not known (Get App Attributes, once per app: [`ph::AppCache`]), shown without it when that cannot be asked.
    fn notif_fetched(&mut self, b: &mut impl Board, a: ph::Attributes) {
        self.phone.busy = false;
        let call = self.phone.call.is_some_and(|(uid, _)| uid == a.uid);
        if !call {
            self.show_held(b); // one still waiting for a name goes first, without it
            let id = a.app.as_str();
            // known, or not to be asked: no id, a cut one (24), a request that does not fit one ATT write at this MTU, or
            // one already out
            let known = id.is_empty()
                || id.len() >= 24
                || id.len() + 3 > usize::from(self.phone.mtu) - 3
                || self.phone.apps.get(id).is_some()
                || self.phone.asking == Some(a.app);
            if !known && self.phone.queue(Op::App(a.app)) {
                self.phone.asking = Some(a.app);
                self.phone.held = Some(a);
                if let Some(conn) = self.phone.conn {
                    self.next_op(b, conn);
                }
                return;
            }
        }
        if let Some(conn) = self.phone.conn {
            self.next_op(b, conn);
        }
        match &mut self.phone.call {
            Some((uid, who)) if *uid == a.uid => {
                *who = a.title;
                let who = *who;
                // over whatever screen is up: the page between keeps updating (area()); a banner waits under it
                self.notes.drawn = false;
                self.phone.ringing = true;
                let mut p = Panel(b, (0, ui::H), self.look());
                ui::draw_call(&mut p, who.as_str());
                if self.screen == Screen::Main {
                    p.fill(0, ui::BANNER_H, ui::W, ui::TODAY_BOX.1 - ui::BANNER_H, ui::BLACK); // the date row's lower half
                }
                self.tones.play(&BEEP_ALERT);
            }
            _ => self.show_notif(b, a),
        }
    }

    /// The notification held for its app's name, shown now with what the cache has.
    fn show_held(&mut self, b: &mut impl Board) {
        if let Some(a) = self.phone.held.take() {
            self.show_notif(b, a);
        }
    }

    /// The app being asked about gets no name (refused, or no answer): cached empty so it is not asked again, and the
    /// held notification shown with the bundle id's last part.
    fn app_unknown(&mut self, b: &mut impl Board) {
        if let Some(id) = self.phone.asking.take() {
            self.phone.apps.put(ph::AppInfo { id, name: Line::new() });
        }
        self.show_held(b);
    }

    /// The banner ("APP: title: message") and the inbox.
    fn show_notif(&mut self, b: &mut impl Board, mut a: ph::Attributes) {
        a.name = self.phone.apps.get(a.app.as_str()).unwrap_or_default();
        if a.name.as_str().is_empty() {
            ph::app_name(&mut a.name, a.app.as_str());
        }
        let mut l = Line::<40>::new();
        if !a.name.as_str().is_empty() {
            l.push_str(a.name.as_str());
            l.push_str(": ");
        }
        l.push_str(a.title.as_str());
        if !a.message.as_str().is_empty() {
            l.push_str(": ");
            l.push_str(a.message.as_str());
        }
        self.notice(l.as_str());
        self.keep_notif(b, &a);
    }

    /// A notification or indication from the phone.
    fn phone_hvx(&mut self, b: &mut impl Board, handle: u16, kind: u8, data: &[u8]) {
        if let (2, Some(conn)) = (kind, self.phone.conn) {
            let _ = ble::hv_confirm(b, &mut self.rx, conn, handle);
        }
        let h = self.phone.h;
        if handle == 0 {
        } else if handle == h[NS] {
            let Some(n) = ph::Notif::parse(data) else { return };
            match n.event {
                0 if n.fresh() && self.phone.stored.notify => {
                    if n.category == ph::INCOMING_CALL {
                        self.phone.call = Some((n.uid, Line::new())); // its caller's name follows
                    }
                    self.phone.queue(Op::Fetch(n.uid));
                }
                2 if self.phone.call.is_some_and(|(uid, _)| uid == n.uid) => {
                    self.phone.call = None; // answered or declined on the phone, or missed
                    self.end_ring(b);
                }
                _ => {}
            }
            if let Some(conn) = self.phone.conn {
                self.next_op(b, conn);
            }
        } else if handle == h[DS] {
            match self.phone.ds.push_any(data) {
                Some(ph::Answer::Notif(a)) => self.notif_fetched(b, a),
                Some(ph::Answer::App(info)) => {
                    self.phone.apps.put(info);
                    if self.phone.asking == Some(info.id) {
                        (self.phone.asking, self.phone.app_out, self.phone.busy) = (None, false, false);
                        if self.phone.held.is_some_and(|h| h.app == info.id) {
                            self.show_held(b);
                        }
                        if let Some(conn) = self.phone.conn {
                            self.next_op(b, conn);
                        }
                    }
                }
                None => {}
            }
        } else if handle == h[EU] {
            self.phone.now.update(data);
            if core::mem::take(&mut self.phone.now.new_title) {
                let mut l = Line::<40>::new();
                l.push_str(self.phone.now.title.as_str());
                if self.screen != Screen::Media {
                    self.say(l.as_str());
                }
            }
        } else if handle == h[RC] {
            self.phone.commands = ph::ams_commands(data);
        } else if handle == h[CT] {
            self.phone_time(b, Some(data), None);
        }
    }

    /// The phone's time: the RTC set to its local time, and the offset from UTC kept for the GPS and the FIT times
    /// (replacing the one learnt at a fix). Not while a ride is open: its record times must not jump (the golden
    /// rule); the next link after it sets them.
    fn phone_time(&mut self, b: &mut impl Board, current: Option<&[u8]>, local: Option<&[u8]>) {
        if self.ride.state() != ride::State::Idle || self.rec.clean.phase() != recorder::Phase::Done {
            return;
        }
        let set = current.and_then(ph::current_time);
        if let Some(t) = set {
            let (tr, dr) = rtc_words(t);
            b.rtc_set(tr, dr);
        }
        if let Some(offset) = local.and_then(ph::local_offset)
            && self.gps.aid.offset_15min != Some(offset)
        {
            self.gps.aid.offset_15min = Some(offset);
            self.write_aid(b);
        }
        if let Some(t) = set {
            self.show_clock(t); // with the offset just learnt (a TIME ZONE needs it)
            self.seconds = t.5;
        }
    }

    /// What the media screen shows: the track from AMS, None without a secure link.
    pub(crate) fn media_now(&self) -> ui::media::Now<'_> {
        let n = &self.phone.now;
        ui::media::Now {
            title: (self.phone.link() == Link::Secure).then(|| n.title.as_str()),
            artist: n.artist.as_str(),
            album: n.album.as_str(),
            playing: n.playing,
            elapsed: n.elapsed(),
            duration: n.duration_s,
        }
    }

    /// Redraws what changed on the media screen (the time and the bar's new columns, a second).
    pub(crate) fn update_media(&mut self, b: &mut impl Board) {
        let mut shown = self.phone.shown;
        ui::media::update(&mut Panel(b, self.area(), self.look()), &self.media_now(), &mut shown, false);
        self.phone.shown = shown;
    }
}
