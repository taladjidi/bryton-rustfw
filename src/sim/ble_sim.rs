//! The simulated Nordic chip's BLE peripheral side, and a scripted phone at the other end of the link: the chip
//! keeps the GATT table the app builds and answers its calls as the SoftDevice API v5 serialization does
//! (drivers/ble.rs has the sources); the phone connects to an advertising unit, pairs (Just Works) or encrypts with
//! the key it kept, enables the HID report, serves ANCS, AMS and CTS from its own table, and posts notifications
//! and calls at scripted times. A second peer, an FTMS trainer ([`FtmsSim`]), advertises, takes the unit's
//! connection as its central on connection 1, serves its table and answers its Control Point; it can be scripted
//! to fail (a subscription forgotten, the control taken back, the link dropped, records split over notifications).
//! The chip takes or refuses the two links' configuration (`sd_ble_cfg_set`), and refuses a connection it has no
//! configuration for.

use super::*;

/// A 128-bit UUID (little endian) or a SIG 16-bit one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum U {
    Sig(u16),
    Vs([u8; 16]),
}

/// An attribute of the phone's table.
#[derive(Clone, Debug)]
pub struct Attr {
    pub handle: u16,
    pub uuid: U,
    pub kind: Kind,
    pub value: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Service,
    /// A characteristic declaration with these properties (its value is the next handle).
    Decl(u8),
    Value,
    Cccd,
}

/// What the phone does at a scripted time.
#[derive(Clone, Debug)]
pub enum Act {
    /// A notification posted: category, uid, title, message.
    Notify(u8, u32, &'static str, &'static str),
    /// A notification removed (a call answered on the phone).
    Remove(u32),
    /// The track changes.
    Track(&'static str, &'static str),
    /// The link drops (the phone walks away).
    Drop,
}

/// The phone.
pub struct PhoneSim {
    /// It connects once the unit advertises and this time has come.
    pub connect_at: u64,
    /// The LTK it kept from a bond with the unit.
    pub key: Option<[u8; 16]>,
    pub script: Vec<(u64, Act)>,
    pub db: Vec<Attr>,
    /// What it received: the unit's HID reports and AMS remote commands, the ANCS actions.
    pub reports: Vec<u8>,
    pub commands: Vec<u8>,
    pub actions: Vec<(u32, u8)>,
    /// The CCCDs of its table the unit enabled (handles).
    pub subscribed: Vec<u16>,
    /// Pending notifications (uid -> attributes) for the Data Source.
    posted: Vec<(u32, &'static str, &'static str)>,
    connected: bool,
    secure: bool,
    /// The largest ATT MTU it accepts (iOS: 185); 0: it never answers an Exchange MTU Request, 23: it refuses.
    pub max_mtu: u16,
    /// The bundle id its notifications come from.
    pub app_id: &'static str,
    /// It never answers Get App Attributes.
    pub mute_apps: bool,
    /// An Exchange MTU Request never answered: the stack refuses other GATTC calls (BUSY) until the link ends.
    pub mtu_pending: bool,
    /// The link's ATT MTU: 23 until the unit's exchange.
    pub mtu: u16,
    /// The apps it names (bundle id, display name), and the bundle ids the unit asked about, in order.
    pub apps: Vec<(&'static str, &'static str)>,
    pub app_asks: Vec<std::string::String>,
}

impl PhoneSim {
    /// The chip was reset: the link is gone (the phone is still in range, and connects again to the next advertising).
    pub fn hang_up(&mut self) {
        (self.connected, self.secure, self.mtu, self.mtu_pending) = (false, false, 23, false);
        self.subscribed.clear();
    }

    /// The link is lost without the chip's event reaching the unit (it was among bytes dropped): the phone is
    /// still in range, and connects again to the next advertising.
    pub fn lose_link(&mut self, now: u64) {
        self.hang_up();
        self.connect_at = now;
    }

    /// An iPhone: ANCS (handles 0x10..), AMS (0x20..), CTS (0x30..); `ios` false leaves only CTS.
    pub fn new(ios: bool) -> Self {
        let vs = |i: usize| U::Vs(crate::phone::VS[i]);
        let mut db = Vec::new();
        let mut add = |handle: u16, uuid: U, kind: Kind, value: &[u8]| {
            db.push(Attr { handle, uuid, kind, value: value.to_vec() })
        };
        if ios {
            add(0x10, vs(0), Kind::Service, &[]);
            add(0x11, vs(1), Kind::Decl(0x10), &[]);
            add(0x12, vs(1), Kind::Value, &[]);
            add(0x13, U::Sig(0x2902), Kind::Cccd, &[0, 0]);
            add(0x14, vs(2), Kind::Decl(0x08), &[]);
            add(0x15, vs(2), Kind::Value, &[]);
            add(0x16, vs(3), Kind::Decl(0x10), &[]);
            add(0x17, vs(3), Kind::Value, &[]);
            add(0x18, U::Sig(0x2902), Kind::Cccd, &[0, 0]);
            add(0x20, vs(4), Kind::Service, &[]);
            add(0x21, vs(5), Kind::Decl(0x18), &[]);
            add(0x22, vs(5), Kind::Value, &[]);
            add(0x23, U::Sig(0x2902), Kind::Cccd, &[0, 0]);
            add(0x24, vs(6), Kind::Decl(0x18), &[]);
            add(0x25, vs(6), Kind::Value, &[]);
            add(0x26, U::Sig(0x2902), Kind::Cccd, &[0, 0]);
            add(0x27, vs(7), Kind::Decl(0x0a), &[]);
            add(0x28, vs(7), Kind::Value, &[]);
        }
        add(0x30, U::Sig(0x1805), Kind::Service, &[]);
        add(0x31, U::Sig(0x2a2b), Kind::Decl(0x12), &[]);
        // 2026-10-25 09:41:07, a Sunday
        add(0x32, U::Sig(0x2a2b), Kind::Value, &[0xea, 0x07, 10, 25, 9, 41, 7, 7, 0, 1]);
        add(0x33, U::Sig(0x2902), Kind::Cccd, &[0, 0]);
        add(0x34, U::Sig(0x2a0f), Kind::Decl(0x02), &[]);
        add(0x35, U::Sig(0x2a0f), Kind::Value, &[4, 0]); // UTC + 1 h, no DST
        PhoneSim {
            connect_at: 0,
            key: None,
            script: Vec::new(),
            db,
            reports: Vec::new(),
            commands: Vec::new(),
            actions: Vec::new(),
            subscribed: Vec::new(),
            posted: Vec::new(),
            connected: false,
            secure: false,
            max_mtu: 185,
            app_id: "com.phone",
            mute_apps: false,
            mtu_pending: false,
            mtu: 23,
            apps: vec![("com.phone", "Phone")],
            app_asks: Vec::new(),
        }
    }
}

