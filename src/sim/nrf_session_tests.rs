//! The fast link's whole runtime sequence against Bryton's real radio-chip app (`nrf_emu_tests.rs`'s bridge, 2026-11-07).
//! Layouts: Nordic's pc-ble-driver sd_api_v5 headers (github.com/NordicSemiconductor/pc-ble-driver, `include/sd_api_v5/`
//! ble_gap.h, ble_gatts.h, ble_gattc.h: the SVC enums from their bases, the argument structures, the event unions).
//! What this settles is the chip APP's layer: does it know each command, which SoftDevice call does it make with which
//! arguments, does it encode each event to the host. The real SoftDevice's acceptance (its RAM for the configuration, the
//! 2M PHY, the queue of 4) still needs the unit. `#[ignore]`d like its bridge: `cargo test-host -- --ignored nrf_emu`.

use super::nrf_emu_tests::{Pipe, emu_sim};
use super::*;
use crate::ble::Event;
use crate::nrf::Rx;
use std::{string::String, vec, vec::Vec};

impl Pipe {
    /// The BLE calls the app made so far, one decoded line each (`Emu.describe`), then forgotten.
    fn calls(&mut self) -> Vec<String> {
        let t = self.req(b"S", true);
        String::from_utf8_lossy(&t).lines().map(String::from).collect()
    }

    /// How many BLE calls the app made so far (a long run's lines would not fit one answer), then forgotten.
    fn call_count(&mut self) -> usize {
        let t = self.req(b"C", true);
        t.first_chunk().map_or(0, |b| u32::from_le_bytes(*b) as usize)
    }

    /// One raw SoftDevice event handed to the app.
    fn inject(&mut self, raw: &[u8]) {
        self.req(&[b"E".as_slice(), &u16::try_from(raw.len()).unwrap().to_le_bytes(), raw].concat(), false);
    }
}

/// The connection handle of the scripted phone.
const CONN: u16 = 0x0123;

fn le16(v: u16) -> [u8; 2] {
    v.to_le_bytes()
}

/// `ble_evt_t`: `evt_id`, `evt_len` (it counts these 4 bytes; the app reads a shorter one as zeros), the body.
fn raw_evt(id: u16, body: &[u8]) -> Vec<u8> {
    [&le16(id)[..], &le16(body.len() as u16 + 4), body].concat()
}

/// A GAP event: `conn_handle`, then the parameters at +4 (the union holds pointers, so it is 4-byte aligned).
fn gap_evt(id: u16, params: &[u8]) -> Vec<u8> {
    raw_evt(id, &[&le16(CONN)[..], &[0, 0], params].concat())
}

/// A GATTS event: `conn_handle`, then the parameters.
fn gatts_evt(id: u16, params: &[u8]) -> Vec<u8> {
    raw_evt(id, &[&le16(CONN)[..], params].concat())
}

/// The BLE packets the app has sent, after `ms` of emulated time.
fn packets(sim: &mut Sim, ms: u32) -> Vec<Vec<u8>> {
    sim.delay_ms(ms);
    let (mut rx, mut got, mut buf) = (Rx::new(), Vec::new(), [0u8; 64]);
    loop {
        let n = sim.nrf_recv(&mut buf);
        if n == 0 {
            return got;
        }
        for &b in &buf[..n] {
            if let Some(p) = rx.push(b) {
                got.push(p.to_vec());
            }
        }
    }
}

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| std::format!("{b:02x}")).collect()
}

/// 244 bytes that differ from one notification to the next.
fn value(k: u8) -> Vec<u8> {
    (0..244u32).map(|i| (i as u8).wrapping_mul(7).wrapping_add(k)).collect()
}

/// A started unit on the fast link's configuration.
fn fast_start() -> Option<(super::nrf_emu_tests::Shared, Sim)> {
    let (pipe, mut sim) = emu_sim()?;
    sim.fast_link = true;
    nrf::start(&mut sim).expect("start");
    Some((pipe, sim))
}

