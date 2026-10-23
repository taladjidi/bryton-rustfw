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

type KeyScript = Box<dyn Fn(u64) -> u8>;

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
    /// The battery voltage the ADC sees, mV (None: the ADC did not come up), and USB power.
    pub battery_mv: Option<u16>,
    pub charger: bool,
}

/// The Nordic chip as seen over its link: replies to the commands as the unit did, scans, and has ANT channels that
/// hear the [`AntSensor`]s in range.
#[derive(Default)]
pub struct Nordic {
    /// False: the chip never answers (not running).
    pub dead: bool,
    /// Reset since power-up: before that, commands would go nowhere.
    pub reset: bool,
    pub enabled: bool,
    /// Simulated time (ms) at which the running scan times out.
    scan_end: Option<u64>,
    /// ANT+ sensors in range.
    pub sensors: Vec<AntSensor>,
    channels: [Channel; 8],
    /// Every payload received, in order.
    pub got: Vec<Vec<u8>>,
    rx: crate::nrf::Rx,
    out: std::collections::VecDeque<u8>,
}

/// An ANT+ sensor in range: its device type and number, and its 8 data bytes as a function of the time (ms).
pub struct AntSensor {
    pub device_type: u8,
    pub id: u16,
    pub data: fn(u64) -> [u8; 8],
}

/// An ANT channel: the ID it was set to, open or not, the sensor it found, its next broadcast.
#[derive(Default, Clone, Copy)]
struct Channel {
    device_type: u8,
    device: u16,
    open: bool,
    found: Option<usize>,
    next: u64,
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
    /// An ANT event packet: `message` then the event code and the channel, 44 bytes.
    fn ant_event(&mut self, message: &[u8], event: u8, ch: u8) {
        let mut p = vec![8];
        p.extend(message);
        p.resize(42, 0);
        p.extend([event, ch]);
        self.queue(&p);
    }

