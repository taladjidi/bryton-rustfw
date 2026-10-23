//! A simulated S510 for host tests. It implements [`Board`] with:
//! - a model of the ST7789: it checks the command bytes (power, sleep, pixel format, RAMCTRL, windows) and
//!   paints its framebuffer from RAMWR as the real panel would;
//! - a clock driven by the delays, so the RTC advances as the app waits;
//! - keys scripted as a function of time;
//! - a Nordic chip ([`Nordic`]) that answers the commands the way the real one did on the unit (BOARD.md 11).
//!
//! Screenshots: [`Sim::save`] writes the screen as a BMP. [`Sim::record`] saves one frame after every step that
//! changed the screen (target/sim/<name>/NNNNN.bmp), which gives a continuous capture for debugging.

extern crate std;

use crate::panel::*;
use crate::ui::{H, W};
use crate::*;
use std::boxed::Box;
use std::format;
use std::path::PathBuf;
use std::vec;
use std::vec::Vec;

type KeyScript = Box<dyn Fn(u64) -> (bool, bool)>;

pub struct Sim {
    pub screen: Vec<u16>,
    window: (u32, u32, u32, u32),
    /// Every panel command, in order.
    pub cmds: Vec<u8>,
    pub powered: bool,
    pub asleep: bool,
    pub display_on: bool,
    colmod: u8,
    ramctrl: [u8; 2],
    pub backlight: bool,
    pub standby: bool,
    /// Simulated time since reset.
    pub ms: u64,
    start_s: u64,
    keys: KeyScript,
    record: Option<(PathBuf, u32)>,
    dirty: bool,
    pub nordic: Nordic,
    /// RTC backup registers (they survive an app restart, as on the unit).
    pub backup: [u32; 32],
}

/// The Nordic chip as seen over its link: replies to `sd_ble_enable` and `sd_ble_version_get` as the unit did.
#[derive(Default)]
pub struct Nordic {
    /// False: the chip never answers (not running).
    pub dead: bool,
    /// Reset since power-up: before that, commands would go nowhere.
    pub reset: bool,
    pub enabled: bool,
    /// Simulated time (ms) at which the running scan times out.
    scan_end: Option<u64>,
    /// A heart-rate strap in range sending this rate; it is heard once the HR channel is open for it.
    pub strap: Option<u8>,
    /// That strap's device number (the unit's strap: 25126, transmission type 0xf1).
    pub strap_id: u16,
    /// The device number the HR channel was opened for (0: any).
    channel_dev: u16,
    /// Simulated time of the strap's next broadcast, once the HR channel is open.
    next_hr: Option<u64>,
    /// Every payload received, in order.
    pub got: Vec<Vec<u8>>,
    rx: crate::nrf::Rx,
    out: std::collections::VecDeque<u8>,
}

/// The advertisers the simulated chip hears: two devices, one of them named, one report repeated.
pub const ADVERTISERS: [(u8, i8, &[u8]); 3] = [
    (0xa1, -40, &[2, 0x01, 0x06, 5, 0x09, b'B', b'i', b'k', b'e']),
    (0xb2, -70, &[2, 0x01, 0x06, 5, 0xff, 0x4c, 0, 0x10, 6]),
    (0xa1, -42, &[]),
];

/// A BLE_GAP_EVT_ADV_REPORT payload as the chip encodes it (BOARD.md 11).
fn adv_report(addr: u8, rssi: i8, data: &[u8]) -> Vec<u8> {
    let mut p = vec![2, 0x1d, 0, 0xff, 0xff, 0, addr, 1, 2, 3, 4, 5];
    p.extend([0; 7]);
    p.extend([rssi as u8, 0, data.len() as u8, 1]);
    p.extend(data);
    p
}

impl Nordic {
    /// The strap is in range and the channel is open for it.
    fn heard(&self) -> bool {
        self.strap.is_some() && (self.channel_dev == 0 || self.channel_dev == self.strap_id)
    }

    fn queue(&mut self, payload: &[u8]) {
        if !self.dead {
            self.out.extend((payload.len() as u16).to_le_bytes());
            self.out.extend(payload);
        }
    }