#[test]
#[ignore = "needs python3 + unicorn + Bryton's nordic.bin; cargo test-host -- --ignored nrf_emu"]
fn nrf_emu_the_fast_links_calls_reach_the_softdevice_as_meant() {
    let Some((pipe, mut sim)) = emu_sim() else { return };
    sim.fast_link = true;
    nrf::start(&mut sim).expect("start");
    let want = [
        "softdevice_enable",
        "cfg_set id=0x20 tag=1 conn_count=2 event_length=3",
        "cfg_set id=0x40 periph=1 central=1 central_sec=0",
        "cfg_set id=0x23 tag=1 att_mtu=247",
        "cfg_set id=0x22 tag=1 hvn_tx_queue_size=4",
        // the RAM base the app hands the SoftDevice: its own link's start of RAM (the SoftDevice owns the bytes below)
        "enable ram_base=0x20006800",
        "version_get",
    ];
    assert_eq!(pipe.0.borrow_mut().calls(), want, "the start");

    // a session: the exchange, the bond's attributes, the boost, four notifications at the queue, the restore
    let mut rx = Rx::new();
    let sys = value(9)[..64].to_vec();
    ble::exchange_mtu_reply(&mut sim, &mut rx, CONN, nrf::FAST_MTU).expect("mtu reply");
    ble::sys_attr_set(&mut sim, &mut rx, CONN, Some(&sys)).expect("sys attr");
    ble::conn_param_update(&mut sim, &mut rx, CONN, (12, 24, 0, 400)).expect("params");
    ble::data_length_set(&mut sim, &mut rx, CONN, 251).expect("data length");
    ble::phy_set(&mut sim, &mut rx, CONN, 2).expect("phy");
    for k in 0..4 {
        ble::notify(&mut sim, &mut rx, CONN, 0x2a, &value(k)).expect("notify");
    }
    ble::conn_param_update(&mut sim, &mut rx, CONN, (24, 48, 0, 400)).expect("restore params");
    ble::phy_set(&mut sim, &mut rx, CONN, 1).expect("restore phy");
    let c = CONN;
    let mut want = vec![
        std::format!("exchange_mtu_reply conn={c} mtu=247"),
        std::format!("sys_attr_set conn={c} data=set len=64 flags=0x0"),
        std::format!("conn_param_update conn={c} params=12,24,0,400"),
        std::format!("data_length_update conn={c} params=251,251,0,0 limitation=null"),
        std::format!("phy_update conn={c} phys=2,2"),
    ];
    want.extend(
        (0..4).map(|k| std::format!("hvx conn={c} handle=0x2a type=1 offset=0 len=244 data={}", hex(&value(k)))),
    );
    want.push(std::format!("conn_param_update conn={c} params=24,48,0,400"));
    want.push(std::format!("phy_update conn={c} phys=1,1"));
    assert_eq!(pipe.0.borrow_mut().calls(), want);
    assert_eq!(pipe.0.borrow().flags & 4, 0, "the app is alive");
}

/// A `sd_ble_gatts_hvx` command whose value is `n` bytes, framed (the length header, then the payload of 14 + n).
fn hvx_frame(n: usize) -> Vec<u8> {
    let v: Vec<u8> = (0..n).map(|i| (i * 7) as u8).collect();
    let p = [&[nrf::CMD, 0xa6][..], &le16(CONN), &[1], &le16(0x2a), &[1], &le16(0), &[1], &le16(n as u16), &[1], &v]
        .concat();
    [&le16(p.len() as u16)[..], &p].concat()
}

#[test]
#[ignore = "needs python3 + unicorn + Bryton's nordic.bin; cargo test-host -- --ignored nrf_emu"]
fn nrf_emu_takes_a_command_of_384_bytes_and_hangs_on_385() {
    let Some((pipe, mut sim)) = fast_start() else { return };
    pipe.0.borrow_mut().calls();
    // four notifications of the fast link's largest (244 + 14 + 2 bytes each) back to back: all taken, in order
    let four: Vec<u8> = (0..4).flat_map(|_| hvx_frame(244)).collect();
    sim.nrf_send(&four);
    let answers = packets(&mut sim, 30);
    assert_eq!(answers.iter().filter(|p| p[..2] == [nrf::RESP, 0xa6]).count(), 4, "four answers");
    assert_eq!(pipe.0.borrow_mut().calls().len(), 4);
    // the app's receive buffer: a payload of 384 bytes is the most it takes (nrf.rs MAX_CMD is the same number)
    sim.nrf_send(&hvx_frame(384 - 14));
    let answers = packets(&mut sim, 30);
    assert!(answers.iter().any(|p| p[..2] == [nrf::RESP, 0xa6]), "384 bytes taken");
    assert_eq!(pipe.0.borrow_mut().calls().len(), 1);
    // one byte more and the app hangs
    pipe.0.borrow_mut().soft = true;
    sim.nrf_send(&hvx_frame(385 - 14));
    packets(&mut sim, 100);
    assert!(pipe.0.borrow().hung.is_some(), "385 bytes hang the app");
}