/// The unit's side, kept by the chip: its GATT table and how it advertises.
#[derive(Default)]
pub struct Server {
    /// (handle, 16-bit UUID, is a CCCD, value).
    pub attrs: Vec<(u16, u16, bool, Vec<u8>)>,
    /// The vendor UUID bases registered, by type - 2.
    pub vs: Vec<[u8; 16]>,
    /// Advertising: interval, time-out (s), since when.
    pub adv: Option<(u16, u16, u64)>,
    pub adv_data: Vec<u8>,
    /// The system attributes set, as the app gave them.
    pub sys_attrs: Option<Vec<u8>>,
    /// Notifications sent: (handle, value).
    pub sent: Vec<(u16, Vec<u8>)>,
}

impl Server {
    fn next(&self) -> u16 {
        self.attrs.last().map_or(1, |a| a.0 + 1)
    }

    /// The CCCDs on: from the system attributes set, as (handle, value) pairs (this model's own format).
    fn cccd_on(&self, h: u16) -> bool {
        self.sys_attrs.as_ref().is_some_and(|s| s.chunks_exact(4).any(|c| c == [h as u8, (h >> 8) as u8, 1, 0]))
    }
}

/// The LTK the unit gives the phone at pairing, and its EDIV.
pub const LTK: [u8; 16] = [0x5a; 16];

impl Nordic {
    pub(super) fn evt(&mut self, id: u16, body: &[u8]) {
        let mut p = vec![2];
        p.extend(id.to_le_bytes());
        p.extend(body);
        self.queue(&p);
    }

    fn gattc(&mut self, id: u16, status: u16, body: &[u8]) {
        self.gattc_on(0, id, status, body);
    }

    fn gattc_on(&mut self, conn: u16, id: u16, status: u16, body: &[u8]) {
        if self.drop_gattc == Some((conn, id)) {
            self.drop_gattc = None;
            return;
        }
        if self.mute_gattc == Some((conn, id)) {
            return;
        }
        let mut p = conn.to_le_bytes().to_vec();
        p.extend(status.to_le_bytes());
        p.extend([0, 0]);
        p.extend(body);
        self.evt(id, &p);
    }

    /// A UUID as the unit's stack names it: 16-bit part and type (0 for a base not registered).
    fn name(&self, u: U) -> (u16, u8) {
        match u {
            U::Sig(v) => (v, 1),
            U::Vs(b) => {
                let base = |x: &[u8; 16]| {
                    let mut x = *x;
                    (x[12], x[13]) = (0, 0);
                    x
                };
                let t = self.server.vs.iter().position(|v| base(v) == base(&b)).map_or(0, |i| i as u8 + 2);
                (u16::from_le_bytes([b[12], b[13]]), t)
            }
        }
    }

    /// The scripted phone's moves at `now`.
    pub(super) fn phone_tick(&mut self, now: u64) {
        let Some(mut ph) = self.phone.take() else { return };
        if !ph.connected && self.server.adv.is_some() && now >= ph.connect_at {
            ph.connected = true;
            self.server.adv = None;
            let mut body = vec![0, 0, 3, 1, 2, 3, 4, 5, 6, 1]; // conn 0, a random address, role peripheral
            body.extend([24, 0, 24, 0, 0, 0, 0x90, 1]);
            self.evt(0x10, &body);
            if let Some(_key) = ph.key {
                // it encrypts with the key it kept: the unit is asked for it by EDIV and Rand
                let mut b = vec![0, 0, 3, 1, 2, 3, 4, 5, 6];
                b.extend(0x1234u16.to_le_bytes());
                b.extend([7; 8]);
                b.push(1);
                self.evt(0x14, &b);
            }
        }
        if let Some((_, timeout, since)) = self.server.adv
            && timeout > 0
            && now >= since + 1000 * u64::from(timeout)
        {
            self.server.adv = None;
            self.evt(0x1b, &[0xff, 0xff, 0]); // advertising timed out
        }
        while ph.connected && ph.secure && ph.script.first().is_some_and(|(t, _)| now >= *t) {
            // a notification or a track waits for the unit to subscribe, as on a phone (nothing is sent before)
            let ready = |h| ph.subscribed.contains(&h);
            match ph.script[0].1 {
                Act::Notify(..) | Act::Remove(_) if !ready(0x13) => break,
                Act::Track(..) if !ready(0x26) => break,
                _ => {}
            }
            let (_, act) = ph.script.remove(0);
            match act {
                Act::Notify(cat, uid, title, msg) => {
                    ph.posted.push((uid, title, msg));
                    let mut v = vec![0, 0, cat, 1];
                    v.extend(uid.to_le_bytes());
                    self.hvx(0x12, &v);
                }
                Act::Remove(uid) => {
                    let mut v = vec![2, 0, 1, 0];
                    v.extend(uid.to_le_bytes());
                    self.hvx(0x12, &v);
                }
                Act::Track(title, artist) => {
                    self.hvx(0x22, &[0, 1, 2, 3, 4, 5, 6]); // a music app: no skips (AMS Remote Command)
                    self.hvx(0x25, &[&[2, 2, 0][..], title.as_bytes()].concat());
                    self.hvx(0x25, &[&[2, 0, 0][..], artist.as_bytes()].concat());
                    self.hvx(0x25, &[&[2, 1, 0][..], b"An Album"].concat());
                    self.hvx(0x25, &[&[2, 3, 0][..], b"215.4"].concat()); // the duration, s
                    self.hvx(0x25, &[0, 1, 0, b'1', b',', b'1', b'.', b'0', b',', b'6', b'4', b'.', b'0']);
                }
                Act::Drop => {
                    (ph.connected, ph.secure) = (false, false);
                    ph.connect_at = u64::MAX; // gone for good
                    self.evt(0x11, &[0, 0, 0x08]); // supervision time-out
                }
            }
        }
        self.phone = Some(ph);
    }

