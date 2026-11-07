//! Every command rustfw can send, built with representative and boundary arguments, against the Nordic chip's table
//! (chip_table.rs): the simulated chip hangs, and panics, on any it could not decode. Plus a test per fatal rule that
//! the model hangs, with its reason.

use super::*;
use crate::{ble, nrf};
use chip_table::{CMDS, Hang, Stock};

/// A simulated chip that only checks the commands against the table.
fn table_sim() -> Sim {
    let mut s = Sim::new(8, 0, 0);
    s.nordic.table_only = true;
    s
}

#[test]
fn every_command_rustfw_builds_decodes_on_the_chip() {
    let mut sim = table_sim();
    let mut rx = nrf::Rx::new();
    let b = &mut sim;
    let rx = &mut rx;
    // start: both configurations, enable, version; the scan
    nrf::start(b).expect("start");
    let _ = nrf::scan_start(b, rx, 0);
    let _ = nrf::scan_start(b, rx, u16::MAX);
    // ANT: keys on both networks, eight channels, every one opened for any sensor and for a given one, closed, asked
    let nets = [0, 0, 0, 1, 1, 0, 0, 0];
    let _ = nrf::ant_start(b, rx, [1; 8], Some([2; 8]), &nets);
    let id = nrf::SensorId { device: 0xffff, trans: 0xff };
    for ch in 0..8 {
        for id in [None, Some(id)] {
            let _ = nrf::ant_open(b, rx, ch, 0x78, u16::MAX, id);
        }
        let _ = nrf::ant_ack(b, rx, ch, [0xff; 8]);
        let _ = nrf::ant_channel_id(b, rx, ch);
        let _ = nrf::ant_close(b, rx, ch);
    }
    // BLE peripheral: the longest name and data, every optional part
    let name = [b'n'; 31];
    for n in [&name[..], &[], b"S510"] {
        let _ = ble::device_name_set(b, rx, n);
    }
    let _ = ble::appearance_set(b, rx, 0x03c4);
    let _ = ble::ppcp_set(b, rx, ble::CENTRAL_PARAMS);
    let _ = ble::adv_data_set(b, rx, &[7; 31], &[8; 31]);
    let _ = ble::adv_data_set(b, rx, &[7; 3], &[]);
    for tag in [0, ble::TAG] {
        let _ = ble::adv_start(b, rx, 0x0640, 0, tag);
    }
    let _ = ble::adv_stop(b, rx);
    let _ = ble::scan_stop(b, rx);
    let _ = ble::connect(b, rx, &[3, 1, 2, 3, 4, 5, 6]);
    let _ = ble::connect_cancel(b, rx);
    for conn in [0, 1, 0xffff] {
        let _ = ble::disconnect(b, rx, conn);
        let _ = ble::authenticate(b, rx, conn);
        for ok in [true, false] {
            let _ = ble::sec_params_reply(b, rx, conn, ok);
        }
        let _ = ble::sec_info_reply(b, rx, conn, Some(([9; 16], 0x40)));
        let _ = ble::sec_info_reply(b, rx, conn, None);
        let _ = ble::conn_param_update(b, rx, conn, (6, 3200, 499, 3200));
        let _ = ble::phy_update(b, rx, conn);
        let _ = ble::data_length_update(b, rx, conn);
        let _ = ble::user_mem_reply(b, rx, conn);
        let _ = ble::exchange_mtu_reply(b, rx, conn, ble::ATT_MTU);
        let _ = ble::sys_attr_set(b, rx, conn, None);
        let _ = ble::sys_attr_set(b, rx, conn, Some(&[5; 80]));
        let _ = ble::sys_attr_get(b, rx, conn, &mut [0; 80]);
        // GATT client: the longest write the chip takes
        let _ = ble::discover_service(b, rx, conn, (0x1812, ble::UUID_BLE));
        let _ = ble::discover_chars(b, rx, conn, 1, 0xffff);
        let _ = ble::discover_descs(b, rx, conn, 1, 0xffff);
        let _ = ble::read(b, rx, conn, 0xffff);
        let _ = ble::hv_confirm(b, rx, conn, 0xffff);
        for n in [0, 1, 2, 19, ble::WRITE_MAX] {
            let _ = ble::write(b, rx, conn, 0x0d, &[0x55; ble::WRITE_MAX][..n]);
        }
        let _ = ble::notify(b, rx, conn, 0x0e, &[1; 100]);
        let _ = ble::notify(b, rx, conn, 0x0e, &[]);
    }
    // GATT server tables: with and without notify, empty and long values, growing ones
    let svc = ble::service_add(b, rx, 0x1812).unwrap_or(1);
    let _ = ble::uuid_vs_add(b, rx, &[1; 16]);
    for (props, value, max) in [
        (ble::READ, &[0u8; 0][..], 0),
        (ble::READ | ble::NOTIFY, &[1u8][..], 20),
        (ble::INDICATE, &[1; 100][..], 100),
        (ble::WRITE_CMD, &[1][..], 1),
    ] {
        let ch = ble::Char { uuid: 0x2a4d, props, value, max, read: ble::ENCRYPTED, write: ble::OPEN };
        let _ = ble::characteristic_add(b, rx, svc, &ch);
    }
    let _ = ble::descriptor_add(b, rx, 2, 0x2908, &[0, 1], ble::ENCRYPTED);
    let _ = ble::value_set(b, rx, 3, &[7; 100]);
    nrf::system_off(b, &mut nrf::Rx::new(), true);
    // every modelled row was exercised
    let sent: Vec<(u8, u8)> = b.nordic.got.iter().map(|p| (p[0], p[1])).collect();
    let missing: Vec<&str> = CMDS.iter().filter(|c| !sent.contains(&(c.kind, c.op))).map(|c| c.name).collect();
    assert!(missing.is_empty(), "rows never sent: {missing:?}");
    assert!(b.nordic.hang_reason.is_none());
}

