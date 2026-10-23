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
    /// Every buzzer frequency set, in order (0: silence).
    pub tones: Vec<u32>,
    /// Backlight level, percent.
    pub light_level: u8,
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
    pub gnss: Gnss,
    /// Pixels written to the panel so far (the drawing cost: each one is 16 bits over SPI).
    pub pixels: u64,
}

/// Where the simulated fix is (degrees x 1e-7).
pub const FIX_LAT: i32 = 488_566_123;
pub const FIX_LON: i32 = -22_945_678;

/// The GNSS receiver as seen over SPI: once powered, a NAV-PVT frame every second (with `fix`: 3D at that speed,
/// else no fix, as indoors on the unit); 0xff when it has nothing.
#[derive(Default)]
pub struct Gnss {
    pub powered: bool,
    /// Ground speed of a 3D fix in mm/s; None: no fix.
    pub fix: Option<i32>,
    /// Where a fix is, as a function of the simulated time in ms: latitude, longitude (degrees x 1e-7) and heading
    /// (degrees x 1e-5); None: [`FIX_LAT`], [`FIX_LON`], heading north.
    pub route: Option<fn(u64) -> (i32, i32, i32)>,
    /// Every byte received, in order (0xff included).
    pub got: Vec<u8>,
    /// UTC of the next report, seconds since 1970 (set by the board: its RTC minus 2 h).
    utc: i64,
    /// When its power line was last driven low (0: from reset, as the board does at boot).
    off_since: u64,
    out: std::collections::VecDeque<u8>,
    next_ms: u64,
}

