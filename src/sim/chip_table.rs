//! The Nordic chip's command and event table, as data, and the check the simulated chip runs on every command it
//! receives. It lives in the simulator (not in `drivers/`) because only tests use it: the firmware builds its
//! commands by hand and must not pay for a table.
//!
//! What the real chip does (BOARD.md 11, "The chip's command table"): it accepts a packet of 1 to 384 bytes whose
//! type is 00 (BLE or SoC), 03 (DTM), 05 (reset) or 06 (ANT) and whose opcode is one of 120; the decoder of that opcode
//! must consume the arguments exactly. Anything else makes it hang for ever (no response, no event, until its reset
//! line): `check` returns the reason as a [`Hang`].
//!
//! Sources, per row (`Cmd::src`): Nordic's open nRF5 SDK 14.2.0 serialization, connectivity side
//! (`components/serialization/connectivity/codecs/{ble,ant}/serializers/*.c`, the `*_req_dec` functions) and the
//! common struct codecs (`components/serialization/common/struct_ser/{ble,ant}/*.c`, the `*_dec` functions), with
//! `common/ble_serialization.c` (`buf_dec`, `len16data_dec`, `len8data_dec`, `uint8_vector_dec`),
//! `common/cond_field_serialization.c` (`cond_field_dec`: a presence byte, 0 or 1, anything else is an error) and
//! the macros of `common/ble_serialization.h` (`SER_REQ_DEC_END`: the parsed length must equal the packet length).
//! Bryton's nRF app, reversed: the opcode set, the 384 byte packet limit, the decoders' exact-length rule (checked
//! for 0x69) and the GATT client write's 20 byte value buffer (the SDK default, ATT MTU 23 minus 3).
//! Opcodes the chip has and rustfw never sends are listed as present with the layout "not modelled".
//!
//! Each row also says whether stock firmware sends the same command ([`Stock`]). That flag is from stock firmware,
//! reversed (its command list, BOARD.md 11): `Sends` for every command stock encodes, `Never` for the rest. A `Never`
//! row is proven on Bryton's real app by `tools/nrf_emu.py selftest`, and chip_table_tests.rs fails for one without
//! that proof.

extern crate std;
use std::{format, string::String, vec::Vec};

/// Packets the chip accepts: 1 to this many payload bytes (Bryton's nRF app, reversed).
pub const MAX_PAYLOAD: usize = 384;
/// The largest BLE event the chip can pull from the SoftDevice, before encoding (Bryton's nRF app, reversed).
pub const MAX_BLE_EVENT: usize = 262;
/// An ANT event packet: type 8 and the 43 bytes of `ant_evt_t` as encoded (BOARD.md 11).
pub const ANT_EVENT_LEN: usize = 44;
/// The GATT client write's value buffer in the chip: a longer value does not decode, so the chip hangs.
pub const GATTC_WRITE_MAX: usize = 20;

/// One item of a command's arguments, in the order the decoder reads it.
#[derive(Clone, Copy, Debug)]
pub enum L {
    /// One byte, any value.
    U8,
    /// Little endian u16.
    U16,
    /// Little endian u32.
    U32,
    /// This many bytes, any value (a fixed struct or array).
    Raw(usize),
    /// A presence byte (0 or 1; any other value does not decode), then the items when 1 (`SER_PULL_COND`).
    Cond(&'static [L]),
    /// A presence byte that must be 0 here: the present form is not modelled.
    Absent,
    /// `len16data`: u16 length, presence byte, then the bytes when present, at most this many (`buf_dec`).
    Len16(usize),
    /// `len8data`: u8 length, presence byte, then the bytes when present, at most this many.
    Len8(usize),
    /// `uint8_vector_dec` after a u8 size: the size, then that many bytes. The SDK checks only the remaining length;
    /// the bound of 9 (an ANT message: 8 payload bytes, `ANT_STANDARD_DATA_PAYLOAD_SIZE`, plus the channel) is not enforced;
    /// the S332 SDK's own bound was not found (searched Nordic's public S212 docs).
    Sized,
    /// `ble_gatts_hvx_params_t_dec`'s tail: a presence byte and a u16 length, then (if present) a presence byte and
    /// that many bytes.
    Hvx,
    /// `sd_ble_cfg_set`'s id and body (`ble_cfg_set_req_dec`).
    Cfg,
}

/// What a command's response carries after its 4 result bytes when the result is 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    /// Exactly this many bytes.
    Fixed(usize),
    /// Not modelled (any).
    Any,
    /// No response at all (the chip powers off).
    None,
}

/// Whether stock firmware sends a command too (stock firmware, reversed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stock {
    /// Stock encodes this command.
    Sends,
    /// Stock never encodes it: the chip's answer to it is proven on Bryton's real app, not on stock's traffic.
    Never,
}

/// A command the table models.
pub struct Cmd {
    /// 0 BLE or SoC, 6 ANT.
    pub kind: u8,
    /// The opcode (the SoftDevice's SVC number).
    pub op: u8,
    /// The SoftDevice call.
    pub name: &'static str,
    /// The arguments after `[kind][op]`.
    pub args: &'static [L],
    /// The response's outputs.
    pub out: Out,
    /// Whether stock firmware sends it too.
    pub stock: Stock,
    /// The SDK decoder (file: function).
    pub src: &'static str,
}