/// A chip that expects a hang whose reason contains `want`, sent `payload` framed (the simulator panics otherwise).
fn hangs_on(payload: &[u8], want: &'static str) {
    let mut s = Sim::new(8, 0, 0);
    s.nordic.expect_hang = Some(want);
    s.nrf_reset();
    let _ = nrf::send(&mut s, &mut nrf::Rx::new(), payload);
    assert!(s.nordic.hung, "{payload:02x?} did not hang the chip");
}

#[test]
fn the_simulated_chip_hangs_as_the_real_one_does() {
    hangs_on(&[0, 0x61], "UnknownOpcode(0, 97)");
    hangs_on(&[2, 0x60], "Type(2)");
    hangs_on(&[0], "TypeOnly");
    hangs_on(&[0, 0x6c], "NotModelled");
    hangs_on(&[0, 0x60, 0], "bytes left over"); // one byte too many
    hangs_on(&[0, 0x78, 1], "cut short"); // one byte too few
    hangs_on(&[0, 0x9e, 0, 0, 1, 0, 0], "NotModelled"); // present, not modelled
    let mut long = vec![0, 0x9c, 0, 0, 1, 1, 0, 0x0d, 0, 0, 0, 21, 0, 1];
    long.extend([0; 21]);
    hangs_on(&long, "buffer longer than the chip's"); // a write over 20 bytes
    // a packet of 0 or over 384 bytes: the length header alone
    let mut s = Sim::new(8, 0, 0);
    s.nordic.expect_hang = Some("Length(0)");
    s.nrf_reset();
    s.nrf_send(&[0, 0]);
    assert_eq!(s.nordic.hang_reason.as_deref(), Some("Length(0) on []"));
    let mut s = Sim::new(8, 0, 0);
    s.nordic.expect_hang = Some("Length(385)");
    s.nrf_reset();
    s.nrf_send(&385u16.to_le_bytes());
    assert!(s.nordic.hung);
    // and a hung chip says nothing more
    let mut out = [0; 4];
    assert_eq!(s.nrf_recv(&mut out), 0);
    assert!(matches!(chip_table::check(&[0, 0x61]), Err(Hang::UnknownOpcode(..))));
}

