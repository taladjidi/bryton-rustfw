//! The phone's side of the link, pure logic: the formats of HID over GATT (media keys), Apple's notification and
//! media services (ANCS, AMS), the Current Time Service, and the bond kept on the card. The radio and the GATT
//! procedures are in app/phone.rs and drivers/ble.rs.
//!
//! Sources (AGENTS.md, look it up):
//! - ANCS: Apple, "Apple Notification Center Service (ANCS) Specification" (developer.apple.com/library/archive,
//!   CoreBluetooth/Reference/AppleNotificationCenterServiceSpecification): the UUIDs, the Notification Source
//!   fields (EventID, EventFlags, CategoryID, CategoryCount, NotificationUID; their sizes 1, 1, 1, 1, 4 bytes are
//!   in the spec's figure 2-1, as Nordic's ble_ancs_c reads them), Get Notification Attributes (command 0, the
//!   UID, attribute ids, a 2-byte max length after Title (1), Subtitle (2), Message (3)), its answer on the Data
//!   Source (command, UID, then id / u16 length / value tuples, fragmented over notifications), Perform Notification
//!   Action (command 2, UID, ActionID 0 positive, 1 negative), the tables of the appendix.
//! - AMS: Apple, "Apple Media Service Reference" (same archive): the UUIDs, Remote Command ids (play 0, pause 1,
//!   toggle 2, next 3, previous 4, volume up 5, down 6), Entity Update (write entity then attribute ids; notified
//!   as entity, attribute, flags, value), Player PlaybackInfo "state,rate,elapsed", Track artist 0, title 2.
//! - CTS: Bluetooth SIG, Current Time Service 1.1 and the GATT Specification Supplement: Current Time 0x2a2b
//!   (Exact Time 256: year u16, month, day, hours, minutes, seconds, day of week, fractions, adjust reason), Local
//!   Time Information 0x2a0f (time zone sint8 in 15 minutes, DST offset 0 / 2 / 4 / 8 in 15 minutes, 255 unknown).
//! - HID: Bluetooth SIG HID over GATT Profile 1.0 and HID Service 1.0 (HID Information, Report Map, Report with its
//!   Report Reference descriptor 0x2908, Control Point, Protocol Mode); USB HID Usage Tables 1.4, Consumer page
//!   0x0c: Play/Pause 0xcd, Scan Next Track 0xb5, Scan Previous Track 0xb6, Volume Increment 0xe9, Volume Decrement
//!   0xea, Mute 0xe2; the PnP ID values are those of Nordic's ble_app_hids_keyboard example (vendor id source 2,
//!   vendor 0x1915, product 0xeeee, version 1).

use crate::drivers::ble::EncKey;
use crate::text::Line;

/// A 128-bit UUID written as usual (most significant byte first), turned into the stack's order (least first).
const fn le(be: [u8; 16]) -> [u8; 16] {
    let mut out = [0; 16];
    let mut i = 0;
    while i < 16 {
        out[i] = be[15 - i];
        i += 1;
    }
    out
}

/// The vendor UUIDs registered with the stack, in this order ([`VS_ANCS`]...), little endian.
pub const VS: [[u8; 16]; 8] = [
    // ANCS 7905F431-B5CE-4E99-A40F-4B1E122D00D0, Notification Source, Control Point, Data Source
    le([0x79, 0x05, 0xf4, 0x31, 0xb5, 0xce, 0x4e, 0x99, 0xa4, 0x0f, 0x4b, 0x1e, 0x12, 0x2d, 0x00, 0xd0]),
    le([0x9f, 0xbf, 0x12, 0x0d, 0x63, 0x01, 0x42, 0xd9, 0x8c, 0x58, 0x25, 0xe6, 0x99, 0xa2, 0x1d, 0xbd]),
    le([0x69, 0xd1, 0xd8, 0xf3, 0x45, 0xe1, 0x49, 0xa8, 0x98, 0x21, 0x9b, 0xbd, 0xfd, 0xaa, 0xd9, 0xd9]),
    le([0x22, 0xea, 0xc6, 0xe9, 0x24, 0xd6, 0x4b, 0xb5, 0xbe, 0x44, 0xb3, 0x6a, 0xce, 0x7c, 0x7b, 0xfb]),
    // AMS 89D3502B-0F36-433A-8EF4-C502AD55F8DC, Remote Command, Entity Update, Entity Attribute
    le([0x89, 0xd3, 0x50, 0x2b, 0x0f, 0x36, 0x43, 0x3a, 0x8e, 0xf4, 0xc5, 0x02, 0xad, 0x55, 0xf8, 0xdc]),
    le([0x9b, 0x3c, 0x81, 0xd8, 0x57, 0xb1, 0x4a, 0x8a, 0xb8, 0xdf, 0x0e, 0x56, 0xf7, 0xca, 0x51, 0xc2]),
    le([0x2f, 0x7c, 0xab, 0xce, 0x80, 0x8d, 0x41, 0x1f, 0x9a, 0x0c, 0xbb, 0x92, 0xba, 0x96, 0xc1, 0x02]),
    le([0xc6, 0xb2, 0xf3, 0x8c, 0x23, 0xab, 0x46, 0xd8, 0xa6, 0xab, 0xa3, 0xa8, 0x70, 0xbb, 0xd5, 0xd7]),
];
/// Index in [`VS`]: the ANCS service.
pub const VS_ANCS: usize = 0;
/// Index in [`VS`]: the AMS service.
pub const VS_AMS: usize = 4;