    fn hvx(&mut self, handle: u16, data: &[u8]) {
        let mut b = handle.to_le_bytes().to_vec();
        b.push(1);
        b.extend((data.len() as u16).to_le_bytes());
        b.extend(data);
        self.gattc(0x39, 0, &b);
    }

    /// The link is now encrypted.
    fn secured(&mut self, ph: &mut PhoneSim) {
        ph.secure = true;
        self.evt(0x1a, &[0, 0, 0x21, 16]);
    }

    /// After pairing, the phone, as a HID host, enables the unit's report.
    fn host_enables_report(&mut self) {
        if let Some(&(h, ..)) = self.server.attrs.iter().find(|a| a.2 && self.report_cccd(a.0)) {
            let mut b = vec![0, 0];
            b.extend(h.to_le_bytes());
            b.extend([0x02, 0x29, 1, 1, 0, 0, 0, 2, 0, 1, 0]);
            if let Some(a) = self.server.attrs.iter_mut().find(|a| a.0 == h) {
                a.3 = vec![1, 0];
            }
            self.evt(0x50, &b);
        }
    }

    /// `h` is the CCCD of the HID report (0x2a4d).
    fn report_cccd(&self, h: u16) -> bool {
        self.server.attrs.iter().any(|a| a.1 == 0x2a4d && a.0 + 1 == h)
    }

    /// The attributes in `start..=end` of the peer on connection `conn` (1 the trainer's, else the phone's) that `f`
    /// takes.
    fn phone_attrs(&self, conn: u16, start: u16, end: u16, f: impl Fn(&Attr) -> bool) -> Vec<Attr> {
        let db = match conn {
            1 => self.ftms.as_ref().map_or(&[][..], |t| &t.db[..]),
            _ => self.phone.as_ref().map_or(&[][..], |p| &p.db[..]),
        };
        db.iter().filter(|a| (start..=end).contains(&a.handle) && f(a)).cloned().collect()
    }

