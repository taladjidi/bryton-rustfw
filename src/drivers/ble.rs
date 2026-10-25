//! BLE through the Nordic chip, as a peripheral and GATT server and client (the phone link, app/phone.rs): the
//! serialized SoftDevice calls and the events, encoded and decoded here, sent with [`nrf::call`].
//!
//! **Which SoftDevice API** (BOARD.md 11): API version 5 (S132 / S332 v5.x, nRF5 SDK 14). Established from the stock
//! image (Ghidra, ghidra/notes/agent_b2.md): its GATTS request encoders use opcodes 0xa0 (service add), 0xa2
//! (characteristic add), 0xa6 (hvx), 0xa7, 0xa8, 0xa9 / 0xaa (system attributes), 0xab and 0xad (MTU reply), and
//! its GATTC ones 0x94 / 0x96 / 0x97 / 0x9c: GATTS starts at 0xa0 and GATTC at 0x94 in API v5 only (API v6 moved
//! them to 0xa8 and 0x9b); and on the unit, the scan parameters and the advertising report have the v5 layouts
//! (no PHYs, no report buffer). The chip's image is not in the stock image (`\Nordic.bin`), so its exact version is
//! not known; the image byte it reports is Bryton's (4).
//!
//! **Sources** (look it up, AGENTS.md): Nordic's pc-ble-driver (github.com/NordicSemiconductor/pc-ble-driver,
//! master of 2024-04): `include/sd_api_v5/ble_ranges.h`, `ble.h`, `ble_gap.h`, `ble_gatts.h`, `ble_gattc.h` for
//! the opcodes, event ids and constants, and the nRF5 SDK serialization codecs it carries
//! (`src/sd_api_v5/sdk/components/serialization/application/codecs/ble/serializers/ble_*_app.c` and
//! `src/sd_api_common/sdk/components/serialization/common/struct_ser/ble/*_struct_serialization.c`) for the byte
//! layouts. What was taken: a pointer argument is a presence byte (1 / 0) then the structure; `len8data` /
//! `len16data` are the length, a presence byte, the bytes; bit fields are packed into one byte as those encoders
//! do (security mode `sm | lv << 4`, characteristic properties as the ATT bits, attribute metadata `vlen | vloc << 1
//! | rd_auth << 3 | wr_auth << 4`); the keyset after BLE_GAP_EVT_AUTH_STATUS is the serialization's own extension.
//!
//! **Configuration** (`nrf::start`): two links, the phone's (we are its peripheral) and a trainer's (we are its
//! central, app/ftms.rs), on connection configuration tag [`TAG`]; when the chip refuses that, the defaults (no
//! `sd_ble_cfg_set`, tag 0: one peripheral link). Either way ATT MTU 23, an attribute table of 1408 B, 10 vendor
//! UUIDs, one queued notification (ble_gap.h, ble_gatts.h, ble.h `*_DEFAULT`).
//!
//! **As a central** (the trainer): `sd_ble_gap_connect` (`ble_gap_connect_req_enc`: the peer's address, the scan
//! parameters and the connection parameters, each behind a presence byte, then the configuration tag; the address
//! byte `addr_id_peer | addr_type << 1` as `ble_gap_addr_t_enc` packs it, which is the advertising report's byte),
//! `sd_ble_gap_connect_cancel` and `sd_ble_gap_scan_stop` (no arguments), and the reply to a peripheral's request for
//! other connection parameters, `sd_ble_gap_conn_param_update` with the parameters it asked for (as stock does,
//! BOARD.md 11). The GATT client's events carry the connection they belong to, so the phone's and the trainer's
//! are told apart.

use crate::Board;
use crate::nrf::{self, CMD, Error, Rx};

/// `sd_ble_uuid_vs_add`.
const UUID_VS_ADD: u8 = 0x62;
/// `sd_ble_user_mem_reply`.
const USER_MEM_REPLY: u8 = 0x66;
const GAP_ADV_DATA_SET: u8 = 0x72;
const GAP_ADV_START: u8 = 0x73;
const GAP_ADV_STOP: u8 = 0x74;
const GAP_CONN_PARAM_UPDATE: u8 = 0x75;
const GAP_DISCONNECT: u8 = 0x76;
const GAP_APPEARANCE_SET: u8 = 0x78;
const GAP_PPCP_SET: u8 = 0x7a;
const GAP_DEVICE_NAME_SET: u8 = 0x7c;
const GAP_AUTHENTICATE: u8 = 0x7e;
const GAP_SEC_PARAMS_REPLY: u8 = 0x7f;
const GAP_SEC_INFO_REPLY: u8 = 0x86;
const GAP_PHY_UPDATE: u8 = 0x8f;
const GAP_DATA_LENGTH_UPDATE: u8 = 0x90;
const GAP_SCAN_STOP: u8 = 0x8b;
const GAP_CONNECT: u8 = 0x8c;
const GAP_CONNECT_CANCEL: u8 = 0x8d;
const GATTC_PRIMARY_DISCOVER: u8 = 0x94;
const GATTC_CHARS_DISCOVER: u8 = 0x96;
const GATTC_DESCS_DISCOVER: u8 = 0x97;
const GATTC_READ: u8 = 0x9a;
const GATTC_WRITE: u8 = 0x9c;
const GATTC_HV_CONFIRM: u8 = 0x9d;
const GATTS_SERVICE_ADD: u8 = 0xa0;
const GATTS_CHARACTERISTIC_ADD: u8 = 0xa2;
const GATTS_DESCRIPTOR_ADD: u8 = 0xa3;
const GATTS_VALUE_SET: u8 = 0xa4;
const GATTS_HVX: u8 = 0xa6;
const GATTS_SYS_ATTR_SET: u8 = 0xa9;
const GATTS_SYS_ATTR_GET: u8 = 0xaa;
const GATTS_EXCHANGE_MTU_REPLY: u8 = 0xad;