    fn take(&mut self, p: &[u8], now: u64) {
        self.got.push(p.to_vec());
        if p == [0, 0x32] {
            return; // sd_power_system_off: no response
        }
        if let [0, 0x8a, 1, _, _, _, _, _, t0, t1] = *p {
            self.queue(&[1, 0x8a, 0, 0, 0, 0]);
            for (addr, rssi, data) in ADVERTISERS {
                self.queue(&adv_report(addr, rssi, data));
            }
            self.scan_end = Some(now + 1000 * u64::from(u16::from_le_bytes([t0, t1])));
            return;
        }
        if let [6, op, rest @ ..] = p {
            match (*op, rest) {
                (0xc4, [0, ..]) => self.next_hr = Some(now), // channel 0 open
                (0xd3, [0, d0, d1, ..]) => self.channel_dev = u16::from_le_bytes([*d0, *d1]),
                (0xd4, [0]) => {
                    let found = self.heard() && self.next_hr.is_some_and(|t| t > now.saturating_sub(1000));
                    let [d0, d1] = if found { self.strap_id } else { self.channel_dev }.to_le_bytes();
                    self.queue(&[1, 0xd4, 0, 0, 0, 0, d0, d1, 0x78, if found { 0xf1 } else { 0 }]);
                    return;
                }
                _ => {}
            }
            self.queue(&[1, *op, 0, 0, 0, 0]); // ANT commands answer as RESP (seen on the unit)
            return;
        }
        let reply: Vec<u8> = match p {
            [0, 0x60] => {
                self.enabled = true;
                vec![1, 0x60, 0, 0, 0, 0]
            }
            [0, 0x65, 1] if !self.enabled => vec![1, 0x65, 0x01, 0x30, 0, 0], // BLE_ERROR_NOT_ENABLED
            [0, 0x65, 1] => vec![1, 0x65, 0, 0, 0, 0, 4, 1, 0, 0x32, 0],
            [0, op, ..] => vec![1, *op, 6, 0, 0, 0], // NRF_ERROR_NOT_SUPPORTED
            _ => return,
        };
        self.queue(&reply);
    }
}

fn to_bcd(v: u64) -> u32 {
    ((v / 10) << 4 | v % 10) as u32
}

impl Sim {
    /// A unit whose RTC reads `h:m:s` at reset, no key pressed.
    pub fn new(h: u64, m: u64, s: u64) -> Self {
        Sim {
            screen: vec![0x1234; (W * H) as usize], // panel RAM is garbage until cleared
            window: (0, W - 1, 0, H - 1),
            cmds: Vec::new(),
            powered: false,
            asleep: true,
            display_on: false,
            colmod: 0x06,
            ramctrl: [0x10, 0xf0],
            backlight: false,
            standby: false,
            ms: 0,
            start_s: h * 3600 + m * 60 + s,
            keys: Box::new(|_| (false, false)),
            record: None,
            dirty: false,
            nordic: Nordic { strap_id: 25126, ..Nordic::default() },
            backup: [0; 32],
        }
    }

    /// Keys as a function of the simulated time in ms: (page, back).
    pub fn keys(mut self, f: impl Fn(u64) -> (bool, bool) + 'static) -> Self {
        self.keys = Box::new(f);
        self
    }

    /// What a person would see: powered, awake, display on, backlight on.
    pub fn visible(&self) -> bool {
        self.powered && !self.asleep && self.display_on && self.backlight
    }

    /// Saves a frame after every step that changed the screen, into target/sim/`name`/.
    pub fn record(mut self, name: &str) -> Self {
        let dir = out_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        self.record = Some((dir, 0));
        self
    }

    /// Writes the screen to target/sim/`name`.bmp.
    pub fn save(&self, name: &str) {
        std::fs::create_dir_all(out_dir()).unwrap();
        std::fs::write(out_dir().join(format!("{name}.bmp")), bmp(&self.screen)).unwrap();
    }
}

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/sim")
}

/// RGB565 screen as a 24-bit BMP (bottom-up rows; 240 * 3 bytes is already a multiple of 4).
pub fn bmp(px: &[u16]) -> Vec<u8> {
    let mut out = b"BM".to_vec();
    for v in [54 + W * H * 3, 0, 54, 40, W, H] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&[1, 0, 24, 0]); // planes, bits per pixel
    out.extend_from_slice(&[0; 24]);
    for y in (0..H).rev() {
        for x in 0..W {
            let p = px[(y * W + x) as usize] as u32;
            out.extend_from_slice(&[((p & 0x1f) << 3) as u8, (((p >> 5) & 0x3f) << 2) as u8, ((p >> 11) << 3) as u8]);
        }
    }
    out
}