    /// Answers a BLE command of the peripheral side; false if it is not one.
    pub(super) fn take_ble(&mut self, p: &[u8], now: u64) -> bool {
        let [0, op, args @ ..] = p else { return false };
        if self.refuse == Some(*op) {
            self.queue(&[1, *op, 4, 0, 0, 0]);
            return true;
        }
        if matches!(*op, 0x94 | 0x96 | 0x97 | 0x9a | 0x9c)
            && args.len() >= 2
            && u16::from_le_bytes([args[0], args[1]]) != 1
            && self.phone.as_ref().is_some_and(|ph| ph.mtu_pending)
        {
            self.queue(&[1, *op, 17, 0, 0, 0]); // NRF_ERROR_BUSY
            return true;
        }
        let ok = |n: &mut Nordic, out: &[u8]| n.queue(&[&[1, *op, 0, 0, 0, 0][..], out].concat());
        let u16_at = |i: usize| u16::from_le_bytes([args[i], args[i + 1]]);
        match *op {
            0x62 => {
                let mut b = [0; 16];
                b.copy_from_slice(&args[1..17]);
                self.server.vs.push(b);
                let t = self.server.vs.len() as u8 + 1;
                ok(self, &[1, t]);
            }
            0x72 => {
                self.server.adv_data = args[2..2 + usize::from(args[0])].to_vec();
                ok(self, &[]);
            }
            0x73 => {
                assert!(self.phone.as_ref().is_none_or(|p| !p.connected), "advertising while connected");
                assert_eq!(args[9], self.tag(), "advertising on the configured tag");
                self.server.adv = Some((u16_at(4), u16_at(6), now));
                ok(self, &[]);
            }
            0x74 => {
                self.server.adv = None;
                ok(self, &[]);
            }
            0xa0 => {
                let h = self.server.next();
                self.server.attrs.push((h, u16_at(2), false, vec![]));
                ok(self, &[1, h as u8, (h >> 8) as u8]);
            }
            0xa2 => {
                // service, then the metadata: properties at 3; the value's UUID and data after the CCCD's metadata
                let props = args[3];
                let notify = props & 0x30 != 0;
                let at = if notify { 17 } else { 14 }; // the value attribute, past the metadata and its CCCD's
                let uuid = u16::from_le_bytes([args[at + 2], args[at + 3]]);
                let len = usize::from(u16::from_le_bytes([args[at + 13], args[at + 14]]));
                let value = args[at + 16..at + 16 + len].to_vec();
                let decl = self.server.next();
                self.server.attrs.push((decl, 0x2803, false, vec![props]));
                self.server.attrs.push((decl + 1, uuid, false, value));
                let cccd = if notify {
                    self.server.attrs.push((decl + 2, 0x2902, true, vec![0, 0]));
                    decl + 2
                } else {
                    0
                };
                let [v0, v1] = (decl + 1).to_le_bytes();
                let [c0, c1] = cccd.to_le_bytes();
                ok(self, &[1, v0, v1, 0, 0, c0, c1, 0, 0]);
            }
            0xa3 => {
                let h = self.server.next();
                self.server.attrs.push((h, u16_at(4), false, vec![]));
                ok(self, &[1, h as u8, (h >> 8) as u8]);
            }
            0xa6 => {
                let (handle, len) = (u16_at(3), usize::from(u16_at(9)));
                let data = args[12..12 + len].to_vec();
                let cccd_on = self.server.cccd_on(handle + 1)
                    || self.server.attrs.iter().any(|a| a.0 == handle + 1 && a.2 && a.3.first() == Some(&1));
                if !cccd_on {
                    self.queue(&[1, 0xa6, 0x08, 0, 0, 0]); // NRF_ERROR_INVALID_STATE: notifications off
                    return true;
                }
                if self.server.attrs.iter().any(|a| a.0 == handle && a.1 == 0x2a4d)
                    && let Some(ph) = &mut self.phone
                {
                    ph.reports.push(data[0]);
                }
                self.server.sent.push((handle, data));
                ok(self, &[1, len as u8, 0]);
            }
            0xa9 => {
                let len = usize::from(u16_at(2));
                self.server.sys_attrs = (args[4] == 1).then(|| args[5..5 + len].to_vec());
                ok(self, &[]);
            }
            0xaa if u16_at(0) == 1 && self.ftms.as_ref().is_some_and(|t| !t.connected) && self.phone.is_none() => {
                self.queue(&[1, 0xaa, 0x02, 0x30, 0, 0]); // the trainer's link is gone
            }
            0xaa if self.phone.as_ref().is_some_and(|p| !p.connected) => {
                self.queue(&[1, 0xaa, 0x02, 0x30, 0, 0]); // BLE_ERROR_INVALID_CONN_HANDLE: no such link
            }
            0xaa => {
                // this model's system attributes: (handle, value) of each CCCD on
                let mut s = Vec::new();
                for a in self.server.attrs.iter().filter(|a| a.2 && a.3.first() == Some(&1)) {
                    s.extend(a.0.to_le_bytes());
                    s.extend([1, 0]);
                }
                let mut out = vec![1];
                out.extend((s.len() as u16).to_le_bytes());
                out.push(1);
                out.extend(s);
                ok(self, &out);
            }
            0x7e => {
                ok(self, &[]);
                self.evt(0x13, &[0, 0, 0x31, 7, 16, 1, 3]); // the phone answers the request by pairing
            }
            0x7f => {
                ok(self, &[]);
                let Some(mut ph) = self.phone.take() else { return true };
                if args[2] == 0 {
                    ph.key = Some(LTK);
                    let mut a = vec![0, 0, 0, 4, 0x03, 0x03, 0x01, 0x03]; // success, bonded
                    a.push(1);
                    a.extend(LTK);
                    a.push(16 << 2);
                    a.extend(0x1234u16.to_le_bytes());
                    a.extend([7; 8]);
                    a.extend([0, 0, 0]);
                    a.push(1);
                    a.extend([0x33; 27]);
                    a.push(1);
                    a.extend([0x44; 23]);
                    a.extend([0, 0]);
                    self.secured(&mut ph);
                    self.evt(0x19, &a);
                    self.host_enables_report();
                } else {
                    self.evt(0x19, &[0, 0, 0x85, 0, 0, 0, 0, 0]);
                }
                self.phone = Some(ph);
            }
            0x86 => {
                ok(self, &[]);
                let Some(mut ph) = self.phone.take() else { return true };
                let given = (args[2] == 1).then(|| &args[3..19]);
                if given.is_some() && given == ph.key.as_ref().map(|k| &k[..]) {
                    self.secured(&mut ph);
                } else {
                    (ph.connected, ph.key) = (false, None); // the phone drops the link and its bond
                    ph.connect_at = u64::MAX;
                    self.evt(0x11, &[0, 0, 0x3d]); // MIC failure
                }
                self.phone = Some(ph);
            }
            0x76 if u16_at(0) == 1 => {
                ok(self, &[]);
                if let Some(t) = &mut self.ftms {
                    t.hang_up();
                }
                self.evt(0x11, &[1, 0, 0x16]);
            }
            0x76 => {
                ok(self, &[]);
                if let Some(ph) = &mut self.phone {
                    (ph.connected, ph.secure, ph.connect_at, ph.mtu_pending) = (false, false, u64::MAX, false);
                }
                self.evt(0x11, &[0, 0, 0x16]);
            }
            0x94 => {
                let (conn, want) = (u16_at(0), (u16_at(5), args[7]));
                let svcs = self.phone_attrs(conn, 1, 0xffff, |a| a.kind == Kind::Service);
                let last = self.phone_attrs(conn, 1, 0xffff, |_| true).last().map_or(0, |a| a.handle);
                match svcs.iter().find(|s| self.name(s.uuid) == want) {
                    Some(s) => {
                        let next = svcs.iter().map(|t| t.handle).find(|&h| h > s.handle);
                        let end = next.map_or(last, |h| h - 1);
                        let mut b = vec![1, 0];
                        b.extend(want.0.to_le_bytes());
                        b.push(want.1);
                        b.extend(s.handle.to_le_bytes());
                        b.extend(end.to_le_bytes());
                        ok(self, &[]);
                        self.gattc_on(conn, 0x30, 0, &b);
                    }
                    None => {
                        ok(self, &[]);
                        self.gattc_on(conn, 0x30, 0x010a, &[0, 0]);
                    }
                }
            }
            0x96 => {
                let (conn, start, end) = (u16_at(0), u16_at(3), u16_at(5));
                let decls: Vec<_> = self
                    .phone_attrs(conn, start, end, |a| matches!(a.kind, Kind::Decl(_)))
                    .into_iter()
                    .take(3)
                    .collect();
                ok(self, &[]);
                if decls.is_empty() {
                    self.gattc_on(conn, 0x32, 0x010a, &[0, 0]);
                } else {
                    let mut b = (decls.len() as u16).to_le_bytes().to_vec();
                    for d in decls {
                        let Kind::Decl(props) = d.kind else { continue };
                        let (u, t) = self.name(d.uuid);
                        b.extend(u.to_le_bytes());
                        b.extend([t, props, 0]);
                        b.extend(d.handle.to_le_bytes());
                        b.extend((d.handle + 1).to_le_bytes());
                    }
                    self.gattc_on(conn, 0x32, 0, &b);
                }
            }
            0x97 => {
                let (conn, start, end) = (u16_at(0), u16_at(3), u16_at(5));
                let descs = self.phone_attrs(conn, start, end, |a| a.kind == Kind::Cccd);
                ok(self, &[]);
                if descs.is_empty() {
                    self.gattc_on(conn, 0x33, 0x010a, &[0, 0]);
                } else {
                    let mut b = (descs.len() as u16).to_le_bytes().to_vec();
                    for d in descs {
                        b.extend(d.handle.to_le_bytes());
                        b.extend([0x02, 0x29, 1]);
                    }
                    self.gattc_on(conn, 0x33, 0, &b);
                }
            }
            0x9a => {
                let (conn, h) = (u16_at(0), u16_at(2));
                let v = self.phone_attrs(conn, h, h, |_| true).first().map(|a| a.value.clone()).unwrap_or_default();
                ok(self, &[]);
                let mut b = h.to_le_bytes().to_vec();
                b.extend([0, 0]);
                b.extend((v.len() as u16).to_le_bytes());
                b.extend(v);
                self.gattc_on(conn, 0x36, 0, &b);
            }
            0x9c if u16_at(0) == 1 => {
                let (h, len) = (u16_at(5), usize::from(u16_at(9)));
                let data = args[12..12 + len].to_vec();
                ok(self, &[]);
                self.ftms_write(h, &data, now);
            }
            0x9e if u16_at(0) != 1 => {
                // Exchange MTU Request: refused when the chip was not configured for that much (INVALID_PARAM)
                let want = u16_at(2);
                let conf =
                    self.cfg.iter().find(|c| c[..2] == [0x23, 0]).map_or(23, |c| u16::from_le_bytes([c[6], c[7]]));
                if want > conf {
                    self.queue(&[1, *op, 7, 0, 0, 0]);
                } else {
                    ok(self, &[]);
                    if let Some(ph) = &mut self.phone
                        && ph.max_mtu == 0
                    {
                        ph.mtu_pending = true;
                    } else if let Some(ph) = &mut self.phone {
                        ph.mtu = want.min(ph.max_mtu);
                        let m = ph.max_mtu;
                        self.gattc(0x3a, 0, &m.to_le_bytes());
                    }
                }
            }
            0x9d if u16_at(0) == 1 => {
                ok(self, &[]);
                if let Some(t) = &mut self.ftms {
                    t.unconfirmed = false;
                }
            }
            0x9c => {
                let (h, len) = (u16_at(5), usize::from(u16_at(9)));
                let data = args[12..12 + len].to_vec();
                ok(self, &[]);
                let mut b = h.to_le_bytes().to_vec();
                b.extend([1, 0, 0, 0, 0]);
                self.gattc(0x38, 0, &b);
                let Some(mut ph) = self.phone.take() else { return true };
                match (h, &data[..]) {
                    (0x13 | 0x18 | 0x23 | 0x26 | 0x33, [1, 0]) => ph.subscribed.push(h),
                    (0x15, [0, u0, u1, u2, u3, ..]) => {
                        let uid = u32::from_le_bytes([*u0, *u1, *u2, *u3]);
                        if let Some(&(_, title, msg)) = ph.posted.iter().find(|p| p.0 == uid) {
                            // the answer, in notifications of the link's MTU less 3
                            let mut a = vec![0];
                            a.extend(uid.to_le_bytes());
                            a.push(0);
                            a.extend((ph.app_id.len() as u16).to_le_bytes());
                            a.extend(ph.app_id.as_bytes());
                            a.push(1);
                            a.extend((title.len() as u16).to_le_bytes());
                            a.extend(title.as_bytes());
                            a.push(3);
                            a.extend((msg.len() as u16).to_le_bytes());
                            a.extend(msg.as_bytes());
                            for c in a.chunks(usize::from(ph.mtu) - 3) {
                                self.hvx(0x17, c);
                            }
                        }
                    }
                    (0x15, [1, id @ ..]) => {
                        // Get App Attributes: the identifier NUL-terminated, then the attribute ids
                        let z = id.iter().position(|&c| c == 0).unwrap_or(id.len());
                        let id = std::string::String::from_utf8_lossy(&id[..z]).into_owned();
                        let name = ph.apps.iter().find(|a| a.0 == id).map_or("", |a| a.1);
                        let mut a = vec![1];
                        a.extend(id.bytes());
                        a.extend([0, 0]);
                        a.extend((name.len() as u16).to_le_bytes());
                        a.extend(name.as_bytes());
                        ph.app_asks.push(id);
                        if !ph.mute_apps {
                            for c in a.chunks(usize::from(ph.mtu) - 3) {
                                self.hvx(0x17, c);
                            }
                        }
                    }
                    (0x15, [2, u0, u1, u2, u3, action]) => {
                        ph.actions.push((u32::from_le_bytes([*u0, *u1, *u2, *u3]), *action))
                    }
                    (0x22, [cmd]) => ph.commands.push(*cmd),
                    _ => {}
                }
                self.phone = Some(ph);
            }
            0x69 => {
                self.cfg.push(args.to_vec());
                ok(self, &[]);
            }
            0x8b => {
                self.scan_end = None;
                ok(self, &[]);
            }
            0x8c => {
                // the address, the scan and the connection parameters, the tag
                let peer: [u8; 7] = args[1..8].try_into().unwrap();
                if args[25] != self.tag() || self.tag() == 0 {
                    self.queue(&[1, 0x8c, 0x12, 0, 0, 0]); // NRF_ERROR_CONN_COUNT: no link configured for it
                    return true;
                }
                self.connects += 1;
                ok(self, &[]);
                if let Some(t) = &mut self.ftms
                    && t.peer() == peer
                    && t.connectable
                {
                    t.connected = true;
                    t.next_ms = now + 250;
                    let mut body = vec![1, 0];
                    body.extend(peer);
                    body.push(2); // role central
                    body.extend([6, 0, 12, 0, 0, 0, 0x90, 1]);
                    self.evt(0x10, &body);
                }
            }
            0x8d => ok(self, &[]),
            0x75 => {
                self.param_updates.push(args.to_vec());
                ok(self, &[]);
            }
            0x9d | 0x8f | 0x90 | 0x66 | 0xad | 0x78 | 0x7a | 0x7c | 0xa4 => ok(self, &[]),
            _ => return false,
        }
        true
    }
}