/// The serialized security mode: open link (mode 1 level 1).
pub const OPEN: u8 = 0x11;
/// Encrypted link, no MITM (mode 1 level 2): what HID over GATT asks for its reports.
pub const ENCRYPTED: u8 = 0x21;
/// No access (mode 0 level 0).
pub const NO_ACCESS: u8 = 0x00;

/// Characteristic property bits (as ATT's, ble_gatt_char_props_t_enc).
pub const READ: u8 = 0x02;
/// Write without response.
pub const WRITE_CMD: u8 = 0x04;
/// Write with response.
pub const WRITE: u8 = 0x08;
/// Notify.
pub const NOTIFY: u8 = 0x10;
/// Indicate.
pub const INDICATE: u8 = 0x20;

/// The connection configuration tag `nrf::start` sets up for two links (advertising and connecting use it then; 0,
/// the default one, without).
pub const TAG: u8 = 1;

/// UUID type of the Bluetooth SIG's 16-bit UUIDs.
pub const UUID_BLE: u8 = 1;
/// No connection.
pub const NO_CONN: u16 = 0xffff;
/// GATT status: attribute not found (the end of a discovery).
pub const ATTRIBUTE_NOT_FOUND: u16 = 0x010a;
/// The CCCD's UUID.
pub const CCCD: u16 = 0x2902;

/// A command being built: the payload, its length, and whether it overflowed (then it is not sent).
pub struct Cmd {
    buf: [u8; 160],
    len: usize,
    over: bool,
}

impl Cmd {
    /// `[CMD][op]`.
    pub fn new(op: u8) -> Self {
        let mut c = Cmd { buf: [0; 160], len: 0, over: false };
        c.u8(CMD).u8(op);
        c
    }

    /// One byte.
    #[inline(never)] // one copy: a command is a few dozen of these (image size)
    pub fn u8(&mut self, v: u8) -> &mut Self {
        match self.buf.get_mut(self.len) {
            Some(b) => {
                *b = v;
                self.len += 1;
            }
            None => self.over = true,
        }
        self
    }