const ADDR: &[L] = &[L::Raw(7)];
const CONN4: &[L] = &[L::U16, L::U16, L::U16, L::U16];
const SEC: &[L] = &[L::Raw(5)];
const UUID: &[L] = &[L::U16, L::U8];
const MD: &[L] = &[L::Raw(3)];
const ATTR: &[L] = &[L::Cond(UUID), L::Cond(MD), L::U16, L::U16, L::Len16(512)];
const CHAR_MD: &[L] = &[L::Raw(2), L::U16, L::Len16(512), L::Cond(&[L::Raw(7)]), L::Cond(MD), L::Cond(MD), L::Cond(MD)];
const KEYS: &[L] = &[
    L::Cond(&[L::Raw(27)]),
    L::Cond(&[L::Raw(23)]),
    L::Cond(&[L::Raw(16)]),
    L::Cond(&[L::Raw(64)]),
    L::Cond(&[L::Raw(27)]),
    L::Cond(&[L::Raw(23)]),
    L::Cond(&[L::Raw(16)]),
    L::Cond(&[L::Raw(64)]),
];
const NONE: &[L] = &[];
const PTR: L = L::Cond(&[]);

macro_rules! cmd {
    ($k:expr, $op:expr, $name:expr, $args:expr, $out:expr, $stock:expr, $src:expr) => {
        Cmd { kind: $k, op: $op, name: $name, args: $args, out: $out, stock: $stock, src: $src }
    };
}

use Out::{Any, Fixed};