/// An FTMS indoor bike trainer (drivers/ftms.rs), or (`new_fec`) an FE-C over BLE one (drivers/fecble.rs) in range: it advertises the service, takes a central's connection
/// on connection 1, serves Feature, Indoor Bike Data, the ranges, the Control Point and the Machine Status from its
/// table (handles 0x40..), notifies its data every 250 ms once subscribed, and answers each Control Point write
/// with an indication, which it holds until the last one is confirmed (FTMS: one procedure at a time).
pub struct FtmsSim {
    pub addr: u8,
    pub db: Vec<Attr>,
    pub connected: bool,
    /// It takes connections (out of range, or switched off: not).
    pub connectable: bool,
    /// The Control Point writes it received, in order.
    pub got: Vec<Vec<u8>>,
    /// The control is the unit's; the last target op code (0: none) and its values.
    pub control: bool,
    pub mode: u8,
    pub target_w: i16,
    pub resistance: u8,
    pub grade_x100: i16,
    /// The power the rider makes outside ERG, W; the power and speed (mm/s) now.
    pub rider_w: u16,
    pub power: u16,
    pub speed_mm_s: u32,
    /// Faults: its Control Point's subscription forgotten once (the next write gets 0xFD); each record split over
    /// two notifications (More Data); the control taken back (status 0xff) at this time; the link dropped at this
    /// time; the control refused.
    pub forget_cccd: bool,
    pub split: bool,
    pub lose_control_at: Option<u64>,
    pub drop_at: Option<u64>,
    pub refuse_control: bool,
    /// The Control Point's responses never sent.
    pub mute_cp: bool,
    /// A write while a procedure was in progress, or an indication sent before the last was confirmed: client
    /// errors (FTMS 4.16.3), counted.
    pub violations: u32,
    /// FE-C over BLE (the D500's GATT table: Cycling Power, the vendor service 6e40fec1 with its notify and write
    /// characteristics, the DFU service): its checksum is the sum's, not the XOR's; the frames it notifies next;
    /// frames written that were no frame; when the last one was written.
    pub fec: bool,
    pub sum: bool,
    out: Vec<Vec<u8>>,
    pub rejected: u32,
    pub raw: Vec<u8>,
    last_write_ms: Option<u64>,
    /// The response to indicate, and whether the last indication waits for its confirmation.
    pending: Option<Vec<u8>>,
    pub(super) unconfirmed: bool,
    pub(super) next_ms: u64,
}

