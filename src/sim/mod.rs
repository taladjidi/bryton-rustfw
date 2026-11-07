//! A simulated S510 for host tests. It implements [`Board`] with:
//! - a model of the ST7789: it checks the command bytes (power, sleep, pixel format, RAMCTRL, windows) and
//!   paints its framebuffer from RAMWR as the real panel would;
//! - a clock driven by the delays, so the RTC advances as the app waits;
//! - keys scripted as a function of time;
//! - a Nordic chip ([`Nordic`]) that answers the commands the way the real one did on the unit (BOARD.md 11);
//! - an eMMC ([`EmmcCard`]) that answers the commands as JEDEC says, takes writes below B only, and can lose power
//!   after any sector written (`cut_after`), in the middle of a multi-block write too.
//! - the sensor bus ([`Bus`]): the light sensor, the barometer and the accelerometer (BOARD.md 16);
//! - the touch controller ([`TouchChip`]), with a finger scripted as a function of time like the keys.
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

/// The app started on a simulator at 09:00:00 with nothing on the card.
pub fn started() -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    let app = App::start(&mut sim, None);
    (sim, app)
}

/// The Nordic chip's UART as something else than the [`Nordic`] model (`nrf_emu_tests.rs`: Bryton's real app in an
/// emulator). While set on [`Sim::nrf_ext`], the Board's nrf primitives and the clock go to it.
pub trait NrfExt {
    /// The chip's reset.
    fn reset(&mut self);
    /// Bytes to the chip.
    fn send(&mut self, data: &[u8]);
    /// Bytes from the chip, at most `buf.len()`.
    fn recv(&mut self, buf: &mut [u8]) -> usize;
    /// Time passes.
    fn advance_ms(&mut self, ms: u32);
}

type KeyScript = Box<dyn Fn(u64) -> u8>;

/// What the transfers cost in time on the unit, ns. Computed values say where they come from; "inferred" ones are
/// assumptions until a silent bench on the unit (docs/status.md, "Not yet seen on the unit"). The costs are always paid
/// (the simulator was free before 2026-11-01); `work_ns_per_byte` is the read side and is zero unless a test sets it.
#[derive(Clone, Copy)]
pub struct Cost {
    /// A panel pixel: 16 bits at SPI1's 50 MHz (main.rs `spi_init`, BOARD.md 6): 320 ns.
    pub panel_pixel_ns: u64,
    /// A panel command or window: the DC toggle and the wait for the end of transfer (inferred, 1 us).
    pub panel_cmd_ns: u64,
    /// A panel parameter byte at 50 MHz.
    pub panel_byte_ns: u64,
    /// An I2C byte: 9 clocks of SCL; main.rs i2c_init TIMINGR gives PRESC 15 (250 ns at 64 MHz), SCLL 9, SCLH 7: a
    /// clock of 18 x 250 = 4.5 us (RM0399 I2C, sync delays left out), 40.5 us a byte, about 222 kHz.
    pub i2c_byte_ns: u64,
    /// An I2C transaction's start, restart and stop (about 2 clocks, same TIMINGR).
    pub i2c_frame_ns: u64,
    /// A GNSS byte: SPI4 at 4 MHz (main.rs, BOARD.md 13): 2 us.
    pub gnss_byte_ns: u64,
    /// A GNSS transfer's chip select and polling (inferred, 5 us).
    pub gnss_xfer_ns: u64,
    /// A card write command's own time: the busy after CMD24 / CMD25 and the CMD13 polls that wait it out (inferred,
    /// 1 ms: eMMC 5.x programming busy is typically 0.5 to 2 ms, JESD84-B51 "busy after write"; not looked up on the part).
    pub write_cmd_ns: u64,
    /// A card sector written, on top of the command's: 0.2 ms optimistic (2.5 MB/s like the measured read) or 2 ms
    /// pessimistic (the survey's guess); the budget gate runs at both ([`Cost::pessimistic`]).
    pub write_sector_ns: u64,
    /// The card's stall after this many sectors written (inferred: every 1024 sectors; 0: none).
    pub stall_every: u64,
    /// The stall's length, ms (inferred: 300).
    pub stall_ms: u64,
}

impl Cost {
    /// The survey's pessimistic card: 2 ms a written sector.
    pub fn pessimistic() -> Self {
        Self { write_sector_ns: 2_000_000, ..Self::default() }
    }
}

/// Where the paid time went ([`Sim::paid_ns`]).
pub const PAID: [&str; 6] = ["panel", "i2c", "gnss", "card wr", "card rd", "stall"];
const P_PANEL: usize = 0;
const P_I2C: usize = 1;
const P_GNSS: usize = 2;
const P_WRITE: usize = 3;
const P_READ: usize = 4;
const P_STALL: usize = 5;

impl Default for Cost {
    fn default() -> Self {
        Self {
            panel_pixel_ns: 320,
            panel_cmd_ns: 1000,
            panel_byte_ns: 160,
            i2c_byte_ns: 40_500,
            i2c_frame_ns: 9_000,
            gnss_byte_ns: 2000,
            gnss_xfer_ns: 5000,
            write_cmd_ns: 1_000_000,
            write_sector_ns: 200_000,
            stall_every: 1024,
            stall_ms: 300,
        }
    }
}

pub struct Sim {
    pub screen: Vec<u16>,
    window: (u32, u32, u32, u32),
    /// Every panel command, in order.
    pub cmds: Vec<u8>,
    pub powered: bool,
    pub asleep: bool,
    pub display_on: bool,
    /// The panel's timing and sleep rules ([`PanelRules`]).
    pub panel: PanelRules,
    colmod: u8,
    ramctrl: [u8; 2],
    pub backlight: bool,
    /// Every buzzer frequency set, in order (0: silence).
    pub tones: Vec<u32>,
    /// eMMC writes allowed ([`Board::emmc_writable`]): the unit's builds without EMMC_WRITE=1 say no.
    pub writable: bool,
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
    /// Replaces [`Sim::nordic`] when set.
    pub nrf_ext: Option<Box<dyn NrfExt>>,
    /// PA6 is low (the chip is in reset), and when the chip has booted after the release (BOARD.md 11): a byte the host
    /// sends before that, or left in its TX FIFO at the release, is lost on the hardware, so it fails the test.
    chip_held: bool,
    chip_boot_until: u64,
    /// The USART2 lines parked ([`Board::nrf_park`]) since the last hold, and our pull on PD3 (the chip's RTS).
    nrf_parked: bool,
    cts_pull_up: bool,
    /// A test that provokes the violation sets this: the bytes are counted in `reset_bytes` and dropped instead.
    pub expect_reset_bytes: bool,
    pub reset_bytes: u32,
    /// Times the RTC was set.
    pub rtc_sets: u32,
    /// RTC backup registers (they survive an app restart, as on the unit).
    pub backup: [u32; 32],
    /// The battery voltage the ADC sees, mV (None: the ADC did not come up), and USB power.
    pub battery_mv: Option<u16>,
    pub charger: bool,
    pub gnss: Gnss,
    /// Pixels written to the panel so far (the drawing cost: each one is 16 bits over SPI).
    pub pixels: u64,
    /// Panel windows written so far (each one also costs CASET, RASET and RAMWR: 11 command bytes).
    pub windows: u64,
    pub emmc: EmmcCard,
    pub bus: Bus,
    pub touch: TouchChip,
    /// The time an eMMC read costs, ns a byte: about 430 on the unit, the measured end-to-end rate of a scan (2.3 MB/s,
    /// `tools/dev.py bench scan`), that is the card's own read time (about 95 ns a byte at its raw 10.5 MB/s) plus
    /// parsing, so a bulk read that is not parsed is overcharged. It moves the clock ([`Sim::ms`]) and
    /// [`prof::Clock::us`]. 0 (the default): most tests do not mind the time and it would move the clock under all of
    /// them; the budget gate (budget_tests.rs) and the scan tests set it.
    pub work_ns_per_byte: u64,
    /// Work done so far, ns. Must not advance by more than 21 s between two Clock reads: the u32 cycle counter wraps
    /// at 21.47 s of work (200 MHz) and Clock catches one wrap only.
    pub awake_ns: u64,
    /// Time slept in a transfer's wait (the panel's DMA) not yet taken by [`Board::waited_us`], ns.
    waited_ns: u64,
    /// In-pass `delay_ms` calls count as waits of the work, as on the unit; the loop helpers sleep with
    /// [`Sim::sleep_ms`], which does not.
    pub count_waits: bool,
    /// What each transfer costs in time ([`Cost`]); the sub-millisecond rest of it is in `cost_rem`.
    pub cost: Cost,
    cost_rem: u64,
    /// Longest gap between two reads of the keys, ms, and the time of the last read (the gate's touch/key latency).
    pub longest_key_gap_ms: u64,
    last_keys_ms: u64,
    /// Sectors written so far, for the card's periodic stall.
    sectors_written: u64,
    /// The RTC stands at this many seconds after its start, whatever the time: for a screen hashed against a reference.
    pub frozen_s: Option<u64>,
    /// The RTC calendar's day 0, days since 1970 (2026-10-23; 2000-01-01, RTC_DR's reset value, after a power loss).
    pub rtc_base: i32,
    /// Time paid so far per [`PAID`] category, ns; and, for the longest gap between GNSS transfers, its length and what
    /// was paid inside it (the test's breakdown of a gap).
    pub paid_ns: [u64; 6],
    paid_at_gnss: [u64; 6],
    pub worst_gnss_gap: (u64, [u64; 6]),
    /// What was paid inside every gap past the receiver's 1.5 s time-out, summed, and how many such gaps.
    pub long_gnss_gaps: (u32, [u64; 6]),
    /// The cycle counter ([`Board::cycles`]): each read moves it on by [`Sim::cycles_step`], wrapping as CYCCNT does.
    pub cycles: u32,
    /// What each read of the cycle counter adds (0, the default: the counter stands still).
    pub cycles_step: u32,
    /// The channels `adc_read` was asked, in order (the first [`Sim::adc_n`] entries; more are not kept).
    pub adc_calls: [u8; 4],
    /// How many channels were asked.
    pub adc_n: u8,
    /// A channel that answers `None` (a failed conversion), or none.
    pub adc_fail: Option<u8>,
    /// The main loop's side of the board ([`LoopBoard`]: loop_model.rs).
    pub main: LoopModel,
}