    /// Little endian.
    #[inline(never)]
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    /// Bytes as they are.
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        for &b in v {
            self.u8(b);
        }
        self
    }

    /// `len16data`: the length, a presence byte, the bytes.
    pub fn data16(&mut self, v: &[u8]) -> &mut Self {
        self.u16(v.len() as u16).u8(1).bytes(v)
    }

    /// The payload; None if it did not fit.
    pub fn payload(&self) -> Option<&[u8]> {
        (!self.over).then_some(self.buf.get(..self.len)?)
    }

    /// Sends it and waits for the response; its outputs go to `out`.
    pub fn call(&self, b: &mut impl Board, rx: &mut Rx, out: &mut [u8]) -> Result<usize, Error> {
        nrf::call(b, rx, self.payload().ok_or(Error::Code(12))?, out) // NRF_ERROR_DATA_SIZE
    }

    /// Sends it, the result only.
    pub fn run(&self, b: &mut impl Board, rx: &mut Rx) -> Result<(), Error> {
        self.call(b, rx, &mut []).map(|_| ())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// GAP
// ---------------------------------------------------------------------------------------------------------------

/// The device name, read only for the peer.
pub fn device_name_set(b: &mut impl Board, rx: &mut Rx, name: &[u8]) -> Result<(), Error> {
    Cmd::new(GAP_DEVICE_NAME_SET).u8(1).u8(NO_ACCESS).data16(name).run(b, rx)
}

/// The GAP appearance.
pub fn appearance_set(b: &mut impl Board, rx: &mut Rx, appearance: u16) -> Result<(), Error> {
    Cmd::new(GAP_APPEARANCE_SET).u16(appearance).run(b, rx)
}

/// Connection parameters, as (min interval, max interval in 1.25 ms units, slave latency, supervision time-out
/// in 10 ms units).
pub type ConnParams = (u16, u16, u16, u16);

/// The preferred connection parameters, which the stack asks the central for after connecting.
pub fn ppcp_set(b: &mut impl Board, rx: &mut Rx, (min, max, lat, sup): ConnParams) -> Result<(), Error> {
    Cmd::new(GAP_PPCP_SET).u8(1).u16(min).u16(max).u16(lat).u16(sup).run(b, rx)
}

/// The advertising data and the scan response (each 31 bytes at most).
pub fn adv_data_set(b: &mut impl Board, rx: &mut Rx, adv: &[u8], sr: &[u8]) -> Result<(), Error> {
    let mut c = Cmd::new(GAP_ADV_DATA_SET);
    c.u8(adv.len() as u8).u8(1).bytes(adv).u8(sr.len() as u8).u8(u8::from(!sr.is_empty())).bytes(sr);
    c.run(b, rx)
}

/// Starts connectable undirected advertising every `interval` (0.625 ms units) for `timeout_s` (0: until stopped),
/// on the three channels, with connection configuration `tag` (0 the default, or [`TAG`]).
pub fn adv_start(b: &mut impl Board, rx: &mut Rx, interval: u16, timeout_s: u16, tag: u8) -> Result<(), Error> {
    // present, ADV_IND, no peer address, filter policy any, interval, time-out, channel mask (all on), tag
    Cmd::new(GAP_ADV_START).u8(1).u8(0).u8(0).u8(0).u16(interval).u16(timeout_s).u8(0).u8(tag).run(b, rx)
}

/// Stops a scan (an error when none runs: harmless).
pub fn scan_stop(b: &mut impl Board, rx: &mut Rx) -> Result<(), Error> {
    Cmd::new(GAP_SCAN_STOP).run(b, rx)
}

/// The connection parameters asked for as a central: stock's (BOARD.md 11): 7.5 to 15 ms, no latency, supervision
/// time-out 4 s.
pub const CENTRAL_PARAMS: ConnParams = (6, 12, 0, 400);

/// Connects to `peer` (its address byte and 6 bytes, as an advertising report gives them) as a central, scanning
/// for it as stock does (active, 60 ms in 60 ms, no time-out of the stack's: the app cancels), with
/// [`CENTRAL_PARAMS`] and configuration [`TAG`]. [`Event::Connected`] role 2 answers.
pub fn connect(b: &mut impl Board, rx: &mut Rx, peer: &[u8; 7]) -> Result<(), Error> {
    connect_cmd(peer).run(b, rx)
}

fn connect_cmd(peer: &[u8; 7]) -> Cmd {
    let (min, max, lat, sup) = CENTRAL_PARAMS;
    let mut c = Cmd::new(GAP_CONNECT);
    c.u8(1).bytes(peer).u8(1).u8(1).u16(0x60).u16(0x60).u16(0);
    c.u8(1).u16(min).u16(max).u16(lat).u16(sup).u8(TAG);
    c
}

/// Gives up a connection being made.
pub fn connect_cancel(b: &mut impl Board, rx: &mut Rx) -> Result<(), Error> {
    Cmd::new(GAP_CONNECT_CANCEL).run(b, rx)
}

/// Stops advertising.
pub fn adv_stop(b: &mut impl Board, rx: &mut Rx) -> Result<(), Error> {
    Cmd::new(GAP_ADV_STOP).run(b, rx)
}

/// Ends connection `conn` (remote user terminated, as stock).
pub fn disconnect(b: &mut impl Board, rx: &mut Rx, conn: u16) -> Result<(), Error> {
    Cmd::new(GAP_DISCONNECT).u16(conn).u8(0x13).run(b, rx)
}

/// Our security parameters: bonding, no MITM, no LESC, no keypress, no input or output (Just Works), no OOB; keys
/// 7 to 16 bytes; we give our LTK, the peer its LTK and its identity (IRK and address).
fn sec_params(c: &mut Cmd) {
    c.u8(1 | 3 << 4).u8(7).u8(16).u8(0x01).u8(0x03);
}

/// As a peripheral, asks the central to pair (an SMP Security Request).
pub fn authenticate(b: &mut impl Board, rx: &mut Rx, conn: u16) -> Result<(), Error> {
    let mut c = Cmd::new(GAP_AUTHENTICATE);
    c.u16(conn).u8(1);
    sec_params(&mut c);
    c.run(b, rx)
}

/// Answers BLE_GAP_EVT_SEC_PARAMS_REQUEST: pairing accepted with our security parameters and a keyset for the keys, which
/// come back with BLE_GAP_EVT_AUTH_STATUS ([`Event::AuthStatus`]); or refused (`ok` false: pairing not supported).
pub fn sec_params_reply(b: &mut impl Board, rx: &mut Rx, conn: u16, ok: bool) -> Result<(), Error> {
    let mut c = Cmd::new(GAP_SEC_PARAMS_REPLY);
    c.u16(conn);
    if !ok {
        return c.u8(0x85).u8(0).u8(0).run(b, rx);
    }
    c.u8(0).u8(1);
    sec_params(&mut c);
    // keyset present; own: encryption key (27 bytes: LTK, flags, EDIV, Rand), no identity, signing or LESC key;
    // peer: encryption key and identity key (IRK and address), no signing or LESC key
    c.u8(1).u8(1).bytes(&[0; 27]).u8(0).u8(0).u8(0);
    c.u8(1).bytes(&[0; 27]).u8(1).bytes(&[0; 23]).u8(0).u8(0);
    let mut out = [0; 160];
    c.call(b, rx, &mut out).map(|_| ())
}

/// Answers BLE_GAP_EVT_SEC_INFO_REQUEST with the bond's encryption key (LTK and its flags byte), or none.
pub fn sec_info_reply(b: &mut impl Board, rx: &mut Rx, conn: u16, ltk: Option<([u8; 16], u8)>) -> Result<(), Error> {
    let mut c = Cmd::new(GAP_SEC_INFO_REPLY);
    c.u16(conn);
    match ltk {
        Some((key, flags)) => c.u8(1).bytes(&key).u8(flags),
        None => c.u8(0),
    };
    c.u8(0).u8(0).run(b, rx)
}

/// As a central, accepts the parameters the peripheral asked for (BLE_GAP_EVT_CONN_PARAM_UPDATE_REQUEST): `conn`
/// then the 4 values behind a presence byte, as stock answers (BOARD.md 11).
pub fn conn_param_update(
    b: &mut impl Board,
    rx: &mut Rx,
    conn: u16,
    (min, max, lat, sup): ConnParams,
) -> Result<(), Error> {
    Cmd::new(GAP_CONN_PARAM_UPDATE).u16(conn).u8(1).u16(min).u16(max).u16(lat).u16(sup).run(b, rx)
}

/// Answers a PHY update request: either PHY, the stack chooses.
pub fn phy_update(b: &mut impl Board, rx: &mut Rx, conn: u16) -> Result<(), Error> {
    Cmd::new(GAP_PHY_UPDATE).u16(conn).u8(1).u8(0).u8(0).run(b, rx)
}

/// Answers a data length update request with automatic values.
pub fn data_length_update(b: &mut impl Board, rx: &mut Rx, conn: u16) -> Result<(), Error> {
    Cmd::new(GAP_DATA_LENGTH_UPDATE).u16(conn).u8(0).u8(0).run(b, rx)
}

/// Answers BLE_EVT_USER_MEM_REQUEST with no memory (no long writes queued here).
pub fn user_mem_reply(b: &mut impl Board, rx: &mut Rx, conn: u16) -> Result<(), Error> {
    Cmd::new(USER_MEM_REPLY).u16(conn).u8(0).run(b, rx)
}

// ---------------------------------------------------------------------------------------------------------------
// GATT server
// ---------------------------------------------------------------------------------------------------------------

/// Adds a primary service with a 16-bit UUID; returns its handle.
pub fn service_add(b: &mut impl Board, rx: &mut Rx, uuid: u16) -> Result<u16, Error> {
    let mut out = [0; 3];
    match (Cmd::new(GATTS_SERVICE_ADD).u8(1).u8(1).u16(uuid).u8(UUID_BLE).u8(1).call(b, rx, &mut out)?, out) {
        (3, [1, h0, h1]) => Ok(u16::from_le_bytes([h0, h1])),
        _ => Err(Error::Code(u32::MAX)),
    }
}

/// A characteristic to add: its UUID, properties, value (and how long it may grow), who may read and write it.
pub struct Char<'a> {
    /// 16-bit UUID.
    pub uuid: u16,
    /// Property bits ([`READ`], [`NOTIFY`]...).
    pub props: u8,
    /// Initial value.
    pub value: &'a [u8],
    /// Longest value (variable length when longer than `value`).
    pub max: u16,
    /// Read and write permissions ([`OPEN`], [`ENCRYPTED`], [`NO_ACCESS`]).
    pub read: u8,
    /// See `read`.
    pub write: u8,
}