#[test]
#[ignore = "needs python3 + unicorn + Bryton's nordic.bin; cargo test-host -- --ignored nrf_emu"]
fn nrf_emu_encodes_the_events_the_fast_link_relies_on() {
    let Some((pipe, mut sim)) = fast_start() else { return };
    let big = value(3);
    let hvx = raw_evt(0x39, &[&le16(CONN)[..], &le16(0), &le16(0), &le16(0x2a), &[1, 0], &le16(244), &big].concat());
    let cases: [(&str, Vec<u8>, Event); 9] = [
        ("MTU request", gatts_evt(0x55, &le16(247)), Event::MtuRequest { conn: CONN, client: 247 }),
        ("HVN TX complete", gatts_evt(0x57, &[4]), Event::TxComplete { conn: CONN, count: 4 }),
        (
            "connection parameter update",
            gap_evt(0x12, &[le16(12), le16(24), le16(0), le16(400)].concat()),
            Event::ConnParamUpdate { conn: CONN, params: (12, 24, 0, 400) },
        ),
        (
            "connection parameter request",
            gap_evt(0x1f, &[le16(24), le16(48), le16(0), le16(400)].concat()),
            Event::ConnParamRequest { conn: CONN, params: (24, 48, 0, 400) },
        ),
        ("PHY update request", gap_evt(0x21, &[2, 2]), Event::ParamRequest { conn: CONN, id: 0x21 }),
        (
            "data length update request",
            gap_evt(0x23, &[le16(251), le16(251), le16(0), le16(0)].concat()),
            Event::ParamRequest { conn: CONN, id: 0x23 },
        ),
        ("PHY update (not decoded)", gap_evt(0x22, &[0, 2, 2]), Event::Other),
        (
            "data length update (not decoded)",
            gap_evt(0x24, &[le16(251), le16(251), le16(2120), le16(2120)].concat()),
            Event::Other,
        ),
        ("ANCS notification of 244 bytes", hvx, Event::Hvx { conn: CONN, handle: 0x2a, kind: 1, data: &big }),
    ];
    for (name, raw, want) in cases {
        pipe.0.borrow_mut().inject(&raw);
        let got = packets(&mut sim, 20);
        assert_eq!(got.len(), 1, "{name}: one packet");
        let p = &got[0];
        assert!(p.len() <= nrf::KEPT, "{name}: {} bytes fit the host's buffer", p.len());
        assert_eq!(ble::event(p), Some(want), "{name}: {}", hex(p));
        assert_eq!(pipe.0.borrow().flags & 4, 0, "{name}: the app is alive");
    }
    // a TX-complete carries its count (1 to the queue's 4, and more)
    for count in [1u8, 4, 255] {
        pipe.0.borrow_mut().inject(&gatts_evt(0x57, &[count]));
        assert_eq!(ble::event(&packets(&mut sim, 20)[0]), Some(Event::TxComplete { conn: CONN, count }));
    }
}

#[test]
#[ignore = "needs python3 + unicorn + Bryton's nordic.bin; cargo test-host -- --ignored nrf_emu"]
fn nrf_emu_the_phone_setup_uses_the_ten_vendor_bases_the_softdevice_holds() {
    let Some((pipe, mut sim)) = emu_sim() else { return };
    sim.fast_link = true;
    let mut app = App::start(&mut sim, None);
    for _ in 0..400 {
        app.step(&mut sim);
        sim.sleep_ms(10);
    }
    let calls = pipe.0.borrow_mut().calls();
    let bases = calls.iter().filter(|l| *l == "uuid_vs_add").count();
    std::println!("{} calls, {bases} vendor bases, ble {:?}", calls.len(), app.ble);
    assert!(app.ble.is_some_and(|v| v.mtu == nrf::FAST_MTU), "the app started the fast link");
    assert!(app.phone.files.handles.is_some(), "the file service was added (the 10th base)");
    assert_eq!(bases, 10, "8 of the phone's, FE-C's, the file service's: the SoftDevice's default holds 10");
    assert_eq!(pipe.0.borrow().flags & 4, 0, "the app is alive");
}

/// Emulated seconds to notify `total` bytes in frames of 244 through the real app, `per_pass` commands then a sleep of
/// `pass_ms` (the READ's loop: app/filesvc.rs PER_PASS, FAST_POLL_MS).
fn notify_all(total: usize, per_pass: usize, pass_ms: u32) -> f64 {
    let Some((pipe, mut sim)) = fast_start() else { return 0.0 };
    let (mut rx, v) = (Rx::new(), value(1));
    pipe.0.borrow_mut().call_count(); // the start's
    let t0 = pipe.0.borrow().us;
    let (mut sent, mut n) = (0, 0);
    while sent < total {
        for _ in 0..per_pass.min((total - sent).div_ceil(v.len())) {
            ble::notify(&mut sim, &mut rx, CONN, 0x2a, &v).expect("notify");
            (sent, n) = (sent + v.len(), n + 1);
        }
        sim.sleep_ms(pass_ms);
    }
    assert_eq!(pipe.0.borrow().flags & 4, 0, "the app is alive");
    assert_eq!(pipe.0.borrow_mut().call_count(), n, "one hvx call for each notification");
    (pipe.0.borrow().us - t0) as f64 / 1e6
}

#[test]
#[ignore = "needs python3 + unicorn + Bryton's nordic.bin; cargo test-host -- --ignored nrf_emu"]
fn nrf_emu_the_uart_path_of_a_400_kb_read() {
    let s = notify_all(400_000, 8, 10);
    let rate = 400_000.0 / s.max(1e-9);
    std::println!("400 KB through the real app, 8 notifications per 10 ms pass: {s:.2} s = {rate:.0} B/s");
    let s2 = notify_all(100_000, 1000, 0);
    std::println!("back to back, no sleeps: {s2:.2} s = {:.0} B/s", 100_000.0 / s2.max(1e-9));
    // the simulator's chip (34 KB/s at a READ's connection interval and event length) is the slower: the UART and the
    // app's own work are not what limits a download
    assert!(s == 0.0 || rate > 34_000.0, "{rate} B/s");
}