fn span(p: &[u8], limit: u32) -> (u32, u32) {
    assert_eq!(p.len(), 4, "window needs 4 parameter bytes");
    let (a, b) = (u32::from(p[0]) << 8 | u32::from(p[1]), u32::from(p[2]) << 8 | u32::from(p[3]));
    assert!(a <= b && b < limit, "window {a}..{b} outside 0..{limit}");
    (a, b)
}

impl Board for Sim {
    fn panel_power_on(&mut self) {
        self.powered = true;
        self.ms += 150;
    }

    fn panel_cmd(&mut self, cmd: u8, params: &[u8]) {
        assert!(self.powered, "command 0x{cmd:02x} to an unpowered panel");
        self.cmds.push(cmd);
        match cmd {
            SWRESET => (self.asleep, self.display_on) = (true, false),
            SLPIN => self.asleep = true,
            SLPOUT => self.asleep = false,
            DISPON => self.display_on = true,
            DISPOFF => self.display_on = false,
            COLMOD => self.colmod = params[0],
            RAMCTRL => self.ramctrl = [params[0], params[1]],
            CASET => (self.window.0, self.window.1) = span(params, W),
            RASET => (self.window.2, self.window.3) = span(params, H),
            RAMWR => panic!("RAMWR goes through panel_ram_write"),
            _ => {}
        }
    }