/// The trainer's handles: Indoor Bike Data, the Control Point, the Machine Status (values; each CCCD is the next).
pub const FT_DATA: u16 = 0x44;
pub const FT_CP: u16 = 0x4b;
pub const FT_STATUS: u16 = 0x4e;
/// The FE-C over BLE trainer's handles: its notify characteristic's value (the CCCD is the next) and its write one.
pub const FC_NOTIFY: u16 = 0x42;
pub const FC_WRITE: u16 = 0x45;

impl FtmsSim {
    pub fn new() -> Self {
        let mut db = Vec::new();
        let mut add = |handle: u16, uuid: u16, kind: Kind, value: &[u8]| {
            db.push(Attr { handle, uuid: U::Sig(uuid), kind, value: value.to_vec() })
        };
        add(0x40, 0x1826, Kind::Service, &[]);
        add(0x41, 0x2acc, Kind::Decl(0x02), &[]);
        // cadence, power; resistance, power and simulation targets
        add(0x42, 0x2acc, Kind::Value, &[0x02, 0x40, 0, 0, 0x0c, 0x20, 0, 0]);
        add(0x43, 0x2ad2, Kind::Decl(0x10), &[]);
        add(FT_DATA, 0x2ad2, Kind::Value, &[]);
        add(0x45, 0x2902, Kind::Cccd, &[0, 0]);
        add(0x46, 0x2ad6, Kind::Decl(0x02), &[]);
        add(0x47, 0x2ad6, Kind::Value, &[0, 0, 200, 0, 10, 0]); // 0 to 20.0
        add(0x48, 0x2ad8, Kind::Decl(0x02), &[]);
        add(0x49, 0x2ad8, Kind::Value, &[0, 0, 0xdc, 0x05, 1, 0]); // 0 to 1500 W
        add(0x4a, 0x2ad9, Kind::Decl(0x28), &[]);
        add(FT_CP, 0x2ad9, Kind::Value, &[]);
        add(0x4c, 0x2902, Kind::Cccd, &[0, 0]);
        add(0x4d, 0x2ada, Kind::Decl(0x10), &[]);
        add(FT_STATUS, 0x2ada, Kind::Value, &[]);
        add(0x4f, 0x2902, Kind::Cccd, &[0, 0]);
        FtmsSim {
            addr: 0xc4,
            db,
            connected: false,
            connectable: true,
            got: Vec::new(),
            control: false,
            mode: 0,
            target_w: 0,
            resistance: 0,
            grade_x100: 0,
            rider_w: 150,
            power: 0,
            speed_mm_s: 0,
            forget_cccd: false,
            split: false,
            lose_control_at: None,
            drop_at: None,
            refuse_control: false,
            mute_cp: false,
            violations: 0,
            fec: false,
            sum: false,
            out: Vec::new(),
            rejected: 0,
            raw: Vec::new(),
            last_write_ms: None,
            pending: None,
            unconfirmed: false,
            next_ms: 0,
        }
    }