/// Every command rustfw sends (or may send), with its layout.
pub const CMDS: &[Cmd] = &[
    cmd!(0, 0x32, "sd_power_system_off", NONE, Out::None, Stock::Sends, "nrf_soc_conn.c: power_system_off_req_dec"),
    cmd!(
        0,
        0x60,
        "sd_ble_enable",
        NONE,
        Fixed(0),
        Stock::Sends,
        "ble_conn.c: ble_enable_req_dec (API 4 and later: no arguments)"
    ),
    cmd!(
        0,
        0x62,
        "sd_ble_uuid_vs_add",
        &[L::Cond(&[L::Raw(16)]), PTR],
        Fixed(2),
        Stock::Sends,
        "ble_conn.c: ble_uuid_vs_add_req_dec"
    ),
    cmd!(0, 0x65, "sd_ble_version_get", &[PTR], Fixed(5), Stock::Sends, "ble_conn.c: ble_version_get_req_dec"),
    cmd!(
        0,
        0x66,
        "sd_ble_user_mem_reply",
        &[L::U16, L::Absent],
        Fixed(0),
        Stock::Sends,
        "ble_conn.c: ble_user_mem_reply_req_dec"
    ),
    cmd!(0, 0x69, "sd_ble_cfg_set", &[L::Cfg], Fixed(0), Stock::Sends, "ble_conn.c: ble_cfg_set_req_dec"),
    cmd!(
        0,
        0x72,
        "sd_ble_gap_adv_data_set",
        &[L::Len8(31), L::Len8(31)],
        Fixed(0),
        Stock::Sends,
        "ble_gap_conn.c: ble_gap_adv_data_set_req_dec"
    ),
    cmd!(
        0,
        0x73,
        "sd_ble_gap_adv_start",
        &[L::Cond(&[L::U8, L::Cond(ADDR), L::U8, L::U16, L::U16, L::U8]), L::U8],
        Fixed(0),
        Stock::Sends,
        "ble_gap_conn.c: ble_gap_adv_start_req_dec; ble_gap_struct_serialization.c: ble_gap_adv_params_t_dec"
    ),
    cmd!(0, 0x74, "sd_ble_gap_adv_stop", NONE, Fixed(0), Stock::Never, "ble_gap_conn.c: ble_gap_adv_stop_req_dec"),
    cmd!(
        0,
        0x75,
        "sd_ble_gap_conn_param_update",
        &[L::U16, L::Cond(CONN4)],
        Fixed(0),
        Stock::Sends,
        "ble_gap_conn.c: ble_gap_conn_param_update_req_dec"
    ),
    cmd!(
        0,
        0x76,
        "sd_ble_gap_disconnect",
        &[L::U16, L::U8],
        Fixed(0),
        Stock::Sends,
        "ble_gap_conn.c: ble_gap_disconnect_req_dec"
    ),
    cmd!(
        0,
        0x78,
        "sd_ble_gap_appearance_set",
        &[L::U16],
        Fixed(0),
        Stock::Never,
        "ble_gap_conn.c: ble_gap_appearance_set_req_dec"
    ),
    cmd!(
        0,
        0x7a,
        "sd_ble_gap_ppcp_set",
        &[L::Cond(CONN4)],
        Fixed(0),
        Stock::Sends,
        "ble_gap_conn.c: ble_gap_ppcp_set_req_dec"
    ),
    cmd!(
        0,
        0x7c,
        "sd_ble_gap_device_name_set",
        &[L::Cond(&[L::U8]), L::Len16(31)],
        Fixed(0),
        Stock::Sends,
        "ble_gap_conn.c: ble_gap_device_name_set_req_dec (name buffer BLE_GAP_DEVNAME_MAX_LEN, conn_mw_ble_gap.c)"
    ),
    cmd!(
        0,
        0x7e,
        "sd_ble_gap_authenticate",
        &[L::U16, L::Cond(SEC)],
        Fixed(0),
        Stock::Sends,
        "ble_gap_conn.c: ble_gap_authenticate_req_dec; ble_gap_sec_params_t_dec"
    ),
    cmd!(
        0,
        0x7f,
        "sd_ble_gap_sec_params_reply",
        &[L::U16, L::U8, L::Cond(SEC), L::Cond(KEYS)],
        Fixed(0),
        Stock::Sends,
        "ble_gap_conn.c: ble_gap_sec_params_reply_req_dec; ble_gap_sec_keyset_t_dec"
    ),
    cmd!(
        0,
        0x86,
        "sd_ble_gap_sec_info_reply",
        &[L::U16, L::Cond(&[L::Raw(17)]), L::Cond(&[L::Raw(16)]), L::Cond(&[L::Raw(16)])],
        Fixed(0),
        Stock::Sends,
        "ble_gap_conn.c: ble_gap_sec_info_reply_req_dec; ble_gap_enc_info_t_dec, ble_gap_irk_t_dec, ble_gap_sign_info_t_dec"
    ),
    cmd!(
        0,
        0x8a,
        "sd_ble_gap_scan_start",
        &[L::Cond(&[L::U8, L::U16, L::U16, L::U16])],
        Fixed(0),
        Stock::Sends,
        "ble_gap_conn.c: ble_gap_scan_start_req_dec; ble_gap_scan_params_t_dec"
    ),
    cmd!(0, 0x8b, "sd_ble_gap_scan_stop", NONE, Fixed(0), Stock::Sends, "ble_gap_conn.c: ble_gap_scan_stop_req_dec"),
    cmd!(
        0,
        0x8c,
        "sd_ble_gap_connect",
        &[L::Cond(ADDR), L::Cond(&[L::U8, L::U16, L::U16, L::U16]), L::Cond(CONN4), L::U8],
        Fixed(0),
        Stock::Sends,
        "ble_gap_conn.c: ble_gap_connect_req_dec"
    ),
    cmd!(
        0,
        0x8d,
        "sd_ble_gap_connect_cancel",
        NONE,
        Fixed(0),
        Stock::Never,
        "ble_gap_conn.c: ble_gap_connect_cancel_req_dec"
    ),
    cmd!(
        0,
        0x8f,
        "sd_ble_gap_phy_update",
        &[L::U16, L::Cond(&[L::U8, L::U8])],
        Fixed(0),
        Stock::Sends,
        "ble_gap_conn.c: ble_gap_phy_update_req_dec"
    ),
    cmd!(
        0,
        0x90,
        "sd_ble_gap_data_length_update",
        &[L::U16, L::Cond(CONN4), PTR],
        Fixed(0),
        Stock::Sends,
        "ble_gap_conn.c: ble_gap_data_length_update_req_dec"
    ),
    cmd!(
        0,
        0x94,
        "sd_ble_gattc_primary_services_discover",
        &[L::U16, L::U16, L::Cond(UUID)],
        Fixed(0),
        Stock::Sends,
        "ble_gattc_conn.c: ble_gattc_primary_services_discover_req_dec"
    ),
    cmd!(
        0,
        0x96,
        "sd_ble_gattc_characteristics_discover",
        &[L::U16, L::Cond(&[L::U16, L::U16])],
        Fixed(0),
        Stock::Sends,
        "ble_gattc_conn.c: ble_gattc_characteristics_discover_req_dec"
    ),
    cmd!(
        0,
        0x97,
        "sd_ble_gattc_descriptors_discover",
        &[L::U16, L::Cond(&[L::U16, L::U16])],
        Fixed(0),
        Stock::Sends,
        "ble_gattc_conn.c: ble_gattc_descriptors_discover_req_dec"
    ),
    cmd!(
        0,
        0x9a,
        "sd_ble_gattc_read",
        &[L::U16, L::U16, L::U16],
        Fixed(0),
        Stock::Sends,
        "ble_gattc_conn.c: ble_gattc_read_req_dec"
    ),
    cmd!(
        0,
        0x9c,
        "sd_ble_gattc_write",
        &[L::U16, L::Cond(&[L::U8, L::U8, L::U16, L::U16, L::Len16(GATTC_WRITE_MAX)])],
        Fixed(0),
        Stock::Sends,
        "ble_gattc_conn.c: ble_gattc_write_req_dec; ble_gattc_write_params_t_dec; value buffer BLE_GATTC_WRITE_P_VALUE_LEN_MAX, 20 bytes in Bryton's build"
    ),
    cmd!(
        0,
        0x9d,
        "sd_ble_gattc_hv_confirm",
        &[L::U16, L::U16],
        Fixed(0),
        Stock::Never,
        "ble_gattc_conn.c: ble_gattc_hv_confirm_req_dec"
    ),
    cmd!(
        0,
        0xa0,
        "sd_ble_gatts_service_add",
        &[L::U8, L::Cond(UUID), PTR],
        Fixed(3),
        Stock::Sends,
        "ble_gatts_conn.c: ble_gatts_service_add_req_dec"
    ),
    cmd!(
        0,
        0xa2,
        "sd_ble_gatts_characteristic_add",
        &[L::U16, L::Cond(CHAR_MD), L::Cond(ATTR), PTR],
        Fixed(9),
        Stock::Sends,
        "ble_gatts_conn.c: ble_gatts_characteristic_add_req_dec; ble_gatts_char_md_t_dec, ble_gatts_attr_t_dec, ble_gatts_attr_md_t_dec"
    ),
    cmd!(
        0,
        0xa3,
        "sd_ble_gatts_descriptor_add",
        &[L::U16, L::Cond(ATTR), PTR],
        Fixed(3),
        Stock::Never,
        "ble_gatts_conn.c: ble_gatts_descriptor_add_req_dec"
    ),
    cmd!(
        0,
        0xa4,
        "sd_ble_gatts_value_set",
        &[L::U16, L::U16, L::Cond(&[L::U16, L::Len16(512)])],
        Any,
        Stock::Never,
        "ble_gatts_conn.c: ble_gatts_value_set_req_dec; ble_gatts_value_t_dec"
    ),
    cmd!(
        0,
        0xa6,
        "sd_ble_gatts_hvx",
        &[L::U16, L::Cond(&[L::U16, L::U8, L::U16, L::Hvx])],
        Any,
        Stock::Sends,
        "ble_gatts_conn.c: ble_gatts_hvx_req_dec; ble_gatts_hvx_params_t_dec"
    ),
    cmd!(
        0,
        0xa9,
        "sd_ble_gatts_sys_attr_set",
        &[L::U16, L::Len16(512), L::U32],
        Fixed(0),
        Stock::Sends,
        "ble_gatts_conn.c: ble_gatts_sys_attr_set_req_dec"
    ),
    cmd!(
        0,
        0xaa,
        "sd_ble_gatts_sys_attr_get",
        &[L::U16, L::Cond(&[L::U16]), PTR, L::U32],
        Any,
        Stock::Sends,
        "ble_gatts_conn.c: ble_gatts_sys_attr_get_req_dec"
    ),
    cmd!(
        0,
        0xad,
        "sd_ble_gatts_exchange_mtu_reply",
        &[L::U16, L::U16],
        Fixed(0),
        Stock::Sends,
        "ble_gatts_conn.c: ble_gatts_exchange_mtu_reply_req_dec"
    ),
    cmd!(
        6,
        0xfa,
        "sd_ant_enable",
        &[L::Cond(&[L::U8, L::U8, L::U16, L::U32, L::U16])],
        Fixed(0),
        Stock::Sends,
        "ant_enable.c: ant_enable_req_dec; ANT_ENABLE_dec"
    ),
    cmd!(
        6,
        0xcc,
        "sd_ant_network_address_set",
        &[L::U8, L::Raw(8)],
        Fixed(0),
        Stock::Sends,
        "ant_network_address_set.c: ant_network_address_set_req_dec"
    ),
    cmd!(
        6,
        0xc2,
        "sd_ant_channel_assign",
        &[L::U8, L::U8, L::U8, L::U8],
        Fixed(0),
        Stock::Sends,
        "ant_channel_assign.c: ant_channel_assign_req_dec"
    ),
    cmd!(
        6,
        0xd3,
        "sd_ant_channel_id_set",
        &[L::U8, L::U16, L::U8, L::U8],
        Fixed(0),
        Stock::Sends,
        "ant_channel_id_set.c: ant_channel_id_set_req_dec"
    ),
    cmd!(
        6,
        0xda,
        "sd_ant_channel_low_priority_rx_search_timeout_set",
        &[L::U8, L::U8],
        Fixed(0),
        Stock::Sends,
        "ant_channel_low_priority_rx_search_timout_set.c"
    ),
    cmd!(
        6,
        0xcd,
        "sd_ant_channel_radio_freq_set",
        &[L::U8, L::U8],
        Fixed(0),
        Stock::Sends,
        "ant_channel_radio_freq_set.c: ant_channel_radio_freq_set_req_dec"
    ),
    cmd!(
        6,
        0xd1,
        "sd_ant_channel_period_set",
        &[L::U8, L::U16],
        Fixed(0),
        Stock::Sends,
        "ant_channel_period_set.c: ant_channel_period_set_req_dec"
    ),
    cmd!(
        6,
        0xc4,
        "sd_ant_channel_open_with_offset",
        &[L::U8, L::U16],
        Fixed(0),
        Stock::Sends,
        "ant_channel_open_with_offset.c: ant_channel_open_with_offset_req_dec"
    ),
    // the size byte is bounded by the ANT message (9: 8 payload bytes and the channel); the SDK checks only the bytes left
    cmd!(
        6,
        0xc8,
        "sd_ant_acknowledge_message_tx",
        &[L::U8, L::Sized],
        Fixed(0),
        Stock::Sends,
        "ant_acknowledge_message_tx.c: ant_acknowledge_message_tx_req_dec"
    ),
    cmd!(
        6,
        0xc5,
        "sd_ant_channel_close",
        &[L::U8],
        Fixed(0),
        Stock::Sends,
        "ant_channel_close.c: ant_channel_close_req_dec"
    ),
    cmd!(
        6,
        0xd4,
        "sd_ant_channel_id_get",
        &[L::U8],
        Fixed(4),
        Stock::Sends,
        "ant_channel_id_get.c: ant_channel_id_get_req_dec (response: u16 number, u8 type, u8 transmission)"
    ),
];