/// Attribute metadata: permissions, then variable length, value in the stack's memory, no authorization.
fn attr_md(c: &mut Cmd, read: u8, write: u8, vlen: bool) {
    c.u8(read).u8(write).u8(u8::from(vlen) | 1 << 1);
}

/// Adds characteristic `ch` to service `svc`; returns (value handle, CCCD handle: 0 without notify / indicate).
pub fn characteristic_add(b: &mut impl Board, rx: &mut Rx, svc: u16, ch: &Char) -> Result<(u16, u16), Error> {
    let mut c = Cmd::new(GATTS_CHARACTERISTIC_ADD);
    c.u16(svc).u8(1);
    // characteristic metadata: properties, no extended ones, no user description, no presentation format, a
    // CCCD (written by the peer only on an encrypted link when the value is encrypted) when it notifies
    c.u8(ch.props).u8(0).u16(0).u16(0).u8(0).u8(0).u8(0);
    if ch.props & (NOTIFY | INDICATE) != 0 {
        c.u8(1);
        attr_md(&mut c, OPEN, ch.read, false);
    } else {
        c.u8(0);
    }
    c.u8(0);
    // the value attribute
    c.u8(1).u8(1).u16(ch.uuid).u8(UUID_BLE).u8(1);
    attr_md(&mut c, ch.read, ch.write, ch.max as usize > ch.value.len());
    c.u16(0).u16(ch.max).data16(ch.value).u8(1);
    let mut out = [0; 9];
    match (c.call(b, rx, &mut out)?, out) {
        (9, [1, v0, v1, _, _, c0, c1, ..]) => Ok((u16::from_le_bytes([v0, v1]), u16::from_le_bytes([c0, c1]))),
        _ => Err(Error::Code(u32::MAX)),
    }
}

/// Adds a read-only descriptor (`read` permission) after characteristic `ch`'s value; returns its handle.
pub fn descriptor_add(
    b: &mut impl Board,
    rx: &mut Rx,
    ch: u16,
    uuid: u16,
    value: &[u8],
    read: u8,
) -> Result<u16, Error> {
    let mut c = Cmd::new(GATTS_DESCRIPTOR_ADD);
    c.u16(ch).u8(1).u8(1).u16(uuid).u8(UUID_BLE).u8(1);
    attr_md(&mut c, read, NO_ACCESS, false);
    c.u16(0).u16(value.len() as u16).data16(value).u8(1);
    let mut out = [0; 3];
    match (c.call(b, rx, &mut out)?, out) {
        (3, [1, h0, h1]) => Ok(u16::from_le_bytes([h0, h1])),
        _ => Err(Error::Code(u32::MAX)),
    }
}