impl Gnss {
    fn tick(&mut self, now: u64) {
        if !self.powered || now < self.next_ms {
            return;
        }
        self.next_ms = now + 1000;
        let mut p = [0u8; 92];
        p[4..12].copy_from_slice(&[0xe5, 0x07, 3, 7, 0, 0, 0, 0xf0]);
        if let Some(speed) = self.fix {
            let (y, mo, d, h, mi, sec) = crate::from_epoch_s(self.utc);
            p[4..6].copy_from_slice(&y.to_le_bytes());
            p[6..12].copy_from_slice(&[mo, d, h, mi, sec, 0x07]); // date and time valid
            (p[20], p[21], p[23]) = (3, 1, 9);
            let (lat, lon, heading) = self.route.map_or((FIX_LAT, FIX_LON, 0), |f| f(now));
            p[24..28].copy_from_slice(&lon.to_le_bytes());
            p[28..32].copy_from_slice(&lat.to_le_bytes());
            p[60..64].copy_from_slice(&speed.to_le_bytes());
            p[64..68].copy_from_slice(&heading.to_le_bytes());
        }
        let body: Vec<u8> = [0x01, 0x07, 92, 0].iter().chain(&p).copied().collect();
        let (mut a, mut b) = (0u8, 0u8);
        for &x in &body {
            a = a.wrapping_add(x);
            b = b.wrapping_add(a);
        }
        self.out.extend([0xb5, 0x62]);
        self.out.extend(body);
        self.out.extend([a, b]);
    }
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
    pub channels: [Channel; 8],
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

/// An ANT channel: the ID it was set to, open or not, the sensor it found, its next broadcast, when it opened and
/// its low-priority search time-out (2.5 s units).
#[derive(Default, Clone, Copy)]
pub struct Channel {
    device_type: u8,
    device: u16,
    pub open: bool,
    found: Option<usize>,
    next: u64,
    opened: u64,
    search_timeout: u8,
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
            let Some(i) = c.found.or_else(|| self.sensors.iter().position(hit)) else {
                if c.search_timeout != 0xff && now >= c.opened + 2500 * u64::from(c.search_timeout) {
                    // nothing found: EVENT_RX_SEARCH_TIMEOUT, then the channel closes
                    self.channels[ch].open = false;
                    self.ant_event(&[1, 0x40, ch as u8, 1, 1], 1, ch as u8);
                    self.ant_event(&[1, 0x40, ch as u8, 1, 7], 7, ch as u8);
                }
                continue;
            };
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
                    self.channels[ch] =
                        Channel { open: true, found: None, next: now, opened: now, ..self.channels[ch] };
                }
                (0xda, [t]) => self.channels[ch].search_timeout = *t,
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
            tones: Vec::new(),
            light_level: 0,
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
            gnss: Gnss::default(),
            pixels: 0,
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
        self.pixels += u64::from(count);
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.screen[(y * W + x) as usize] = next();
            }
        }
        self.dirty = true;
    }

    fn beep(&mut self, hz: u32) {
        self.tones.push(hz);
    }

    fn backlight(&mut self, level: u8) {
        self.backlight = level > 0;
        self.light_level = level;
    }

    fn keys_down(&mut self) -> u8 {
        (self.keys)(self.ms)
    }

    fn rtc(&mut self) -> (u32, u32) {
        // 2026-10-23 at the start; the day rolls over at midnight
        let t = self.start_s + self.ms / 1000;
        let (y, mo, d) = crate::civil_from_days(crate::days_from_civil(2026, 10, 23) + (t / 86400) as i32);
        let t = t % 86400;
        let tr = to_bcd(t / 3600) << 16 | to_bcd(t / 60 % 60) << 8 | to_bcd(t % 60);
        (tr, to_bcd(u64::from(y) % 100) << 16 | to_bcd(u64::from(mo)) << 8 | to_bcd(u64::from(d)))
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

    fn gnss_power(&mut self, on: bool) {
        if on && !self.gnss.powered {
            // as on the unit: switched on too soon after its line was driven low, the receiver does not start
            assert!(
                self.ms >= self.gnss.off_since + 500,
                "GNSS powered {} ms after off",
                self.ms - self.gnss.off_since
            );
            self.gnss.next_ms = self.ms + 500; // its first fix report
        }
        if !on && self.gnss.powered {
            self.gnss.off_since = self.ms;
        }
        self.gnss.powered = on;
    }

    fn gnss_transfer(&mut self, buf: &mut [u8]) {
        assert_eq!(buf.len() % 4, 0, "SPI transfers move 4 bytes at a time");
        let (tr, dr) = self.rtc();
        let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), crate::rtc_date_from_dr(dr));
        let g = &mut self.gnss;
        g.utc = crate::epoch_s((y, mo, d, h, m, s)) - 2 * 3600; // the RTC runs on UTC + 2 h
        g.tick(self.ms);
        for b in buf.iter_mut() {
            g.got.push(*b);
            *b = if g.powered { g.out.pop_front().unwrap_or(0xff) } else { 0xff };
        }
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
        let (h, m, _) = rtc_time_from_tr(sim.rtc().0);
        let mut f = Frame::new();
        crate::ui::draw_strip(
            &mut f,
            &Strip {
                time: (h, m),
                radio,
                gps: app.strip.gps,
                blink: app.strip.blink,
                usb: UsbDot::Off,
                battery: Some(63),
                charging: false,
            },
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
        let (h, m, _) = rtc_time_from_tr(sim.rtc().0);
        format!("{h:02}:{m:02}")
    }

    /// The main screen as the drawing code means it, at the sim's time, under a strip with `usb`.
    fn main_screen(sim: &mut Sim, app: &App, usb: UsbDot) -> Vec<u16> {
        let (h, m, s) = rtc_time_from_tr(sim.rtc().0);
        let mut f = Frame::new();
        crate::ui::draw_strip(
            &mut f,
            &Strip {
                time: (h, m),
                radio: true,
                gps: app.strip.gps,
                blink: app.strip.blink,
                usb,
                battery: Some(63),
                charging: false,
            },
        );
        crate::ui::draw_main_time(&mut f, [h / 10, h % 10, m / 10, m % 10, s / 10, s % 10]);
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
        assert!(main_screen(&mut sim, &app, UsbDot::Ready) == sim.screen);
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
        assert!(main_screen(&mut sim, &app, UsbDot::Off) == sim.screen);
    }

    /// Page goes from the main screen through the ride pages that are on (page 3 is the map by default), and back.
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
        assert_eq!(app.screen(), Screen::Ride(2), "the map");
        assert_consistent(&sim, &app);
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
        assert_eq!(app.screen(), Screen::Menu(Menu::Main, 8), "Lap from the top wraps to NEARBY BLE");
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
        assert_eq!(app.screen(), Screen::Menu(Menu::Main, 8));
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

    /// A paired strap that is not sending: its channel searches 30 s, closes, rests a minute, searches again; the
    /// strap is found when it comes back.
    #[test]
    fn a_silent_sensor_searches_then_rests() {
        let mut sim = Sim::new(9, 0, 0);
        sim.backup[Kind::Hr.backup()] = STRAP.to_word();
        let mut app = App::start(&mut sim, Some(KEY));
        let opens = |sim: &Sim| sim.nordic.got.iter().filter(|p| p.starts_with(&[6, 0xc4, 0])).count();
        assert_eq!(opens(&sim), 1);
        assert!(sim.nordic.got.contains(&vec![6, 0xda, 0, crate::nrf::SEARCH_TIMEOUT]));
        run(&mut sim, &mut app, 30_000 + 1000);
        assert!(!sim.nordic.channels[0].open, "closed by its search time-out");
        run(&mut sim, &mut app, u64::from(SEARCH_REST_MS) - 2000);
        assert_eq!(opens(&sim), 1, "resting");
        run(&mut sim, &mut app, 2000);
        assert_eq!(opens(&sim), 2, "searching again");
        sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72 });
        run(&mut sim, &mut app, 1000);
        assert_eq!(app.live.get(Metric::Hr), Some(72), "found");
        run(&mut sim, &mut app, 60_000);
        assert!(sim.nordic.channels[0].open, "a sensor that sends keeps its channel");
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

    /// The receiver is powered at start and gets its configuration (RAM layer) once; with a fix, "GPS" turns green
    /// and the speed comes from it when no speed sensor is paired; it is powered off with the unit.
    #[test]
    fn gps_fix_and_speed() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        assert!(!sim.gnss.powered, "powered later, after its line has been low long enough");
        run(&mut sim, &mut app, 3000);
        assert!(sim.gnss.powered);
        let sent = |sim: &Sim, f: [u8; 32]| sim.gnss.got.windows(32).any(|w| w == f);
        assert!(sent(&sim, crate::gnss::config()), "configuration");
        let frames = sim.gnss.got.iter().filter(|&&b| b == 0xb5).count();
        assert_eq!(frames, 2, "the RTC time (14 h accuracy, no offset learnt yet), no position yet");
        assert_eq!(app.strip.gps, Some(0), "searching");
        sim.gnss.fix = Some(7000); // 25.2 km/h
        run(&mut sim, &mut app, 2000);
        assert_eq!(app.strip.gps, Some(3), "9 satellites: 3 bars");
        app.press(&mut sim, Press::Short(Key::Page));
        run(&mut sim, &mut app, 100);
        let (h, m, _) = rtc_time_from_tr(sim.rtc().0);
        let mut want = Frame::new();
        want.px = ride(&mut sim, &app, 0, &["25.2", "--", "--"], true);
        let strip = Strip {
            time: (h, m),
            radio: true,
            gps: Some(3),
            blink: true,
            usb: UsbDot::Off,
            battery: Some(63),
            charging: false,
        };
        crate::ui::draw_strip(&mut want, &strip);
        assert!(want.px == sim.screen, "GPS speed, green GPS");
        sim.save("gps");
        app.press(&mut sim, Press::Long(Key::Page));
        for _ in 0..7 {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(app.screen(), Screen::Menu(Menu::Gps, 0));
        run(&mut sim, &mut app, 1100);
        assert_consistent(&sim, &app);
        sim.save("gps_menu");
        app.press(&mut sim, Press::Short(Key::Back));
        app.press(&mut sim, Press::Short(Key::Back));
        // the fix taught the RTC's offset (+2 h) and the position (to 1e-4 degrees): kept for the next power-up
        let aid = crate::gnss::Aid::from_words([sim.backup[17], sim.backup[18]]);
        assert_eq!(aid, crate::gnss::Aid { offset_15min: Some(8), pos: Some((488_566_000, -22_945_000)) });
        sim.gnss.powered = false; // a power cycle: the board drives the line low at boot
        sim.gnss.off_since = sim.ms;
        let mut again = App::start(&mut sim, None);
        sim.gnss.got.clear();
        run(&mut sim, &mut again, 2500);
        let (tr, dr) = sim.rtc();
        let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), crate::rtc_date_from_dr(dr));
        let utc = crate::from_epoch_s(crate::epoch_s((y, mo, d, h, m, s)) - 2 * 3600);
        let sent_time = (0..3).any(|back| {
            sim.gnss
                .got
                .windows(32)
                .any(|w| w == crate::gnss::mga_time(crate::from_epoch_s(crate::epoch_s(utc) - back), 2))
        });
        assert!(sent_time, "UTC from the RTC at the next power-up");
        assert!(sent(&sim, crate::gnss::mga_pos(488_566_000, -22_945_000, 10_000)), "and the last position");
        sim.gnss.powered = false; // silent receiver: the fix ages out
        run(&mut sim, &mut app, u64::from(PVT_STALE_MS) + 200);
        assert_eq!(app.strip.gps, None);
        app.press(&mut sim, Press::Long(Key::Back));
        assert!(!sim.gnss.powered, "off with the unit");
    }

    /// The backlight: level and time-out from the menu (kept with the wheel size); after the time-out any key
    /// brings it back (and acts), Back only brings it back.
    #[test]
    fn backlight_level_and_timeout() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        assert_eq!(sim.light_level, 100, "100 % and never off by default");
        app.press(&mut sim, Press::Long(Key::Page));
        for _ in 0..5 {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        app.press(&mut sim, Press::Short(Key::Start)); // LIGHT %: 60
        app.press(&mut sim, Press::Short(Key::Page));
        app.press(&mut sim, Press::Short(Key::Start)); // LIGHT OFF: 10 s
        assert_eq!(sim.light_level, 60);
        assert_consistent(&sim, &app);
        sim.save("light_menu");
        app.press(&mut sim, Press::Short(Key::Back));
        run(&mut sim, &mut app, 11_000);
        assert_eq!(sim.light_level, 0, "timed out");
        app.press(&mut sim, Press::Short(Key::Page));
        assert_eq!((sim.light_level, app.screen()), (60, Screen::Ride(0)), "a key brings it back and acts");
        run(&mut sim, &mut app, 11_000);
        app.press(&mut sim, Press::Short(Key::Back));
        assert_eq!(sim.light_level, 60, "Back after the time-out: on, not toggled off");
        app.press(&mut sim, Press::Short(Key::Back));
        assert_eq!(sim.light_level, 0, "then Back toggles");
        let again = App::start(&mut sim, None);
        assert_eq!(sim.light_level, 60, "kept: {:x}", sim.backup[11]);
        drop(again);
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
        assert_eq!(sim.tones, [1500, 2500, 0, 2500, 1500, 0], "rising at power-on, falling at power-off");
    }

    /// A synthetic ride at 8 m/s (28.8 km/h): 240 s east with a gentle wiggle, then a loop of 600 m radius
    /// counterclockwise, starting heading east. Latitude, longitude (1e-7 degrees) and heading (1e-5 degrees).
    fn ride_route(ms: u64) -> (i32, i32, i32) {
        let at = |t: f64| {
            if t < 240.0 {
                (8.0 * t, 30.0 * (t * core::f64::consts::TAU / 60.0).sin())
            } else {
                let a = (t - 240.0) * 8.0 / 600.0;
                (1920.0 + 600.0 * a.sin(), 600.0 - 600.0 * a.cos())
            }
        };
        let t = ms as f64 / 1000.0;
        let ((x, y), (x1, y1)) = (at(t), at(t + 0.5));
        let heading = (x1 - x).atan2(y1 - y).to_degrees().rem_euclid(360.0);
        let m_per_unit = 0.011_132; // metres per 1e-7 degree of latitude, as map.rs
        let cos = (f64::from(FIX_LAT) * 1e-7).to_radians().cos();
        (FIX_LAT + (y / m_per_unit) as i32, FIX_LON + (x / m_per_unit / cos) as i32, (heading * 1e5) as i32)
    }

    /// The map page (page 3 by default) through a ride: "NO FIX" until the first fix, then the track and the arrow;
    /// fitted again a few times while the ride goes out, never once the loop is closed; between those, only small
    /// boxes are redrawn. The panel always equals the screenshot rendered row by row.
    #[test]
    fn map_page_follows_a_ride() {
        let mut sim = Sim::new(9, 0, 0);
        sim.gnss.route = Some(ride_route);
        let mut app = App::start(&mut sim, None);
        for _ in 0..3 {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        assert_eq!(app.screen(), Screen::Ride(2));
        run(&mut sim, &mut app, 3000);
        assert_consistent(&sim, &app);
        sim.save("map_nofix");
        sim.gnss.fix = Some(8000);
        let area = crate::map::area(crate::ui::field_box(1, crate::ui::RIDE_H, 0).unwrap());
        let t0 = sim.ms;
        let (mut refits, mut refit_px, mut small_px, mut late_refits) = (0, 0, 0, 0);
        let mut seconds = 0;
        while sim.ms < t0 + 840_000 {
            let (view, px) = (app.map_shown[0].view, sim.pixels);
            app.step(&mut sim);
            sim.delay_ms(POLL_MS);
            let cost = sim.pixels - px;
            if app.map_shown[0].view != view {
                refits += 1;
                refit_px += cost;
                late_refits += u32::from(sim.ms > t0 + 720_000);
            } else {
                small_px += cost;
            }
            if (sim.ms - t0) / 1000 > seconds {
                seconds = (sim.ms - t0) / 1000;
                if seconds % 60 == 0 {
                    assert_consistent(&sim, &app);
                }
                match seconds {
                    30 => sim.save("map_start"),
                    240 => sim.save("map_out"),
                    _ => {}
                }
            }
        }
        assert_consistent(&sim, &app);
        sim.save("map_loop");
        let s = (sim.ms - t0) / 1000;
        std::println!(
            "map: {refits} full redraws ({} px each), {} px/s otherwise, {} px/s in all, over {s} s",
            refit_px / refits.max(1),
            small_px / s,
            (refit_px + small_px) / s
        );
        assert!((2..=12).contains(&refits), "fitted again as the ride grows: {refits}");
        assert_eq!(late_refits, 0, "the loop closed: the view stays");
        assert!(small_px / s < 2000, "{} px/s between full redraws", small_px / s);
        let v = app.map_shown[0].view.unwrap();
        let (x, y) = v.px(area, app.track.pos().unwrap());
        assert_eq!(sim.screen[(y as u32 * W + x as u32) as usize], crate::ui::WHITE, "the arrow at the position");
        let track = sim.screen.iter().filter(|&&p| p == crate::ui::ACCENT).count();
        assert!(track > 500, "the track drawn: {track} px");
        // the map as one field of three, over speed and heart rate
        app.pages[0] =
            PageCfg { layout: 3, fields: [Field::Map, Field::Speed, Field::Hr, Field::Time, Field::Time, Field::Time] };
        app.press(&mut sim, Press::Short(Key::Page));
        app.press(&mut sim, Press::Short(Key::Page));
        assert_eq!(app.screen(), Screen::Ride(0));
        run(&mut sim, &mut app, 10_000);
        assert_consistent(&sim, &app);
        sim.save("map_field");
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
    use crate::dev::{Effect, Session, Stats};

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
        assert_eq!(s.feed(cmd.as_bytes(), app, Stats::default(), pc), Effect::None);
        let mut effect = Effect::None;
        for chunk in img.chunks(512) {
            effect = s.feed(chunk, app, Stats::default(), pc);
        }
        effect
    }

    #[test]
    fn peek_and_poke_registers_only() {
        let (_, app) = app_at(1, 2, 3);
        let mut pc = Pc::default();
        let mut s = Session::new();
        s.feed(b"poke 0x40013014 4098\npeek 0x40013010 2\n", &app, Stats::default(), &mut pc);
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
            s.feed(format!("{bad}\n").as_bytes(), &app, Stats::default(), &mut pc);
            assert!(pc.got.starts_with(b"err usage"), "{bad}");
        }
        assert_eq!(pc.regs.len(), 1);
    }

    #[test]
    fn info_answers() {
        let (_, app) = app_at(1, 2, 3);
        let mut pc = Pc::default();
        Session::new().feed(b"info\n", &app, Stats { uptime_ms: 1234, awake_permille: 0 }, &mut pc);
        assert_eq!(
            pc.got,
            format!("bryton-rustfw {} up 1234 ms, awake 0 permille\n", env!("CARGO_PKG_VERSION")).as_bytes()
        );
    }

    /// The screenshot (rendered row by row from the app's state) must equal what the panel shows.
    #[test]
    fn shot_equals_the_panel() {
        let (sim, app) = app_at(12, 34, 56);
        let mut pc = Pc::default();
        Session::new().feed(b"shot\n", &app, Stats::default(), &mut pc);
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
        assert_eq!(s.feed(b"load 99999999 0\nreset\n", &app, Stats::default(), &mut pc), Effect::Reset);
        assert!(pc.got.starts_with(b"err usage"));
        assert!(pc.got.ends_with(b"ok\n"));
    }
}