/// The 16-bit part the stack names a vendor UUID by (bytes 12 and 13).
///
/// ```
/// assert_eq!(bryton_rustfw::phone::uuid16(&bryton_rustfw::phone::VS[0]), 0xf431);
/// ```
pub const fn uuid16(le: &[u8; 16]) -> u16 {
    u16::from_le_bytes([le[12], le[13]])
}

// ---------------------------------------------------------------------------------------------------------------
// HID over GATT: a Consumer Control report of one byte, one bit per key
// ---------------------------------------------------------------------------------------------------------------

/// The report map: Consumer Control (usage page 0x0c, usage 1), report id 1, six one-bit buttons and two bits of
/// padding.
pub const REPORT_MAP: [u8; 35] = [
    0x05, 0x0c, // usage page (consumer)
    0x09, 0x01, // usage (consumer control)
    0xa1, 0x01, // collection (application)
    0x85, 0x01, //   report id 1
    0x15, 0x00, //   logical minimum 0
    0x25, 0x01, //   logical maximum 1
    0x75, 0x01, //   report size 1
    0x95, 0x06, //   report count 6
    0x09, 0xcd, //   play / pause
    0x09, 0xb5, //   next track
    0x09, 0xb6, //   previous track
    0x09, 0xe9, //   volume up
    0x09, 0xea, //   volume down
    0x09, 0xe2, //   mute
    0x81, 0x02, //   input (data, variable, absolute)
    0x95, 0x02, //   report count 2
    0x81, 0x01, //   input (constant): padding
    0xc0, // end collection
];

/// A media key, by its bit in the report (the order of [`REPORT_MAP`]'s usages).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Media {
    /// Play or pause.
    PlayPause = 0,
    /// Next track.
    Next = 1,
    /// Previous track.
    Prev = 2,
    /// Volume up (also the camera's shutter on a phone).
    VolUp = 3,
    /// Volume down.
    VolDown = 4,
    /// Mute.
    Mute = 5,
    /// Skip forward in the track (AMS only: the app sets how far, often 15 or 30 s; podcasts).
    SkipFwd = 6,
    /// Skip backward, the same.
    SkipBack = 7,
}

impl Media {
    /// The HID report with this key down (None: no HID usage, AMS only).
    pub fn report(self) -> Option<u8> {
        (self as u8 <= Media::Mute as u8).then(|| 1 << self as u8)
    }

    /// The AMS remote command for it, if AMS has one.
    pub fn ams(self) -> Option<u8> {
        Some(match self {
            Media::PlayPause => 2,
            Media::Next => 3,
            Media::Prev => 4,
            Media::VolUp => 5,
            Media::VolDown => 6,
            Media::SkipFwd => 9, // SkipForward (AMS RemoteCommandID)
            Media::SkipBack => 10,
            Media::Mute => return None,
        })
    }
}

/// The AMS remote commands the playing app accepts, one bit per RemoteCommandID, from a Remote Command
/// notification (its value is the list of the IDs; Apple's AMS specification). A music app often has no skip, a
/// podcast app no next track.
///
/// ```
/// use bryton_rustfw::phone::ams_commands;
/// assert_eq!(ams_commands(&[0, 1, 2, 3, 4]), 0b1_1111);
/// assert_eq!(ams_commands(&[9, 10, 200]), 0b110_0000_0000); // 200: no such command, left out
/// ```
pub fn ams_commands(list: &[u8]) -> u16 {
    list.iter().filter(|&&c| c < 16).fold(0, |m, &c| m | 1 << c)
}