/// Sets a value in the table (what the next read gets), for any connection.
pub fn value_set(b: &mut impl Board, rx: &mut Rx, handle: u16, value: &[u8]) -> Result<(), Error> {
    let mut out = [0; 8];
    Cmd::new(GATTS_VALUE_SET).u16(NO_CONN).u16(handle).u8(1).u16(0).data16(value).call(b, rx, &mut out).map(|_| ())
}

/// Notifies `value` on `handle` (the value is set too).
pub fn notify(b: &mut impl Board, rx: &mut Rx, conn: u16, handle: u16, value: &[u8]) -> Result<(), Error> {
    let mut c = Cmd::new(GATTS_HVX);
    c.u16(conn).u8(1).u16(handle).u8(1).u16(0).u8(1).u16(value.len() as u16).u8(1).bytes(value);
    let mut out = [0; 3];
    c.call(b, rx, &mut out).map(|_| ())
}

/// The system attributes (CCCD values) for `conn`: the bond's, or none (all notifications off).
pub fn sys_attr_set(b: &mut impl Board, rx: &mut Rx, conn: u16, data: Option<&[u8]>) -> Result<(), Error> {
    let mut c = Cmd::new(GATTS_SYS_ATTR_SET);
    c.u16(conn);
    match data {
        Some(d) => c.data16(d),
        None => c.u16(0).u8(0),
    };
    c.bytes(&[0; 4]).run(b, rx)
}

/// Reads `conn`'s system attributes into `out`; returns their length.
pub fn sys_attr_get(b: &mut impl Board, rx: &mut Rx, conn: u16, out: &mut [u8]) -> Result<usize, Error> {
    let mut c = Cmd::new(GATTS_SYS_ATTR_GET);
    c.u16(conn).u8(1).u16(out.len() as u16).u8(1).bytes(&[0; 4]);
    let mut got = [0; 80];
    let n = c.call(b, rx, &mut got)?;
    match got.get(..n) {
        Some([1, l0, l1, 1, data @ ..]) => {
            let len = usize::from(u16::from_le_bytes([*l0, *l1])).min(data.len()).min(out.len());
            out[..len].copy_from_slice(&data[..len]);
            Ok(len)
        }
        _ => Err(Error::Code(u32::MAX)),
    }
}

/// Answers the peer's MTU exchange with the default MTU (23: no larger one configured).
pub fn exchange_mtu_reply(b: &mut impl Board, rx: &mut Rx, conn: u16) -> Result<(), Error> {
    Cmd::new(GATTS_EXCHANGE_MTU_REPLY).u16(conn).u16(23).run(b, rx)
}

// ---------------------------------------------------------------------------------------------------------------
// GATT client
// ---------------------------------------------------------------------------------------------------------------

/// Registers a vendor 128-bit UUID base (little endian, bytes 12 and 13 its 16-bit part); returns its UUID type.
pub fn uuid_vs_add(b: &mut impl Board, rx: &mut Rx, uuid: &[u8; 16]) -> Result<u8, Error> {
    let mut out = [0; 2];
    match (Cmd::new(UUID_VS_ADD).u8(1).bytes(uuid).u8(1).call(b, rx, &mut out)?, out) {
        (2, [1, t]) => Ok(t),
        _ => Err(Error::Code(u32::MAX)),
    }
}

/// A UUID as the stack names it: 16-bit part and type ([`UUID_BLE`] or a registered vendor base).
pub type Uuid = (u16, u8);

/// Starts the discovery of the peer's primary service `uuid` from handle 1 ([`Event::Services`] answers).
pub fn discover_service(b: &mut impl Board, rx: &mut Rx, conn: u16, (u, t): Uuid) -> Result<(), Error> {
    Cmd::new(GATTC_PRIMARY_DISCOVER).u16(conn).u16(1).u8(1).u16(u).u8(t).run(b, rx)
}

/// Starts the discovery of the characteristics in `start..=end` ([`Event::Chars`]).
pub fn discover_chars(b: &mut impl Board, rx: &mut Rx, conn: u16, start: u16, end: u16) -> Result<(), Error> {
    Cmd::new(GATTC_CHARS_DISCOVER).u16(conn).u8(1).u16(start).u16(end).run(b, rx)
}

/// Starts the discovery of the descriptors in `start..=end` ([`Event::Descs`]).
pub fn discover_descs(b: &mut impl Board, rx: &mut Rx, conn: u16, start: u16, end: u16) -> Result<(), Error> {
    Cmd::new(GATTC_DESCS_DISCOVER).u16(conn).u8(1).u16(start).u16(end).run(b, rx)
}

/// Reads the peer's attribute `handle` ([`Event::Read`]).
pub fn read(b: &mut impl Board, rx: &mut Rx, conn: u16, handle: u16) -> Result<(), Error> {
    Cmd::new(GATTC_READ).u16(conn).u16(handle).u16(0).run(b, rx)
}

/// Writes `value` to the peer's attribute `handle`, with a response ([`Event::Written`]).
pub fn write(b: &mut impl Board, rx: &mut Rx, conn: u16, handle: u16, value: &[u8]) -> Result<(), Error> {
    Cmd::new(GATTC_WRITE).u16(conn).u8(1).u8(1).u8(0).u16(handle).u16(0).data16(value).run(b, rx)
}