mod touch_model;
pub use touch_model::TouchChip;

/// SEC_COUNT of the simulated eMMC: about 31 GB, as the unit's.
pub const EMMC_SECTORS: u32 = 0x03a3_e000;

mod emmc_model;
pub use emmc_model::EmmcCard;
use emmc_model::expected_resp;

/// Start held on a paused ride, then SAVE on the end-ride screen (selected when it opens: Start): the end and save.
pub(crate) fn save_ride(sim: &mut Sim, app: &mut App) {
    app.press(sim, Press::Long(Key::Start));
    app.press(sim, Press::Short(Key::Start));
}

/// What the simulated eMMC holds in sector `lba` when it has no image: the sector number, then a pattern that depends on it.
pub fn emmc_sector(lba: u32) -> [u8; 512] {
    let mut s = [0u8; 512];
    for (i, b) in s.iter_mut().enumerate() {
        *b = (i as u8) ^ (lba as u8).wrapping_mul(7) ^ (lba >> 8) as u8;
    }
    s[..4].copy_from_slice(&lba.to_le_bytes());
    s
}

mod sensor_model;
pub use sensor_model::Bus;

/// Where the simulated fix is (degrees x 1e-7): in the sea near 0 N 0 E, nowhere real, as the synthetic volume's
/// rides and tiles (AGENTS.md, "Public repository").
pub const FIX_LAT: i32 = 5_123_456;
pub const FIX_LON: i32 = 4_234_567;

/// A simulated ride's position over time: ms to latitude, longitude (degrees x 1e-7) and heading (degrees x 1e-5).
pub type Route = fn(u64) -> (i32, i32, i32);

mod panel_model;
pub use panel_model::PanelRules;
mod gnss_model;
pub use gnss_model::Gnss;

/// The Nordic chip as seen over its link: replies to the commands as the unit did, scans, and has ANT channels that
/// hear the [`AntSensor`]s in range.
#[derive(Default)]
pub struct Nordic {
    /// False: the chip never answers (not running).
    pub dead: bool,
    /// A BLE command the chip refuses (NRF_ERROR_NO_MEM), as one whose configuration it lacks would.
    pub refuse: Option<u8>,
    /// An ANT command the chip refuses on a channel (op, channel), with an error code.
    pub ant_refuse: Option<[u8; 2]>,
    /// Reset since power-up: before that, commands would go nowhere.
    pub reset: bool,
    /// Hung: no answer, no event, until the next reset (unlike `dead`, which a reset does not cure).
    pub hung: bool,
    /// In System OFF (BOARD.md 11 "Power off"): `sd_power_system_off` taken and its TX buffer got (the frame in flight
    /// out first), then the transport closed: nothing sent, our bytes not taken, its RTS let go (our CTS reads "hold").
    /// Until the next reset.
    pub off: bool,
    off_pending: bool,
    /// Never goes off (a chip that ignores `sd_power_system_off`); survives resets.
    pub stays_on: bool,
    /// A slow but alive chip: each response comes this many ms after its command (the wait is real time).
    pub slow_ms: u32,
    /// Hangs at this ANT command (1 is the first) after each reset, 0 never (survives resets).
    pub die_at_ant: u8,
    ant_cmds: u8,
    /// Resets so far, and the simulated time (ms) of each.
    pub resets: Vec<u64>,
    /// Damage to the chip's stream: bytes dropped, added or flipped (xorshift state, per mille of the bytes).
    pub chaos: Option<(u32, u32)>,
    pub enabled: bool,
    /// Simulated time (ms) at which the running scan times out.
    scan_end: Option<u64>,
    /// ANT+ sensors in range.
    pub sensors: Vec<AntSensor>,
    pub channels: [Channel; 8],
    /// The FE-C trainer (device type 0x11) among `sensors` is this model: its pages instead of the sensor's `data`.
    pub trainer: SimTrainer,
    /// The power meter's (device type 0x0b) answers to calibration requests (page 0x01).
    pub meter: SimMeter,
    /// The chip refuses an acknowledged message (op 0xc8) with TRANSFER_IN_PROGRESS for 250 ms (a channel period)
    /// after one went out, as the real one does while the last is still queued (review of ecbe541); the count of
    /// those refusals, and when the chip is free again.
    pub ack_model: bool,
    pub ack_busy_until: u64,
    pub ack_refused: u32,
    /// The chip never answers an acknowledged message (the 0xc8 layout is inferred, not seen on the unit).
    pub ack_silent: bool,
    /// Acknowledged messages that fail on air: EVENT_TRANSFER_TX_FAILED (code 6) after each, instead of
    /// TX_COMPLETED, while this is non-zero (counted down); the trainer does not get them.
    pub ack_fail: u32,
    /// Acknowledged messages that fail on air and whose TX_FAILED never reaches the unit (read during another
    /// command's wait, or among bytes a resync dropped), counted down.
    pub ack_lost: u32,
    /// The next GATT client event of this id on this connection is lost (conn, id): an answer that never comes.
    pub drop_gattc: Option<(u16, u16)>,
    /// Every GATT client event of this id on this connection is lost (conn, id).
    pub mute_gattc: Option<(u16, u16)>,
    /// The network keys set (`06 cc [net] [key]`), by network.
    pub keys: [Option<[u8; 8]>; 2],
    /// The BLE peripheral side (ble_sim.rs), and the phone in range.
    pub server: ble_sim::Server,
    pub phone: Option<ble_sim::PhoneSim>,
    /// The FTMS trainer in range (ble_sim.rs), on connection 1 once the unit connects to it.
    pub ftms: Option<ble_sim::FtmsSim>,
    /// The `sd_ble_cfg_set` arguments received since the reset; `sd_ble_enable` after them answers NO_MEM (a chip
    /// whose RAM cannot hold them); connections asked for (`sd_ble_gap_connect`) and the parameter updates sent.
    pub cfg: Vec<Vec<u8>>,
    pub no_mem: bool,
    pub connects: u32,
    pub param_updates: Vec<Vec<u8>>,
    /// Every payload received, in order.
    pub got: Vec<Vec<u8>>,
    /// The host's bytes of the frame being received, and the (type, opcode) of the command being answered.
    frame: Vec<u8>,
    cur: Option<u8>,
    /// Why the chip hung on a command (chip_table.rs), and the part of the reason a test expects (`Some("")`: any hang):
    /// else the simulator panics. It survives a reset.
    pub hang_reason: Option<std::string::String>,
    pub expect_hang: Option<&'static str>,
    /// Answer every valid command with an empty success (no model behind it): for the test of the command builders.
    pub table_only: bool,
    /// The link and the queues, byte by byte (chip_link.rs): the frame in flight on the chip's TX, our USART2's RX FIFO,
    /// our TX FIFO, the responses and events waiting for the chip's single TX buffer, and what the SoftDevice holds.
    out: std::collections::VecDeque<u8>,
    fifo: std::collections::VecDeque<u8>,
    txq: std::collections::VecDeque<u8>,
    resp: std::collections::VecDeque<Vec<u8>>,
    /// For each response, the events queued before it (they go out first).
    resp_ahead: std::collections::VecDeque<usize>,
    events: std::collections::VecDeque<Vec<u8>>,
    sd_ble: Vec<Vec<u8>>,
    sd_ant: std::collections::VecDeque<Vec<u8>>,
    /// The chip stopped pulling events (0 slots free) until more than half are free.
    suspended: bool,
    /// Bytes taken since a command's header was read while its predecessor's response waited for the TX buffer.
    stall: Option<u8>,
    /// The chip's clock, us (it never runs behind the simulated ms), when its transmitter is free, and when the last
    /// frame ended.
    t: u64,
    tx_at: u64,
    frame_end: u64,
    /// The simulated ms of the last `sync`.
    now_ms: u64,
    /// Bytes a hung chip took since it hung, and the security contexts in use.
    hung_rx: u8,
    sec: u8,
    /// ANT events the SoftDevice's queue of 32 had no room for (the newest is lost: inferred); each overflow is reported
    /// with event 0x35 once there is room.
    pub ant_dropped: u32,
    /// An ANT event was dropped and the SoftDevice owes the app an EVENT_QUE_OVERFLOW (0x35) once there is room.
    ant_overflow: bool,
    /// The chip's real limits (chip_link.rs).
    pub limits: ChipLimits,
    /// Times the chip hung (queue overflow, a command it cannot decode, ...) and the most events ever queued; both
    /// survive a reset.
    pub hangs: u32,
    pub high_water: usize,
    /// Our RTS held high for a Stop (chip_link.rs `hold`): the chip sends nothing, its events queue.
    held: bool,
}