/// HID Information: HID 1.11, no country, normally connectable (bit 1), no remote wake.
pub const HID_INFO: [u8; 4] = [0x11, 0x01, 0, 0x02];
/// Report Reference of the input report: id 1, input (1).
pub const REPORT_REF: [u8; 2] = [1, 1];
/// PnP ID (module doc).
pub const PNP_ID: [u8; 7] = [2, 0x15, 0x19, 0xee, 0xee, 1, 0];

/// The advertising data: flags (LE general discoverable, no BR/EDR), the HID and battery services, the appearance
/// (generic HID, 0x03c0), and a solicitation of ANCS, so an iPhone offers its notifications (31 bytes, the most).
pub const ADV: [u8; 31] = {
    let mut a = [0; 31];
    let head = [2, 0x01, 0x06, 5, 0x03, 0x12, 0x18, 0x0f, 0x18, 3, 0x19, 0xc0, 0x03, 17, 0x15];
    let mut i = 0;
    while i < head.len() {
        a[i] = head[i];
        i += 1;
    }
    while i < 31 {
        a[i] = VS[VS_ANCS][i - head.len()];
        i += 1;
    }
    a
};
/// The scan response: the complete name.
pub const SCAN_RSP: [u8; 6] = [5, 0x09, b'S', b'5', b'1', b'0'];

// ---------------------------------------------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------------------------------------------

/// Appends UTF-8 `s` as the font can draw it (printable ASCII): Latin-1 letters lose their accent (é -> e), other
/// characters become '?', control characters a space.
///
/// ```
/// let mut l = bryton_rustfw::text::Line::<16>::new();
/// bryton_rustfw::phone::fold(&mut l, "Café ☕\nOK".as_bytes());
/// assert_eq!(l.as_str(), "Cafe ? OK");
/// ```
pub fn fold<const N: usize>(l: &mut Line<N>, s: &[u8]) {
    // U+00C0..U+00FF, as their base letters
    const LATIN1: &[u8; 64] = b"AAAAAAACEEEEIIIIDNOOOOOxOUUUUYTsaaaaaaaceeeeiiiidnooooo/ouuuuyty";
    let mut i = 0;
    while let Some(&c) = s.get(i) {
        let (n, ch) = match c {
            0x20..=0x7e => (1, c),
            0..=0x1f | 0x7f => (1, b' '),
            0xc3 => (2, s.get(i + 1).map_or(b'?', |&d| LATIN1.get(usize::from(d & 0x3f)).copied().unwrap_or(b'?'))),
            0xc0..=0xdf => (2, b'?'),
            0xe0..=0xef => (3, b'?'),
            0xf0..=0xf7 => (4, b'?'),
            _ => (1, b'?'), // a stray continuation byte
        };
        let c = [ch];
        l.push_str(core::str::from_utf8(&c).unwrap_or("?"));
        i += n;
    }
}

// ---------------------------------------------------------------------------------------------------------------
// ANCS
// ---------------------------------------------------------------------------------------------------------------

/// A Notification Source notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Notif {
    /// 0 added, 1 modified, 2 removed.
    pub event: u8,
    /// Bit 0 silent, 1 important, 2 pre-existing, 3 positive action, 4 negative action.
    pub flags: u8,
    /// 1 incoming call, 2 missed call, 4 social, 6 email...
    pub category: u8,
    /// The notification's id.
    pub uid: u32,
}

/// Category: an incoming call.
pub const INCOMING_CALL: u8 = 1;

impl Notif {
    /// From the 8 bytes notified.
    ///
    /// ```
    /// let n = bryton_rustfw::phone::Notif::parse(&[0, 0x18, 1, 1, 7, 0, 0, 0]).unwrap();
    /// assert_eq!((n.event, n.category, n.uid), (0, 1, 7));
    /// ```
    pub fn parse(p: &[u8]) -> Option<Notif> {
        match *p {
            [event, flags, category, _, u0, u1, u2, u3, ..] => {
                Some(Notif { event, flags, category, uid: u32::from_le_bytes([u0, u1, u2, u3]) })
            }
            _ => None,
        }
    }