/// Confirms an indication on `handle`.
pub fn hv_confirm(b: &mut impl Board, rx: &mut Rx, conn: u16, handle: u16) -> Result<(), Error> {
    Cmd::new(GATTC_HV_CONFIRM).u16(conn).u16(handle).run(b, rx)
}

// ---------------------------------------------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------------------------------------------

/// A bond's encryption key: LTK, its flags byte (bit 0 LESC, bit 1 MITM, bits 2.. the key length), EDIV and Rand.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EncKey {
    /// Long term key.
    pub ltk: [u8; 16],
    /// Flags as serialized.
    pub flags: u8,
    /// Encrypted diversifier.
    pub ediv: u16,
    /// Random number.
    pub rand: [u8; 8],
}

impl EncKey {
    /// From the 27 serialized bytes.
    fn from(b: &[u8]) -> Option<EncKey> {
        let mut k = EncKey::default();
        k.ltk.copy_from_slice(b.get(..16)?);
        k.flags = *b.get(16)?;
        k.ediv = u16::from_le_bytes([*b.get(17)?, *b.get(18)?]);
        k.rand.copy_from_slice(b.get(19..27)?);
        Some(k)
    }

    /// No key length: an empty slot.
    pub fn is_empty(&self) -> bool {
        self.flags >> 2 == 0
    }
}

/// A BLE event from the chip, decoded (the fields used here only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event<'a> {
    /// A central connected to us (role 1), or we to a peripheral (2).
    Connected {
        /// Connection handle.
        conn: u16,
        /// The peer's address: type byte, then 6 bytes LSB first.
        peer: [u8; 7],
        /// 1 peripheral, 2 central.
        role: u8,
    },
    /// The link is gone.
    Disconnected {
        /// Connection handle.
        conn: u16,
        /// HCI reason.
        reason: u8,
    },
    /// The central wants to pair: answer with [`sec_params_reply`].
    SecParamsRequest {
        /// Connection handle.
        conn: u16,
    },
    /// The central encrypts with a key it keeps from a bond: answer with [`sec_info_reply`].
    SecInfoRequest {
        /// Connection handle.
        conn: u16,
        /// EDIV of the key asked for (0 with LESC).
        ediv: u16,
    },
    /// Pairing ended: status 0 is success; the keys exchanged (ours and the peer's encryption key).
    AuthStatus {
        /// Connection handle.
        conn: u16,
        /// 0: success.
        status: u8,
        /// Bonded (keys to keep).
        bonded: bool,
        /// Our encryption key, if distributed.
        own: Option<EncKey>,
        /// The peer's, if distributed (with LESC, the LTK is here, inferred from the SoftDevice's MSCs).
        peer: Option<EncKey>,
    },
    /// The link's security changed: level 2 and up is encrypted.
    ConnSecUpdate {
        /// Connection handle.
        conn: u16,
        /// Security level (1 open, 2 encrypted, 3 MITM, 4 LESC MITM).
        level: u8,
    },
    /// A time-out: advertising (0), scan (1), connection (2).
    Timeout {
        /// What timed out.
        src: u8,
    },
    /// The peripheral asks us, its central, for other connection parameters: accepted ([`conn_param_update`]).
    ConnParamRequest {
        /// Connection handle.
        conn: u16,
        /// What it asks for.
        params: ConnParams,
    },
    /// The peer asks for other PHYs or data length; answered with the defaults.
    ParamRequest {
        /// Connection handle.
        conn: u16,
        /// Which: the event id.
        id: u16,
    },
    /// The peer asks for memory for a long write.
    UserMemRequest {
        /// Connection handle.
        conn: u16,
    },
    /// The peer wrote one of our attributes.
    Write {
        /// Connection handle.
        conn: u16,
        /// The attribute.
        handle: u16,
        /// Its UUID's 16-bit part.
        uuid: u16,
        /// What was written.
        data: &'a [u8],
    },
    /// The peer accessed an attribute whose CCCDs are not set yet: answer with [`sys_attr_set`].
    SysAttrMissing {
        /// Connection handle.
        conn: u16,
    },
    /// The peer asks to exchange the ATT MTU.
    MtuRequest {
        /// Connection handle.
        conn: u16,
    },
    /// Primary services found: (16-bit UUID, type, start, end) entries of 7 bytes ([`services`]).
    Services {
        /// Connection handle.
        conn: u16,
        /// GATT status (0 or [`ATTRIBUTE_NOT_FOUND`]...).
        status: u16,
        /// The entries.
        list: &'a [u8],
    },
    /// Characteristics found, 9-byte entries ([`chars`]).
    Chars {
        /// Connection handle.
        conn: u16,
        /// GATT status.
        status: u16,
        /// The entries.
        list: &'a [u8],
    },
    /// Descriptors found, 5-byte entries ([`descs`]).
    Descs {
        /// Connection handle.
        conn: u16,
        /// GATT status.
        status: u16,
        /// The entries.
        list: &'a [u8],
    },
    /// A read's answer.
    Read {
        /// Connection handle.
        conn: u16,
        /// GATT status.
        status: u16,
        /// The attribute.
        handle: u16,
        /// Its value.
        data: &'a [u8],
    },
    /// A write's answer.
    Written {
        /// Connection handle.
        conn: u16,
        /// GATT status.
        status: u16,
        /// The attribute.
        handle: u16,
    },
    /// A notification (type 1) or indication (2) from the peer.
    Hvx {
        /// Connection handle.
        conn: u16,
        /// The attribute.
        handle: u16,
        /// 1 notification, 2 indication.
        kind: u8,
        /// The value.
        data: &'a [u8],
    },
    /// The peer did not answer a GATT request in time (the link is no good).
    GattTimeout {
        /// Connection handle.
        conn: u16,
    },
    /// Anything else.
    Other,
}