    fn panel_ram_write(&mut self, count: u32, next: &mut dyn FnMut() -> u16) {
        assert_eq!(self.ramctrl[0] & 0x03, 0, "RAMCTRL routes RAM writes to the RGB interface, not SPI");
        assert_eq!(self.colmod & 0x07, 0x05, "pixel format is not 16-bit");
        let (x0, x1, y0, y1) = self.window;
        assert_eq!(count, (x1 - x0 + 1) * (y1 - y0 + 1), "pixel count does not fill the window");
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.screen[(y * W + x) as usize] = next();
            }
        }
        self.dirty = true;
    }

    fn backlight(&mut self, on: bool) {
        self.backlight = on;
    }

    fn keys_down(&mut self) -> (bool, bool) {
        (self.keys)(self.ms)
    }

    fn rtc_tr(&mut self) -> u32 {
        let t = (self.start_s + self.ms / 1000) % 86400;
        to_bcd(t / 3600) << 16 | to_bcd(t / 60 % 60) << 8 | to_bcd(t % 60)
    }

    fn delay_ms(&mut self, ms: u32) {
        self.ms += u64::from(ms);
        if let Some((dir, n)) = &mut self.record
            && self.dirty
        {
            std::fs::write(dir.join(format!("{:05}.bmp", *n)), bmp(&self.screen)).unwrap();
            *n += 1;
            self.dirty = false;
        }
    }

    fn standby(&mut self) {
        self.standby = true;
    }

    fn nrf_reset(&mut self) {
        let n = &self.nordic;
        self.nordic = Nordic { dead: n.dead, strap: n.strap, strap_id: n.strap_id, reset: true, ..Nordic::default() };
        self.ms += 950;
    }

    fn nrf_send(&mut self, data: &[u8]) {
        let (n, now) = (&mut self.nordic, self.ms);
        assert!(n.reset, "bytes to the Nordic chip before its reset");
        for &b in data {
            if let Some(p) = n.rx.push(b) {
                let p = p.to_vec();
                n.take(&p, now);
            }
        }
    }

    fn backup_read(&mut self, n: usize) -> u32 {
        self.backup[n]
    }

    fn backup_write(&mut self, n: usize, v: u32) {
        self.backup[n] = v;
    }

    fn nrf_recv(&mut self, buf: &mut [u8]) -> usize {
        if self.nordic.scan_end.is_some_and(|end| self.ms >= end) {
            self.nordic.scan_end = None;
            self.nordic.queue(&[2, 0x1b, 0, 0xff, 0xff, 1]); // BLE_GAP_EVT_TIMEOUT, source scan
        }
        let n = &mut self.nordic;
        if let (true, Some(bpm), Some(t)) = (n.heard(), n.strap, n.next_hr)
            && self.ms >= t
        {
            n.next_hr = Some(t + 246); // 8070 / 32768 s
            let mut p = vec![8, 9, 0x4e, 0, 4, 0, 0, 0, 0, 0, 0, bpm]; // HR data page 4, rate in data byte 7
            p.resize(42, 0);
            p.extend([0x80, 0]); // EVENT_RX, channel 0
            n.queue(&p);
        }
        let n = buf.len().min(self.nordic.out.len());
        for (slot, b) in buf.iter_mut().zip(self.nordic.out.drain(..n)) {
            *slot = b;
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::Frame;
    use crate::ui::{Clock, draw_static};

    /// The screen as the drawing code means it, without the panel protocol in between.
    /// The version the simulated Nordic chip reports, as the unit did.
    const V4: crate::nrf::Version = crate::nrf::Version { image: 4, bootloader: 1, sd: 0x32 };

    fn expected(h: u8, m: u8, s: u8) -> Vec<u16> {
        let mut f = Frame::new();
        draw_static(&mut f);
        Clock::new().show(&mut f, h, m, s);
        crate::ui::draw_ble(&mut f, Some(V4));
        f.px
    }

    /// A stand-in for the ANT+ network key (the real one is never in the repository).
    const KEY: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

    fn run(sim: &mut Sim, app: &mut App, ms: u64) {
        let end = sim.ms + ms;
        while sim.ms < end && !sim.standby {
            app.step(sim);
            sim.delay_ms(POLL_MS);
        }
    }

    #[test]
    fn boots_into_the_clock() {
        let mut sim = Sim::new(12, 34, 56);
        let mut app = App::start(&mut sim, None);
        app.step(&mut sim);
        app.set_usb(&mut sim, crate::ui::UsbDot::Ready);
        assert!(sim.visible());
        let pos = |c| sim.cmds.iter().position(|&x| x == c).unwrap();
        assert!(pos(SWRESET) < pos(SLPOUT) && pos(SLPOUT) < pos(DISPON));
        let (h, m, s) = rtc_time_from_tr(sim.rtc_tr());
        let mut want = Frame::new();
        want.px = expected(h, m, s);
        crate::ui::draw_usb_dot(&mut want, crate::ui::UsbDot::Ready);
        assert!(sim.screen == want.px, "screen differs from the drawing code");
        sim.save("preview");
    }

    /// Stock's order: reset, sd_ble_enable, then the version; the label shows the image number in green.
    #[test]
    fn starts_the_nordic_chip() {
        let mut sim = Sim::new(12, 0, 0);
        let app = App::start(&mut sim, None);
        assert_eq!(sim.nordic.got, [vec![0, 0x60], vec![0, 0x65, 1]], "no ANT without a key, no scan before the list");
        assert_eq!(
            app.render_row(crate::ui::BLE_LABEL.1 + 4).px,
            {
                let mut f = Frame::new();
                crate::ui::draw_ble(&mut f, Some(V4));
                f.px[((crate::ui::BLE_LABEL.1 + 4) * W) as usize..((crate::ui::BLE_LABEL.1 + 5) * W) as usize].to_vec()
            }[..]
        );
    }

    /// A chip that never answers costs one timeout per command, then the clock runs and the label is red.
    #[test]
    fn a_silent_nordic_chip_does_not_stop_the_clock() {
        let mut sim = Sim::new(12, 0, 0);
        sim.nordic.dead = true;
        let t0 = sim.ms;
        let mut app = App::start(&mut sim, None);
        assert!(sim.ms - t0 < 3000, "start took {} ms", sim.ms - t0);
        assert_eq!(sim.nordic.got, [vec![0, 0x60]], "gave up after the first timeout");
        run(&mut sim, &mut app, 2000);
        let (h, m, s) = rtc_time_from_tr(sim.rtc_tr());
        let mut f = Frame::new();
        draw_static(&mut f);
        crate::ui::draw_ble(&mut f, None);
        Clock::new().show(&mut f, h, m, s);
        assert!(sim.screen == f.px);
        assert!((0..W).any(|x| f.at(x, crate::ui::BLE_LABEL.1 + 4) == crate::ui::RED));
    }

    /// Page opens the device list and starts a scan: two distinct devices in three reports, strongest first, the
    /// title "2 NEARBY..." until the scan's timeout; Page again goes back to the clock.
    #[test]
    fn page_lists_the_devices_nearby() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        app.next_page(&mut sim);
        assert_eq!(sim.nordic.got.last().unwrap()[..2], [0, 0x8a], "the scan starts with the list");
        let want = |app: &App| {
            let mut f = Frame::new();
            crate::ui::draw_static(&mut f);
            crate::ui::clear_page(&mut f);
            crate::ui::draw_bar(&mut f);
            crate::ui::draw_ble(&mut f, Some(V4));
            crate::ui::draw_list(&mut f, &app.nearby);
            crate::ui::draw_nearby(&mut f, app.nearby.count(), app.scanning);
            f.px
        };
        run(&mut sim, &mut app, 1500);
        let names: Vec<_> = app.nearby.strongest().map(|d| (d.rssi, std::string::String::from(d.name()))).collect();
        assert_eq!(names, [(-42, "Bike".into()), (-70, "".into())], "last RSSI kept, the name from the first report");
        assert!(app.scanning && sim.screen == want(&app), "the list while scanning");
        sim.save("nearby");
        run(&mut sim, &mut app, u64::from(SCAN_S) * 1000);
        assert!(!app.scanning && sim.screen == want(&app), "2 NEARBY once the scan timed out");
        for y in [0, crate::ui::NEARBY.1 + 5, crate::ui::LIST_Y + 5, H - 1] {
            assert!(app.render_row(y).px[..] == sim.screen[(y * W) as usize..((y + 1) * W) as usize], "row {y}");
        }
        app.next_page(&mut sim);
        run(&mut sim, &mut app, 200);
        let (h, m, s) = rtc_time_from_tr(sim.rtc_tr());
        assert!(sim.screen == expected(h, m, s), "back to the clock");
    }

    /// The line under the bar, rows NEARBY.1 .. +20.
    fn line(px: &[u16]) -> Vec<u16> {
        let y = crate::ui::NEARBY.1;
        px[(y * W) as usize..((y + 20) * W) as usize].to_vec()
    }

    fn hr_line(bpm: Option<u8>, paired: bool) -> Vec<u16> {
        let mut f = Frame::new();
        crate::ui::draw_hr(&mut f, bpm, paired);
        line(&f.px)
    }

    /// With a key: ANT enabled and the HR channel opened as stock does, for any strap; the first rate pairs it
    /// (kept in a backup register), "HR --" once it stops.
    #[test]
    fn pairs_a_strap_and_shows_the_heart_rate() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, Some(KEY));
        let ops: Vec<_> = sim.nordic.got.iter().filter(|p| p[0] == 6).map(|p| p[1]).collect();
        assert_eq!(ops, [0xfa, 0xcc, 0xc2, 0xd3, 0xda, 0xcd, 0xd1, 0xc4]);
        assert!(sim.nordic.got.contains(&[&[6, 0xcc, 0][..], &KEY].concat()), "the key on network 0");
        assert!(sim.nordic.got.contains(&vec![6, 0xd3, 0, 0, 0, 0x78, 0]), "any strap");
        assert!(line(&sim.screen) == hr_line(None, false), "HR PAIRING");
        run(&mut sim, &mut app, 1000);
        assert!(line(&sim.screen) == hr_line(None, false), "no strap in range: still pairing");
        sim.nordic.strap = Some(72);
        run(&mut sim, &mut app, 1000);
        assert!(line(&sim.screen) == hr_line(Some(72), true), "HR 72");
        let strap = crate::nrf::Strap { device: 25126, trans: 0xf1 };
        assert_eq!(sim.backup[STRAP_BKP], strap.to_word(), "paired and kept");
        sim.nordic.strap = None;
        run(&mut sim, &mut app, u64::from(HR_STALE_MS) + 500);
        assert!(line(&sim.screen) == hr_line(None, true), "HR -- once the strap is quiet");
    }

    /// After a restart the channel opens for the paired strap only: another strap is not taken. Holding Page
    /// forgets it and pairs with the one in range.
    #[test]
    fn keeps_the_paired_strap_until_page_is_held() {
        let strap = crate::nrf::Strap { device: 25126, trans: 0xf1 };
        let mut sim = Sim::new(9, 0, 0).keys(|ms| ((5000..7500).contains(&ms), false));
        sim.backup[STRAP_BKP] = strap.to_word();
        sim.nordic.strap = Some(80);
        sim.nordic.strap_id = 999; // a friend's strap
        let mut app = App::start(&mut sim, Some(KEY));
        assert!(sim.nordic.got.contains(&vec![6, 0xd3, 0, 0x26, 0x62, 0x78, 0xf1]), "the paired strap only");
        let t = 4000 - sim.ms;
        run(&mut sim, &mut app, t);
        assert!(line(&sim.screen) == hr_line(None, true), "the friend's strap is not shown");
        run(&mut sim, &mut app, 4000);
        assert_eq!(app.page(), Page::Clock, "a long hold does not change page");
        assert_eq!(sim.backup[STRAP_BKP], crate::nrf::Strap { device: 999, trans: 0xf1 }.to_word(), "paired again");
        assert!(line(&sim.screen) == hr_line(Some(80), true), "HR 80");
    }

    #[test]
    fn ticks_through_midnight() {
        let mut sim = Sim::new(23, 59, 58);
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 3000);
        let (h, m, s) = rtc_time_from_tr(sim.rtc_tr());
        assert_eq!((h, m), (0, 0), "past midnight");
        assert!(sim.screen == expected(h, m, s));
    }

    #[test]
    fn back_short_press_toggles_the_backlight() {
        // Presses after the start (which takes about 1 s with the Nordic chip): at 3 s and at 4 s.
        let mut sim = Sim::new(8, 0, 0).keys(|ms| (false, (3000..3200).contains(&ms) || (4000..4100).contains(&ms)));
        let mut app = App::start(&mut sim, None);
        let t0 = sim.ms;
        assert!(t0 < 3000);
        run(&mut sim, &mut app, 3500 - t0);
        assert!(!sim.backlight);
        run(&mut sim, &mut app, 1000);
        assert!(sim.backlight);
    }

    #[test]
    fn back_held_3s_powers_off_after_release() {
        let mut sim = Sim::new(8, 0, 0).keys(|ms| (false, (1000..6000).contains(&ms)));
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 10_000);
        assert!(sim.standby);
        assert!(sim.ms >= 6000, "standby while the key is still down would wake the unit at once");
        assert!(sim.asleep && !sim.display_on && !sim.backlight);
        assert_eq!(sim.nordic.got.last(), Some(&vec![0, 0x32]), "the radio chip switched off too");
    }

    /// Continuous capture: 4 s with a Page press, one BMP per changed frame in target/sim/run/.
    #[test]
    fn record_a_run() {
        let mut sim = Sim::new(9, 59, 57).keys(|ms| (ms / 100 == 25, false)).record("run");
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 4000);
        let frames = std::fs::read_dir(out_dir().join("run")).unwrap().count();
        assert!(frames >= 4, "one frame per second change at least, got {frames}");
    }
}

