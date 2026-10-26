//! Everything the firmware does, written against [`Board`]: the few operations that touch hardware.
//!
//! `main.rs` implements [`Board`] with registers on the unit; `sim.rs` implements it on the host, with a model
//! of the panel, so the whole app (power-up sequence, drawing protocol, clock, keys, power off) runs in
//! `cargo test-host`. Rule: logic goes here, `main.rs` only moves bytes and reads pins (AGENTS.md).
//!
//! This file holds the [`Board`] trait and [`App`]: its state, [`App::start`] and the main loop's pass
//! ([`App::step`]). The rest of the app is in `app/`, one `impl App` per concern (app.rs lists them).

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod alerts;
pub mod altitude;
mod app;
pub mod course;
pub mod dev;
pub mod drivers;
pub mod hrv;
pub mod msc;
pub mod page;
pub mod phone;
pub mod power;
pub mod power_curve;
pub mod ride;
#[cfg(test)]
mod sim;
pub mod storage;
pub mod trainer;
pub mod ui;
pub mod workout;

// the modules by their own names too (crate::ambient, bryton_rustfw::fit): the folders only sort the files
pub use drivers::{ambient, battery, ble, emmc, fec, fecble, ftms, gnss, nrf, panel, sensors, touch};
pub use storage::{fat, fit, fitread, history, ign, ini, recorder, stock, trash};
pub use ui::{fonts, map, menu, text, tiles};
// the app's parts (app.rs), at the root as before the split
pub use app::*;

use menu::{Input, Setting};
use nrf::SensorId;
use panel::Panel;
use sensors::{Decoders, Kind, Live, Metric};
use text::Line;
use touch::{Dir, Gesture};
use ui::{Canvas, Field, Rec, Row, Strip, UsbDot};

