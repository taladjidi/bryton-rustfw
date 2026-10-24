//! The simulated Nordic chip's BLE peripheral side, and a scripted phone at the other end of the link: the chip
//! keeps the GATT table the app builds and answers its calls as the SoftDevice API v5 serialization does
//! (drivers/ble.rs has the sources); the phone connects to an advertising unit, pairs (Just Works) or encrypts with
//! the key it kept, enables the HID report, serves ANCS, AMS and CTS from its own table, and posts notifications
//! and calls at scripted times.

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
}

impl PhoneSim {
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
    fn evt(&mut self, id: u16, body: &[u8]) {
        let mut p = vec![2];
        p.extend(id.to_le_bytes());
        p.extend(body);
        self.queue(&p);
    }

    fn gattc(&mut self, id: u16, status: u16, body: &[u8]) {
        let mut p = vec![0, 0];
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
                    self.hvx(0x25, &[&[2, 2, 0][..], title.as_bytes()].concat());
                    self.hvx(0x25, &[&[2, 0, 0][..], artist.as_bytes()].concat());
                    self.hvx(0x25, &[0, 1, 0, b'1', b',', b'1', b'.', b'0', b',', b'0', b'.', b'0']);
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

    /// The phone's attributes in `start..=end` of `kind`.
    fn phone_attrs(&self, start: u16, end: u16, f: impl Fn(&Attr) -> bool) -> Vec<Attr> {
        let db = self.phone.as_ref().map_or(&[][..], |p| &p.db[..]);
        db.iter().filter(|a| (start..=end).contains(&a.handle) && f(a)).cloned().collect()
    }

    /// Answers a BLE command of the peripheral side; false if it is not one.
    pub(super) fn take_ble(&mut self, p: &[u8], now: u64) -> bool {
        let [0, op, args @ ..] = p else { return false };
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
            0x76 => {
                ok(self, &[]);
                if let Some(ph) = &mut self.phone {
                    (ph.connected, ph.secure, ph.connect_at) = (false, false, u64::MAX);
                }
                self.evt(0x11, &[0, 0, 0x16]);
            }
            0x94 => {
                let want = (u16_at(5), args[7]);
                let svcs = self.phone_attrs(1, 0xffff, |a| a.kind == Kind::Service);
                let last = self.phone_attrs(1, 0xffff, |_| true).last().map_or(0, |a| a.handle);
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
                        self.gattc(0x30, 0, &b);
                    }
                    None => {
                        ok(self, &[]);
                        self.gattc(0x30, 0x010a, &[0, 0]);
                    }
                }
            }
            0x96 => {
                let (start, end) = (u16_at(3), u16_at(5));
                let decls: Vec<_> =
                    self.phone_attrs(start, end, |a| matches!(a.kind, Kind::Decl(_))).into_iter().take(3).collect();
                ok(self, &[]);
                if decls.is_empty() {
                    self.gattc(0x32, 0x010a, &[0, 0]);
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
                    self.gattc(0x32, 0, &b);
                }
            }
            0x97 => {
                let (start, end) = (u16_at(3), u16_at(5));
                let descs = self.phone_attrs(start, end, |a| a.kind == Kind::Cccd);
                ok(self, &[]);
                if descs.is_empty() {
                    self.gattc(0x33, 0x010a, &[0, 0]);
                } else {
                    let mut b = (descs.len() as u16).to_le_bytes().to_vec();
                    for d in descs {
                        b.extend(d.handle.to_le_bytes());
                        b.extend([0x02, 0x29, 1]);
                    }
                    self.gattc(0x33, 0, &b);
                }
            }
            0x9a => {
                let h = u16_at(2);
                let v = self.phone_attrs(h, h, |_| true).first().map(|a| a.value.clone()).unwrap_or_default();
                ok(self, &[]);
                let mut b = h.to_le_bytes().to_vec();
                b.extend([0, 0]);
                b.extend((v.len() as u16).to_le_bytes());
                b.extend(v);
                self.gattc(0x36, 0, &b);
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
                            // the answer, in 20-byte notifications (ATT MTU 23)
                            let mut a = vec![0];
                            a.extend(uid.to_le_bytes());
                            a.extend([0, 9, 0]);
                            a.extend(b"com.phone");
                            a.push(1);
                            a.extend((title.len() as u16).to_le_bytes());
                            a.extend(title.as_bytes());
                            a.push(3);
                            a.extend((msg.len() as u16).to_le_bytes());
                            a.extend(msg.as_bytes());
                            for c in a.chunks(20) {
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
            0x9d | 0x75 | 0x8f | 0x90 | 0x66 | 0xad | 0x78 | 0x7a | 0x7c | 0xa4 => ok(self, &[]),
            _ => return false,
        }
        true
    }
}