/// The computer at the other end of the dev link: what it received, and the staging area.
#[derive(Default)]
pub struct Pc {
    pub got: Vec<u8>,
    pub staged: Vec<u8>,
    /// Registers poked, read back by peek (0 if never written).
    pub regs: std::collections::BTreeMap<u32, u32>,
}

impl crate::dev::Host for Pc {
    fn send(&mut self, data: &[u8]) {
        self.got.extend_from_slice(data);
    }
    fn stage(&mut self, offset: u32, data: &[u8]) {
        assert_eq!(offset as usize, self.staged.len(), "staged out of order");
        self.staged.extend_from_slice(data);
    }
    fn peek(&mut self, addr: u32) -> u32 {
        self.regs.get(&addr).copied().unwrap_or(0)
    }
    fn poke(&mut self, addr: u32, value: u32) {
        self.regs.insert(addr, value);
    }
}

#[cfg(test)]
mod dev_tests {
    use super::*;
    use crate::dev::{Effect, Session};

    fn app_at(h: u64, m: u64, s: u64) -> (Sim, App) {
        let mut sim = Sim::new(h, m, s);
        let mut app = App::start(&mut sim, None);
        app.step(&mut sim);
        (sim, app)
    }

    /// A minimal image as pack.py makes it: SP, reset vector, tag, size word.
    fn image(size: u32) -> Vec<u8> {
        let mut img = vec![0u8; size as usize];
        img[0..4].copy_from_slice(&0x2002_0000u32.to_le_bytes());
        img[4..8].copy_from_slice(&0xd000_0101u32.to_le_bytes());
        img[0x94..0x98].copy_from_slice(b"R039");
        img[0x98..0x9c].copy_from_slice(&size.to_le_bytes());
        img
    }