/// The hardware the app needs, as primitive operations. Each one is a few register writes on the unit.
// ponytail: one trait while the board is small; split per peripheral (Panel, Rtc, Keys...) when it grows.
pub trait Board {
    /// Panel power on and reset pulse, with the waits the panel needs before its first command.
    fn panel_power_on(&mut self);
    /// One panel command and its parameters (D/C low for the command byte, high for the parameters).
    fn panel_cmd(&mut self, cmd: u8, params: &[u8]);
    /// RAMWR (0x2c) then `count` pixels, each from `next` (called once per pixel, in order), into the window set by
    /// CASET / RASET.
    fn panel_ram_write(&mut self, count: u32, next: &mut dyn FnMut() -> u16);
    /// RAMWR then `count` pixels of one colour (a fill: no per-pixel callback on the unit).
    fn panel_fill(&mut self, count: u32, color: u16) {
        self.panel_ram_write(count, &mut || color);
    }
    /// Backlight at `level` percent (0: off).
    fn backlight(&mut self, level: u8);
    /// The buzzer sounding a tone at `hz` (0: silence).
    fn beep(&mut self, hz: u32);
    /// The keys held down, as a mask of [`Key`] codes.
    fn keys_down(&mut self) -> u8;
    /// RTC_TR and RTC_DR, read once the shadow registers are in sync (RSF): the time of day and the date, BCD.
    fn rtc(&mut self) -> (u32, u32);
    /// Sets the RTC to RTC_TR / RTC_DR values ([`rtc_words`]): the clock keeps running from there.
    fn rtc_set(&mut self, tr: u32, dr: u32);
    /// Busy wait.
    fn delay_ms(&mut self, ms: u32);
    /// Microseconds the CPU has been awake (working, not asleep in a wait) since start-up, wrapping: what a pass's
    /// work took, so the app can bound its background work by time and count a key's hold in real time (a long
    /// pass is longer than [`POLL_MS`]). The DWT cycle counter on the unit. 0 always: work is not timed.
    fn awake_us(&mut self) -> u32 {
        0
    }
    /// The main loop's last sleep between passes: how long it lasted, ms (wall time), and the work done in it, us
    /// (USB served while a cable is in), which [`Board::awake_us`] counts too. A held key counts the sleep by its
    /// length, not that work again. Default: [`POLL_MS`] and none.
    fn last_sleep(&mut self) -> (u32, u32) {
        (POLL_MS, 0)
    }
    /// Standby, woken by the power button (a reset). Never returns on the unit; the simulation records it.
    fn standby(&mut self);
    /// Nordic chip: resets it as stock does with its UART up (BOARD.md 11), and returns once it can take commands.
    fn nrf_reset(&mut self);
    /// Bytes to the Nordic chip, in order (the chip's flow control may hold them a little).
    fn nrf_send(&mut self, data: &[u8]);
    /// Bytes received from the Nordic chip, at most `buf.len()`; never waits.
    fn nrf_recv(&mut self, buf: &mut [u8]) -> usize;
    /// The battery as ADC3 reads it (BOARD.md 12): the battery channel, VREFINT, and VREFINT_CAL (VREFINT at
    /// VDDA = 3.3 V), all 16-bit; None if the ADC did not come up. Takes about half a millisecond.
    fn battery_adc(&mut self) -> Option<(u16, u16, u16)>;
    /// GNSS receiver power (PI6, BOARD.md 13).
    fn gnss_power(&mut self, on: bool);
    /// Exchanges `buf` with the GNSS receiver over SPI, in place; `buf.len()` is a multiple of 4. The receiver
    /// sends 0xff when it has nothing.
    fn gnss_transfer(&mut self, buf: &mut [u8]);
    /// USB power (charger) present.
    fn charger_present(&mut self) -> bool;
    /// RTC backup register `n` (0..32): kept through resets and power off (BOARD.md 8).
    fn backup_read(&mut self, n: usize) -> u32;
    /// Writes RTC backup register `n`.
    fn backup_write(&mut self, n: usize, v: u32);
    /// eMMC controller (SDMMC1, BOARD.md 15) from reset: pins, kernel clock, powered, bus 1 bit at 400 kHz, and the
    /// wait the card needs before its first command. The card keeps the state it was in.
    fn emmc_power_on(&mut self);
    /// eMMC bus clock: 400 kHz (identification) or 25 MHz (`fast`); width 1 or 4 bits (`wide`).
    fn emmc_bus(&mut self, fast: bool, wide: bool);
    /// Sends eMMC command `index` with `arg` and waits for the response `resp` names: its words (R2: bits 127..96
    /// first; others: word 0), all zero for [`emmc::Resp::None`].
    fn emmc_cmd(&mut self, index: u8, arg: u32, resp: emmc::Resp) -> Result<[u32; 4], emmc::BusError>;
    /// Sends eMMC command `index` with `arg`, a read of `buf.len()` bytes (whole 512-byte blocks) whose R1 answer
    /// it returns, and reads the data into `buf`, every wait bounded.
    fn emmc_read(&mut self, index: u8, arg: u32, buf: &mut [u8]) -> Result<u32, emmc::BusError>;
    /// Sends eMMC command `index` with `arg`, a write of `buf` (whole 512-byte blocks) whose R1 answer it returns,
    /// and sends the data, every wait bounded. The card's programming busy is left to the caller (CMD13).
    fn emmc_write(&mut self, index: u8, arg: u32, buf: &[u8]) -> Result<u32, emmc::BusError>;
    /// eMMC writes are allowed in this build (on the unit, only with EMMC_WRITE=1 until the first writes are done
    /// with the owner): without, `emmc::Card::write` refuses every write (the one gate), and the recorder does not
    /// touch the card and says so at a ride's start.
    fn emmc_writable(&self) -> bool {
        true
    }
    /// Reads `buf.len()` bytes from register `reg` on, of the chip at 7-bit address `addr` on the sensor bus (I2C4,
    /// BOARD.md 16); false if it did not answer (no acknowledge, or a time-out: it never hangs).
    fn i2c_read(&mut self, addr: u8, reg: u8, buf: &mut [u8]) -> bool;
    /// Writes `data` to register `reg` on, of the chip at `addr`; false if it did not answer.
    fn i2c_write(&mut self, addr: u8, reg: u8, data: &[u8]) -> bool;
    /// Reads `buf.len()` bytes from register `reg` on, of the touch controller (I2C1 address 0x55, BOARD.md 17);
    /// false if it did not answer.
    fn touch_read(&mut self, reg: u8, buf: &mut [u8]) -> bool;
    /// The touch controller's reset line (PH3): `run` releases it, false holds the chip in reset.
    fn touch_reset(&mut self, run: bool);
    /// The touch controller pulls its INT line (PC6) low: it has a point to read.
    fn touch_int(&mut self) -> bool;
}