    /// The D500 as seen from a computer: Cycling Power, the vendor service 6e40fec1 (fec2 notify, fec3 write and
    /// write without response), the Nordic DFU service nobody touches; 0x1818, 0x1816 in its advertising, the
    /// 128-bit UUID and the name in the scan response.
    pub fn new_fec() -> Self {
        let mut t = Self::new();
        let vs = |p: u16| {
            let mut b = crate::fecble::BASE;
            b[12..14].copy_from_slice(&p.to_le_bytes());
            U::Vs(b)
        };
        let mut db = Vec::new();
        let mut add = |handle: u16, uuid: U, kind: Kind, value: &[u8]| {
            db.push(Attr { handle, uuid, kind, value: value.to_vec() })
        };
        add(0x20, U::Sig(0x1818), Kind::Service, &[]);
        add(0x21, U::Sig(0x2a63), Kind::Decl(0x10), &[]);
        add(0x22, U::Sig(0x2a63), Kind::Value, &[]);
        add(0x23, U::Sig(0x2902), Kind::Cccd, &[0, 0]);
        add(0x40, vs(0xfec1), Kind::Service, &[]);
        add(0x41, vs(0xfec2), Kind::Decl(0x10), &[]);
        add(FC_NOTIFY, vs(0xfec2), Kind::Value, &[]);
        add(0x43, U::Sig(0x2902), Kind::Cccd, &[0, 0]);
        add(0x44, vs(0xfec3), Kind::Decl(0x0c), &[]);
        add(FC_WRITE, vs(0xfec3), Kind::Value, &[]);
        add(0x50, U::Sig(0xfe59), Kind::Service, &[]);
        (t.db, t.fec, t.addr) = (db, true, 0xc5);
        t
    }

    /// A frame written to its write characteristic (or the CCCD): the GATT status. A page is applied as the
    /// trainer would (49 target power, 48 resistance, 51 track), answered by a page 71, the capabilities (54) when
    /// asked; messages closer than 200 ms are a violation (the unit sends one per 250 ms).
    fn fec_write(&mut self, h: u16, data: &[u8], now: u64) -> u16 {
        if h != FC_WRITE {
            if let Some(a) = self.db.iter_mut().find(|a| a.handle == h && a.kind == Kind::Cccd) {
                a.value = data.to_vec();
            }
            return 0;
        }
        let Some((p, _)) = crate::fecble::parse(data) else {
            self.rejected += 1;
            return 0;
        };
        if self.last_write_ms.is_some_and(|at| now < at + 200) {
            self.violations += 1;
        }
        self.last_write_ms = Some(now);
        self.got.push(p.to_vec());
        self.raw = data.to_vec();
        let u16_at = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
        match p[0] {
            0x31 => (self.mode, self.target_w, self.control) = (5, (u16_at(6) / 4) as i16, true),
            0x30 => (self.mode, self.resistance, self.control) = (4, p[7], true),
            0x33 => (self.mode, self.grade_x100, self.control) = (0x11, (i32::from(u16_at(5)) - 20_000) as i16, true),
            0x46 if p[6] == 54 => {
                self.out.push(crate::fecble::broadcast([54, 255, 255, 255, 255, 0xe8, 0x03, 0x07], self.sum).to_vec())
            }
            _ => {}
        }
        if matches!(p[0], 0x30 | 0x31 | 0x33) {
            self.out.push(crate::fecble::broadcast([71, p[0], 0xff, 0, 255, 255, 0x20, 0x03], self.sum).to_vec());
        }
        0
    }

    /// Its address as the unit's chip reports it (`adv_report` in src/sim/: type byte 0, then the 6 bytes).
    pub fn peer(&self) -> [u8; 7] {
        [0, self.addr, 1, 2, 3, 4, 5]
    }

    /// The link is gone: its subscriptions and the control with it.
    pub fn hang_up(&mut self) {
        (self.connected, self.control, self.pending, self.unconfirmed) = (false, false, None, false);
        (self.out, self.last_write_ms) = (Vec::new(), None);
        for a in self.db.iter_mut().filter(|a| a.kind == Kind::Cccd) {
            a.value = vec![0, 0];
        }
    }

    /// The CCCD after `value` is set to `bits` (1 notifications, 2 indications).
    pub fn on(&self, value: u16, bits: u8) -> bool {
        self.db.iter().any(|a| a.handle == value + 1 && a.value.first() == Some(&bits))
    }

    /// Its advertising: the Service Data (available, an indoor bike), then the scan response with its name.
    pub fn reports(&self) -> [(u8, i8, Vec<u8>); 2] {
        if self.fec {
            let mut scan = b"\x09\x09SIM D500".to_vec();
            scan.extend([17, 0x07]);
            scan.extend(crate::fecble::BASE);
            return [(self.addr, -55, vec![2, 0x01, 0x06, 5, 0x03, 0x18, 0x18, 0x16, 0x18]), (self.addr, -56, scan)];
        }
        [
            (self.addr, -55, vec![2, 0x01, 0x06, 6, 0x16, 0x26, 0x18, 0x01, 0x20, 0x00]),
            (self.addr, -56, b"\x09\x09SIM BIKE".to_vec()),
        ]
    }
}

impl Nordic {
    /// The connection configuration tag the app configured for two links (1), or 0 (the default).
    pub(super) fn tag(&self) -> u8 {
        let gap = self.cfg.iter().any(|c| c[..2] == [0x20, 0] && c[5] == 1 && c[6] >= 2);
        let roles = self.cfg.iter().any(|c| c[..2] == [0x40, 0] && c[6] >= 1);
        if gap && roles { 1 } else { 0 }
    }