#[test]
#[should_panic(expected = "the Nordic chip hangs on a command it cannot decode")]
fn a_hang_in_a_test_that_does_not_expect_one_fails_loudly() {
    let mut s = Sim::new(8, 0, 0);
    s.nrf_reset();
    let _ = nrf::send(&mut s, &mut nrf::Rx::new(), &[0, 0x61]);
}

#[test]
fn a_long_app_identifier_is_not_sent_to_the_chip() {
    // Get App Attributes for "com.apple.MobileSMS" is 22 bytes: the chip's write buffer holds 20, and it hangs
    let mut s = Sim::new(8, 0, 0);
    nrf::start(&mut s).expect("start");
    let mut rx = nrf::Rx::new();
    let mut app = [0; 28];
    let n = crate::phone::get_app("com.apple.MobileSMS", &mut app);
    assert_eq!(n, 22);
    let sent = s.nordic.got.len();
    assert_eq!(ble::write(&mut s, &mut rx, 0, 0x0d, &app[..n]), Err(nrf::Error::Code(12)));
    assert_eq!(s.nordic.got.len(), sent, "nothing went to the chip");
}

/// The commands stock never sends (`Stock::Never` rows), proven on Bryton's real app by `tools/nrf_emu.py selftest`:
/// its NEVER list replays the bytes of [`NEVER_BYTES`], and the app answers each with `01 <op> <result>`, no hang.
/// Keep the two lists in step.
const EMULATED: &[u8] = &[0x74, 0x78, 0x8d, 0x9d, 0xa3, 0xa4];

/// The `Stock::Never` commands as rustfw's builders send them, in hex (what the selftest replays).
const NEVER_BYTES: &[&str] = &[
    "0074",
    "0078c403",
    "008d",
    "009d00000d00",
    "00a3020001010829010121000200000200020001000101",
    "00a4ffff0300010000030001070707",
];

#[test]
fn every_command_stock_never_sends_is_proven() {
    // a dated "seen on the unit" line in docs/status.md (its opcode in hex) proves one too
    let seen = include_str!("../../docs/status.md").to_lowercase();
    let unproven: Vec<&str> = CMDS
        .iter()
        .filter(|c| c.stock == Stock::Never)
        .filter(|c| !EMULATED.contains(&c.op))
        .filter(|c| !seen.lines().any(|l| l.contains(&format!("0x{:02x}", c.op)) && l.contains("2026-")))
        .map(|c| c.name)
        .collect();
    assert!(unproven.is_empty(), "stock never sends these, and nothing proves the chip answers them: {unproven:?}");
}

#[test]
fn the_never_commands_go_out_as_the_selftest_replays_them() {
    let mut sim = table_sim();
    let mut rx = nrf::Rx::new();
    let b = &mut sim;
    let rx = &mut rx;
    b.nrf_reset();
    let _ = ble::adv_stop(b, rx);
    let _ = ble::appearance_set(b, rx, 0x03c4);
    let _ = ble::connect_cancel(b, rx);
    let _ = ble::hv_confirm(b, rx, 0, 0x000d);
    let _ = ble::descriptor_add(b, rx, 2, 0x2908, &[0, 1], ble::ENCRYPTED);
    let _ = ble::value_set(b, rx, 3, &[7; 3]);
    let hex: Vec<std::string::String> =
        b.nordic.got.iter().map(|p| p.iter().map(|x| format!("{x:02x}")).collect()).collect();
    let want: Vec<std::string::String> = NEVER_BYTES.iter().map(|s| (*s).into()).collect();
    assert_eq!(hex, want, "the bytes tools/nrf_emu.py selftest must replay");
}