/// The opcodes of type 0 the chip's handler table has (Bryton's nRF app, reversed: 121 entries, 0x86 twice), as
/// inclusive ranges. Those without a row in [`CMDS`] are present, layout not modelled.
pub const PRESENT_BLE: &[(u8, u8)] = &[
    (0x32, 0x32),
    (0x43, 0x43),
    (0x49, 0x49),
    (0x60, 0x60),
    (0x62, 0x69),
    (0x6c, 0x90),
    (0x94, 0x9e),
    (0xa0, 0xad),
    (0xb0, 0xb4),
];
/// The same for type 6 (ANT).
pub const PRESENT_ANT: &[(u8, u8)] =
    &[(0xc0, 0xc0), (0xc2, 0xc8), (0xcc, 0xdb), (0xdd, 0xe1), (0xe7, 0xe7), (0xe9, 0xec), (0xf2, 0xf5), (0xf8, 0xfa)];

/// Why the chip would hang on a packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hang {
    /// Payload length 0 or over 384.
    Length(usize),
    /// A type other than 00, 03, 05, 06.
    Type(u8),
    /// A type with no opcode.
    TypeOnly(u8),
    /// An opcode the chip's table lacks.
    UnknownOpcode(u8, u8),
    /// Present in the chip, layout not modelled (or type 03 / 05, never sent).
    NotModelled(u8, u8),
    /// The arguments do not parse to exactly the length, or a field is out of range: the call, what, the offset.
    Layout(&'static str, &'static str, usize),
}

