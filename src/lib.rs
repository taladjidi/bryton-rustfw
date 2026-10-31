//! Everything the firmware does, written against [`Board`]: the few operations that touch hardware.
//!
//! `main.rs` implements [`Board`] with registers on the unit; `src/sim/` implements it on the host, with a model
//! of the panel, so the whole app (power-up sequence, drawing protocol, clock, keys, power off) runs in
//! `cargo test-host`. Rule: logic goes here, `main.rs` only moves bytes and reads pins (AGENTS.md).
//!
//! This file holds the [`Board`] trait and [`App`]: its state, [`App::start`] and the main loop's pass
//! ([`App::step`]). The rest of the app is in `app/`, one `impl App` per concern (app/mod.rs lists them).

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::needless_pass_by_ref_mut)]

mod app;
pub mod drivers;
pub mod link;
pub mod nav;
#[cfg(test)]
mod sim;
pub mod storage;
pub mod training;
pub mod ui;

// the modules by their own names too (crate::ambient, bryton_rustfw::fit): the folders only sort the files
pub use drivers::{ambient, battery, ble, emmc, fec, fecble, ftms, gnss, mga, nrf, panel, powercal, sensors, touch};
pub use link::{dev, msc, phone, prof};
pub use nav::{altitude, climbs_ahead, course, places, segment, segment_match, sun};
pub use storage::{
    fat, fit, fitread, history, ign, inflate, ini, named, recorder, route, stock, thresholds, trash, volume,
};
pub use training::{
    alerts, hrtss, hrv, hrvt, load, pacing, power, power_curve, power_match, ramp, ride, trainer, workout,
};
pub use ui::{fonts, map, menu, page, text, theme, tiles};
// the app's parts (app/mod.rs), at the root as before the split
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
    /// Sleeps `ms` milliseconds (SysTick and WFI on the unit; the simulator advances its clock).
    fn delay_ms(&mut self, ms: u32);
    /// Microseconds the CPU has been awake (working, not asleep in a wait) since start-up, wrapping: what a pass's
    /// work took, so the app can bound its background work by time and count a key's hold in real time (a long
    /// pass is longer than [`POLL_MS`]). The DWT cycle counter on the unit. 0 always: work is not timed.
    fn awake_us(&mut self) -> u32 {
        0
    }
    /// The DWT cycle counter (CYCCNT, 32 bits, wrapping; stopped in WFI): the profiler's clock ([`prof`]). 0 always:
    /// nothing is timed.
    fn cycles(&mut self) -> u32 {
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

/// The sun key (local date and minute) and its rise and set (local minutes), see [`App::refresh_sun`].
type SunNow = (((u16, u8, u8), (u8, u8)), Option<(u32, u32)>);

/// The app: what is on screen, the keys, the sensors and what the Nordic chip reported.
pub struct App {
    keys: Keys,
    /// The backlight.
    light: Light,
    /// The chips found on the sensor bus.
    chips: ambient::Chips,
    /// The barometer's last reading: hPa x 10, C x 100.
    baro: Option<(u32, i16)>,
    /// The barometric altitude and its calibration (altitude.rs); since the barometer was last read; the file.
    alt: altitude::Altimeter,
    baro_ms: u32,
    alt_store: AltStore,
    /// The favourite places and their file (app/places.rs).
    places: PlaceStore,
    /// The RTC's seconds (the main screen shows them).
    seconds: u8,
    /// What the status strip shows, and what it showed when last drawn.
    strip: Strip,
    strip_shown: Strip,
    screen: Screen,
    ble: Option<nrf::Version>,
    rx: nrf::Rx,
    /// The Nordic link's watchdog (app/nrf_watch.rs), and the keys to start ANT again with.
    watch: Watch,
    ant_key: Option<[u8; 8]>,
    di2_key: Option<[u8; 8]>,
    /// The ANT+ sensors and the search.
    radio: Radio,
    redraw_ms: u32,
    /// A second ticked since the page's boxes were last looked at (app/screens.rs `update_page`).
    value_due: bool,
    list_dirty: bool,
    /// ANT started (needs BLE and a built-in ANT+ key).
    ant: bool,
    /// Shimano's network up (its key built in): the Di2 channel can open.
    di2: bool,
    /// This ride's profile (app/profile.rs), and whether the rider set it by hand (the trainer's silence no longer
    /// changes it).
    profile: ride::Profile,
    profile_by_hand: bool,
    /// What the ANT+ trainer link heard and acknowledged, and the trainer screen as drawn.
    tlink: TrainerLink,
    /// The home trainer (trainer.rs, app/trainer.rs).
    trainer: trainer::Trainer,
    /// The pairing file's state.
    sensors_store: SensorStore,
    /// The power meter calibration (app/powercal.rs).
    powercal: powercal::Cal,
    /// What the calibration screen shows (phase, auto zero state); None: not drawn yet.
    powercal_shown: Option<(powercal::Phase, Option<powercal::AutoZero>)>,
    /// This ride's best power per duration, computed as the ride goes (app/curve.rs), and what the POWER CURVE screen shows.
    ride_curve: Option<&'static mut power_curve::Rolling>,
    curve_shown: [u16; power_curve::N],
    curve_ready: bool,
    /// Training load and month totals from the scan's rides (app/pmc.rs).
    pmc: app::pmc::State,
    /// The title a menu was drawn with (app/menus.rs `title_print`).
    menu_title_print: u32,
    /// What the quick panel shows, box by box, and its shape (app/quick.rs).
    quick_shown: [u32; app::QUICK_BOXES],
    quick_shape: u8,
    /// The title and artist scroll: the track's fingerprint and the steps since it began (app/quick.rs).
    quick_scroll: (u32, u32),
    /// ZERO armed until, and its result shown until (epoch seconds, app/quick.rs).
    quick_zero: (Option<u32>, Option<u32>),
    /// The Bluetooth trainer's link (app/ftms.rs).
    ftms: Ftms,
    live: Live,
    /// The climbs ridden to the top this ride (app/climbs_done.rs).
    climbs_done: ClimbsDone,
    /// The ride menu open: its row count and selected row at the last pass ([`App::track_ride_menu`]).
    ride_menu: Option<(usize, menu::Row)>,
    /// The zones the plain power fields (3, 10 and 30 s) and the heart rate show, with hysteresis ([`App::field_bar`]).
    zones: [ride::ZoneHold; 5],
    /// The seconds the ride spent in each heart-rate and power zone (app/riding.rs): cleared at a Start and a resume.
    zone_s: ride::ZoneTime,
    /// hrTSS of the ride so far (app/riding.rs), cleared with `zone_s`: the TSS fields show it for a ride without power.
    hrtss: hrtss::Hrtss,
    decoders: Decoders,
    /// RMSSD and DFA alpha1 from the strap's beats.
    hrv: hrv::Hrv,
    battery: battery::Battery,
    /// Since the last battery reading.
    battery_ms: u32,
    /// The GNSS receiver.
    gps: Gps,
    /// Time run since the start, ms (saturating): a minute of it clears the fault count.
    up_ms: u32,
    /// Wheel, backlight, max heart rate, FTP.
    settings: Settings,
    /// The colour theme in use (THEME), DARK until the settings file is read ([`App::look`]).
    theme: ui::Theme,
    /// AUTO's day and night themes, resolved into RAM between rides ([`App::load_day_night`]): the switch never reads
    /// the card. Built-in defaults until the settings are loaded (OUTDOOR by day, DARK by night).
    auto_themes: [ui::Theme; 2],
    /// The selected menu row's number is being changed (Lap up, Page down, Start or Back to leave).
    editing: bool,
    /// The selected menu row was chosen (by a key or a tap) since the menu opened: a tap on it acts. A tap on another
    /// row, or on the row a menu opens on, only selects it (one stray touch never acts).
    menu_chosen: bool,
    /// A row that cannot be undone (forget a sensor or the phone, import stock's settings) asked to be confirmed: Start
    /// on it again acts; any other input clears it (App::confirmed). A past ride's DELETE, asked on its card, is
    /// [`PAST_DELETE`].
    confirm: Option<(Menu, usize)>,
    /// GIVE UP SAVE's step while its row waits in [`App::confirm`]: two banners of explanation, then two questions.
    give_up_step: u8,
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
    /// The MTB profile's pages (at most [`page::MTB_PAGES`]), in the file only.
    mtb: page::Set,
    /// The set PAGES and the editor show while no ride is open (app/profile.rs).
    pages_set: ride::Profile,
    /// The set the ride pages on screen were taken from at the last pass ([`App::ride_profile`] then).
    set_shown: ride::Profile,
    /// The climb page is in the round, the page it follows, and the climb handled: its start (m) and whether the page opened, or was skipped, for it.
    climb: ClimbPage,
    /// The ride page whose MORE variant is up: only while that page is the screen (App::more_shown).
    more_on: Option<usize>,
    /// `\Rustfw\pages.bin` (app/editor.rs): read yet, to be written, its copies.
    store: PageStore,
    /// The page switches, names and auto scroll (app/pagemeta.rs).
    sw: Switches,
    /// `\Rustfw\settings.bin` (app/settings_file.rs): read yet, to be written, its copies.
    settings_store: SettingsStore,
    /// A write of pages.bin, settings.bin or places.bin failed since the FAT copies were last compared (app/editor.rs).
    fats_suspect: bool,
    /// SYSTEM > SCREENSHOT asked for a screenshot at the next pass (app/screenshot.rs).
    shot_due: bool,
    /// A fault marker found at the start and its time, its line not yet in the fault log, the uptime (ms) to try at
    /// and the tries that failed (app/faultlog.rs).
    fault_due: Option<(u32, recorder::Local, u32, u8)>,
    /// The page editor's work, while it is open.
    edit: Option<Editing>,
    /// The strip's place shows the editor's hint (app/editor.rs), not the strip.
    hint_shown: bool,
    /// Today as shown (the RTC, moved by TIME ZONE: app/clock.rs [`App::show_clock`]), for the overview.
    date: (u16, u8, u8),
    /// The shown clock minus the RTC, seconds (0 under TIME ZONE CLOCK or before an offset from UTC is learnt).
    clock_delta_s: i32,
    /// The sun's rise and set (local minutes) for the date and minute of the last pass that changed it: the key, then
    /// the times (None without a fix, the zone learnt, or in polar day or night). Worked out at most once a minute.
    sun: Option<SunNow>,
    /// The compass point on the screen for the heading (app/fields.rs `held_point`), None without a course.
    heading: Option<u8>,
    /// The notices and the banner over the screen.
    notes: Notices,
    /// The battery was reported low.
    told_low: bool,
    /// BATTERY SAVER AUTO has turned the saver on ([`App::saver_on`]): from [`BATTERY_SAVER_PERCENT`] until the charger.
    saver_auto: bool,
    /// A ride was just started from a key: the page is drawn again (its map and graphs start over).
    new_ride: bool,
    /// The card's map and elevation profile as drawn (set when the card is shown).
    summary: Option<(map::Shown, GraphShown)>,
    /// Reminders and alerts of the ride, the one due (said and beeped at the next pass), and the one tone player
    /// (alerts, courses, workouts).
    alerts: alerts::Alerts,
    alert: alerts::Queue,
    tones: Tones,
    /// A first hold of Back during a ride armed switching off for this long ([`OFF_CONFIRM_MS`]).
    off_armed_ms: u32,
    /// The AUTO OFF timer (app/device.rs).
    auto_off: AutoOff,
    /// When the ride started: date, hours and minutes (its card's title).
    ride_started: RideStart,
    /// The start reminder's count (app/start_cues.rs).
    cues: StartCues,
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
    /// The same without the value ([`App::menu_row_print`]): a row whose head is unchanged repaints its value only.
    menu_head: [u32; ui::MENU_ROWS],
    /// Graph bars computed (the tests count them: a redraw clipped away must compute none).
    #[cfg(test)]
    pub(crate) bars: core::cell::Cell<u32>,
    /// The nearby list's rows on screen, as fingerprints ([`ui::list_prints`]).
    list_shown: [u32; ui::LIST_ROWS],
    /// The ride's track, from the good fixes.
    track: map::Track,
    /// The map's zoom level set by its "+" and "-" ([`map::mpp16`]); None: fitted to the track (per power-up).
    map_zoom: Option<u32>,
    /// Zoomed in and panned by hand: the map's centre (metres on the track's plane); None: it follows the rider.
    map_centre: Option<(i32, i32)>,
    /// The crosshair with a course followed showed the whole route ([`map::View::fit`]) instead of the default view.
    map_whole: bool,
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
    /// The card's survey for STORAGE (app/survey.rs).
    survey: volume::Survey,
    /// A ride's delete the last run may have left unfinished: looked for when the scan of the rides first runs
    /// ([`trash::finish`]).
    trash_check: bool,
    /// [`trash::finish`] tried in this run of the scan (once a scan: a lasting card error is not tried every pass).
    trash_tried: bool,
    /// Workouts: those planned from the day it was read on; the one opened (its steps menu) or running, and its
    /// file; where it is (workout.rs).
    plan: Plan,
    /// The ramp test being ridden, its last result, and the result screen due (app/ramp.rs).
    ramp: RampState,
    /// The card may be written ([`Board::emmc_writable`]); without, the recorder is not used at all.
    card_writable: bool,
    /// The ride recorders (clean and full) and the ride model.
    rec: Recording,
    /// The courses on the card, the one followed and where the rider is on it (memory from main.rs:
    /// [`App::set_course_memory`]).
    courses: Option<&'static mut storage::course_file::Courses>,
    /// The named points along the course followed (its own cell: main.rs `make_pois`), and its scan.
    pois: Option<&'static mut named::Pois>,
    /// The live segments: index, active segment, matcher (memory from main.rs: [`App::set_segment_memory`]).
    segments: Option<&'static mut Segments>,
    /// The live segment screens' state (app/segment_ui.rs).
    seg_ui: SegUi,
    /// The recent full copies read for the heart rate variability thresholds (FITNESS, app/fitness.rs; memory from
    /// main.rs: [`App::set_threshold_memory`], 5 KB the app has no room for).
    thresholds: Option<&'static mut thresholds::Scan>,
    /// The climb pacer's plan (app/pace.rs).
    pace: PaceState,
    /// The road ahead: climbs found with no course followed (memory from main.rs: [`App::set_ahead_memory`]).
    ahead: Option<(&'static mut climbs_ahead::Ahead, &'static mut [u16])>,
    /// The last and the longest tile load of the road ahead, and the longest slice, us ([`App::ahead_tile_us`]).
    ahead_us: AheadUs,
    /// The time spent so far on the tile being loaded, us.
    ahead_acc: u32,
    /// Where the CPU's time goes, by section of the pass (link/prof.rs; the dev link's `prof`).
    prof: prof::Prof,
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
        let ChipStart { ble, ant, di2, refusal, sensors } = start_chip(b, &mut rx, ant_key, di2_key);
        let misc = Settings::from_word(b.backup_read(WHEEL_BKP))
            .with_power_word(b.backup_read(POWER_BKP))
            .with_alerts_word(b.backup_read(ALERTS_BKP))
            .with_auto_off_word(b.backup_read(AID_BKP[1]));
        let (pages, page_count) = pages_from_registers(b);
        let store = PageStore::at_boot(b);
        let settings_store = SettingsStore::at_boot(b, misc);
        App {
            keys: Keys::at_start(b.keys_down()),
            light: Light::new(),
            chips: ambient::init(b),
            baro: None,
            alt: altitude::Altimeter::new(),
            baro_ms: BATTERY_MS - BARO_RIDE_MS, // the first reading a second after the start (the conversion's)
            alt_store: AltStore::default(),
            places: PlaceStore::new(),
            seconds: rtc_time_from_tr(b.rtc().0).2,
            strip: Strip { time: rtc_hm(b), radio: ble.is_some(), blink: true, ..Strip::default() },
            strip_shown: Strip { blink: true, ..Strip::default() },
            screen: Screen::Main,
            ble,
            rx,
            watch: Watch::new(ble.is_some() && (ant || ant_key.is_none()), refusal),
            ant_key,
            di2_key,
            radio: Radio::new(sensors),
            redraw_ms: 0,
            value_due: true,
            list_dirty: false,
            ant,
            di2,
            profile: ride::Profile::Road,
            profile_by_hand: false,
            tlink: TrainerLink::new(),
            live: Live::default(),
            climbs_done: ClimbsDone::new(),
            ride_menu: None,
            zones: [ride::ZoneHold::default(); 5],
            zone_s: ride::ZoneTime::default(),
            hrtss: hrtss::Hrtss::default(),
            decoders: Decoders::default(),
            hrv: hrv::Hrv::default(),
            battery: battery::Battery::default(),
            battery_ms: BATTERY_MS, // read at the first step
            gps: Gps::new(gnss::Aid::from_words(AID_BKP.map(|r| b.backup_read(r)))),
            up_ms: 0,
            settings: misc,
            theme: ui::Theme::DARK,
            auto_themes: [ui::Theme::OUTDOOR, ui::Theme::DARK],
            editing: false,
            menu_chosen: false,
            confirm: None,
            give_up_step: 0,
            repeated: false,
            under: [Screen::Main; 8],
            depth: 0,
            pages,
            page_count,
            more: [None; PAGES],
            indoor: page::Set::indoor_default(),
            mtb: page::Set::mtb_default(),
            pages_set: ride::Profile::Road,
            set_shown: ride::Profile::Road,
            climb: ClimbPage::new(),
            more_on: None,
            store,
            sw: Switches::new(),
            settings_store,
            fats_suspect: false,
            shot_due: false,
            fault_due: None,
            edit: None,
            hint_shown: false,
            date: rtc_date_from_dr(b.rtc().1),
            clock_delta_s: 0, // the registers hold no TIME ZONE: CLOCK until the settings file is read
            sun: None,
            heading: None,
            notes: Notices::new(),
            told_low: false,
            saver_auto: false,
            graph_whole: false,
            plot_taps: [[None; page::CELLS]; PAGES + 1],
            ride_started: RideStart::new((2000, 1, 1), (0, 0)),
            cues: StartCues::new(),
            alerts: alerts::Alerts::new(),
            off_armed_ms: 0,
            auto_off: AutoOff::new(),
            alert: alerts::Queue::default(),
            tones: Tones::default(),
            summary: None,
            new_ride: false,
            drawn: Default::default(),
            ride: ride::Ride::new(),
            history: ride::History::default(),
            power: power::Power::default(),
            menu_shown: [0; ui::MENU_ROWS],
            menu_head: [0; ui::MENU_ROWS],
            #[cfg(test)]
            bars: core::cell::Cell::new(0),
            list_shown: [0; ui::LIST_ROWS],
            track: map::Track::new(),
            map_zoom: None,
            map_centre: None,
            map_whole: false,
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
            survey: volume::Survey::new(),
            trash_check: false,
            trash_tried: false,
            plan: Plan::new(),
            ramp: RampState::default(),
            rec: Recording::new(),
            card_writable: false,
            courses: None,
            pois: None,
            segments: None,
            seg_ui: SegUi::new(),
            pace: PaceState::new(),
            thresholds: None,
            ahead: None,
            ahead_us: AheadUs::default(),
            ahead_acc: 0,
            drive: msc::Drive::new(),
            restart: false,
            phone: Phone::default(),
            trainer: trainer::Trainer::default(),
            sensors_store: SensorStore::default(),
            powercal: powercal::Cal::default(),
            powercal_shown: None,
            ride_curve: None,
            curve_shown: [0; power_curve::N],
            curve_ready: false,
            pmc: app::pmc::State::new(),
            menu_title_print: 0,
            quick_shown: [0; app::QUICK_BOXES],
            quick_shape: 0,
            quick_scroll: (0, 0),
            quick_zero: (None, None),
            ftms: Ftms::default(),
            prof: prof::Prof::default(),
        }
    }

    /// The second half of [`App::start`], on the app in place: what is left on the card and in the backup
    /// registers acted on, the first screen drawn, the backlight on.
    pub fn begin(&mut self, b: &mut impl Board) {
        // a ride left open on the card: reopened at the first step (not here: its card work in its own frame)
        self.card_writable = b.emmc_writable();
        self.awake_at = b.awake_us();
        self.rec.resume_if(self.card_writable && b.backup_read(RIDE_BKP) == RIDE_OPEN);
        self.trash_check = self.card_writable;
        if self.chips.baro {
            ambient::baro_start(b); // read at the first step
        }
        if let Some((lat, lon)) = self.gps.aid.pos {
            self.track.seed(lat, lon); // the map shows where the unit last was until the first fix
        }
        let fault = b.backup_read(FAULT_BKP);
        if fault >> 24 == 0xfa {
            // the last run ended in a fault or a panic, and hw.rs restarted the unit
            let mut l = Line::<20>::new();
            l.push_str("FAULT RESET ");
            l.push_u32(fault & 0xff);
            self.notice(l.as_str());
            self.fault_found(fault); // its line to the fault log once the card is quiet
        }
        if b.backup_read(STOCK_BKP19) >> 28 == 0xb {
            // pages kept from register 19 by builds of 2026-10-23: kept where stock does not look, 19 given back
            self.save_page_registers(b);
            b.backup_write(STOCK_BKP19, 0);
        }
        // pages kept one register each by the first builds (14, 15; their 16 is POWER_BKP's now), read at each boot
        // but never saved where they are now: saved there once, before a shifting system's pairing reuses 14 (review
        // of 571e613)
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
        ui::draw_strip(&mut Panel(b, (0, ui::H), self.look()), &self.strip);
        self.show(b);
        self.update_light(b);
    }

    /// One pass of the main loop: read the time, the radio and the keys, redraw what changed. The caller sleeps
    /// [`App::pass_ms`] between passes (servicing USB meanwhile on the unit).
    pub fn step(&mut self, b: &mut impl Board) {
        use prof::Sec;
        let mut t = b.cycles(); // each section's start (link/prof.rs): the end of the one before
        // the time since the last pass: its sleep and the work of that pass (bounded: a first pass, a stall); the
        // work done during the sleep (USB) is in the sleep's length already (2026-10-25 review: counted twice)
        let now = b.awake_us();
        let (sleep_ms, sleep_work_us) = b.last_sleep();
        let work_us = now.wrapping_sub(core::mem::replace(&mut self.awake_at, now)).saturating_sub(sleep_work_us);
        let pass_ms = (sleep_ms + (work_us / 1000)).min(MAX_PASS_MS);
        self.dt_ms = pass_ms;
        let up_before = self.up_ms;
        self.up_ms = self.up_ms.saturating_add(pass_ms);
        let sec = self.read_clock(b);
        self.refresh_sun();
        self.refresh_heading();
        self.catch_up_seconds(sec);
        self.settle_set(b);
        self.settle_segment(b); // before the climb: a segment that starts with a climb takes the screen
        self.settle_climb(b);
        self.auto_scroll(b, pass_ms);
        if core::mem::take(&mut self.ramp.show) {
            self.open(Screen::RampResult); // the test ended this pass: its result over the page
            self.show(b);
        }
        self.say_alert();
        self.play_tones(b);
        if self.step_auto_off(b) {
            return; // quiet for the whole time-out: switched off
        }
        self.off_armed_ms = self.off_armed_ms.saturating_sub(pass_ms);
        t = self.timed(b, Sec::Second, t);
        if self.poll_battery(b, pass_ms) {
            return; // empty: switched off
        }
        t = self.timed(b, Sec::Battery, t);
        self.poll_light(b); // timed with the battery: one Light section a pass, the backlight's at the end
        if self.ble.is_some() {
            self.poll_nrf(b);
        }
        self.watch_radio(b);
        t = self.timed(b, Sec::Radio, t);
        self.poll_gnss(b);
        t = self.timed(b, Sec::Gnss, t);
        // the app's own run time, not the receiver's (switched off on an indoor ride, its clock starts again)
        if up_before < FAULT_CLEAR_MS && self.up_ms >= FAULT_CLEAR_MS && b.backup_read(FAULT_BKP) >> 24 == 0xfa {
            b.backup_write(FAULT_BKP, 0); // a minute of running: the next fault is the first in a row again
        }
        self.gps.note_fix();
        self.strip.rec = self.rec.mark(&self.ride, self.card_writable);
        self.update_strip(b); // after everything that changes it (time, battery, GPS, the ride)
        t = self.timed(b, Sec::Strip, t);
        self.pair_asked(b);
        self.live.tick(pass_ms);
        self.radio.tick(pass_ms);
        self.reopen_rested(b, pass_ms);
        t = self.timed(b, Sec::Radio, t);
        if !self.emmc.lent() {
            // the card is the computer's (the USB drive): the app's card work waits
            self.step_stock(b);
            t = self.timed(b, Sec::Scan, t);
            self.step_courses(b);
            self.step_segments(b);
            self.step_segment_ui(b);
            t = self.timed(b, Sec::Files, t);
            self.step_ahead(b);
            t = self.timed(b, Sec::Ahead, t);
            self.step_past(b);
            self.step_survey(b);
            self.step_thresholds(b);
            self.step_settings(b);
            self.step_places(b);
            self.step_pages(b);
            self.step_fault_log(b);
            t = self.timed(b, Sec::Files, t);
        }
        if self.edit.is_some() && self.ride.state() == ride::State::Running {
            self.leave_editor(b, true); // recording again (by no key of the editor's): its work kept, never lost
        }
        self.redraw_ms = self.redraw_ms.saturating_add(pass_ms);
        self.run_banner(b, pass_ms);
        self.redraw_screen(b);
        self.step_screenshot(b); // after the draw: the file equals the panel
        t = self.timed(b, Sec::Draw, t);
        let mask = b.keys_down();
        // a key stuck since the start is no use: it does not keep the light on either
        self.light.idle(self.keys.live(mask) != 0, pass_ms);
        if let Some(p) = self.keys.poll(mask, pass_ms) {
            self.press(b, p);
        }
        if mask == 0 {
            self.repeated = false;
        }
        t = self.timed(b, Sec::Keys, t);
        self.step_recorder(b);
        t = self.timed(b, Sec::Recorder, t);
        self.step_altitude(b);
        t = self.timed(b, Sec::Altitude, t);
        self.step_trainer(b);
        self.step_powercal(b);
        self.step_curve(b);
        self.step_pmc(b);
        t = self.timed(b, Sec::Trainer, t);
        if self.ble.is_some() {
            self.step_phone(b);
        }
        if self.screen == Screen::Media && self.redraw_ms >= LIST_REDRAW_MS {
            self.update_media(b);
            self.redraw_ms = 0;
        }
        if self.screen == Screen::Quick && self.redraw_ms + 10 >= app::QUICK_STEP_MS {
            self.update_quick(b);
            self.redraw_ms = 0;
        }
        t = self.timed(b, Sec::Phone, t);
        self.update_light(b);
        self.timed(b, Sec::Light, t);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    /// The app's size (printed): it lives in AXI SRAM (main.rs make_app; about 19 KB of the 512 KB free on
    /// 2026-10-27), and its one temporary copy at start-up is on the stack (123 KB of DTCM: 64 KB here plus main's
    /// 16 KB fits). Raised from 56 KB at the owner's go (2026-10-27: App was 57 104 B of 57 344).
    #[test]
    fn app_size_budget() {
        std::println!("App is {} B", core::mem::size_of::<App>());
        assert!(core::mem::size_of::<App>() < 64 * 1024, "App is {} B", core::mem::size_of::<App>());
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