/// The app: what is on screen, the keys, the sensors and what the Nordic chip reported.
pub struct App {
    keys: Keys,
    /// The level the backlight is at (0: off).
    light_shown: u8,
    /// Since the last key press.
    idle_ms: u32,
    /// The chips found on the sensor bus.
    chips: ambient::Chips,
    /// The automatic backlight's state, and the level it asks for.
    auto: ambient::Auto,
    auto_level: u8,
    /// Since the last light measurement started; whether it is still running.
    light_ms: u32,
    measuring: bool,
    /// The barometer's last reading: hPa x 10, C x 100.
    baro: Option<(u32, i16)>,
    /// The barometric altitude and its calibration (altitude.rs); since the barometer was last read; the file.
    alt: altitude::Altimeter,
    baro_ms: u32,
    alt_store: AltStore,
    /// The RTC's seconds (the main screen shows them).
    seconds: u8,
    /// What the status strip shows, and what it showed when last drawn.
    strip: Strip,
    strip_shown: Strip,
    screen: Screen,
    ble: Option<nrf::Version>,
    rx: nrf::Rx,
    /// The Nordic link's watchdog (app/link.rs), and the keys to start ANT again with.
    watch: Watch,
    ant_key: Option<[u8; 8]>,
    di2_key: Option<[u8; 8]>,
    nearby: nrf::Nearby,
    scanning: bool,
    redraw_ms: u32,
    list_dirty: bool,
    /// ANT started (needs BLE and a built-in ANT+ key).
    ant: bool,
    /// Shimano's network up (its key built in): the Di2 channel can open.
    di2: bool,
    sensors: [Sensor; Kind::ALL.len()],
    /// Since each sensor's last broadcast.
    heard_ms: [u32; Kind::ALL.len()],
    /// Since each sensor's channel was closed by its search time-out (`u32::MAX`: not resting).
    resting_ms: [u32; Kind::ALL.len()],
    /// The gear last heard (front, rear: [`Metric::Gear`]), to count the shifts and record them.
    gear: Option<u16>,
    /// Gear changes in this ride.
    shifts: u16,
    /// This ride's profile (app/profile.rs), whether the rider set it by hand (the trainer's silence no longer
    /// changes it), and the seconds the trainer has been silent while it is indoor and automatic.
    profile: ride::Profile,
    profile_by_hand: bool,
    trainer_quiet_s: u16,
    /// Channels that sent data while searching: ask the chip which sensor it found.
    ask: u8,
    /// The home trainer (trainer.rs, app/trainer.rs), the trainer screen as drawn, and whether it was heard last pass.
    trainer: trainer::Trainer,
    trainer_shown: ui::trainer::Shown,
    trainer_up: bool,
    /// The pairing file's state, the time since the last acknowledged message, whether the second ticked since the
    /// screen was drawn, and whether the chip never answered one (control off for this power-up).
    sensors_store: SensorStore,
    ack_gap_ms: u32,
    trainer_second: bool,
    trainer_no_control: bool,
    /// The Bluetooth trainer's link (app/ftms.rs).
    ftms: Ftms,
    live: Live,
    /// The zones the plain power fields (3, 10 and 30 s) and the heart rate show, with hysteresis ([`App::field_bar`]).
    zones: [ride::ZoneHold; 4],
    decoders: Decoders,
    /// RMSSD and DFA alpha1 from the strap's beats.
    hrv: hrv::Hrv,
    battery: battery::Battery,
    /// Since the last battery reading.
    battery_ms: u32,
    gnss: gnss::Parser,
    /// The last fix, and its age.
    pvt: Option<gnss::Pvt>,
    pvt_ms: u32,
    /// Since the app's first step: the receiver is powered at [`GNSS_ON_MS`], its configuration and aiding sent
    /// once from [`GNSS_CONFIG_MS`].
    gnss_ms: u32,
    /// Time run since the start, ms (saturating): a minute of it clears the fault count.
    up_ms: u32,
    /// Messages sent to the receiver so far: configuration, time, position.
    gnss_sent: u8,
    /// The receiver is switched off for an indoor ride (app/gps.rs `gnss_switch`).
    gnss_off: bool,
    /// The RTC's offset from UTC and the last position, as kept in backup registers.
    aid: gnss::Aid,
    /// Wheel, backlight, max heart rate, FTP.
    settings: Settings,
    /// The selected menu row's number is being changed (Lap up, Page down, Start or Back to leave).
    editing: bool,
    /// The selected menu row was chosen (by a key or a tap) since the menu opened: a tap on it acts. A tap on another
    /// row, or on the row a menu opens on, only selects it (one stray touch never acts).
    menu_chosen: bool,
    /// A row that cannot be undone (forget a sensor or the phone, import stock's settings) asked to be confirmed: Start
    /// on it again acts; any other input clears it (App::confirmed). A past ride's DELETE, asked on its card, is
    /// [`PAST_DELETE`].
    confirm: Option<(Menu, usize)>,
    /// A held key already acted (a list scrolled, a value stepped), so its release does not act once more; cleared
    /// when no key is down.
    repeated: bool,
    /// The screens under this one, the latest last: Back returns to them, with their selection. At most six now (a
    /// page, the ride menu, ALL SETTINGS, PAGES, a page, its field groups, under a group), so eight is room to spare.
    under: [Screen; 8],
    depth: usize,
    /// The ride pages (the first `page_count`), then the overview's fields ([`OVERVIEW`]).
    pages: [page::Page; PAGES + 1],
    page_count: usize,
    /// Each ride page's MORE variant when the rider made one, else [`page::Page::more_of`] (app/more.rs).
    more: [Option<page::Page>; PAGES],
    /// The INDOOR profile's pages with their MORE variants, in the file only (app/profile.rs, app/editor.rs).
    indoor: page::Set,
    /// The set PAGES and the editor show while no ride is open: false ROAD, true INDOOR (app/profile.rs).
    pages_indoor: bool,
    /// The set the ride pages on screen were taken from at the last pass ([`App::indoors`] then).
    set_shown: bool,
    /// The ride page whose MORE variant is up: only while that page is the screen (App::more_shown).
    more_on: Option<usize>,
    /// `\Rustfw\pages.bin` (app/editor.rs): read yet, to be written, its copies.
    store: PageStore,
    /// The page editor's work, while it is open.
    edit: Option<Editing>,
    /// The strip's place shows the editor's hint (app/editor.rs), not the strip.
    hint_shown: bool,
    /// Today (RTC), for the overview.
    date: (u16, u8, u8),
    /// The latest notices, newest first (empty lines: none).
    notices: [Line<40>; ui::NOTICES],
    /// The banner over the screen (the latest notice) and how long it has been up; drawn yet. Nothing under it is
    /// redrawn meanwhile; when it goes, the screen is drawn again.
    banner_ms: Option<u32>,
    /// What the banner says (two lines at most) and its kind (an alarm white on red); the one waiting for it to go.
    banner: Line<40>,
    banner_kind: Banner,
    banner_next: Option<(Line<40>, Banner)>,
    banner_drawn: bool,
    /// The GPS had a good fix since power-on; the battery was reported low.
    had_fix: bool,
    told_low: bool,
    /// A ride was just started from a key: the page is drawn again (its map and graphs start over).
    new_ride: bool,
    /// The card's map and elevation profile as drawn (set when the card is shown).
    summary: Option<(map::Shown, GraphShown)>,
    /// Reminders and alerts of the ride, the one due (said and beeped at the next pass), and the one tone player
    /// (alerts, courses, workouts).
    alerts: alerts::Alerts,
    alert: Option<alerts::Alert>,
    tones: Tones,
    /// A first hold of Back during a ride armed switching off for this long ([`OFF_CONFIRM_MS`]).
    off_armed_ms: u32,
    /// When the ride started: date, hours and minutes (its card's title).
    ride_started: ((u16, u8, u8), (u8, u8)),
    /// The graphs show the whole ride rather than the last minutes (a hold on a graph toggles it).
    graph_whole: bool,
    /// A graph's plot chosen by a tap, per page (the overview last) and box: shown instead of the page's until the
    /// power goes or the pages change, never saved (the owner's decision, UI review step 7).
    plot_taps: [[Option<Field>; page::CELLS]; PAGES + 1],
    /// The page's boxes as drawn ([`App::widgets`]).
    drawn: [Drawn; BOXES],
    /// The ride: timer, totals, laps.
    ride: ride::Ride,
    /// Heart rate and power over the last minutes, and the graphs on screen by ride field.
    history: ride::History,
    /// Rolling power, NP, TSS, work, W' balance.
    power: power::Power,
    /// The menu rows on screen, as fingerprints ([`App::menu_row_print`]).
    menu_shown: [u32; ui::MENU_ROWS],
    /// The ride's track, from the good fixes.
    track: map::Track,
    /// The map's zoom level set by its "+" and "-" ([`map::mpp16`]); None: fitted to the track (per power-up).
    map_zoom: Option<u32>,
    /// Zoomed in and panned by hand: the map's centre (metres on the track's plane); None: it follows the rider.
    map_centre: Option<(i32, i32)>,
    /// The eMMC, for the basemap and the dev link (identified at first use, never at boot).
    emmc: emmc::Emmc,
    /// Roads and water under the first map field of a page, drawn once per view.
    basemap: tiles::Basemap,
    /// The touch controller answered at the start; the gestures from its points.
    touch_ok: bool,
    touch: touch::Recogniser,
    /// A gesture that ended during a USB drive command, acted on after it ([`App::touch`]).
    touch_pending: Option<Input>,
    /// The panel was sampled during a drive command: the main loop's next `dt_ms` (its sleep since its own last
    /// read) is time those samples already counted.
    touch_sampled: bool,
    /// The scan of stock's rides (the stock data menu, the overview's totals), over its cache when main.rs gives
    /// it one ([`App::set_scan_cache`]).
    stock: stock::Scan,
    /// [`Board::awake_us`] at the start of the last pass: the work since, plus the sleep ([`Board::last_sleep`]),
    /// is the time a held key counts.
    awake_at: u32,
    /// The length of this pass, ms: the sleep plus the work since the last ([`App::step`] sets it once, capped at
    /// [`MAX_PASS_MS`]); every timer counts it, never a fixed [`POLL_MS`], so a long pass (a slow card) or a
    /// shorter or longer sleep (stop mode) keeps them right.
    dt_ms: u32,
    /// The card of the past ride shown ([`Screen::Past`]).
    past: history::Past,
    /// A ride's delete the last run may have left unfinished: looked for when the scan of the rides first runs
    /// ([`trash::finish`]).
    trash_check: bool,
    /// Workouts: those planned from the day it was read on; the one opened (its steps menu) or running, and its
    /// file; where it is (workout.rs).
    calendar: storage::workout::Calendar,
    calendar_day: Option<(u16, u8, u8)>,
    plan: storage::workout::Workout,
    plan_file: Option<fat::File>,
    player: Option<workout::Player>,
    /// The ride recorder (FIT file, checkpoints, save), and a ride to look for on the card at the first step.
    /// The card may be written ([`Board::emmc_writable`]); without, the recorder is not used at all.
    card_writable: bool,
    recorder: recorder::Recorder,
    resume_pending: bool,
    /// The ride's full copy (developer fields: recorder.rs, "Two copies"), driven after `recorder`; a ride of its
    /// own to look for on the card once the clean one's is found.
    full: recorder::Recorder,
    full_pending: bool,
    /// While seconds are caught up after a long pass: how many seconds ago the one being ticked was (its time).
    lag_s: u8,
    /// The courses on the card, the one followed and where the rider is on it (memory from main.rs:
    /// [`App::set_course_memory`]).
    courses: Option<&'static mut storage::course::Courses>,
    /// The USB drive (msc.rs): the medium is in while the card is lent ([`emmc::Emmc::lend`], app/drive.rs).
    drive: msc::Drive,
    /// A system reset is wanted (after the computer wrote to the volume: app/drive.rs).
    restart: bool,
    /// The phone link (app/phone.rs).
    phone: Phone,
}