/// Whether `op` of `kind` is in the chip's table.
fn present(kind: u8, op: u8) -> bool {
    let list = if kind == 6 { PRESENT_ANT } else { PRESENT_BLE };
    list.iter().any(|&(a, b)| (a..=b).contains(&op))
}

/// The table row for (`kind`, `op`).
pub fn row(kind: u8, op: u8) -> Option<&'static Cmd> {
    CMDS.iter().find(|c| c.kind == kind && c.op == op)
}

fn u8_at(p: &[u8], at: &mut usize) -> Result<u8, &'static str> {
    let b = *p.get(*at).ok_or("cut short")?;
    *at += 1;
    Ok(b)
}

fn skip(p: &[u8], at: &mut usize, n: usize) -> Result<(), &'static str> {
    if p.len() < *at + n {
        return Err("cut short");
    }
    *at += n;
    Ok(())
}

fn u16_at(p: &[u8], at: &mut usize) -> Result<usize, &'static str> {
    let (a, b) = (u8_at(p, at)?, u8_at(p, at)?);
    Ok(usize::from(u16::from_le_bytes([a, b])))
}

/// `buf_dec`: a presence byte, then `n` bytes when present, `n` at most `max`.
fn buf(p: &[u8], at: &mut usize, n: usize, max: usize) -> Result<(), &'static str> {
    match u8_at(p, at)? {
        0 => Ok(()),
        1 if n > max => Err("buffer longer than the chip's"),
        1 => skip(p, at, n),
        _ => Err("presence byte not 0 or 1"),
    }
}

fn walk(items: &[L], p: &[u8], at: &mut usize) -> Result<(), &'static str> {
    for it in items {
        match *it {
            L::U8 => drop(u8_at(p, at)?),
            L::U16 => drop(u16_at(p, at)?),
            L::U32 => skip(p, at, 4)?,
            L::Raw(n) => skip(p, at, n)?,
            L::Cond(sub) => match u8_at(p, at)? {
                0 => {}
                1 => walk(sub, p, at)?,
                _ => return Err("presence byte not 0 or 1"),
            },
            L::Absent => match u8_at(p, at)? {
                0 => {}
                1 => return Err("present form not modelled"),
                _ => return Err("presence byte not 0 or 1"),
            },
            L::Len16(max) => {
                let n = u16_at(p, at)?;
                buf(p, at, n, max)?;
            }
            L::Len8(max) => {
                let n = usize::from(u8_at(p, at)?);
                buf(p, at, n, max)?;
            }
            L::Sized => {
                let n = usize::from(u8_at(p, at)?);
                skip(p, at, n)?;
            }
            L::Hvx => match u8_at(p, at)? {
                0 => {}
                1 => {
                    let n = u16_at(p, at)?;
                    buf(p, at, n, usize::MAX)?;
                }
                _ => return Err("presence byte not 0 or 1"),
            },
            L::Cfg => {
                skip(p, at, 4)?;
                let id = u32::from_le_bytes(p.get(*at - 4..*at).and_then(|b| b.try_into().ok()).ok_or("cut short")?);
                match u8_at(p, at)? {
                    0 => {}
                    1 => {
                        // tag byte for the connection configurations 0x20..=0x23
                        if (0x20..=0x23).contains(&id) {
                            u8_at(p, at)?;
                        }
                        match id {
                            0x20 => walk(&[L::U8, L::U16], p, at)?,
                            0x22 => walk(&[L::U8], p, at)?,
                            0x23 => walk(&[L::U16], p, at)?,
                            0x40 => walk(&[L::U8, L::U8, L::U8], p, at)?,
                            _ => return Err("cfg id not modelled"),
                        }
                    }
                    _ => return Err("presence byte not 0 or 1"),
                }
            }
        }
    }
    Ok(())
}

/// What the chip does with packet `p` (`[kind][op][arguments]`): the row that decodes it, or why it hangs.
pub fn check(p: &[u8]) -> Result<&'static Cmd, Hang> {
    if p.is_empty() || p.len() > MAX_PAYLOAD {
        return Err(Hang::Length(p.len()));
    }
    let kind = p[0];
    if !matches!(kind, 0 | 3 | 5 | 6) {
        return Err(Hang::Type(kind));
    }
    let Some(&op) = p.get(1) else { return Err(Hang::TypeOnly(kind)) };
    if kind == 3 || kind == 5 {
        return Err(Hang::NotModelled(kind, op));
    }
    if !present(kind, op) {
        return Err(Hang::UnknownOpcode(kind, op));
    }
    let Some(c) = row(kind, op) else { return Err(Hang::NotModelled(kind, op)) };
    let mut at = 2;
    walk(c.args, p, &mut at).map_err(|why| Hang::Layout(c.name, why, at))?;
    if at != p.len() {
        return Err(Hang::Layout(c.name, "bytes left over", at));
    }
    Ok(c)
}