/// The Nordic app's limits (chip_link.rs for the model, BOARD.md 11 for the facts): `slots` queue slots, then a hang
/// for ever; `frame_gap_us` between two frames (15 us, measured in an emulator of Bryton's app); the host's receive
/// wait `wait_us` (a call that finds no byte for that long ends its burst).
#[derive(Clone, Copy)]
pub struct ChipLimits {
    /// Event queue depth.
    pub slots: usize,
    /// Time from the end of a frame to the start of the next, us.
    pub frame_gap_us: u32,
    /// How long one `nrf_recv` waits for the next byte, us: the board's `nrf_wait_cycles` (200; 20 before 2026-11-01).
    pub wait_us: u32,
    /// The host's time to parse the bytes one `nrf_recv` returned, before it calls again, us. Inferred (not measured on
    /// the unit): a frame's decode at 200 MHz is a few thousand cycles. 0: back to back.
    pub parse_us: u32,
    /// Security contexts: `sd_ble_gap_sec_params_reply` takes one, AUTH_STATUS or DISCONNECTED frees it (8). The
    /// link fuzz injects pairing requests the chip would never send in a row, and lifts this.
    pub sec_contexts: u8,
}

/// The board's receive wait, us: `nrf_wait_cycles` at the simulated clock, so the ride tests follow a change of it.
pub(crate) fn board_wait_us() -> u32 {
    let hz = <Sim as Board>::CPU_HZ;
    nrf_wait_cycles(hz) / (hz / 1_000_000)
}

impl Default for ChipLimits {
    fn default() -> Self {
        Self { slots: 96, frame_gap_us: 15, wait_us: board_wait_us(), parse_us: 20, sec_contexts: 8 }
    }
}

/// A simulated FE-C trainer (drivers/fec.rs): sends pages 16 and 25 (and 80, 81, 54 when asked), applies the control
/// pages it is sent (power follows the target in ERG, the speed follows the grade in SIM) and answers each with
/// page 71. Every control page received is kept in `got`.
#[derive(Default)]
pub struct SimTrainer {
    /// Every control page received, in order.
    pub got: Vec<[u8; 8]>,
    /// The data pages asked for (page 70), in order.
    pub asked: Vec<u8>,
    /// The page of the last control command (0: none yet, the trainer is on its own), its target power W, resistance
    /// 0.5 %, grade 0.01 %.
    pub mode: u8,
    pub target_w: u16,
    pub resistance: u8,
    pub grade_x100: i32,
    /// The power the rider makes when the trainer does not hold a target, W.
    pub rider_w: u16,
    /// A ceiling on the power it makes, W (0: none): a rider who cannot hold the target in ERG.
    pub cap_w: u16,
    /// The rider has stopped pedalling: no power, no cadence, whatever the target.
    pub stopped: bool,
    /// The cadence it reports while there is power, rpm (0: 90).
    pub rpm: u8,
    /// The status nibble of page 25 (1 and 2 calibration, 4 user configuration), and its target flags.
    pub status: u8,
    pub limits: u8,
    /// The user configuration page last received.
    pub user: Option<[u8; 8]>,
    /// The power and speed (mm/s) now.
    pub power: u16,
    pub speed_mm_s: u32,
    /// Distance, mm; pages sent.
    pub dist_mm: u64,
    /// Fuzzing: the pages are pseudo-random ones of the profile (xorshift state), not the model's.
    pub fuzz: Option<u32>,
    /// A calibration is scripted to fail (its response says nothing succeeded), or the trainer ignores the request.
    pub cal_fails: bool,
    pub cal_mute: bool,
    /// Where the calibration is: 0 none, 1 waiting for the speed, 2 coasting (ticks counted in `cal_coast`).
    pub cal: u8,
    /// The calibration requests received (byte 1), in order.
    pub cal_requests: Vec<u8>,
    /// The target speed of the calibration, mm/s (0: 8333, 30 km/h).
    pub cal_target_mm_s: u16,
    cal_coast: u8,
    ticks: u32,
    events: u8,
    acc: u16,
    answer: Option<[u8; 8]>,
    capabilities: bool,
}

impl SimTrainer {
    fn command(&mut self, d: [u8; 8]) {
        self.got.push(d);
        match d[0] {
            0x30 => (self.mode, self.resistance) = (0x30, d[7]),
            0x31 => (self.mode, self.target_w) = (0x31, u16::from_le_bytes([d[6], d[7]]) / 4),
            0x33 => {
                self.mode = 0x33;
                self.grade_x100 = i32::from(u16::from_le_bytes([d[5], d[6]])) - 20_000;
            }
            0x01 => {
                self.cal_requests.push(d[1]);
                if !self.cal_mute && d[1] & 0xc0 != 0 && self.cal == 0 {
                    (self.cal, self.cal_coast) = (1, 0);
                }
                return;
            }
            0x37 => {
                self.user = Some(d);
                self.status &= !4;
            }
            0x46 => {
                self.asked.push(d[6]);
                self.capabilities |= d[6] == 54;
                return;
            }
            _ => return,
        }
        self.answer = Some([71, d[0], 0xff, 0, 0xff, 0xff, d[6], d[7]]); // passed
    }