impl App {
    /// Panel up, backlight on, the Nordic chip started (BLE, then ANT with `ant_key` and the paired sensors), the
    /// settings read, the first ride page drawn.
    pub fn start(b: &mut impl Board, ant_key: Option<[u8; 8]>) -> Self {
        Self::start_with(b, ant_key, None)
    }

    /// [`App::start`] with Shimano's network key too (`di2_key`: the Di2 channel, sensors.rs).
    pub fn start_with(b: &mut impl Board, ant_key: Option<[u8; 8]>, di2_key: Option<[u8; 8]>) -> Self {
        let mut app = Self::new_with(b, ant_key, di2_key);
        app.begin(b);
        app
    }

    /// The first half of [`App::start_with`]: the panel and the chips started, the state read back. It returns the
    /// app as one struct expression, which the caller can place where it lives (main.rs `make_app`) before
    /// [`App::begin`] runs on it there: a named local returned at the end was one more copy of the app on the stack.
    pub fn new_with(b: &mut impl Board, ant_key: Option<[u8; 8]>, di2_key: Option<[u8; 8]>) -> Self {
        panel::init(b);
        play(b, &BEEP_ON);
        let mut rx = nrf::Rx::new();
        let ble = nrf::start(b).ok();
        let nets = Kind::ALL.map(Kind::network);
        let mut refusal = None;
        let (ant, di2) = match (ble, ant_key) {
            (Some(_), Some(key)) => match nrf::ant_start(b, &mut rx, key, di2_key, &nets) {
                Ok(di2) => (true, di2),
                Err(e) => {
                    refusal = rx.failed.filter(|_| matches!(e, nrf::Error::Code(_))); // an answer (link.rs)
                    (false, false)
                }
            },
            _ => (false, false),
        };
        let mut sensors = [Sensor::Off; Kind::ALL.len()];
        for (kind, s) in Kind::ALL.into_iter().zip(&mut sensors) {
            if let Some(id) = kind.from_word(b.backup_read(kind.backup()))
                && ant
                && (kind.network() == 0 || di2)
                && nrf::ant_open(b, &mut rx, kind.channel(), kind.device_type(), kind.period(), Some(id)).is_ok()
            {
                *s = Sensor::Paired(id);
            }
        }
        let misc = Settings::from_word(b.backup_read(WHEEL_BKP))
            .with_power_word(b.backup_read(POWER_BKP))
            .with_alerts_word(b.backup_read(ALERTS_BKP));
        let (pages, page_count) = pages_from_registers(b);
        let store = PageStore::at_boot(b);
        App {
            keys: Keys::at_start(b.keys_down()),
            light_shown: 0,
            idle_ms: 0,
            chips: ambient::init(b),
            auto: ambient::Auto::default(),
            auto_level: LIGHT_LEVELS[1], // until the first reading
            light_ms: LIGHT_MS,          // a measurement at the first step
            measuring: false,
            baro: None,
            alt: altitude::Altimeter::new(),
            baro_ms: BATTERY_MS - BARO_RIDE_MS, // the first reading a second after the start (the conversion's)
            alt_store: AltStore::default(),
            seconds: rtc_time_from_tr(b.rtc().0).2,
            strip: Strip {
                time: rtc_hm(b),
                radio: ble.is_some(),
                gps: None,
                blink: true,
                usb: UsbDot::Off,
                battery: None,
                charging: false,
                rec: Rec::Idle,
                locked: false,
                more: false,
            },
            strip_shown: Strip {
                time: (0, 0),
                radio: false,
                gps: None,
                blink: true,
                usb: UsbDot::Off,
                battery: None,
                charging: false,
                rec: Rec::Idle,
                locked: false,
                more: false,
            },
            screen: Screen::Main,
            ble,
            rx,
            watch: Watch::new(ble.is_some() && (ant || ant_key.is_none()), refusal),
            ant_key,
            di2_key,
            nearby: nrf::Nearby::new(),
            scanning: false,
            redraw_ms: 0,
            list_dirty: false,
            ant,
            di2,
            sensors,
            heard_ms: [u32::MAX; Kind::ALL.len()],
            resting_ms: [u32::MAX; Kind::ALL.len()],
            gear: None,
            shifts: 0,
            profile: ride::Profile::Road,
            profile_by_hand: false,
            trainer_quiet_s: 0,
            ask: 0,
            live: Live::default(),
            zones: [ride::ZoneHold::default(); 4],
            decoders: Decoders::default(),
            hrv: hrv::Hrv::default(),
            battery: battery::Battery::default(),
            battery_ms: BATTERY_MS, // read at the first step
            gnss: gnss::Parser::new(),
            pvt: None,
            pvt_ms: 0,
            gnss_ms: 0,
            up_ms: 0,
            gnss_sent: 0,
            gnss_off: false,
            aid: gnss::Aid::from_words(AID_BKP.map(|r| b.backup_read(r))),
            settings: misc,
            editing: false,
            menu_chosen: false,
            confirm: None,
            repeated: false,
            under: [Screen::Main; 8],
            depth: 0,
            pages,
            page_count,
            more: [None; PAGES],
            indoor: page::Set::indoor_default(),
            pages_indoor: false,
            set_shown: false,
            more_on: None,
            store,
            edit: None,
            hint_shown: false,
            date: rtc_date_from_dr(b.rtc().1),
            notices: Default::default(),
            banner_ms: None,
            banner: Line::new(),
            banner_kind: Banner::Info,
            banner_next: None,
            banner_drawn: false,
            had_fix: false,
            told_low: false,
            graph_whole: false,
            plot_taps: [[None; page::CELLS]; PAGES + 1],
            ride_started: ((2000, 1, 1), (0, 0)),
            alerts: alerts::Alerts::new(),
            off_armed_ms: 0,
            alert: None,
            tones: Tones::default(),
            summary: None,
            new_ride: false,
            drawn: Default::default(),
            ride: ride::Ride::new(),
            history: ride::History::default(),
            power: power::Power::default(),
            menu_shown: [0; ui::MENU_ROWS],
            track: map::Track::new(),
            map_zoom: None,
            map_centre: None,
            emmc: emmc::Emmc::new(),
            basemap: tiles::Basemap::new(),
            touch_ok: touch::init(b),
            touch: touch::Recogniser::default(),
            touch_pending: None,
            touch_sampled: false,
            stock: stock::Scan::new(),
            awake_at: 0,
            dt_ms: POLL_MS,
            past: history::Past::new(),
            trash_check: false,
            calendar: Default::default(),
            calendar_day: None,
            plan_file: None,
            plan: storage::workout::Workout::new(),
            player: None,
            recorder: recorder::Recorder::new(),
            card_writable: false,
            resume_pending: false,
            full: recorder::Recorder::of(recorder::Kind::Full),
            full_pending: false,
            lag_s: 0,
            courses: None,
            drive: msc::Drive::new(),
            restart: false,
            phone: Phone::default(),
            trainer: trainer::Trainer::default(),
            trainer_shown: ui::trainer::Shown::default(),
            trainer_up: false,
            sensors_store: SensorStore::default(),
            ack_gap_ms: 0,
            trainer_second: false,
            trainer_no_control: false,
            ftms: Ftms::default(),
        }
    }