/// Layer 4 of the testing strategy (AGENTS.md): random bytes through every parser and decoder that takes bytes
/// from outside (the Nordic chip, the GNSS receiver). A panic there would freeze the unit.
#[cfg(test)]
mod fuzz {
    extern crate std;
    /// xorshift32: deterministic, so a failure reproduces.
    fn bytes(seed: u32, n: usize) -> impl Iterator<Item = u8> {
        let mut x = seed;
        (0..n).map(move |_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            // lengths and markers more often than chance, so the parsers get past their headers
            match x % 16 {
                0 => 0xb5,
                1 => 0x62,
                2 => 44,
                3 => 0,
                _ => (x >> 8) as u8,
            }
        })
    }

    #[test]
    fn nordic_packets() {
        let mut rx = crate::nrf::Rx::new();
        for seed in 1..=4u32 {
            for b in bytes(seed, 1 << 20) {
                if let Some(p) = rx.push(b) {
                    let _ = crate::nrf::event_id(p);
                    let _ = crate::nrf::ant_broadcast(p);
                    let _ = crate::nrf::ant_closed(p);
                    if let Some(r) = crate::nrf::adv_report(p) {
                        let _ = (crate::nrf::ad_name(r.data), crate::nrf::ad_maker(r.data));
                    }
                }
            }
        }
    }

    /// Random bytes from the computer, with command words mixed in, into the dev link protocol.
    #[test]
    fn dev_link_input() {
        use crate::dev::{Session, Stats};
        let mut sim = super::Sim::new(9, 0, 0);
        let app = crate::App::start(&mut sim, None);
        let words: [&[u8]; 8] = [b"peek ", b"poke ", b"load ", b"key ", b"0x4", b"\n", b"shot\n", b"bat\n"];
        for seed in 1..=4u32 {
            let (mut session, mut pc) = (Session::new(), super::Pc::default());
            let mut buf = std::vec::Vec::new();
            for (k, b) in bytes(seed, 1 << 16).enumerate() {
                if b % 5 == 0 {
                    buf.extend_from_slice(words[usize::from(b) % words.len()]);
                } else {
                    buf.push(b);
                }
                if k % 64 == 63 {
                    let _ = session.feed(&buf, &app, Stats::default(), &mut pc);
                    buf.clear();
                    pc.got.clear();
                }
            }
        }
    }

    #[test]
    fn ubx_frames() {
        let mut parser = crate::gnss::Parser::new();
        for seed in 1..=4u32 {
            for b in bytes(seed, 1 << 20) {
                if let Some((c, i, p)) = parser.push(b) {
                    let _ = crate::gnss::pvt(c, i, p);
                }
            }
        }
    }
}