    /// Broadcasts from the open channels that hear a sensor, every 250 ms.
    fn ant_tick(&mut self, now: u64) {
        for ch in 0..self.channels.len() {
            let c = self.channels[ch];
            if !c.open || now < c.next {
                continue;
            }
            self.channels[ch].next = c.next + 250;
            let hit = |s: &AntSensor| s.device_type == c.device_type && (c.device == 0 || c.device == s.id);
            let Some(i) = c.found.or_else(|| self.sensors.iter().position(hit)) else { continue };
            self.channels[ch].found = Some(i);
            let data = (self.sensors[i].data)(now);
            self.ant_event(&[&[9, 0x4e, ch as u8][..], &data].concat(), 0x80, ch as u8); // EVENT_RX
        }
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
        if let [6, op, ch, rest @ ..] = p {
            let ch = usize::from(*ch);
            match (*op, rest) {
                (0xd3, [d0, d1, t, _]) => {
                    self.channels[ch].device = u16::from_le_bytes([*d0, *d1]);
                    self.channels[ch].device_type = *t;
                }
                (0xc4, _) => {
                    assert!(!self.channels[ch].open, "channel {ch} opened twice");
                    self.channels[ch] = Channel { open: true, found: None, next: now, ..self.channels[ch] };
                }
                (0xc5, _) => {
                    self.channels[ch].open = false;
                    self.queue(&[1, 0xc5, 0, 0, 0, 0]);
                    self.ant_event(&[1, 0x40, ch as u8, 1, 7], 7, ch as u8); // EVENT_CHANNEL_CLOSED
                    return;
                }
                (0xd4, []) => {
                    let c = self.channels[ch];
                    let id = c.found.map_or(c.device, |i| self.sensors[i].id);
                    let [d0, d1] = id.to_le_bytes();
                    self.queue(&[1, 0xd4, 0, 0, 0, 0, d0, d1, c.device_type, if id > 0 { 0xf1 } else { 0 }]);
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
            keys: Box::new(|_| 0),
            record: None,
            dirty: false,
            nordic: Nordic::default(),
            backup: [0; 32],
            battery_mv: Some(3900),
            charger: false,
        }
    }

    /// Keys held as a function of the simulated time in ms: a mask of [`Key`] codes.
    pub fn keys(mut self, f: impl Fn(u64) -> u8 + 'static) -> Self {
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

    fn keys_down(&mut self) -> u8 {
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
        let sensors = std::mem::take(&mut self.nordic.sensors);
        self.nordic = Nordic { dead: self.nordic.dead, sensors, reset: true, ..Nordic::default() };
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

    fn battery_adc(&mut self) -> Option<(u16, u16, u16)> {
        // VDDA 3.3 V: VREFINT reads its calibration value; the pin sees the battery through the divider by 1.5
        const CAL: u16 = 24_000;
        let mv = self.battery_mv?;
        Some(((u32::from(mv) * 2 * 65535).div_ceil(3 * 3300) as u16, CAL, CAL))
    }

    fn charger_present(&mut self) -> bool {
        self.charger
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
        self.nordic.ant_tick(self.ms);
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
    use crate::ui::{Field, Strip, UsbDot};
    /// A stand-in for the ANT+ network key (the real one is never in the repository).
    const KEY: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
    /// The owner's strap, as the unit paired it.
    const STRAP: SensorId = SensorId { device: 25126, trans: 0xf1 };

    fn hr72(_: u64) -> [u8; 8] {
        [4, 0, 0, 0, 0, 0, 0, 72]
    }
    fn hr80(_: u64) -> [u8; 8] {
        [4, 0, 0, 0, 0, 0, 0, 80]
    }
    /// Speed and cadence: every second one crank turn and three wheel turns.
    fn spd_cad(ms: u64) -> [u8; 8] {
        let (t, n) = (((ms / 1000) * 1024) as u16, (ms / 1000) as u16);
        let [t0, t1] = t.to_le_bytes();
        let ([c0, c1], [w0, w1]) = (n.to_le_bytes(), (3 * n).to_le_bytes());
        [t0, t1, c0, c1, t0, t1, w0, w1]
    }
    fn power300(_: u64) -> [u8; 8] {
        [0x10, 1, 0xff, 85, 0, 0, 0x2c, 0x01]
    }

    fn run(sim: &mut Sim, app: &mut App, ms: u64) {
        let end = sim.ms + ms;
        while sim.ms < end && !sim.standby {
            app.step(sim);
            sim.delay_ms(POLL_MS);
        }
    }

    /// The screen drawn step by step on the panel equals the screen drawn at once from the state (the screenshot).
    fn assert_consistent(sim: &Sim, app: &App) {
        for y in 0..H {
            let (drawn, panel) = (app.render_row(y).px, &sim.screen[(y * W) as usize..((y + 1) * W) as usize]);
            let bad: Vec<_> = (0..W as usize).filter(|&x| drawn[x] != panel[x]).collect();
            assert!(
                bad.is_empty(),
                "row {y}, x {bad:?}: screenshot {:04x}, panel {:04x}",
                drawn[bad[0]],
                panel[bad[0]]
            );
        }
    }

    /// Ride page `n` of `app` as the drawing code means it, with these values, under a strip at the sim's time
    /// (`radio`: the Nordic chip answered).
    fn ride(sim: &mut Sim, app: &App, n: usize, values: &[&str], radio: bool) -> Vec<u16> {
        let (h, m, _) = rtc_time_from_tr(sim.rtc_tr());
        let mut f = Frame::new();
        crate::ui::draw_strip(
            &mut f,
            &Strip { time: (h, m), radio, usb: UsbDot::Off, battery: Some(63), charging: false },
        );
        let page = app.pages()[n];
        for (i, v) in values.iter().enumerate() {
            let bx = crate::ui::field_box(page.layout, crate::ui::RIDE_H, i).unwrap();
            crate::ui::draw_field_frame(&mut f, bx, page.fields[i]);
            crate::ui::draw_field_value(&mut f, bx, page.fields[i], v);
        }
        f.px
    }

    fn hm(sim: &mut Sim) -> std::string::String {
        let (h, m, _) = rtc_time_from_tr(sim.rtc_tr());
        format!("{h:02}:{m:02}")
    }

    /// The main screen as the drawing code means it, at the sim's time, under a strip with `usb`.
    fn main_screen(sim: &mut Sim, usb: UsbDot) -> Vec<u16> {
        let (h, m, s) = rtc_time_from_tr(sim.rtc_tr());
        let mut f = Frame::new();
        crate::ui::draw_strip(&mut f, &Strip { time: (h, m), radio: true, usb, battery: Some(63), charging: false });
        crate::ui::draw_main_time(&mut f, &format!("{h:02}:{m:02}"), &format!("{s:02}"));
        crate::ui::draw_ferris(&mut f);
        f.px
    }

    /// The main screen: the strip, the time big with its seconds, Ferris.
    #[test]
    fn boots_into_the_main_screen() {
        let mut sim = Sim::new(12, 34, 56);
        let mut app = App::start(&mut sim, None);
        app.step(&mut sim);
        app.set_usb(&mut sim, UsbDot::Ready);
        assert!(sim.visible());
        let pos = |c| sim.cmds.iter().position(|&x| x == c).unwrap();
        assert!(pos(SWRESET) < pos(SLPOUT) && pos(SLPOUT) < pos(DISPON));
        assert!(main_screen(&mut sim, UsbDot::Ready) == sim.screen);
        assert_consistent(&sim, &app);
        sim.save("preview");
    }

    /// Stock's order: reset, sd_ble_enable, then the version; no ANT without a key, no scan before the list.
    #[test]
    fn starts_the_nordic_chip() {
        let mut sim = Sim::new(12, 0, 0);
        App::start(&mut sim, None);
        assert_eq!(sim.nordic.got, [vec![0, 0x60], vec![0, 0x65, 1]]);
    }

    /// A chip that never answers costs one timeout per command, then the pages run and the label is red.
    #[test]
    fn a_silent_nordic_chip_does_not_stop_the_pages() {
        let mut sim = Sim::new(12, 0, 0);
        sim.nordic.dead = true;
        let t0 = sim.ms;
        let mut app = App::start(&mut sim, Some(KEY));
        assert!(sim.ms - t0 < 3000, "start took {} ms", sim.ms - t0);
        assert_eq!(sim.nordic.got, [vec![0, 0x60]], "gave up after the first timeout");
        run(&mut sim, &mut app, 2000);
        app.press(&mut sim, Press::Short(Key::Page));
        assert!(ride(&mut sim, &app, 0, &["--", "--", "--"], false) == sim.screen, "BLE in red");
    }

    #[test]
    fn ticks_through_midnight() {
        let mut sim = Sim::new(23, 59, 58);
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 3000);
        assert_eq!(hm(&mut sim), "00:00");
        assert!(main_screen(&mut sim, UsbDot::Off) == sim.screen);
    }

    /// Page goes from the main screen through the ride pages that are on (page 3 is off by default), and back.
    #[test]
    fn page_turns_the_ride_pages() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        app.step(&mut sim);
        app.press(&mut sim, Press::Short(Key::Page));
        assert_eq!(app.screen(), Screen::Ride(0));
        assert!(ride(&mut sim, &app, 0, &["--"; 3], true) == sim.screen, "full height, no Ferris");
        app.press(&mut sim, Press::Short(Key::Page));
        assert_eq!(app.screen(), Screen::Ride(1));
        assert_eq!(app.pages()[1].fields[..4], [Field::Speed, Field::Hr, Field::Cadence, Field::Power]);
        assert!(ride(&mut sim, &app, 1, &["--"; 4], true) == sim.screen);
        app.press(&mut sim, Press::Short(Key::Page));
        assert_eq!(app.screen(), Screen::Main);
        assert_consistent(&sim, &app);
    }

    /// Holding Page opens the menu; Page / Lap move, Start on NEARBY BLE opens the list and scans: two devices in
    /// three reports, strongest first, "2 NEARBY..." until the timeout. Back goes out level by level.
    #[test]
    fn menu_lists_the_devices_nearby() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        app.press(&mut sim, Press::Long(Key::Page));
        assert_eq!(app.screen(), Screen::Menu(Menu::Main, 0));
        app.press(&mut sim, Press::Short(Key::Lap));
        assert_eq!(app.screen(), Screen::Menu(Menu::Main, 5), "Lap from the top wraps to NEARBY BLE");
        assert_consistent(&sim, &app);
        sim.save("menu");
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(app.screen(), Screen::Nearby);
        assert_eq!(sim.nordic.got.last().unwrap()[..2], [0, 0x8a], "the scan starts with the list");
        run(&mut sim, &mut app, 1500);
        let names: Vec<_> = app.nearby.strongest().map(|d| (d.rssi, std::string::String::from(d.name()))).collect();
        assert_eq!(names, [(-42, "Bike".into()), (-70, "".into())], "last RSSI kept, the name from the first report");
        assert!(app.scanning);
        assert_consistent(&sim, &app);
        sim.save("nearby");
        run(&mut sim, &mut app, u64::from(SCAN_S) * 1000);
        assert!(!app.scanning);
        assert_consistent(&sim, &app);
        app.press(&mut sim, Press::Short(Key::Back));
        assert_eq!(app.screen(), Screen::Menu(Menu::Main, 5));
        app.press(&mut sim, Press::Short(Key::Back));
        assert_eq!(app.screen(), Screen::Main);
        assert_consistent(&sim, &app);
    }

    /// Sensors menu: Start on a sensor searches; the first one heard is paired (kept in its backup register) and
    /// its value reaches the ride page. With a key, ANT is started and every channel assigned, none opened yet.
    #[test]
    fn pairs_sensors_from_the_menu() {
        let mut sim = Sim::new(9, 0, 0);
        sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72 });
        sim.nordic.sensors.push(AntSensor { device_type: 0x79, id: 555, data: spd_cad });
        let mut app = App::start(&mut sim, Some(KEY));
        let ops: Vec<_> = sim.nordic.got.iter().filter(|p| p[0] == 6).map(|p| p[1]).collect();
        assert_eq!(ops, [0xfa, 0xcc, 0xc2, 0xc2, 0xc2, 0xc2, 0xc2], "enable, key, assign 5 channels");
        assert!(sim.nordic.got.contains(&[&[6, 0xcc, 0][..], &KEY].concat()), "the key on network 0");
        app.press(&mut sim, Press::Long(Key::Page));
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(app.screen(), Screen::Menu(Menu::Sensors, 0));
        app.press(&mut sim, Press::Short(Key::Start)); // heart rate: search
        assert!(sim.nordic.got.contains(&vec![6, 0xd3, 0, 0, 0, 0x78, 0]), "any strap");
        for _ in 0..3 {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        app.press(&mut sim, Press::Short(Key::Start)); // speed + cadence: search
        run(&mut sim, &mut app, 3000);
        assert_eq!(sim.backup[Kind::Hr.backup()], STRAP.to_word(), "strap paired and kept");
        assert_eq!(sim.backup[Kind::SpeedCadence.backup()], SensorId { device: 555, trans: 0xf1 }.to_word());
        assert_consistent(&sim, &app);
        sim.save("sensors");
        app.press(&mut sim, Press::Short(Key::Back));
        app.press(&mut sim, Press::Short(Key::Back));
        app.press(&mut sim, Press::Short(Key::Page));
        run(&mut sim, &mut app, 200);
        assert!(ride(&mut sim, &app, 0, &["22.7", "72", "60"], true) == sim.screen, "22.7 km/h, HR 72, 60 rpm");
        app.press(&mut sim, Press::Short(Key::Page));
        run(&mut sim, &mut app, 200);
        assert!(ride(&mut sim, &app, 1, &["22.7", "72", "60", "--"], true) == sim.screen);
        sim.save("ride");
    }

    /// After a restart the paired sensor's channel opens for it only: a friend's strap is not taken. Start on it
    /// in the sensors menu forgets it (channel closed, register cleared).
    #[test]
    fn keeps_the_paired_sensor_until_forgotten() {
        let mut sim = Sim::new(9, 0, 0);
        sim.backup[Kind::Hr.backup()] = STRAP.to_word();
        sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: 999, data: hr80 }); // a friend's strap
        let mut app = App::start(&mut sim, Some(KEY));
        assert!(sim.nordic.got.contains(&vec![6, 0xd3, 0, 0x26, 0x62, 0x78, 0xf1]), "the paired strap only");
        run(&mut sim, &mut app, 2000);
        app.press(&mut sim, Press::Short(Key::Page));
        assert!(ride(&mut sim, &app, 0, &["--", "--", "--"], true) == sim.screen, "the friend's strap is not shown");
        app.press(&mut sim, Press::Long(Key::Page));
        app.press(&mut sim, Press::Short(Key::Start));
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(sim.nordic.got.iter().filter(|p| p[..2] == [6, 0xc5]).count(), 1, "closed");
        assert_eq!(sim.backup[Kind::Hr.backup()], 0, "forgotten");
        app.press(&mut sim, Press::Short(Key::Start));
        run(&mut sim, &mut app, 1000);
        assert_eq!(sim.backup[Kind::Hr.backup()], SensorId { device: 999, trans: 0xf1 }.to_word(), "paired anew");
    }

    /// The page menu: Start cycles the layout and the fields; kept in a backup register, read back at the next
    /// start. A power meter's value shows in the new field.
    #[test]
    fn pages_are_configured_from_the_menu() {
        let mut sim = Sim::new(9, 0, 0);
        sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
        sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power300 });
        let mut app = App::start(&mut sim, Some(KEY));
        app.press(&mut sim, Press::Long(Key::Page));
        app.press(&mut sim, Press::Short(Key::Page));
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(app.screen(), Screen::Menu(Menu::Page(0), 0));
        app.press(&mut sim, Press::Short(Key::Start)); // layout 3 -> 4
        app.press(&mut sim, Press::Short(Key::Page));
        for _ in 0..2 {
            app.press(&mut sim, Press::Short(Key::Start)); // field 1: speed -> cadence -> power
        }
        assert_consistent(&sim, &app);
        sim.save("page_menu");
        let want = PageCfg {
            layout: 4,
            fields: [Field::Power, Field::Hr, Field::Cadence, Field::Time, Field::Time, Field::Time],
        };
        assert_eq!(app.pages()[0], want);
        assert_eq!(PageCfg::from_word(sim.backup[14]), Some(want));
        app.press(&mut sim, Press::Short(Key::Back));
        app.press(&mut sim, Press::Short(Key::Back));
        app.press(&mut sim, Press::Short(Key::Page));
        run(&mut sim, &mut app, 1000);
        let t = hm(&mut sim);
        assert_consistent(&sim, &app);
        assert!(ride(&mut sim, &app, 0, &["300", "--", "85", &t], true) == sim.screen);
        let app = App::start(&mut sim, Some(KEY));
        assert_eq!(app.pages()[0], want, "read back after a restart");
    }

    /// The battery level in the strip (green while charging); an empty battery switches the unit off, but not
    /// while charging.
    #[test]
    fn battery_in_the_strip_and_off_when_empty() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        app.step(&mut sim);
        assert_eq!(app.strip.battery, Some(63), "3.9 V");
        sim.charger = true;
        sim.battery_mv = Some(3100);
        run(&mut sim, &mut app, 3 * u64::from(BATTERY_MS) + 1000);
        assert!(!sim.standby && app.strip.charging, "charging: stays on");
        sim.charger = false;
        run(&mut sim, &mut app, 2 * u64::from(BATTERY_MS));
        assert!(!sim.standby, "two low readings: not yet");
        run(&mut sim, &mut app, u64::from(BATTERY_MS));
        assert!(sim.standby && !sim.display_on, "empty: switched off");
    }

    #[test]
    fn back_short_press_toggles_the_backlight() {
        // Presses after the start (which takes about 1 s with the Nordic chip): at 3 s and at 4 s.
        let back = |ms| (3000..3200).contains(&ms) || (4000..4100).contains(&ms);
        let mut sim = Sim::new(8, 0, 0).keys(move |ms| if back(ms) { Key::Back as u8 } else { 0 });
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
        let mut sim = Sim::new(8, 0, 0).keys(|ms| if (1000..6000).contains(&ms) { Key::Back as u8 } else { 0 });
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
        let mut sim = Sim::new(9, 59, 57).keys(|ms| if ms / 100 == 25 { Key::Page as u8 } else { 0 }).record("run");
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 4000);
        let frames = std::fs::read_dir(out_dir().join("run")).unwrap().count();
        assert!(frames >= 2, "the first page, then the second, got {frames}");
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
            "peek 0x10000000",
            "peek 0x2001fffc 2",
            "poke 0x20000000 1",
            "poke 0xd0000000 1",
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