    /// The second half of [`App::start`], on the app in place: what is left on the card and in the backup
    /// registers acted on, the first screen drawn, the backlight on.
    pub fn begin(&mut self, b: &mut impl Board) {
        // a ride left open on the card: reopened at the first step (not here: its card work in its own frame)
        self.card_writable = b.emmc_writable();
        self.awake_at = b.awake_us();
        self.resume_pending = self.card_writable && b.backup_read(RIDE_BKP) == RIDE_OPEN;
        self.full_pending = self.resume_pending;
        self.trash_check = self.card_writable;
        if self.chips.baro {
            ambient::baro_start(b); // read at the first step
        }
        if let Some((lat, lon)) = self.aid.pos {
            self.track.seed(lat, lon); // the map shows where the unit last was until the first fix
        }
        let fault = b.backup_read(FAULT_BKP);
        if fault >> 24 == 0xfa {
            // the last run ended in a fault or a panic, and hw.rs restarted the unit
            let mut l = Line::<20>::new();
            l.push_str("FAULT RESET ");
            l.push_u32(fault & 0xff);
            self.notice(l.as_str());
        }
        if b.backup_read(STOCK_BKP19) >> 28 == 0xb {
            // pages kept from register 19 by builds of 2026-10-23: kept where stock does not look, 19 given back
            self.save_page_registers(b);
            b.backup_write(STOCK_BKP19, 0);
        }
        // pages kept one register each by the first builds (14 to 16), read at each boot but never saved where they
        // are now: saved there once, before a shifting system's pairing reuses 14 (review of 571e613)
        let old = PAGE_BKP.iter().zip(OLD_PAGE_BKP).any(|(new, old)| {
            PageCfg::from_words(new.map(|r| b.backup_read(r))).is_none()
                && PageCfg::from_word(b.backup_read(old)).is_some()
        });
        if old {
            self.save_page_registers(b);
        }
        self.first_start(b); // before the first page is drawn: stock's pages on it
        if self.ble.is_some() {
            self.phone_setup(b);
        }
        self.strip_shown = self.strip;
        ui::draw_strip(&mut Panel(b, (0, ui::H)), &self.strip);
        self.show(b);
        self.update_light(b);
    }