/// Whether response `r` (`[01][op][result u32][outputs]`) to the command (`kind`, `op`) has the layout of its row.
pub fn response_ok(kind: u8, r: &[u8]) -> Result<(), String> {
    let [1, op, r0, r1, r2, r3, outputs @ ..] = r else { return Err(format!("not a response: {r:?}")) };
    let Some(c) = row(kind, *op) else { return Ok(()) };
    if u32::from_le_bytes([*r0, *r1, *r2, *r3]) != 0 {
        return Ok(());
    }
    match c.out {
        Fixed(n) if outputs.len() != n => {
            Err(format!("{}: {} output bytes, the table says {n}", c.name, outputs.len()))
        }
        _ => Ok(()),
    }
}

/// How an event's bytes after its id (`f`, starting with the connection handle) are laid out.
#[derive(Clone, Copy, Debug)]
pub enum Shape {
    /// Fixed fields, at least `min` bytes (the SDK's encoder may add fields rustfw does not read).
    Plain,
    /// A u16 length at `at`, the data from `from`.
    Len { at: usize, from: usize },
    /// Header of 6 bytes (conn, status, error handle), a u16 count, then entries of `item` bytes.
    List { item: usize },
    /// BLE_GAP_EVT_AUTH_STATUS: 8 bytes, then the keyset's eight presence-prefixed keys.
    Auth,
    /// BLE_GAP_EVT_ADV_REPORT: 19 bytes, rssi, flags, data length (u8), a presence byte, the data.
    Adv,
}

/// A BLE event rustfw parses.
pub struct Ev {
    /// Event id (BLE_GAP_EVT_BASE 0x10, GATTC 0x30, GATTS 0x50, common 0x01).
    pub id: u16,
    /// The SoftDevice event.
    pub name: &'static str,
    /// Bytes after the id that the parser needs.
    pub min: usize,
    /// The layout.
    pub shape: Shape,
    /// The SDK encoder (file: function).
    pub src: &'static str,
}

macro_rules! ev {
    ($id:expr, $name:expr, $min:expr, $shape:expr, $src:expr) => {
        Ev { id: $id, name: $name, min: $min, shape: $shape, src: $src }
    };
}

use Shape::*;

/// Every BLE event rustfw's `ble::event` and `nrf::adv_report` decode. Each is sent as `[02][id u16][f...]`, at most
/// [`MAX_BLE_EVENT`] + 1 bytes (the chip hangs on a bigger one); the exact size of the fixed ones is not modelled.
pub const EVENTS: &[Ev] = &[
    ev!(0x01, "BLE_EVT_USER_MEM_REQUEST", 2, Plain, "ble_evt_conn.c: ble_evt_user_mem_request_enc"),
    ev!(0x10, "BLE_GAP_EVT_CONNECTED", 10, Plain, "ble_gap_evt_conn.c: ble_gap_evt_connected_enc"),
    ev!(0x11, "BLE_GAP_EVT_DISCONNECTED", 3, Plain, "ble_gap_evt_conn.c: ble_gap_evt_disconnected_enc"),
    ev!(0x13, "BLE_GAP_EVT_SEC_PARAMS_REQUEST", 2, Plain, "ble_gap_evt_conn.c: ble_gap_evt_sec_params_request_enc"),
    ev!(0x14, "BLE_GAP_EVT_SEC_INFO_REQUEST", 11, Plain, "ble_gap_evt_conn.c: ble_gap_evt_sec_info_request_enc"),
    ev!(0x19, "BLE_GAP_EVT_AUTH_STATUS", 8, Auth, "ble_gap_evt_conn.c: ble_gap_evt_auth_status_enc"),
    ev!(0x1a, "BLE_GAP_EVT_CONN_SEC_UPDATE", 3, Plain, "ble_gap_evt_conn.c: ble_gap_evt_conn_sec_update_enc"),
    ev!(0x1b, "BLE_GAP_EVT_TIMEOUT", 3, Plain, "ble_gap_evt_conn.c: ble_gap_evt_timeout_enc"),
    ev!(0x1d, "BLE_GAP_EVT_ADV_REPORT", 20, Adv, "ble_gap_evt_conn.c: ble_gap_evt_adv_report_enc"),
    ev!(
        0x1f,
        "BLE_GAP_EVT_CONN_PARAM_UPDATE_REQUEST",
        10,
        Plain,
        "ble_gap_evt_conn.c: ble_gap_evt_conn_param_update_request_enc"
    ),
    ev!(0x21, "BLE_GAP_EVT_PHY_UPDATE_REQUEST", 2, Plain, "ble_gap_evt_conn.c: ble_gap_evt_phy_update_request_enc"),
    ev!(
        0x23,
        "BLE_GAP_EVT_DATA_LENGTH_UPDATE_REQUEST",
        2,
        Plain,
        "ble_gap_evt_conn.c: ble_gap_evt_data_length_update_request_enc"
    ),
    ev!(
        0x30,
        "BLE_GATTC_EVT_PRIM_SRVC_DISC_RSP",
        8,
        List { item: 7 },
        "ble_gattc_evt_conn.c: ble_gattc_evt_prim_srvc_disc_rsp_enc"
    ),
    ev!(
        0x32,
        "BLE_GATTC_EVT_CHAR_DISC_RSP",
        8,
        List { item: 9 },
        "ble_gattc_evt_conn.c: ble_gattc_evt_char_disc_rsp_enc"
    ),
    ev!(
        0x33,
        "BLE_GATTC_EVT_DESC_DISC_RSP",
        8,
        List { item: 5 },
        "ble_gattc_evt_conn.c: ble_gattc_evt_desc_disc_rsp_enc"
    ),
    ev!(
        0x36,
        "BLE_GATTC_EVT_READ_RSP",
        12,
        Len { at: 10, from: 12 },
        "ble_gattc_evt_conn.c: ble_gattc_evt_read_rsp_enc"
    ),
    ev!(0x38, "BLE_GATTC_EVT_WRITE_RSP", 8, Plain, "ble_gattc_evt_conn.c: ble_gattc_evt_write_rsp_enc"),
    ev!(0x39, "BLE_GATTC_EVT_HVX", 11, Len { at: 9, from: 11 }, "ble_gattc_evt_conn.c: ble_gattc_evt_hvx_enc"),
    ev!(0x3b, "BLE_GATTC_EVT_TIMEOUT", 2, Plain, "ble_gattc_evt_conn.c: ble_gattc_evt_timeout_enc"),
    ev!(0x50, "BLE_GATTS_EVT_WRITE", 13, Len { at: 11, from: 13 }, "ble_gatts_evt_conn.c: ble_gatts_evt_write_enc"),
    ev!(0x52, "BLE_GATTS_EVT_SYS_ATTR_MISSING", 2, Plain, "ble_gatts_evt_conn.c: ble_gatts_evt_sys_attr_missing_enc"),
    ev!(
        0x55,
        "BLE_GATTS_EVT_EXCHANGE_MTU_REQUEST",
        4,
        Plain,
        "ble_gatts_evt_conn.c: ble_gatts_evt_exchange_mtu_request_enc"
    ),
    ev!(0x56, "BLE_GATTS_EVT_TIMEOUT", 2, Plain, "ble_gatts_evt_conn.c: ble_gatts_evt_timeout_enc"),
    ev!(0x57, "BLE_GATTS_EVT_HVN_TX_COMPLETE", 3, Plain, "ble_gatts_evt_conn.c: ble_gatts_evt_hvn_tx_complete_enc"),
];