    /// The next page, a quarter second on.
    fn page(&mut self) -> [u8; 8] {
        self.ticks += 1;
        let t = self.ticks;
        if let Some(x) = &mut self.fuzz {
            let mut next = || {
                *x ^= *x << 13;
                *x ^= *x >> 17;
                *x ^= *x << 5;
                *x
            };
            let page = [16, 25, 25, 54, 71, 80, 81, 0, 255, 1, 2][(next() % 11) as usize];
            let mut d: [u8; 8] = core::array::from_fn(|_| (next() >> 8) as u8);
            d[0] = page;
            return d;
        }
        // calibrating, the brake is off: the rider's power, whatever the mode was
        self.power = if self.mode == 0x31 && self.cal == 0 { self.target_w } else { self.rider_w };
        if self.stopped {
            self.power = 0;
        }
        if self.cap_w != 0 {
            self.power = self.power.min(self.cap_w);
        }
        self.speed_mm_s = match self.mode {
            0x33 => (10_000 - self.grade_x100 * 8).clamp(1000, 20_000) as u32,
            0x30 if self.resistance > 0 => 8000 - u32::from(self.resistance) * 20,
            // free (the release): the rider's own speed for the power made
            0x30 => u32::from(self.power) * 42,
            _ => u32::from(self.power) * 42,
        };
        self.dist_mm += u64::from(self.speed_mm_s) / 4;
        if let Some(a) = self.answer.take() {
            return a;
        }
        if core::mem::take(&mut self.capabilities) {
            return [54, 255, 255, 255, 255, 0xe8, 0x03, 0x07];
        }
        if self.cal != 0 && t % 4 == 3 {
            let target = if self.cal_target_mm_s == 0 { 8333 } else { self.cal_target_mm_s };
            if self.cal == 1 && self.speed_mm_s >= u32::from(target) {
                self.cal = 2;
            }
            if self.cal == 2 {
                self.cal_coast += 1;
                if self.cal_coast > 6 {
                    // the end: the response; the trainer stops asking when it succeeded
                    self.cal = 0;
                    let ok = if self.cal_fails { 0 } else { 0x80 };
                    self.status &= if self.cal_fails { 0xff } else { !3 };
                    return [1, ok, 0, 90, 0xff, 0xff, 0xc4, 0x09]; // 2500 ms
                }
            }
            let speed = if self.cal == 1 { 1 } else { 2 };
            let [t0, t1] = target.to_le_bytes();
            return [2, 0x80, speed << 6 | 2 << 4, 90, t0, t1, 0xb8, 0x0b]; // spin-down time 3000 ms
        }
        match t % 64 {
            1 => return [80, 255, 255, 1, 0x0f, 0x00, 0x34, 0x12], // manufacturer 15
            2 => return [81, 255, 255, 1, 0x78, 0x56, 0x34, 0x12],
            _ => {}
        }
        if t.is_multiple_of(4) {
            let [s0, s1] = (self.speed_mm_s as u16).to_le_bytes();
            [16, 25, t as u8, (self.dist_mm / 1000) as u8, s0, s1, 0xff, 0x38]
        } else {
            self.events = self.events.wrapping_add(1);
            self.acc = self.acc.wrapping_add(self.power);
            let [a0, a1] = self.acc.to_le_bytes();
            let [p0, p1] = self.power.to_le_bytes();
            [
                25,
                self.events,
                if self.power > 0 {
                    [90, self.rpm][usize::from(self.rpm > 0)]
                } else if self.stopped {
                    0
                } else {
                    0xff
                },
                a0,
                a1,
                p0,
                p1 & 0xf | self.status << 4,
                0x30 | self.limits,
            ]
        }
    }
}

/// How the simulated power meter answers a manual zero request.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum MeterAnswer {
    /// Not at all.
    #[default]
    Silent,
    /// 0xAC with this offset.
    Zero(i16),
    /// 0xAF.
    Fail,
}

/// The power meter's calibration side: the pages it received, how it answers, its auto zero status byte (0xff: none),
/// and the answer being broadcast (page, broadcasts left).
#[derive(Default)]
pub struct SimMeter {
    pub got: Vec<[u8; 8]>,
    pub answer: MeterAnswer,
    pub auto: Option<u8>,
    pub reply: Option<([u8; 8], u8)>,
}

/// An ANT sensor in range: its device type and number, its 8 data bytes as a function of the time (ms), the
/// network it is on (0 ANT+, 1 Shimano's: heard only by a channel assigned to it once that network has a key), and
/// the pages it sends as acknowledged data (message 0x4f, as a Di2 sends its button page) rather than broadcast.
pub struct AntSensor {
    pub device_type: u8,
    pub id: u16,
    pub data: fn(u64) -> [u8; 8],
    pub network: u8,
    pub ack: &'static [u8],
}