impl Event<'_> {
    /// The connection the event belongs to, if it names one.
    pub fn conn(&self) -> Option<u16> {
        use Event::*;
        match *self {
            Connected { conn, .. }
            | Disconnected { conn, .. }
            | SecParamsRequest { conn }
            | SecInfoRequest { conn, .. }
            | AuthStatus { conn, .. }
            | ConnSecUpdate { conn, .. }
            | ConnParamRequest { conn, .. }
            | ParamRequest { conn, .. }
            | UserMemRequest { conn }
            | Write { conn, .. }
            | SysAttrMissing { conn }
            | MtuRequest { conn }
            | Services { conn, .. }
            | Chars { conn, .. }
            | Descs { conn, .. }
            | Read { conn, .. }
            | Written { conn, .. }
            | Hvx { conn, .. }
            | GattTimeout { conn } => Some(conn),
            Timeout { .. } | Other => None,
        }
    }
}

fn u16_at(p: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*p.get(i)?, *p.get(i + 1)?]))
}

/// A `count u16` then `count` entries of `size` bytes: the entries (as many as there are whole).
fn entries(p: &[u8], size: usize) -> Option<&[u8]> {
    let n = usize::from(u16_at(p, 0)?);
    let list = p.get(2..)?;
    list.get(..(n * size).min(list.len() / size * size))
}

/// Decodes BLE event packet `p` (`[BLE_EVT][id u16][fields]`); None if it is not one or is cut short.
pub fn event(p: &[u8]) -> Option<Event<'_>> {
    let id = nrf::event_id(p)?;
    let f = p.get(3..)?;
    let conn = u16_at(f, 0);
    Some(match id {
        0x10 => {
            let mut peer = [0; 7];
            peer.copy_from_slice(f.get(2..9)?);
            Event::Connected { conn: conn?, peer, role: *f.get(9)? }
        }
        0x11 => Event::Disconnected { conn: conn?, reason: *f.get(2)? },
        0x13 => Event::SecParamsRequest { conn: conn? },
        0x14 => Event::SecInfoRequest { conn: conn?, ediv: u16_at(f, 9)? },
        0x19 => {
            // status, flags (bit 2 bonded), two levels bytes, the two kdist bytes, then the keyset when the
            // reply gave one: own enc, id, sign, pk keys, then the peer's, each a presence byte and its bytes
            let (status, flags) = (*f.get(2)?, *f.get(3)?);
            let mut keys: [Option<EncKey>; 2] = [None, None];
            let mut at = 8;
            for k in &mut keys {
                for (n, size) in [27, 23, 16, 64].into_iter().enumerate() {
                    match f.get(at) {
                        Some(1) => {
                            if n == 0 {
                                *k = EncKey::from(f.get(at + 1..at + 1 + size)?);
                            }
                            at += 1 + size;
                        }
                        Some(_) => at += 1,
                        None => break,
                    }
                }
            }
            let [own, peer] = keys;
            Event::AuthStatus { conn: conn?, status, bonded: flags & 4 != 0, own, peer }
        }
        0x1a => Event::ConnSecUpdate { conn: conn?, level: *f.get(2)? >> 4 },
        0x1b => Event::Timeout { src: *f.get(2)? },
        0x1f => {
            let p = |i| u16_at(f, i);
            Event::ConnParamRequest { conn: conn?, params: (p(2)?, p(4)?, p(6)?, p(8)?) }
        }
        0x21 | 0x23 => Event::ParamRequest { conn: conn?, id },
        0x01 => Event::UserMemRequest { conn: conn? },
        0x50 => {
            let len = usize::from(u16_at(f, 11)?);
            Event::Write { conn: conn?, handle: u16_at(f, 2)?, uuid: u16_at(f, 4)?, data: f.get(13..13 + len)? }
        }
        0x52 => Event::SysAttrMissing { conn: conn? },
        0x55 => Event::MtuRequest { conn: conn? },
        0x56 | 0x3b => Event::GattTimeout { conn: conn? },
        0x30 | 0x32 | 0x33 | 0x36 | 0x38 | 0x39 => {
            let (conn, status, rest) = (conn?, u16_at(f, 2)?, f.get(6..)?);
            match id {
                0x30 => Event::Services { conn, status, list: entries(rest, 7)? },
                0x32 => Event::Chars { conn, status, list: entries(rest, 9)? },
                0x33 => Event::Descs { conn, status, list: entries(rest, 5)? },
                0x36 => {
                    let len = usize::from(u16_at(rest, 4)?);
                    Event::Read { conn, status, handle: u16_at(rest, 0)?, data: rest.get(6..6 + len)? }
                }
                0x38 => Event::Written { conn, status, handle: u16_at(rest, 0)? },
                _ => {
                    let len = usize::from(u16_at(rest, 3)?);
                    Event::Hvx { conn, handle: u16_at(rest, 0)?, kind: *rest.get(2)?, data: rest.get(5..5 + len)? }
                }
            }
        }
        _ => Event::Other,
    })
}