    /// A write to the trainer's attribute `h`: a CCCD, or its Control Point.
    fn ftms_write(&mut self, h: u16, data: &[u8], now: u64) {
        let Some(mut t) = self.ftms.take() else { return };
        let mut status = 0u16;
        if t.fec {
            status = t.fec_write(h, data, now);
        } else if h == FT_CP {
            if !t.on(FT_CP, 2) || core::mem::take(&mut t.forget_cccd) {
                if let Some(a) = t.db.iter_mut().find(|a| a.handle == FT_CP + 1) {
                    a.value = vec![0, 0];
                }
                status = 0x01fd;
            } else if t.pending.is_some() || t.unconfirmed {
                t.violations += 1;
                status = 0x01fe;
            } else {
                t.got.push(data.to_vec());
                let result = match *data {
                    [0x00] if t.refuse_control => 4,
                    [0x00] => {
                        t.control = true;
                        1
                    }
                    [op, ..] if !t.control && matches!(op, 0x04 | 0x05 | 0x11) => 5,
                    [0x05, p0, p1] => {
                        (t.mode, t.target_w) = (5, i16::from_le_bytes([p0, p1]));
                        1
                    }
                    [0x04, r] => {
                        (t.mode, t.resistance) = (4, r);
                        1
                    }
                    [0x11, _, _, g0, g1, _, _] => {
                        (t.mode, t.grade_x100) = (0x11, i16::from_le_bytes([g0, g1]));
                        1
                    }
                    _ => 2,
                };
                t.pending = Some(vec![0x80, data.first().copied().unwrap_or(0), result]);
            }
        } else if let Some(a) = t.db.iter_mut().find(|a| a.handle == h && a.kind == Kind::Cccd) {
            a.value = data.to_vec();
        }
        let mut b = h.to_le_bytes().to_vec();
        b.extend([1, 0, 0, 0, 0]);
        self.gattc_on(1, 0x38, status, &b);
        self.ftms = Some(t);
        self.ftms_tick(now);
    }

    fn hvx_on(&mut self, conn: u16, handle: u16, kind: u8, data: &[u8]) {
        let mut b = handle.to_le_bytes().to_vec();
        b.push(kind);
        b.extend((data.len() as u16).to_le_bytes());
        b.extend(data);
        self.gattc_on(conn, 0x39, 0, &b);
    }

    /// The FE-C over BLE trainer's moves at `now`: its drop, the frames it owes, pages 16 and 25 every 250 ms.
    fn fec_tick(&mut self, t: &mut FtmsSim, now: u64) {
        if !t.connected {
            return;
        }
        if t.drop_at.is_some_and(|at| now >= at) {
            t.drop_at = None;
            t.hang_up();
            self.evt(0x11, &[1, 0, 0x08]);
            return;
        }
        if t.on(FC_NOTIFY, 1) {
            for f in std::mem::take(&mut t.out) {
                self.hvx_on(1, FC_NOTIFY, 1, &f);
            }
        }
        while now >= t.next_ms {
            t.next_ms += 250;
            t.power = if t.mode == 5 && t.control { t.target_w.max(0) as u16 } else { t.rider_w };
            t.speed_mm_s = match t.mode {
                0x11 => (10_000 - i32::from(t.grade_x100) * 8).clamp(1000, 20_000) as u32,
                4 if t.resistance > 0 => 8000 - u32::from(t.resistance) * 20, // 0: free, the rider's own speed
                _ => u32::from(t.power) * 42,
            };
            if !t.on(FC_NOTIFY, 1) {
                continue;
            }
            let [s0, s1] = (t.speed_mm_s as u16).to_le_bytes();
            let [p0, p1] = t.power.to_le_bytes();
            for page in [[16, 25, 0, 0, s0, s1, 0xff, 0x30], [25, 1, 90, 0, 0, p0, p1 & 0x0f, 0x30]] {
                self.hvx_on(1, FC_NOTIFY, 1, &crate::fecble::broadcast(page, t.sum));
            }
        }
    }

    /// The trainer's moves at `now`: its response indicated, its data notified, its scripted faults.
    pub(super) fn ftms_tick(&mut self, now: u64) {
        let Some(mut t) = self.ftms.take() else { return };
        if t.fec {
            self.fec_tick(&mut t, now);
            self.ftms = Some(t);
            return;
        }
        if t.connected {
            if t.drop_at.is_some_and(|at| now >= at) {
                t.drop_at = None;
                t.hang_up();
                self.evt(0x11, &[1, 0, 0x08]); // supervision time-out
            } else {
                if t.mute_cp {
                    t.pending = None;
                }
                if !t.unconfirmed
                    && t.on(FT_CP, 2)
                    && let Some(r) = t.pending.take()
                {
                    t.unconfirmed = true;
                    self.hvx_on(1, FT_CP, 2, &r);
                }
                if t.lose_control_at.is_some_and(|at| now >= at) {
                    t.lose_control_at = None;
                    t.control = false;
                    if t.on(FT_STATUS, 1) {
                        self.hvx_on(1, FT_STATUS, 1, &[0xff]);
                    }
                }
                while now >= t.next_ms {
                    t.next_ms += 250;
                    t.power = if t.mode == 5 && t.control { t.target_w.max(0) as u16 } else { t.rider_w };
                    t.speed_mm_s = match t.mode {
                        0x11 => (10_000 - i32::from(t.grade_x100) * 8).clamp(1000, 20_000) as u32,
                        4 if t.resistance > 0 => 8000 - u32::from(t.resistance) * 20,
                        _ => u32::from(t.power) * 42,
                    };
                    if !t.on(FT_DATA, 1) {
                        continue;
                    }
                    let speed = ((t.speed_mm_s * 36 / 100) as u16).to_le_bytes();
                    let power = t.power.to_le_bytes();
                    if t.split {
                        // More Data, cadence; then the speed and the power
                        self.hvx_on(1, FT_DATA, 1, &[0x05, 0x00, 180, 0]);
                        self.hvx_on(1, FT_DATA, 1, &[0x40, 0x00, speed[0], speed[1], power[0], power[1]]);
                    } else {
                        self.hvx_on(1, FT_DATA, 1, &[0x44, 0x00, speed[0], speed[1], 180, 0, power[0], power[1]]);
                    }
                }
            }
        }
        self.ftms = Some(t);
    }
}