    /// New, and not one the phone had before the link (those come as a flood after subscribing).
    pub fn fresh(&self) -> bool {
        self.event == 0 && self.flags & 4 == 0
    }
}

/// Longest title and message asked for.
pub const TITLE_MAX: u16 = 24;
/// See [`TITLE_MAX`].
pub const MESSAGE_MAX: u16 = 48;

/// Get Notification Attributes for `uid`: app id, title and message.
pub fn get_attributes(uid: u32) -> [u8; 12] {
    let [u0, u1, u2, u3] = uid.to_le_bytes();
    let [t0, t1] = TITLE_MAX.to_le_bytes();
    let [m0, m1] = MESSAGE_MAX.to_le_bytes();
    [0, u0, u1, u2, u3, 0, 1, t0, t1, 3, m0, m1]
}

/// Perform Notification Action on `uid`: positive (accept a call) or negative (decline).
pub fn action(uid: u32, positive: bool) -> [u8; 6] {
    let [u0, u1, u2, u3] = uid.to_le_bytes();
    [2, u0, u1, u2, u3, u8::from(!positive)]
}

/// A notification's attributes as the Data Source gives them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attributes {
    /// The notification's id.
    pub uid: u32,
    /// Who or what: the sender, the caller.
    pub title: Line<24>,
    /// The text.
    pub message: Line<48>,
}

/// The Data Source's fragments put back together into one answer.
pub struct DataSource {
    buf: [u8; 160],
    len: usize,
}

impl Default for DataSource {
    fn default() -> Self {
        DataSource { buf: [0; 160], len: 0 }
    }
}

impl DataSource {
    /// Takes one notification; returns the attributes once the three asked for ([`get_attributes`]) are complete.
    /// A fragment that does not fit, or an answer that is not one, starts it over.
    pub fn push(&mut self, frag: &[u8]) -> Option<Attributes> {
        match self.buf.get_mut(self.len..self.len + frag.len()) {
            Some(dst) => {
                dst.copy_from_slice(frag);
                self.len += frag.len();
            }
            None => {
                self.len = 0;
                return None;
            }
        }
        let p = &self.buf[..self.len];
        let [0, u0, u1, u2, u3, rest @ ..] = p else {
            self.len = if p.first().is_some_and(|&c| c != 0) { 0 } else { self.len };
            return None;
        };
        let mut a = Attributes { uid: u32::from_le_bytes([*u0, *u1, *u2, *u3]), ..Default::default() };
        let mut at = rest;
        for _ in 0..3 {
            let [id, l0, l1, more @ ..] = at else { return None };
            let n = usize::from(u16::from_le_bytes([*l0, *l1]));
            let value = more.get(..n)?;
            match id {
                1 => fold(&mut a.title, value),
                3 => fold(&mut a.message, value),
                _ => {}
            }
            at = &more[n..];
        }
        self.len = 0;
        Some(a)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// AMS
// ---------------------------------------------------------------------------------------------------------------

/// What to write to Entity Update after subscribing: the player's playback info, the track's artist, album, title
/// and duration (AMS, Apple's specification: entity 2 Track, attributes 0..3; Nordic's AMS client sample writes the
/// same).
pub const AMS_REGISTER: [&[u8]; 2] = [&[0, 1], &[2, 0, 1, 2, 3]];

/// What is playing, from AMS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NowPlaying {
    /// The track (two lines on the media screen).
    pub title: Line<40>,
    /// Its artist.
    pub artist: Line<24>,
    /// Its album.
    pub album: Line<24>,
    /// Its length, s (0: not told).
    pub duration_s: u32,
    /// Playing (else paused or stopped).
    pub playing: bool,
    /// Elapsed seconds when last told, and the ms since.
    pub elapsed_s: u32,
    /// Milliseconds since the elapsed time was told.
    pub since_ms: u32,
    /// The title changed (to say in the banner).
    pub new_title: bool,
}

/// "193.25" -> 193: the integer part of AMS's seconds (at most 7 digits).
fn seconds(v: &[u8]) -> u32 {
    let whole = v.split(|&c| c == b'.').next().unwrap_or(b"");
    whole.iter().take(7).take_while(|c| c.is_ascii_digit()).fold(0, |v, &c| v * 10 + u32::from(c - b'0'))
}

impl NowPlaying {
    /// Takes an Entity Update notification.
    ///
    /// ```
    /// let mut n = bryton_rustfw::phone::NowPlaying::default();
    /// n.update(&[0, 1, 0, b'1', b',', b'1', b'.', b'0', b',', b'7', b'5', b'.', b'2']);
    /// n.update(&[2, 2, 0, b'S', b'o', b'n', b'g']);
    /// n.tick(2000);
    /// assert_eq!((n.title.as_str(), n.playing, n.elapsed()), ("Song", true, 77));
    /// ```
    pub fn update(&mut self, p: &[u8]) {
        let [entity, attr, _flags, value @ ..] = p else { return };
        match (entity, attr) {
            (2, 2) => {
                let before = self.title;
                self.title.clear();
                fold(&mut self.title, value);
                self.new_title |= self.title != before && !self.title.as_str().is_empty();
            }
            (2, 0) => {
                self.artist.clear();
                fold(&mut self.artist, value);
            }
            (2, 1) => {
                self.album.clear();
                fold(&mut self.album, value);
            }
            (2, 3) => self.duration_s = seconds(value),
            (0, 1) => {
                // "state,rate,elapsed": the integer part of the elapsed seconds is enough here
                let mut parts = value.split(|&c| c == b',');
                let state = parts.next().unwrap_or(b"0");
                self.playing = state == b"1";
                self.elapsed_s = seconds(parts.nth(1).unwrap_or(b"0"));
                self.since_ms = 0;
            }
            _ => {}
        }
    }