/// An ANT channel: the network and ID it was set to, open or not, the sensor it found, its next broadcast, when it
/// opened and its low-priority search time-out (2.5 s units).
#[derive(Default, Clone, Copy)]
pub struct Channel {
    net: Option<u8>,
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
            let keyed = |n: u8| self.keys.get(usize::from(n)).is_some_and(Option::is_some);
            let hit = |s: &AntSensor| {
                c.net == Some(s.network)
                    && keyed(s.network)
                    && s.device_type == c.device_type
                    && (c.device == 0 || c.device == s.id)
            };
            // a sensor taken out of range (removed) is lost: the channel searches again
            let found = c.found.filter(|&i| i < self.sensors.len());
            let Some(i) = found.or_else(|| self.sensors.iter().position(hit)) else {
                if c.search_timeout != 0xff && now >= c.opened + 2500 * u64::from(c.search_timeout) {
                    // nothing found: EVENT_RX_SEARCH_TIMEOUT, then the channel closes
                    self.channels[ch].open = false;
                    self.ant_event(&[1, 0x40, ch as u8, 1, 1], 1, ch as u8);
                    self.ant_event(&[1, 0x40, ch as u8, 1, 7], 7, ch as u8);
                }
                continue;
            };
            self.channels[ch].found = Some(i);
            let mut data =
                if self.sensors[i].device_type == 0x11 { self.trainer.page() } else { (self.sensors[i].data)(now) };
            if let (0x0b, Some((page, n))) = (self.sensors[i].device_type, &mut self.meter.reply) {
                data = *page; // the calibration answer, a few broadcasts
                *n -= 1;
                if *n == 0 {
                    self.meter.reply = None;
                }
            }
            let id = if self.sensors[i].ack.contains(&data[0]) { 0x4f } else { 0x4e };
            self.ant_event(&[&[9, id, ch as u8][..], &data].concat(), 0x80, ch as u8); // EVENT_RX
        }
    }

    fn queue(&mut self, payload: &[u8]) {
        if self.dead || self.hung || self.off {
            return;
        }
        if let Some(kind) = self.cur.filter(|_| payload.first() == Some(&1)) {
            chip_table::response_ok(kind, payload)
                .unwrap_or_else(|e| panic!("the simulated chip answers off the table: {e}"));
        } else if payload.first() != Some(&1)
            && let Err(e) = chip_table::event_ok(payload)
        {
            panic!("the simulated chip sends an event off the table: {e}");
        }
        let frame = [&(payload.len() as u16).to_le_bytes()[..], payload].concat();
        match payload {
            [1, ..] => {
                self.touch();
                self.resp.push_back(frame);
                self.resp_ahead.push_back(self.events.len());
            }
            [8, ..] if self.sd_ant.len() >= chip_link::ANT_QUEUE => {
                (self.ant_dropped, self.ant_overflow) = (self.ant_dropped + 1, true)
            }
            [8, ..] => self.sd_ant.push_back(frame),
            _ => {
                self.frees_context(payload);
                self.sd_ble.push(frame);
            }
        }
    }

    /// `frame` as it goes on the wire: with `chaos`, bytes lost, added or flipped.
    fn damage(&mut self, frame: Vec<u8>) -> std::collections::VecDeque<u8> {
        let mut out = std::collections::VecDeque::new();
        let Some((x, rate)) = &mut self.chaos else {
            return frame.into();
        };
        for byte in frame {
            *x ^= *x << 13;
            *x ^= *x >> 17;
            *x ^= *x << 5;
            if *x % 1000 >= *rate {
                out.push_back(byte);
                continue;
            }
            match (*x >> 12) % 3 {
                0 => {}                                           // lost
                1 => out.extend([byte, (*x >> 20) as u8]),        // a stray byte after it
                _ => out.push_back(byte ^ 1 << ((*x >> 20) % 8)), // a bit flipped
            }
        }
        out
    }

    /// A stray byte in the stream, between frames.
    pub fn stray(&mut self, byte: u8) {
        self.touch();
        self.out.push_back(byte);
    }

    /// One byte from the host: a 2 byte length, then the payload; the chip hangs on what its decoder refuses.
    fn feed(&mut self, b: u8, now: u64) {
        self.frame.push(b);
        let [l0, l1, ..] = self.frame[..] else { return };
        let len = usize::from(u16::from_le_bytes([l0, l1]));
        let verdict = if len == 0 || len > chip_table::MAX_PAYLOAD {
            Err(chip_table::Hang::Length(len))
        } else if self.frame.len() < 2 + len {
            return;
        } else {
            chip_table::check(&self.frame[2..])
        };
        let p = self.frame.split_off(2);
        self.frame.clear();
        if verdict.is_ok()
            && let [0, 0x7f, _, _, 0, ..] = p[..]
        {
            // sd_ble_gap_sec_params_reply accepted: a security context, freed by AUTH_STATUS or DISCONNECTED
            self.sec = self.sec.saturating_add(1);
            if self.sec > self.limits.sec_contexts {
                return self.hang_on_why(format!("security contexts exhausted ({}) on {p:02x?}", self.sec));
            }
        }
        match verdict {
            Ok(c) if self.table_only => {
                self.got.push(p.clone());
                self.off_pending |= p == [0, 0x32];
                if c.out != chip_table::Out::None {
                    let mut r = vec![1, c.op, 0, 0, 0, 0];
                    if let chip_table::Out::Fixed(n) = c.out {
                        r.resize(6 + n, 0);
                    }
                    self.cur = Some(c.kind);
                    self.queue(&r);
                }
            }
            Ok(_) => {
                self.cur = Some(p[0]);
                self.take(&p, now);
            }
            Err(h) => self.hang_on(&h, &p),
        }
    }

    fn hang_on(&mut self, h: &chip_table::Hang, p: &[u8]) {
        self.hang_on_why(format!("{h:?} on {p:02x?}"));
    }

    /// The chip hangs for `why` (the simulator panics unless a test expects it).
    fn hang_on_why(&mut self, why: std::string::String) {
        match self.expect_hang {
            Some(want) if why.contains(want) => {}
            _ => panic!("the Nordic chip hangs on a command it cannot decode (expected {:?}): {why}", self.expect_hang),
        }
        (self.hung, self.hangs, self.hang_reason) = (true, self.hangs + 1, Some(why));
        self.hung_rx = 0;
        self.events.clear();
        self.resp.clear();
        self.resp_ahead.clear();
        self.sd_ble.clear();
        self.sd_ant.clear();
        self.ant_overflow = false;
        self.out.clear();
    }

    fn take(&mut self, p: &[u8], now: u64) {
        self.got.push(p.to_vec());
        if p.first() == Some(&6) {
            self.ant_cmds = self.ant_cmds.saturating_add(1);
            if self.die_at_ant != 0 && self.ant_cmds >= self.die_at_ant {
                self.hung = true; // dies after BLE is enabled, in the ANT setup; a reset cures it
                return;
            }
        }
        if p == [0, 0x32] {
            self.off_pending = !self.stays_on; // sd_power_system_off: no response; off once it has the TX buffer
            return;
        }
        if let [0, 0x8a, 1, _, _, _, _, _, t0, t1] = *p {
            self.queue(&[1, 0x8a, 0, 0, 0, 0]);
            for (addr, rssi, data) in ADVERTISERS {
                self.queue(&adv_report(addr, rssi, data));
            }
            let trainer = self.ftms.as_ref().filter(|t| t.connectable && !t.connected).map(|t| t.reports());
            for (addr, rssi, data) in trainer.into_iter().flatten() {
                self.queue(&adv_report(addr, rssi, &data));
            }
            self.scan_end = Some(now + 1000 * u64::from(u16::from_le_bytes([t0, t1])));
            return;
        }
        if let [6, op, ch, rest @ ..] = p {
            if self.ant_refuse == Some([*op, *ch]) {
                self.queue(&[1, *op, 0x15, 0, 0, 0]);
                return;
            }
            let ch = usize::from(*ch);
            match (*op, rest) {
                (0xcc, k) if k.len() == 8 => {
                    if let Some(slot) = self.keys.get_mut(ch) {
                        *slot = k.try_into().ok(); // the channel byte is the network number here
                    }
                }
                (0xc2, [_, net, _]) => self.channels[ch].net = Some(*net),
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
                // sd_ant_acknowledge_message_tx: the size, then the 8 bytes (nrf::ant_ack)
                (0xc8, _) if self.ack_silent => return,
                (0xc8, [8, ..]) if self.ack_model && now < self.ack_busy_until => {
                    self.ack_refused += 1;
                    self.queue(&[1, 0xc8, 0x1f, 0x40, 0, 0]); // NRF_ERROR_... TRANSFER_IN_PROGRESS (0x401f)
                    return;
                }
                (0xc8, [8, d @ ..]) if d.len() == 8 && self.channels[ch].device_type == 0x0b => {
                    self.ack_busy_until = now + 250;
                    if self.ack_fail > 0 {
                        self.ack_fail -= 1;
                        self.queue(&[1, 0xc8, 0, 0, 0, 0]);
                        self.ant_event(&[1, 0x40, ch as u8, 1, 6], 6, ch as u8); // EVENT_TRANSFER_TX_FAILED
                        return;
                    }
                    let d: [u8; 8] = d.try_into().unwrap_or([0; 8]);
                    self.meter.got.push(d);
                    let st = self.meter.auto.unwrap_or(0xff);
                    self.meter.reply = match (d[0], d[1], self.meter.answer) {
                        (1, 0xaa, MeterAnswer::Zero(o)) => {
                            let [o0, o1] = o.to_le_bytes();
                            Some(([1, 0xac, st, 0xff, 0xff, 0xff, o0, o1], 3))
                        }
                        (1, 0xaa, MeterAnswer::Fail) => Some(([1, 0xaf, st, 0xff, 0xff, 0xff, 0, 0], 3)),
                        (1, 0xab, _) if self.meter.auto.is_some() => {
                            self.meter.auto = Some(d[2]);
                            Some(([1, 0x12, 2 | d[2], 0xff, 0xff, 0xff, 0xff, 0xff], 3))
                        }
                        _ => None,
                    };
                    self.queue(&[1, 0xc8, 0, 0, 0, 0]);
                    self.ant_event(&[1, 0x40, ch as u8, 1, 5], 5, ch as u8); // EVENT_TRANSFER_TX_COMPLETED
                    return;
                }
                (0xc8, [8, d @ ..]) if d.len() == 8 && self.channels[ch].device_type == 0x11 => {
                    self.ack_busy_until = now + 250;
                    if self.ack_fail > 0 {
                        self.ack_fail -= 1;
                        self.queue(&[1, 0xc8, 0, 0, 0, 0]);
                        self.ant_event(&[1, 0x40, ch as u8, 1, 6], 6, ch as u8); // EVENT_TRANSFER_TX_FAILED
                        return;
                    }
                    if self.ack_lost > 0 {
                        self.ack_lost -= 1;
                        self.queue(&[1, 0xc8, 0, 0, 0, 0]);
                        return;
                    }
                    self.trainer.command(d.try_into().unwrap_or([0; 8]));
                    self.queue(&[1, 0xc8, 0, 0, 0, 0]);
                    self.ant_event(&[1, 0x40, ch as u8, 1, 5], 5, ch as u8); // EVENT_TRANSFER_TX_COMPLETED
                    return;
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
        if self.take_ble(p, now) {
            return;
        }
        let reply: Vec<u8> = match p {
            [0, 0x60] if self.no_mem && !self.cfg.is_empty() => vec![1, 0x60, 4, 0, 0, 0], // NRF_ERROR_NO_MEM
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
    (((v / 10) << 4) | (v % 10)) as u32
}

impl Sim {
    /// Time spent by a transfer of kind `what` ([`PAID`]): the clock moves on by `ns`, and the work counter too when
    /// the CPU works through it (`cpu`; the panel's pixels go by DMA and the CPU sleeps, BOARD.md 6).
    fn pay(&mut self, what: usize, ns: u64, cpu: bool) {
        self.paid_ns[what] += ns;
        if cpu {
            self.awake_ns += ns;
        } else {
            self.waited_ns += ns;
        }
        self.cost_rem += ns;
        self.ms += self.cost_rem / 1_000_000;
        self.cost_rem %= 1_000_000;
    }

    /// The loop's own sleep: the clock moves on, nothing is owed to the app's clock.
    pub fn sleep_ms(&mut self, ms: u32) {
        self.ms += u64::from(ms);
        if let Some(x) = &mut self.nrf_ext {
            x.advance_ms(ms);
        }
        if let Some((dir, n)) = &mut self.record
            && self.dirty
        {
            std::fs::write(dir.join(format!("{:05}.bmp", *n)), bmp(&self.screen)).unwrap();
            *n += 1;
            self.dirty = false;
        }
    }

    /// This firmware's default settings in the backup registers, as after its first start: the next start does not
    /// import stock's ([`App::first_start`]).
    pub fn own_settings(&mut self) {
        self.backup[crate::WHEEL_BKP] = crate::Settings::from_word(0).to_word();
    }

    /// The ride pages of builds before 2026-10-24 (speed with heart rate and cadence; four fields; the map whole),
    /// which the tests of the map, the sensors and the menus were written against.
    pub fn classic_pages(&mut self) {
        use crate::ui::Field::*;
        let pages = [
            crate::PageCfg { layout: 3, fields: [Speed, Hr, Cadence, Time, Time, Time] },
            crate::PageCfg { layout: 4, fields: [Speed, Hr, Cadence, Power, Time, Time] },
            crate::PageCfg { layout: 1, fields: [Map, Time, Time, Time, Time, Time] },
        ];
        for (regs, p) in crate::PAGE_BKP.iter().zip(pages) {
            for (&r, w) in regs.iter().zip(p.to_words()) {
                self.backup[r] = w;
            }
        }
    }

    /// A unit whose RTC reads `h:m:s` at reset, no key pressed.
    pub fn new(h: u64, m: u64, s: u64) -> Self {
        Sim {
            screen: vec![0x1234; (W * H) as usize], // panel RAM is garbage until cleared
            window: (0, W - 1, 0, H - 1),
            cmds: Vec::new(),
            powered: false,
            asleep: true,
            display_on: false,
            panel: PanelRules::default(),
            colmod: 0x06,
            ramctrl: [0x00, 0xf0], // 9.2.1 default; stock ends at 10h
            backlight: false,
            tones: Vec::new(),
            writable: true,
            light_level: 0,
            standby: false,
            ms: 0,
            start_s: h * 3600 + m * 60 + s,
            keys: Box::new(|_| 0),
            record: None,
            dirty: false,
            nordic: Nordic::default(),
            nrf_ext: None,
            chip_held: false,
            nrf_parked: false,
            cts_pull_up: true,
            chip_boot_until: 0,
            expect_reset_bytes: false,
            reset_bytes: 0,
            rtc_sets: 0,
            backup: core::array::from_fn(|i| if i == 18 { 5 << 28 | 1 << 22 } else { 0 }), // AUTO OFF off: tests idle long
            battery_mv: Some(3900),
            charger: false,
            gnss: Gnss::default(),
            pixels: 0,
            windows: 0,
            emmc: EmmcCard::default(),
            bus: Bus::new(),
            touch: TouchChip::new(),
            work_ns_per_byte: 0,
            cost: Cost::default(),
            cost_rem: 0,
            longest_key_gap_ms: 0,
            last_keys_ms: 0,
            sectors_written: 0,
            frozen_s: None,
            rtc_base: crate::days_from_civil(2026, 10, 23),
            paid_ns: [0; 6],
            paid_at_gnss: [0; 6],
            worst_gnss_gap: (0, [0; 6]),
            long_gnss_gaps: (0, [0; 6]),
            awake_ns: 0,
            waited_ns: 0,
            count_waits: true,
            cycles: 0,
            cycles_step: 0,
            adc_calls: [0; 4],
            adc_n: 0,
            adc_fail: None,
            main: LoopModel::default(),
        }
    }

    /// The power lost with the backup domain (a battery run flat): a unit with this card as it is (torn or whole) and
    /// its reliable-write parameters; RAM and the backup registers all zero (`RIDE_BKP` too, AUTO OFF back to its
    /// default), and the RTC, in the same backup domain (RM0399, RTC and backup registers: a backup domain reset
    /// clears both), back to its reset values: 2000-01-01 00:00:00 (RTC_TR 0, RTC_DR 0x2101), until the GNSS sets it.
    pub fn power_loss(&mut self) -> Sim {
        let mut sim = Sim::new(0, 0, 0);
        sim.rtc_base = crate::days_from_civil(2000, 1, 1);
        sim.emmc.image = core::mem::take(&mut self.emmc.image);
        (sim.emmc.rel_param, sim.emmc.rel_sectors) = (self.emmc.rel_param, self.emmc.rel_sectors);
        sim.backup = [0; 32];
        sim
    }

    /// The finger on the touch panel as a function of the simulated time in ms (None: lifted).
    pub fn finger(mut self, f: impl Fn(u64) -> Option<(u16, u16)> + 'static) -> Self {
        self.touch.set_finger(f);
        self
    }

    /// A second finger on the touch panel, as [`Sim::finger`].
    pub fn finger2(mut self, f: impl Fn(u64) -> Option<(u16, u16)> + 'static) -> Self {
        self.touch.set_finger2(f);
        self
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

impl Sim {
    /// What the Nordic chip has sent by now: the scan's end, the sensors' broadcasts, the phone's and the trainer's
    /// traffic (the chip sends as time goes; the board reads it).
    fn nordic_due(&mut self) {
        if self.nordic.scan_end.is_some_and(|end| self.ms >= end) {
            self.nordic.scan_end = None;
            self.nordic.queue(&[2, 0x1b, 0, 0xff, 0xff, 1]); // BLE_GAP_EVT_TIMEOUT, source scan
        }
        self.nordic.ant_tick(self.ms);
        self.nordic.phone_tick(self.ms);
        self.nordic.ftms_tick(self.ms);
    }
}

impl Sim {
    /// The chip's reset: what the model keeps across it, and a new life.
    fn chip_reset(&mut self) {
        if let Some(x) = &mut self.nrf_ext {
            x.reset();
            return;
        }
        let sensors = std::mem::take(&mut self.nordic.sensors);
        let mut phone = self.nordic.phone.take();
        if let Some(p) = &mut phone {
            p.hang_up();
        }
        let mut ftms = self.nordic.ftms.take();
        if let Some(t) = &mut ftms {
            t.hang_up();
        }
        let no_mem = self.nordic.no_mem;
        let (dead, refuse, ant_refuse) = (self.nordic.dead, self.nordic.refuse, self.nordic.ant_refuse);
        let (die_at_ant, stays_on) = (self.nordic.die_at_ant, self.nordic.stays_on);
        let trainer = std::mem::take(&mut self.nordic.trainer);
        let meter = std::mem::take(&mut self.nordic.meter);
        let (ack_model, ack_silent) = (self.nordic.ack_model, self.nordic.ack_silent);
        let (mut resets, chaos) = (std::mem::take(&mut self.nordic.resets), self.nordic.chaos);
        let (limits, hangs, high_water) = (self.nordic.limits, self.nordic.hangs, self.nordic.high_water);
        let (expect_hang, table_only) = (self.nordic.expect_hang, self.nordic.table_only);
        resets.push(self.ms);
        self.nordic = Nordic {
            die_at_ant,
            stays_on,
            dead,
            refuse,
            ant_refuse,
            sensors,
            trainer,
            meter,
            ack_model,
            ack_silent,
            phone,
            ftms,
            no_mem,
            reset: true,
            resets,
            chaos,
            limits,
            hangs,
            high_water,
            expect_hang,
            table_only,
            ..Nordic::default()
        };
    }
}

impl Board for Sim {
    const CPU_HZ: u32 = 200_000_000; // main.rs's: cycle maths is tested at the unit's clock
    fn panel_power_on(&mut self) {
        self.powered = true;
        self.ms += 150;
        self.panel = PanelRules::default();
    }

    fn panel_cmd(&mut self, cmd: u8, params: &[u8]) {
        assert!(self.powered, "command 0x{cmd:02x} to an unpowered panel");
        self.pay(P_PANEL, self.cost.panel_cmd_ns + params.len() as u64 * self.cost.panel_byte_ns, true);
        self.panel.command(cmd, self.ms);
        self.cmds.push(cmd);
        match cmd {
            SWRESET => (self.asleep, self.display_on, self.ramctrl) = (true, false, [0x00, 0xf0]),
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
        assert_eq!(self.ramctrl[0] & 0x13, 0, "RAMCTRL routes RAM writes to the RGB interface, not SPI");
        assert_eq!(self.colmod & 0x07, 0x05, "pixel format is not 16-bit");
        let (x0, x1, y0, y1) = self.window;
        assert_eq!(count, (x1 - x0 + 1) * (y1 - y0 + 1), "pixel count does not fill the window");
        self.pay(P_PANEL, self.cost.panel_cmd_ns, true);
        self.panel.ram_write(self.ms, self.asleep || !self.display_on);
        self.pay(P_PANEL, u64::from(count) * self.cost.panel_pixel_ns, false);
        self.pixels += u64::from(count);
        self.windows += 1;
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
        self.longest_key_gap_ms = self.longest_key_gap_ms.max(self.ms - self.last_keys_ms);
        self.last_keys_ms = self.ms;
        (self.keys)(self.ms)
    }

    fn rtc(&mut self) -> (u32, u32) {
        // the calendar from `rtc_base` (2026-10-23, or 2000-01-01 after a power loss) at the start; the day rolls over
        let t = self.start_s + self.frozen_s.unwrap_or(self.ms / 1000);
        let (y, mo, d) = crate::civil_from_days(self.rtc_base + (t / 86400) as i32);
        let t = t % 86400;
        let tr = to_bcd(t / 3600) << 16 | to_bcd(t / 60 % 60) << 8 | to_bcd(t % 60);
        (tr, to_bcd(u64::from(y) % 100) << 16 | to_bcd(u64::from(mo)) << 8 | to_bcd(u64::from(d)))
    }

    fn rtc_set(&mut self, tr: u32, dr: u32) {
        let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), crate::rtc_date_from_dr(dr));
        let days = i64::from(crate::days_from_civil(y, mo, d) - self.rtc_base);
        let t = days * 86400 + i64::from(h) * 3600 + i64::from(m) * 60 + i64::from(s);
        assert!(t >= (self.ms / 1000) as i64, "the simulated clock starts on its base day");
        self.start_s = t as u64 - self.ms / 1000;
        self.rtc_sets += 1;
    }

    /// A delay inside the work is a wait the app's clock must count ([`Board::waited_us`]), when
    /// [`Sim::count_waits`] is on; the loop's own sleep is [`Sim::sleep_ms`].
    fn delay_ms(&mut self, ms: u32) {
        if self.count_waits {
            self.waited_ns += u64::from(ms) * 1_000_000;
        }
        self.sleep_ms(ms);
    }

    /// Strict about the radio chip (BOARD.md 11 "Power off"): once it has run, it must be in System OFF with the UART
    /// lines parked, or (when it would not go) held in reset with them left as they were; never running.
    fn standby(&mut self) {
        self.standby = true;
        if self.nrf_ext.is_none() && self.nordic.reset {
            let n = &self.nordic;
            assert!(n.off || self.chip_held || n.dead, "the Nordic chip is left powered and running at the switch-off");
            assert!(
                self.nrf_parked || !n.off,
                "the USART2 lines are left driven into the Nordic chip at the switch-off"
            );
            assert!(!self.nrf_parked || n.off, "the USART2 lines parked although the chip was not seen off");
            assert!(self.cts_pull_up, "PD3 left pulled down");
        }
    }

    /// The chip resets as the line is released (the library holds it 150 ms and waits 800 ms of boot meanwhile).
    fn nrf_hold(&mut self, held: bool) {
        self.chip_held = held;
        self.nrf_parked = false;
        self.cts_pull_up = true;
        if held {
            self.nordic.txq.clear(); // TXFRQ, as main.rs
            return;
        }
        // our TX FIFO is not flushed by the peer's reset (RM0399: only a USART reset or TXFRQ does): what it holds
        // reaches the fresh chip, which is booting
        let stale = std::mem::take(&mut self.nordic.txq);
        self.chip_reset();
        self.chip_boot_until = self.ms + 800;
        if !stale.is_empty() {
            self.reset_bytes += stale.len() as u32;
            assert!(self.expect_reset_bytes, "{} bytes left in the TX FIFO reach the chip at its reset", stale.len());
            self.nordic.txq = stale;
        }
    }

    fn nrf_send(&mut self, data: &[u8]) -> usize {
        if let Some(x) = &mut self.nrf_ext {
            x.send(data);
            return data.len();
        }
        assert!(self.nordic.reset, "bytes to the Nordic chip before its reset");
        if !data.is_empty() && (self.chip_held || self.ms < self.chip_boot_until) {
            self.reset_bytes += data.len() as u32;
            assert!(self.expect_reset_bytes, "{} bytes sent to the Nordic chip while it resets or boots", data.len());
            return data.len();
        }
        self.nordic.send(data, self.ms)
    }

    fn adc_read(&mut self, channel: u8) -> Option<u16> {
        if let Some(slot) = self.adc_calls.get_mut(usize::from(self.adc_n)) {
            *slot = channel;
        }
        self.adc_n = self.adc_n.saturating_add(1);
        // VDDA 3.3 V: VREFINT reads its calibration value; the pin sees the battery through the divider by 1.5
        if self.adc_fail == Some(channel) {
            return None;
        }
        let mv = self.battery_mv?;
        Some(if channel == 19 { 24_000 } else { (u32::from(mv) * 2 * 65535).div_ceil(3 * 3300) as u16 })
    }

    fn vrefint_cal(&mut self) -> u16 {
        24_000
    }

    fn gnss_power(&mut self, on: bool) {
        self.gnss.power(on, self.ms);
    }

    fn gnss_transfer(&mut self, buf: &mut [u8]) {
        assert_eq!(buf.len() % 4, 0, "SPI transfers move 4 bytes at a time");
        self.pay(P_GNSS, self.cost.gnss_xfer_ns + buf.len() as u64 * self.cost.gnss_byte_ns, true);
        if self.gnss.powered {
            let gap = self.ms.saturating_sub(self.gnss.last_xfer);
            let paid: [u64; 6] = core::array::from_fn(|i| self.paid_ns[i] - self.paid_at_gnss[i]);
            if gap > self.worst_gnss_gap.0 {
                self.worst_gnss_gap = (gap, paid);
            }
            if gap > 1500 {
                self.long_gnss_gaps.0 += 1;
                self.long_gnss_gaps.1 = core::array::from_fn(|i| self.long_gnss_gaps.1[i] + paid[i]);
            }
        }
        self.paid_at_gnss = self.paid_ns;
        let (tr, dr) = self.rtc();
        let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), crate::rtc_date_from_dr(dr));
        let utc = crate::epoch_s((y, mo, d, h, m, s)) - 2 * 3600; // the RTC runs on UTC + 2 h
        self.gnss.transfer(self.ms, utc, buf);
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

    fn emmc_power_on(&mut self) {
        let e = &mut self.emmc;
        (e.powered, e.host_fast, e.host_wide) = (true, false, false);
        self.ms += 2;
    }

    fn emmc_bus(&mut self, fast: bool, wide: bool) {
        (self.emmc.host_fast, self.emmc.host_wide) = (fast, wide);
    }

    fn emmc_cmd(&mut self, index: u8, arg: u32, resp: crate::emmc::Resp) -> Result<[u32; 4], crate::emmc::BusError> {
        let want = expected_resp(index);
        assert!(
            want == resp,
            "eMMC command {index}: the driver waits for {resp:?}, the command's response is {want:?}"
        );
        self.emmc.now = self.ms;
        self.emmc.cmd(index, arg)
    }

    fn emmc_read(&mut self, index: u8, arg: u32, buf: &mut [u8]) -> Result<u32, crate::emmc::BusError> {
        self.pay(P_READ, buf.len() as u64 * self.work_ns_per_byte, true); // the work takes time as well
        self.emmc.now = self.ms;
        self.emmc.read(index, arg, buf)
    }

    fn cycles(&mut self) -> u32 {
        // the awake time at the unit's 200 MHz (Board::CPU_HZ): a cycle is 5 ns; cycles_step moves it on a read as well
        self.cycles = self.cycles.wrapping_add(self.cycles_step);
        ((self.awake_ns / 5) as u32).wrapping_add(self.cycles)
    }

    fn waited_us(&mut self) -> u32 {
        let us = self.waited_ns / 1000;
        self.waited_ns %= 1000;
        us.min(u64::from(u32::MAX)) as u32
    }

    fn emmc_writable(&self) -> bool {
        self.writable
    }

    fn emmc_write(&mut self, index: u8, arg: u32, buf: &[u8]) -> Result<u32, crate::emmc::BusError> {
        let sectors = buf.len().div_ceil(512) as u64;
        self.pay(P_WRITE, self.cost.write_cmd_ns + sectors * self.cost.write_sector_ns, true);
        let before = self.sectors_written;
        self.sectors_written += sectors;
        self.emmc.now = self.ms;
        let r = self.emmc.write(index, arg, buf);
        let c = self.cost;
        if c.stall_every > 0 && self.sectors_written / c.stall_every > before / c.stall_every {
            // the card busy after this write (emmc_model.rs E1): the driver's CMD13 polls wait it out on the clock,
            // asleep between them (delay_ms); counted here as the stall's time
            self.emmc.busy_until = self.emmc.busy_until.max(self.ms + c.stall_ms);
            self.paid_ns[P_STALL] += c.stall_ms * 1_000_000;
        }
        r
    }
    fn i2c_read(&mut self, addr: u8, reg: u8, buf: &mut [u8]) -> bool {
        self.pay(P_I2C, (3 + buf.len() as u64) * self.cost.i2c_byte_ns + self.cost.i2c_frame_ns, true);
        self.bus.read(self.ms, addr, reg, buf)
    }

    fn i2c_write(&mut self, addr: u8, reg: u8, data: &[u8]) -> bool {
        self.pay(P_I2C, (2 + data.len() as u64) * self.cost.i2c_byte_ns + self.cost.i2c_frame_ns, true);
        self.bus.write(self.ms, addr, reg, data)
    }

    fn i2c_recover(&mut self) -> bool {
        self.pay(P_I2C, 100_000, true); // nine pulses at 100 kHz and the STOP: busy-waiting CPU time
        self.bus.recover()
    }

    fn touch_read(&mut self, reg: u8, buf: &mut [u8]) -> bool {
        self.pay(P_I2C, (3 + buf.len() as u64) * self.cost.i2c_byte_ns + self.cost.i2c_frame_ns, true);
        self.touch.read(self.ms, reg, buf)
    }

    fn touch_reset(&mut self, run: bool) {
        self.touch.reset(run, self.ms);
    }

    fn touch_int(&mut self) -> bool {
        self.touch.int(self.ms)
    }

    fn nrf_recv(&mut self, buf: &mut [u8]) -> usize {
        if let Some(x) = &mut self.nrf_ext {
            return x.recv(buf);
        }
        self.nordic_due();
        self.nordic.recv(buf, self.ms)
    }

    /// The bridge ([`Sim::nrf_ext`]) has no line state: its TX reads sent and its RTS released (floating), so the
    /// switch-off's checks read "off" there and are not exercised (they are on the model).
    fn nrf_sent(&mut self) -> bool {
        self.nrf_ext.is_some() || self.nordic.sent(self.ms)
    }

    /// PD3 low. The chip drives its RTS while it runs, hung included (held high); in System OFF it lets go and the line
    /// follows our pull.
    fn nrf_cts(&mut self) -> bool {
        if self.nrf_ext.is_some() || self.nordic.off {
            return !self.cts_pull_up;
        }
        self.nordic.cts(self.ms)
    }

    fn nrf_cts_pull(&mut self, up: bool) {
        self.cts_pull_up = up;
    }

    fn nrf_park(&mut self) {
        self.nrf_parked = true;
    }
}

/// Passes of the main loop for `ms`: a step, then the sleep (the test helper every sim test module shares).
fn run(sim: &mut Sim, app: &mut App, ms: u64) {
    let end = sim.ms + ms;
    while sim.ms < end {
        app.step(sim);
        sim.sleep_ms(POLL_MS);
    }
}

#[cfg(test)]
mod assist_tests;
#[cfg(test)]
mod budget_tests;
#[cfg(test)]
mod chip_link;
#[cfg(test)]
mod chip_link_tests;
#[cfg(test)]
mod chip_table;
#[cfg(test)]
mod chip_table_tests;
#[cfg(test)]
mod course_tests;
#[cfg(test)]
mod curve_tests;
#[cfg(test)]
mod gnss_model_tests;
#[cfg(test)]
mod gnss_serve;
#[cfg(test)]
mod laps_tests;
#[cfg(test)]
mod map_tests;
#[cfg(test)]
mod menu_tests;
#[cfg(test)]
mod nrf_diff_tests;
#[cfg(test)]
mod nrf_emu_tests;
#[cfg(test)]
mod panel_model_tests;
#[cfg(test)]
mod perf_tests;
#[cfg(test)]
mod pmc_tests;
mod poi_tests;
#[cfg(test)]
mod ride_radio_tests;
#[cfg(test)]
mod ride_tests;
#[cfg(test)]
mod segment_tests;
#[cfg(test)]
mod start_cue_tests;
#[cfg(test)]
mod stock_tests;
#[cfg(test)]
mod stop_tests;
#[cfg(test)]
mod storage_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod walk_tests;
#[cfg(test)]
mod workout_tests;

mod ble_sim;
pub use ble_sim::{Act, FtmsSim, PhoneSim};
mod loop_model;
pub use loop_model::{LoopModel, SimLink};

#[cfg(test)]
mod filesvc_tests;
#[cfg(test)]
mod phone_tests;

#[cfg(test)]
mod link_tests;

#[cfg(test)]
mod altitude_tests;

#[cfg(test)]
mod fecble_tests;
#[cfg(test)]
mod ftms_tests;
#[cfg(test)]
mod interaction_tests;
#[cfg(test)]
mod overview_tests;
#[cfg(test)]
mod power_tests;
#[cfg(test)]
mod powercal_tests;
#[cfg(test)]
mod ramp_tests;
#[cfg(test)]
mod shift_tests;
#[cfg(test)]
mod trainer_tests;

#[cfg(test)]
mod page_tests;
#[cfg(test)]
mod status_tests;
#[cfg(test)]
mod switch_tests;

#[cfg(test)]
mod settings_tests;

#[cfg(test)]
mod places_tests;

#[cfg(test)]
mod bikes_tests;

#[cfg(test)]
mod theme_tests;

/// The computer at the other end of the dev link: what it received, and the staging area.
#[derive(Default)]
pub struct Pc {
    pub got: Vec<u8>,
    pub staged: Vec<u8>,
    /// Registers poked, read back by peek (0 if never written).
    pub regs: std::collections::BTreeMap<u32, u32>,
    /// Addresses peeked, in order (the fuzz checks each is aligned and in range).
    pub peeked: Vec<u32>,
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
        self.peeked.push(addr);
        self.regs.get(&addr).copied().unwrap_or(0)
    }
    fn poke(&mut self, addr: u32, value: u32) {
        self.regs.insert(addr, value);
    }
}

#[cfg(test)]
mod dev_tests;

#[cfg(test)]
mod emmc_tests;

#[cfg(test)]
mod fuzz;

#[cfg(test)]
mod recorder_tests;

#[cfg(test)]
mod saver_tests;

#[cfg(test)]
mod drive_tests;

#[cfg(test)]
mod diag_tests;

mod key_model;
#[cfg(test)]
mod peripheral_tests;