/// The event row for `id`.
pub fn event_row(id: u16) -> Option<&'static Ev> {
    EVENTS.iter().find(|e| e.id == id)
}

/// Whether event packet `p` (type 2 or 8) is one the chip can send: a known id and at most the chip's sizes.
pub fn event_ok(p: &[u8]) -> Result<(), String> {
    match p {
        [2, a, b, f @ ..] => {
            if 1 + 2 + f.len() > MAX_BLE_EVENT + 1 {
                return Err(format!("BLE event of {} bytes, the chip's limit is {MAX_BLE_EVENT}", p.len()));
            }
            let id = u16::from_le_bytes([*a, *b]);
            match event_row(id) {
                Some(e) if f.len() < e.min => {
                    Err(format!("{}: {} bytes after the id, at least {}", e.name, f.len(), e.min))
                }
                Some(_) => Ok(()),
                None => Ok(()), // an event rustfw does not parse (Other): any
            }
        }
        [8, ..] if p.len() == ANT_EVENT_LEN => Ok(()),
        [8, ..] => Err(format!("ANT event of {} bytes, not {ANT_EVENT_LEN}", p.len())),
        _ => Err(format!("not an event: {p:?}")),
    }
}

/// An event of `e`'s layout built from the random source `r`: valid (`min` fields, the lengths and counts consistent
/// with the bytes), the data random. The packet includes the `[02][id]` prefix.
pub fn build_event(e: &Ev, r: &mut impl FnMut() -> u32) -> Vec<u8> {
    let rnd = |r: &mut dyn FnMut() -> u32| (r() >> 8) as u8;
    let mut f: Vec<u8> = (0..e.min).map(|_| rnd(r)).collect();
    match e.shape {
        Plain => f.extend((0..r() % 12).map(|_| rnd(r))),
        Len { at, from } => {
            let n = (r() % 40) as usize;
            f.truncate(from);
            f[at..at + 2].copy_from_slice(&(n as u16).to_le_bytes());
            f.extend((0..n).map(|_| rnd(r)));
        }
        List { item } => {
            let n = (r() % 8) as usize;
            f.truncate(6);
            f.extend((n as u16).to_le_bytes());
            f.extend((0..n * item).map(|_| rnd(r)));
        }
        Auth => {
            f.truncate(8);
            let mut w = Vec::new();
            for size in [27, 23, 16, 64, 27, 23, 16, 64] {
                if r().is_multiple_of(3) {
                    w.push(1);
                    w.extend((0..size).map(|_| rnd(r)));
                } else {
                    w.push(0);
                }
            }
            f.extend(w);
        }
        Adv => {
            let n = (r() % 31) as usize;
            f.truncate(17);
            f.extend([rnd(r), n as u8, 1]);
            f.extend((0..n).map(|_| rnd(r)));
        }
    }
    let mut p = std::vec![2, e.id as u8, (e.id >> 8) as u8];
    p.extend(f);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hang(p: &[u8]) -> Hang {
        check(p).err().unwrap_or_else(|| panic!("{p:?} should hang the chip"))
    }

    #[test]
    fn the_table_is_consistent() {
        for (i, c) in CMDS.iter().enumerate() {
            assert!(c.src.ends_with(".c") || c.src.contains("_dec"), "{}: no SDK source", c.name);
            assert!(present(c.kind, c.op), "{} is not an opcode of the chip", c.name);
            assert!(CMDS[..i].iter().all(|d| (d.kind, d.op) != (c.kind, c.op)), "{} twice", c.name);
        }
        // 120 distinct opcodes in the chip
        let n: usize = PRESENT_BLE.iter().chain(PRESENT_ANT).map(|&(a, b)| usize::from(b - a) + 1).sum();
        assert_eq!(n, 120);
        assert!(EVENTS.iter().all(|e| e.src.contains("_enc")));
        assert!(EVENTS.windows(2).all(|w| w[0].id < w[1].id));
    }

    #[test]
    fn each_fatal_rule_hangs_the_chip() {
        assert_eq!(hang(&[]), Hang::Length(0));
        assert_eq!(hang(&[0; 385]), Hang::Length(385));
        assert_eq!(hang(&[7, 0x60]), Hang::Type(7));
        assert_eq!(hang(&[1, 0x60]), Hang::Type(1), "a response type is not a command");
        assert_eq!(hang(&[0]), Hang::TypeOnly(0));
        assert_eq!(hang(&[0, 0x61]), Hang::UnknownOpcode(0, 0x61));
        assert_eq!(hang(&[6, 0xc1, 0]), Hang::UnknownOpcode(6, 0xc1));
        assert!(matches!(hang(&[0, 0x6c]), Hang::NotModelled(0, 0x6c)), "present, layout not modelled");
        assert!(matches!(hang(&[5, 0]), Hang::NotModelled(5, 0)));
        // exact length: one byte more, one byte less
        // the MTU request is present in the chip, its layout not modelled (rustfw never sends it)
        assert!(matches!(hang(&[0, 0x9e, 1, 0, 23, 0]), Hang::NotModelled(0, 0x9e)));
        // a presence byte of 2
        assert!(matches!(hang(&[0, 0x65, 2]), Hang::Layout(_, "presence byte not 0 or 1", _)));
        // the write's value over 20 bytes
        let mut w = std::vec![0, 0x9c, 0, 0, 1, 1, 0, 0x0d, 0, 0, 0, 21, 0, 1];
        w.extend([0; 21]);
        assert!(matches!(hang(&w), Hang::Layout(_, "buffer longer than the chip's", _)));
        w.truncate(14 + 20);
        w[11] = 20;
        assert!(check(&w).is_ok());
        // the sec_params_reply of the 9-slot kind is the model's, not this table's: the table only checks layouts
        // an ANT acknowledged message: the size byte is the bytes left (the SDK's bound is not enforced)
        assert!(check(&[6, 0xc8, 0, 3, 1, 2, 3]).is_ok());
        assert!(matches!(hang(&[6, 0xc8, 0, 3, 1, 2]), Hang::Layout(_, "cut short", _)));
        // an unknown cfg id
        assert!(matches!(hang(&[0, 0x69, 0x77, 0, 0, 0, 1, 0]), Hang::Layout(_, "cfg id not modelled", _)));
    }

    #[test]
    fn the_layouts_accept_what_stock_and_the_unit_use() {
        // sd_ble_cfg_set ATT MTU: exactly 9 bytes after nothing but the id (Bryton's app, checked)
        assert!(check(&[0, 0x69, 0x23, 0, 0, 0, 1, 1, 103, 0]).is_ok());
        assert!(check(&[0, 0x69, 0x23, 0, 0, 0, 1, 1, 103]).is_err());
        // the fast link's configurations (nrf::CFG_FAST): the queue size (ids and layouts inferred, decoded by the app)
        assert!(check(&[0, 0x69, 0x22, 0, 0, 0, 1, 1, 4]).is_ok());
        assert!(check(&[0, 0x69, 0x22, 0, 0, 0, 1, 1]).is_err());
        for c in crate::nrf::CFG_FAST {
            assert!(check(c).is_ok(), "{c:?}");
        }
        // the ANT enable as stock sends it
        assert!(check(&[6, 0xfa, 1, 0x0f, 0, 0x20, 0, 0x7c, 0x16, 0x06, 0x24, 0xfc, 0x05]).is_ok());
    }

    #[test]
    fn responses_and_events_follow_the_table() {
        assert!(response_ok(0, &[1, 0x65, 0, 0, 0, 0, 4, 1, 0, 0x32, 0]).is_ok());
        assert!(response_ok(0, &[1, 0x65, 0, 0, 0, 0, 4]).is_err());
        assert!(response_ok(0, &[1, 0x65, 1, 0x30, 0, 0]).is_ok(), "an error code has no outputs");
        assert!(event_ok(&[8; 44]).is_ok() && event_ok(&[8; 43]).is_err());
        assert!(event_ok(&[2; 400]).is_err());
        let mut x = 7u32;
        let mut r = || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        for e in EVENTS {
            let p = build_event(e, &mut r);
            assert!(event_ok(&p).is_ok(), "{}", e.name);
        }
    }
}