/// The services of an [`Event::Services`] list: (UUID, start handle, end handle).
pub fn services(list: &[u8]) -> impl Iterator<Item = (Uuid, u16, u16)> + '_ {
    list.chunks_exact(7).map(|e| {
        let h = |i: usize| u16::from_le_bytes([e[i], e[i + 1]]);
        ((h(0), e[2]), h(3), h(5))
    })
}

/// The characteristics of an [`Event::Chars`] list: (UUID, properties, declaration handle, value handle).
pub fn chars(list: &[u8]) -> impl Iterator<Item = (Uuid, u8, u16, u16)> + '_ {
    list.chunks_exact(9).map(|e| {
        let h = |i: usize| u16::from_le_bytes([e[i], e[i + 1]]);
        ((h(0), e[2]), e[3], h(5), h(7))
    })
}

/// The descriptors of an [`Event::Descs`] list: (handle, UUID).
pub fn descs(list: &[u8]) -> impl Iterator<Item = (u16, Uuid)> + '_ {
    list.chunks_exact(5).map(|e| {
        let h = |i: usize| u16::from_le_bytes([e[i], e[i + 1]]);
        (h(0), (h(2), e[4]))
    })
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn commands_have_the_codec_layouts() {
        // sd_ble_gap_adv_start: present, type, no peer, fp, interval, timeout, channel mask, conn cfg tag
        let mut c = Cmd::new(GAP_ADV_START);
        c.u8(1).u8(0).u8(0).u8(0).u16(0x0640).u16(0).u8(0).u8(0);
        assert_eq!(c.payload(), Some(&[0, 0x73, 1, 0, 0, 0, 0x40, 0x06, 0, 0, 0, 0][..]));
        // a CCCD write as BOARD.md 11 has it from stock: [conn] 01 [op 1][flags 0][handle][offset 0][len 2] 01 01 00
        let mut c = Cmd::new(GATTC_WRITE);
        c.u16(0).u8(1).u8(1).u8(0).u16(0x0d).u16(0).data16(&[1, 0]);
        assert_eq!(c.payload(), Some(&[0, 0x9c, 0, 0, 1, 1, 0, 0x0d, 0, 0, 0, 2, 0, 1, 1, 0][..]));
        // sd_ble_gap_connect as BOARD.md 11 has it from stock: address, scan and connection parameters, tag 1
        let c = connect_cmd(&[3, 1, 2, 3, 4, 5, 6]);
        let want = [0, 0x8c, 1, 3, 1, 2, 3, 4, 5, 6, 1, 1, 0x60, 0, 0x60, 0, 0, 0, 1, 6, 0, 12, 0, 0, 0, 0x90, 1, 1];
        assert_eq!(c.payload(), Some(&want[..]));
        // a connection parameter update request as a central gets: [conn] 01 [the 4 values]
        let req = [2, 0x1f, 0, 1, 0, 24, 0, 40, 0, 0, 0, 0x90, 1];
        assert_eq!(event(&req), Some(Event::ConnParamRequest { conn: 1, params: (24, 40, 0, 400) }));
        let mut c = Cmd::new(0);
        c.bytes(&[0; 200]);
        assert_eq!(c.payload(), None, "too long: never sent cut");
    }

    #[test]
    fn events_decode_and_short_ones_do_not() {
        let mut p = std::vec![2, 0x10, 0, 0, 0, 1, 1, 2, 3, 4, 5, 6, 1];
        p.extend([0; 8]);
        assert_eq!(event(&p), Some(Event::Connected { conn: 0, peer: [1, 1, 2, 3, 4, 5, 6], role: 1 }));
        for n in 0..p.len() - 8 {
            assert!(!matches!(event(&p[..n]), Some(Event::Connected { .. })), "cut at {n}");
        }
        // auth status with a keyset: own enc key present, peer enc and id keys present
        let mut a = std::vec![2, 0x19, 0, 0, 0, 0, 4, 0, 0, 1, 3, 1];
        let mut key = [0u8; 27];
        key[0] = 0xaa;
        key[16] = 16 << 2;
        a.extend(key);
        a.extend([0, 0, 0, 1]);
        a.extend([0; 27]);
        a.extend([1]);
        a.extend([0; 23]);
        a.extend([0, 0]);
        let Some(Event::AuthStatus { status: 0, bonded: true, own: Some(k), peer: Some(pk), .. }) = event(&a) else {
            panic!("{:?}", event(&a))
        };
        assert_eq!((k.ltk[0], k.is_empty(), pk.is_empty()), (0xaa, false, true));
        let hvx = [2, 0x39, 0, 0, 0, 0, 0, 0, 0, 0x0e, 0, 1, 3, 0, 9, 8, 7];
        assert_eq!(event(&hvx), Some(Event::Hvx { conn: 0, handle: 0x0e, kind: 1, data: &[9, 8, 7] }));
        assert_eq!(event(&hvx[..16]), None, "data cut short");
    }
}