    fn load(s: &mut Session, app: &App, pc: &mut Pc, img: &[u8], sum: u32) -> Effect {
        let cmd = format!("load {} {}\n", img.len(), sum);
        assert_eq!(s.feed(cmd.as_bytes(), app, 0, pc), Effect::None);
        let mut effect = Effect::None;
        for chunk in img.chunks(512) {
            effect = s.feed(chunk, app, 0, pc);
        }
        effect
    }

    #[test]
    fn peek_and_poke_registers_only() {
        let (_, app) = app_at(1, 2, 3);
        let mut pc = Pc::default();
        let mut s = Session::new();
        s.feed(b"poke 0x40013014 4098\npeek 0x40013010 2\n", &app, 0, &mut pc);
        assert_eq!(pc.got, b"ok\n00000000\n00001002\n");
        for bad in [
            "peek 0x20000000",
            "peek 0x40013001",
            "peek 0x5ffffffc 2",
            "peek 0x40000000 65",
            "peek 0x",
            "poke 0x40013000",
        ] {
            pc.got.clear();
            s.feed(format!("{bad}\n").as_bytes(), &app, 0, &mut pc);
            assert!(pc.got.starts_with(b"err usage"), "{bad}");
        }
        assert_eq!(pc.regs.len(), 1);
    }