    /// One pass of the main loop: read the time, the radio and the keys, redraw what changed. The caller sleeps
    /// [`POLL_MS`] between passes (servicing USB meanwhile on the unit).
    pub fn step(&mut self, b: &mut impl Board) {
        // the time since the last pass: its sleep and the work of that pass (bounded: a first pass, a stall); the
        // work done during the sleep (USB) is in the sleep's length already (2026-10-25 review: counted twice)
        let now = b.awake_us();
        let (sleep_ms, sleep_work_us) = b.last_sleep();
        let work_us = now.wrapping_sub(core::mem::replace(&mut self.awake_at, now)).saturating_sub(sleep_work_us);
        let pass_ms = (sleep_ms + (work_us / 1000)).min(MAX_PASS_MS);
        self.dt_ms = pass_ms;
        let up_before = self.up_ms;
        self.up_ms = self.up_ms.saturating_add(pass_ms);
        let (tr, dr) = b.rtc();
        let (h, m, sec) = rtc_time_from_tr(tr);
        self.strip.time = (h, m);
        self.date = rtc_date_from_dr(dr);
        if sec != self.seconds {
            // every second gone, not one a pass: a pass that took longer (an eMMC or radio wait) loses no ride time;
            // more than MAX_CATCH_UP is the clock set, not riding
            let gone = (sec + 60 - self.seconds) % 60;
            self.seconds = sec; // the time the recorder reads goes with the minute already read
            // each with its own time for the recorder: the first was `n - 1` seconds ago (the second review, R5)
            let n = gone.clamp(1, MAX_CATCH_UP);
            for k in 0..n {
                self.lag_s = n - 1 - k;
                self.second();
            }
            self.lag_s = 0;
        }
        self.settle_set(b);
        if let Some(a) = self.alert.take() {
            self.say_as(
                a.text(),
                if a == alerts::Alert::HrHigh || a == alerts::Alert::CadenceLow { Banner::Alarm } else { Banner::Ride },
            );
            self.tones.play(&BEEP_ALERT);
        }
        self.play_tones(b);
        self.off_armed_ms = self.off_armed_ms.saturating_sub(pass_ms);
        self.seconds = sec;
        self.battery_ms = self.battery_ms.saturating_add(pass_ms);
        if self.battery_ms >= BATTERY_MS {
            self.battery_ms = 0;
            let charging = b.charger_present();
            let reading = b.battery_adc().and_then(|(raw, vref, cal)| battery::millivolts(raw, vref, cal));
            if let Some(mv) = reading
                && self.battery.update(mv, charging)
            {
                return self.power_off(b); // empty: off before the cell is drawn too low
            }
            self.strip.battery = self.battery.percent();
            self.strip.charging = charging;
            let low = self.strip.battery.is_some_and(|p| p <= 15) && !charging;
            if low && !self.told_low {
                self.alarm_notice("BATTERY LOW");
            }
            self.told_low = low;
        }
        self.poll_light(b);
        if self.ble.is_some() {
            self.poll_nrf(b);
        }
        self.watch_radio(b);
        self.poll_gnss(b);
        // the app's own run time, not the receiver's (switched off on an indoor ride, its clock starts again)
        if up_before < FAULT_CLEAR_MS && self.up_ms >= FAULT_CLEAR_MS && b.backup_read(FAULT_BKP) >> 24 == 0xfa {
            b.backup_write(FAULT_BKP, 0); // a minute of running: the next fault is the first in a row again
        }
        if !self.had_fix && self.pvt.is_some_and(|p| p.good()) {
            self.had_fix = true; // no notice: the strip's GPS says it, and a notice stayed after the fix was lost
        }
        self.strip.rec = match self.ride.state() {
            ride::State::Idle => Rec::Idle,
            _ if !self.card_writable
                || self.recorder.failing()
                || !matches!(self.recorder.phase(), recorder::Phase::Starting | recorder::Phase::Recording) =>
            {
                Rec::NotSaving
            }
            _ if self.ride.moving() => Rec::Running,
            _ => Rec::Paused,
        };
        self.update_strip(b); // after everything that changes it (time, battery, GPS, the ride)
        for kind in Kind::ALL {
            if self.ask & 1 << kind.channel() != 0
                && let Ok(Some(id)) = nrf::ant_channel_id(b, &mut self.rx, kind.channel())
            {
                self.sensors[kind as usize] = Sensor::Paired(id);
                self.keep_pairing(b, kind, id);
                let mut l = Line::<20>::new();
                l.push_str(kind.name());
                l.push_str(" PAIRED");
                self.notice(l.as_str());
            }
        }
        self.ask = 0;
        self.live.tick(pass_ms);
        for h in &mut self.heard_ms {
            *h = h.saturating_add(pass_ms);
        }
        for kind in Kind::ALL {
            let i = kind as usize;
            if self.resting_ms[i] == u32::MAX {
                continue;
            }
            self.resting_ms[i] = self.resting_ms[i].saturating_add(pass_ms).min(u32::MAX - 1);
            if self.resting_ms[i] >= SEARCH_REST_MS {
                let id = match self.sensors[i] {
                    Sensor::Paired(id) => Some(id),
                    _ => None, // still pairing: search for any
                };
                let ok = nrf::ant_open(b, &mut self.rx, kind.channel(), kind.device_type(), kind.period(), id).is_ok();
                self.resting_ms[i] = if ok { u32::MAX } else { 0 }; // failed: another rest
            }
        }
        if !self.emmc.lent() {
            // the card is the computer's (the USB drive): the app's card work waits
            self.step_stock(b);
            self.step_courses(b);
            self.step_past(b);
            self.step_pages(b);
        }
        if self.edit.is_some() && self.ride.state() == ride::State::Running {
            self.leave_editor(b, true); // recording again (by no key of the editor's): its work kept, never lost
        }
        self.redraw_ms = self.redraw_ms.saturating_add(pass_ms);
        // under an incoming call a banner waits (the call says who calls there, end_ring shows it)
        if let Some(ms) = self.banner_ms.filter(|_| !self.phone.ringing) {
            if !core::mem::replace(&mut self.banner_drawn, true) {
                ui::draw_banner(&mut Panel(b, (0, ui::H)), self.banner.as_str(), self.banner_kind == Banner::Alarm);
            }
            self.banner_ms = Some(ms.saturating_add(pass_ms)).filter(|&ms| ms < BANNER_MS);
            if self.banner_ms.is_none() {
                self.unbanner(b);
            }
        }
        match self.screen {
            Screen::Main | Screen::Ride(_) | Screen::Edit(..) => self.update_page(b),
            Screen::Summary => {
                // the elevation stays as it was; the map keeps up with points still coming (the unit moving on)
                let area = self.area();
                if let Some((m, _)) = &mut self.summary {
                    let (base, view) = (Some(&self.basemap), m.view);
                    map::update(&mut Panel(b, area), map::area(ui::card::MAP), &self.track, m, base, None, None);
                    if m.view != view {
                        self.show(b); // refitted: the basemap of the new view under it (else black; review)
                    }
                }
            }
            Screen::Nearby if self.list_dirty && self.redraw_ms >= LIST_REDRAW_MS => {
                ui::draw_list(&mut Panel(b, self.area()), &self.nearby);
                self.list_dirty = false;
                self.redraw_ms = 0;
            }
            Screen::Menu(
                menu @ (Menu::Main
                | Menu::Sensors
                | Menu::Gps
                | Menu::Stock
                | Menu::History
                | Menu::Courses
                | Menu::System
                | Menu::BuiltIn
                | Menu::Ride(_)
                | Menu::Phone
                | Menu::BtTrainers),
                sel,
            ) if self.redraw_ms >= LIST_REDRAW_MS => {
                self.update_menu_rows(b, menu, sel);
                self.redraw_ms = 0;
            }
            _ => {}
        }
        let mask = b.keys_down();
        // a key stuck since the start is no use: it does not keep the light on either
        self.idle_ms = if self.keys.live(mask) != 0 { 0 } else { self.idle_ms.saturating_add(pass_ms) };
        if let Some(p) = self.keys.poll(mask, pass_ms) {
            self.press(b, p);
        }
        if mask == 0 {
            self.repeated = false;
        }
        self.step_recorder(b);
        self.step_altitude(b);
        self.step_trainer(b);
        if self.ble.is_some() {
            self.step_phone(b);
        }
        if self.screen == Screen::Media && self.redraw_ms >= LIST_REDRAW_MS {
            self.update_media(b);
            self.redraw_ms = 0;
        }
        self.update_light(b);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    /// The app's size and its biggest parts (printed): it lives in AXI SRAM (main.rs make_app), and its one
    /// temporary copy at start-up is on the stack.
    #[test]
    fn app_size_budget() {
        assert!(core::mem::size_of::<App>() < 56 * 1024, "App is {} B", core::mem::size_of::<App>());
    }

    #[test]
    fn rtc_decoding() {
        assert_eq!(rtc_time_from_tr(0), (0, 0, 0));
        assert_eq!(rtc_time_from_tr(0x0012_3456), (12, 34, 56));
        assert_eq!(rtc_time_from_tr(0x0040_0000 | 0x0009_0000), (9, 0, 0)); // PM bit ignored
    }

    #[test]
    fn fixed_point() {
        let s = |v, shift, dec| {
            let mut l = Line::<12>::new();
            push_fixed(&mut l, v, shift, dec);
            std::string::String::from(l.as_str())
        };
        assert_eq!(s(51_234_561, 7, 5), "5.12345");
        assert_eq!(s(-1_224_194_200, 7, 5), "-122.41942");
        assert_eq!(s(23_500, 3, 0), "23");
        assert_eq!(s(1_000_500, 7, 5), "0.10005");
    }

    #[test]
    fn short_on_release_long_once_while_held_repeats_between() {
        let mut k = Keys::default();
        assert_eq!(k.poll(Key::Page as u8, 50), None);
        assert_eq!(k.poll(0, 50), Some(Press::Short(Key::Page)));
        assert_eq!(k.poll(0, 50), None);
        let held: std::vec::Vec<_> = (0..42).map(|_| k.poll(Key::Page as u8, 50)).collect();
        let at = |p| held.iter().enumerate().filter(|(_, x)| **x == Some(p)).map(|(i, _)| 50 * (i + 1)).collect();
        let repeats: std::vec::Vec<usize> = at(Press::Repeat(Key::Page));
        assert_eq!(repeats.first(), Some(&500), "repeats from 0.5 s");
        assert!(repeats.windows(2).all(|w| w[1] - w[0] == 100 || w[1] - w[0] == 200), "every 0.1 s");
        assert_eq!(at(Press::Long(Key::Page)), [2000], "long once, at 2 s");
        assert_eq!(k.poll(0, 50), None, "nothing at the release after a long press");
        let back: std::vec::Vec<_> = (0..60).map(|_| k.poll(Key::Back as u8, 50)).collect();
        assert_eq!(back[59], Some(Press::Long(Key::Back)), "Back: at 3 s");
    }
}