    /// Time goes on.
    pub fn tick(&mut self, ms: u32) {
        self.since_ms = self.since_ms.saturating_add(ms);
    }

    /// Elapsed seconds now.
    pub fn elapsed(&self) -> u32 {
        self.elapsed_s + if self.playing { self.since_ms / 1000 } else { 0 }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// CTS
// ---------------------------------------------------------------------------------------------------------------

/// The phone's local time from a Current Time value: (year, month, day, hours, minutes, seconds); None if a field
/// is out of range (0 is "unknown" in the spec).
///
/// ```
/// let t = [0xea, 0x07, 10, 24, 15, 30, 5, 6, 0, 0];
/// assert_eq!(bryton_rustfw::phone::current_time(&t), Some((2026, 10, 24, 15, 30, 5)));
/// assert_eq!(bryton_rustfw::phone::current_time(&t[..6]), None);
/// let feb31 = [0xea, 0x07, 2, 31, 15, 30, 5, 6, 0, 0];
/// assert_eq!(bryton_rustfw::phone::current_time(&feb31), None);
/// ```
pub fn current_time(p: &[u8]) -> Option<(u16, u8, u8, u8, u8, u8)> {
    match *p {
        [y0, y1, mo, d, h, mi, s, ..] if p.len() >= 10 => {
            let y = u16::from_le_bytes([y0, y1]);
            // a real date only (review finding 10: 31 February would go into the RTC)
            let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
            let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
            let ok = (2020..2100).contains(&y)
                && (1..=12).contains(&mo)
                && (1..=days[usize::from(mo.clamp(1, 12)) - 1]).contains(&d);
            (ok && h < 24 && mi < 60 && s < 60).then_some((y, mo, d, h, mi, s))
        }
        _ => None,
    }
}

/// Local time minus UTC in 15-minute steps from a Local Time Information value (time zone plus DST).
///
/// ```
/// assert_eq!(bryton_rustfw::phone::local_offset(&[4, 4]), Some(8)); // CET + summer time: UTC + 2 h
/// assert_eq!(bryton_rustfw::phone::local_offset(&[0x80, 0]), None); // time zone unknown
/// ```
pub fn local_offset(p: &[u8]) -> Option<i8> {
    match *p {
        [tz, dst, ..] if tz != 0x80 && (-48..=56).contains(&(tz as i8)) => {
            Some((tz as i8).saturating_add(if dst == 255 { 0 } else { dst.min(8) as i8 }))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The bond, in `\Rustfw\ble.bin`
// ---------------------------------------------------------------------------------------------------------------

/// Longest system attributes kept (a handle, a value per CCCD and a CRC: about 4 bytes each, a dozen fit).
pub const ATTRS: usize = 64;

/// The phone bonded with: the key to answer it with, its address then, the CCCDs it set; and the phone settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bond {
    /// The LTK the phone encrypts with.
    pub key: EncKey,
    /// Its address when it bonded (type byte, 6 bytes).
    pub peer: [u8; 7],
    /// The system attributes as the stack gave them ([`crate::ble::sys_attr_get`]).
    pub attrs: [u8; ATTRS],
    /// Their length.
    pub attrs_len: u8,
}

/// The file's tag and version.
const MAGIC: &[u8; 4] = b"RBL1";

/// The phone settings and the bond, as one sector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stored {
    /// Phone notifications in the banner.
    pub notify: bool,
    /// Media keys (HID) offered.
    pub keys: bool,
    /// The bond, if any.
    pub bond: Option<Bond>,
}

impl Default for Stored {
    fn default() -> Self {
        Stored { notify: true, keys: true, bond: None }
    }
}

impl Stored {
    /// The sector: tag, flags, the key (27 bytes), the address, the attributes' length and bytes, a CRC-16 of all
    /// before it.
    ///
    /// ```
    /// use bryton_rustfw::phone::*;
    /// let mut s = Stored::default();
    /// s.keys = false;
    /// let mut sector = s.to_sector();
    /// assert_eq!(Stored::from_sector(&sector), Some(s));
    /// sector[9] ^= 1;
    /// assert_eq!(Stored::from_sector(&sector), None, "a damaged sector: nothing");
    /// ```
    pub fn to_sector(&self) -> [u8; 512] {
        let mut s = [0; 512];
        s[..4].copy_from_slice(MAGIC);
        s[4] = u8::from(self.notify) | u8::from(self.keys) << 1 | u8::from(self.bond.is_some()) << 2;
        if let Some(b) = &self.bond {
            s[5..21].copy_from_slice(&b.key.ltk);
            s[21] = b.key.flags;
            s[22..24].copy_from_slice(&b.key.ediv.to_le_bytes());
            s[24..32].copy_from_slice(&b.key.rand);
            s[32..39].copy_from_slice(&b.peer);
            s[39] = b.attrs_len;
            s[40..40 + ATTRS].copy_from_slice(&b.attrs);
        }
        let crc = crate::fit::crc(0, &s[..40 + ATTRS]);
        s[40 + ATTRS..42 + ATTRS].copy_from_slice(&crc.to_le_bytes());
        s
    }

    /// From a sector [`Stored::to_sector`] wrote; None if it is not one (never written, damaged).
    pub fn from_sector(s: &[u8; 512]) -> Option<Stored> {
        let crc = crate::fit::crc(0, &s[..40 + ATTRS]);
        if &s[..4] != MAGIC || s[40 + ATTRS..42 + ATTRS] != crc.to_le_bytes() {
            return None;
        }
        let flags = s[4];
        let bond = (flags & 4 != 0).then(|| {
            let mut b = Bond { key: EncKey::default(), peer: [0; 7], attrs: [0; ATTRS], attrs_len: s[39] };
            b.key.ltk.copy_from_slice(&s[5..21]);
            b.key.flags = s[21];
            b.key.ediv = u16::from_le_bytes([s[22], s[23]]);
            b.key.rand.copy_from_slice(&s[24..32]);
            b.peer.copy_from_slice(&s[32..39]);
            b.attrs.copy_from_slice(&s[40..40 + ATTRS]);
            b.attrs_len = b.attrs_len.min(ATTRS as u8);
            b
        });
        Some(Stored { notify: flags & 1 != 0, keys: flags & 2 != 0, bond })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_source_reassembles_fragments() {
        let mut d = DataSource::default();
        let mut answer = [0u8, 9, 0, 0, 0].to_vec();
        answer.extend([0, 3, 0, b'a', b'p', b'p']);
        answer.extend([1, 3, 0, b'B', b'o', b'b']);
        answer.extend([3, 5, 0, b'H', 0xc3, 0xa9, b'l', b'o']);
        let (first, rest) = answer.split_at(9);
        assert_eq!(d.push(first), None);
        let a = d.push(rest).unwrap();
        assert_eq!((a.uid, a.title.as_str(), a.message.as_str()), (9, "Bob", "Helo"), "the accent folded");
        assert_eq!(d.push(&[0xa1]), None, "not an answer: dropped");
        assert!(d.push(&answer).is_some(), "and the next answer reads whole");
    }

    #[test]
    fn adv_data_is_31_bytes_of_ad_structures() {
        let mut at = 0;
        while at < ADV.len() {
            at += 1 + usize::from(ADV[at]);
        }
        assert_eq!(at, 31);
        assert_eq!(&ADV[15..], &VS[VS_ANCS][..]);
    }
}