    #[test]
    fn info_answers() {
        let (_, app) = app_at(1, 2, 3);
        let mut pc = Pc::default();
        Session::new().feed(b"info\n", &app, 1234, &mut pc);
        assert_eq!(pc.got, format!("bryton-rustfw {} up 1234 ms\n", env!("CARGO_PKG_VERSION")).as_bytes());
    }

    /// The screenshot (rendered row by row from the app's state) must equal what the panel shows.
    #[test]
    fn shot_equals_the_panel() {
        let (sim, app) = app_at(12, 34, 56);
        let mut pc = Pc::default();
        Session::new().feed(b"shot\n", &app, 0, &mut pc);
        let header = b"shot 240 320\n";
        assert_eq!(&pc.got[..header.len()], header);
        let px: Vec<u16> = pc.got[header.len()..].chunks_exact(2).map(|b| u16::from_be_bytes([b[0], b[1]])).collect();
        assert!(px == sim.screen);
    }

    #[test]
    fn load_runs_a_good_image() {
        let (_, app) = app_at(0, 0, 0);
        let (mut s, mut pc) = (Session::new(), Pc::default());
        let img = image(3000);
        let sum = img.iter().map(|&b| u32::from(b)).sum();
        assert_eq!(load(&mut s, &app, &mut pc, &img, sum), Effect::Run(3000));
        assert_eq!(pc.got, b"ready\nok\n");
        assert!(pc.staged == img);
    }

    #[test]
    fn load_refuses_bad_images_and_recovers() {
        let (_, app) = app_at(0, 0, 0);
        let (mut s, mut pc) = (Session::new(), Pc::default());
        let img = image(3000);
        assert_eq!(load(&mut s, &app, &mut pc, &img, 1), Effect::None);
        assert!(pc.got.ends_with(b"err checksum\n"));
        let mut bad = image(3000);
        bad[0x94] = b'X';
        let sum = bad.iter().map(|&b| u32::from(b)).sum();
        pc = Pc::default();
        assert_eq!(load(&mut s, &app, &mut pc, &bad, sum), Effect::None);
        assert!(pc.got.ends_with(b"err no R### tag at 0x94\n"));
        pc = Pc::default();
        assert_eq!(s.feed(b"load 99999999 0\nreset\n", &app, 0, &mut pc), Effect::Reset);
        assert!(pc.got.starts_with(b"err usage"));
        assert!(pc.got.ends_with(b"ok\n"));
    }
}
