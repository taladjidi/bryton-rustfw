//! The USB dev link, board side: OTG_HS with its external ULPI PHY, brought up as stock does it (HAL_PCD_MspInit
//! 0xd0298758, BOARD.md 7), and a composite device (usb-device + synopsys-usb-otg): a CDC serial port carrying the
//! protocol in lib dev.rs, and a mass storage interface (SCSI over Bulk-Only Transport, lib msc.rs) whose medium is
//! the eMMC's FAT volume while the app lends it (app/drive.rs). High speed, so bulk packets are 512 bytes. Polled
//! from the main loop; the OTG_HS interrupt is enabled in the NVIC only so that USB activity wakes WFI (interrupts
//! stay masked, no handler runs).
//!
//! The mass storage interface: class 0x08, subclass 0x06 (SCSI transparent), protocol 0x50 (Bulk-Only), a bulk IN and
//! a bulk OUT endpoint, and the two class requests to the interface, Bulk-Only Mass Storage Reset (0xff, OUT, no
//! data) and Get Max LUN (0xfe, IN, one byte: 0, one LUN) (usb.org, Bulk-Only Transport 1.0, 3.1, 3.2 and 4.3). The
//! CDC function carries an interface association descriptor, so the device is 0xef / 0x02 / 0x01 (USB-IF IAD ECN,
//! usb-device's `composite_with_iads`).

use super::hw::{rd, stage_write, wr};
use crate::{AF, CPU_HZ, OUT, Port, RCC, RCC_CR, cycles, delay_ms, gpio, modify, pin_set};
use bryton_rustfw::dev::{self, Effect, Host, Session, Stats, UsbState, UsbWatch};
use bryton_rustfw::usbsnap::{self, Keeper, UsbSnap};
use bryton_rustfw::{App, Board, msc};
use static_cell::StaticCell;
use synopsys_usb_otg::{PhyType, UsbBus, UsbPeripheral};
use usb_device::bus::{InterfaceNumber, UsbBusAllocator};
use usb_device::class::{ControlIn, ControlOut, UsbClass};
use usb_device::control::{Recipient, RequestType};
use usb_device::descriptor::DescriptorWriter;
use usb_device::endpoint::{EndpointIn, EndpointOut};
use usb_device::prelude::*;
use usbd_serial::CdcAcmClass;

const OTG_HS: u32 = 0x4004_0000;
const GUSBCFG: u32 = OTG_HS + 0x00c;
const GRSTCTL: u32 = OTG_HS + 0x010;
const DCTL: u32 = OTG_HS + 0x804;
const DSTS: u32 = OTG_HS + 0x808;
const DCFG: u32 = OTG_HS + 0x800;
/// OTG_GPVNDCTL: the core's ULPI register access (RegData 7:0, RegAddr 21:16, RegWr 22, NewRegReq 25, VStsDone 27;
/// DWC_otg layout, verified on the unit by peek / poke, 2026-10-31).
const GPVNDCTL: u32 = OTG_HS + 0x034;
/// Experiment switch (docs/install.md): microseconds to wait after each bulk IN packet of the drive, to see whether
/// the high-speed drop depends on the rate. 0 (the default) is off; clamped to 10 ms.
const PACE_US: u32 = parse_us(option_env!("USB_PACE_US"));

const fn parse_us(s: Option<&str>) -> u32 {
    let Some(s) = s else { return 0 };
    let (b, mut i, mut n) = (s.as_bytes(), 0, 0u32);
    while i < b.len() && b[i].is_ascii_digit() && n < 10_000 {
        n = n * 10 + (b[i] - b'0') as u32;
        i += 1;
    }
    if n > 10_000 { 10_000 } else { n }
}

/// Waits PACE_US after a bulk IN packet.
// shortcut: a spin, not a sleep; an experiment build's switch, off (a constant 0, so the clippy lint) by default
#[allow(clippy::absurd_extreme_comparisons)]
fn pace() {
    if FULL_SPEED {
        return; // the default build never stalls
    }
    let t0 = cycles();
    while cycles().wrapping_sub(t0) < PACE_US * (CPU_HZ / 1_000_000) {}
}

/// One ULPI register through GPVNDCTL (ULPI 1.1 register read); `None` if the PHY does not answer in about 1 ms,
/// itself a finding.
fn ulpi_read(addr: u8) -> Option<u8> {
    wr(GPVNDCTL, 1 << 25 | u32::from(addr & 0x3f) << 16);
    let t0 = cycles();
    loop {
        let v = rd(GPVNDCTL);
        if v & (1 << 27) != 0 {
            return Some(v as u8);
        }
        if cycles().wrapping_sub(t0) > CPU_HZ / 1000 {
            return None;
        }
    }
}

/// The core's and the PHY's state now (the core's first: the PHY reads take up to 4 ms).
fn snapshot(event: u8) -> UsbSnap {
    UsbSnap {
        event,
        gintsts: rd(OTG_HS + 0x014),
        gotgint: rd(OTG_HS + 0x004),
        dsts: rd(DSTS),
        diepint3: rd(OTG_HS + 0x968),
        dtxfsts3: rd(OTG_HS + 0x978),
        diepctl3: rd(OTG_HS + 0x960),
        phy: [0x04, 0x13, 0x15, 0x07].map(ulpi_read),
    }
}
/// Full speed (12 Mbit/s, 64-byte bulk packets) unless built with USB_HS=1: high-speed transfers drop on this unit
/// (docs/usb.md; stock's drive too), full speed held for GBs of copies. The PHY itself does high speed (2026-10-31).
const FULL_SPEED: bool = option_env!("USB_HS").is_none();
/// Bulk packet size: 512 at high speed, 64 at full speed (USB 2.0 5.8.3).
const MPS: usize = if FULL_SPEED { 64 } else { 512 };
const RCC_AHB1RSTR: u32 = RCC + 0x080;
const RCC_AHB1ENR: u32 = RCC + 0x0d8;
const RCC_D2CCIP2R: u32 = RCC + 0x054;
const PWR_D3CR: u32 = 0x5802_4818;
const NVIC_ISER2: u32 = 0xe000_e108;
/// OTG_HS's bit in the NVIC's third word (IRQ 77): main.rs `sleep` clears its pending bit before each WFI.
pub const OTG_HS_IRQ: u32 = 1 << (77 - 64);

/// ULPI data, clock, STP, NXT, DIR, all AF10 (BOARD.md 4 / 7).
const ULPI_PINS: [(Port, u32); 12] = [
    (Port::A, 3),
    (Port::A, 5),
    (Port::B, 0),
    (Port::B, 1),
    (Port::B, 5),
    (Port::B, 10),
    (Port::B, 11),
    (Port::B, 12),
    (Port::B, 13),
    (Port::C, 0),
    (Port::H, 4),
    (Port::I, 11),
];
/// PHY reset / enable, high = running (stock: high, then 200 ms).
const PHY_ENABLE: (Port, u32) = (Port::A, 7);

/// OTG_HS for synopsys-usb-otg.
pub struct OtgHs;

#[allow(unsafe_code)]
// SAFETY: REGISTERS is the OTG_HS block (RM0399, 0x40040000), which has the STM32 OTG register layout the crate
// is written for; power_up clocks it before the crate touches it; 9 endpoints and 4 KB of FIFO are the H7 OTG_HS
// figures.
unsafe impl UsbPeripheral for OtgHs {
    const REGISTERS: *const () = OTG_HS as *const ();
    const HIGH_SPEED: bool = true;
    const FIFO_DEPTH_WORDS: usize = 1024;
    const ENDPOINT_COUNT: usize = 9;

    fn enable() {} // power_up has done it

    fn ahb_frequency_hz(&self) -> u32 {
        100_000_000 // HCLK (BOARD.md 1)
    }

    fn phy_type(&self) -> PhyType {
        PhyType::ExternalHighSpeed
    }
}

type Bus = UsbBus<OtgHs>;

static EP_MEMORY: StaticCell<[u32; 1024]> = StaticCell::new();
static ALLOC: StaticCell<UsbBusAllocator<Bus>> = StaticCell::new();

/// The mass storage interface: its endpoints, and a Bulk-Only Mass Storage Reset received.
struct Msc {
    iface: InterfaceNumber,
    inp: EndpointIn<'static, Bus>,
    out: EndpointOut<'static, Bus>,
    reset: bool,
    /// A USB bus reset was seen.
    bus_reset: bool,
    /// The first failure's registers, until the app takes them ([`Link::snapshot`]).
    snap: Keeper,
}

impl UsbClass<Bus> for Msc {
    fn reset(&mut self) {
        self.bus_reset = true;
    }

    fn get_configuration_descriptors(&self, w: &mut DescriptorWriter) -> usb_device::Result<()> {
        w.interface(self.iface, 0x08, 0x06, 0x50)?;
        w.endpoint(&self.inp)?;
        w.endpoint(&self.out)
    }

    fn control_in(&mut self, xfer: ControlIn<Bus>) {
        let r = xfer.request();
        if r.request_type == RequestType::Class
            && r.recipient == Recipient::Interface
            && r.index == u16::from(u8::from(self.iface))
            && r.request == 0xfe
        {
            let _ = xfer.accept_with(&[0]); // Get Max LUN: one LUN
        }
    }

    fn control_out(&mut self, xfer: ControlOut<Bus>) {
        let r = xfer.request();
        if r.request_type == RequestType::Class
            && r.recipient == Recipient::Interface
            && r.index == u16::from(u8::from(self.iface))
            && r.request == 0xff
        {
            self.reset = true; // Bulk-Only Mass Storage Reset
            let _ = xfer.accept();
        }
    }
}

/// The drive's bulk endpoints for msc.rs, servicing USB while they wait, each wait 1 s at most (a computer that
/// stops reading or sending).
struct Bulk<'a> {
    dev: &'a mut UsbDevice<'static, Bus>,
    cdc: &'a mut CdcAcmClass<'static, Bus>,
    msc: &'a mut Msc,
}

impl msc::Pipe for Bulk<'_> {
    fn recv(&mut self, buf: &mut [u8; msc::PACKET]) -> Option<usize> {
        let t0 = cycles();
        loop {
            match self.msc.out.read(buf) {
                Ok(n) => return Some(n),
                Err(UsbError::WouldBlock) if cycles().wrapping_sub(t0) < CPU_HZ => {
                    self.dev.poll(&mut [&mut *self.cdc, &mut *self.msc]);
                }
                Err(_) => return None,
            }
        }
    }

    fn send(&mut self, data: &[u8]) -> bool {
        // in packets of the bus's size (a full-speed build's 64 bytes): Bulk-Only data is a stream
        data.chunks(MPS).all(|p| {
            let t0 = cycles();
            loop {
                match self.msc.inp.write(p) {
                    Ok(_) => {
                        pace();
                        return true;
                    }
                    Err(UsbError::WouldBlock) if cycles().wrapping_sub(t0) < CPU_HZ => {
                        self.dev.poll(&mut [&mut *self.cdc, &mut *self.msc]);
                    }
                    Err(UsbError::WouldBlock) => {
                        // the 1 s guard ran out: nobody is taking the packet. (Any other error is an eject, a bus
                        // reset or a deconfigure, which says nothing about the PHY.)
                        self.msc.snap.note(|| snapshot(usbsnap::EVENT_SEND));
                        return false;
                    }
                    Err(_) => return false,
                }
            }
        })
    }

    fn stall(&mut self) {
        self.msc.inp.stall();
    }
}

/// The running link.
pub struct Link {
    dev: UsbDevice<'static, Bus>,
    cdc: CdcAcmClass<'static, Bus>,
    msc: Msc,
    out: [u8; 512],
    len: usize,
    last_full: bool,
    /// DSTS.FNSOF when last seen changing, and when (ms, the caller's clock).
    sof: (u32, u32),
    /// The bus state the watch saw last, and since when (ms).
    bus: (UsbState, u32),
    /// Configured since the last reconnect.
    was_configured: bool,
    /// The last state before a suspend was Configured (cleared by a reconnect).
    before: bool,
    /// A drive command ran since [`Link::worked`]: its eMMC access may have slept, which takes over SysTick.
    worked: bool,
}

fn wait(cond: impl Fn() -> bool, ms: u32) -> bool {
    let t0 = cycles();
    while !cond() {
        if cycles().wrapping_sub(t0) > ms * (CPU_HZ / 1000) {
            return false;
        }
    }
    true
}

fn power_up() {
    modify(PWR_D3CR, 0, 3 << 14); // VOS1, as stock before USB (Usb_CheckBootIntoMassStorage)
    wait(|| rd(PWR_D3CR) & (1 << 13) != 0, 10); // VOSRDY
    modify(RCC_CR, 0, 1 << 12); // HSI48 for the USB kernel clock, whatever the bootloader left running
    wait(|| rd(RCC_CR) & (1 << 13) != 0, 10);
    modify(RCC_D2CCIP2R, 3 << 20, 3 << 20); // USBSEL = HSI48
    for (p, pin) in ULPI_PINS {
        gpio(p, pin, AF, 10);
    }
    modify(RCC_AHB1ENR, 0, 1 << 26); // USB1OTGHSULPIEN
    gpio(PHY_ENABLE.0, PHY_ENABLE.1, OUT, 0);
    pin_set(PHY_ENABLE.0, PHY_ENABLE.1, true);
    delay_ms(200);
    // Reset the core first: an image started over USB inherits the previous one's running core.
    modify(RCC_AHB1RSTR, 0, 1 << 25);
    modify(RCC_AHB1RSTR, 1 << 25, 0);
    modify(RCC_AHB1ENR, 0, 1 << 25); // USB1OTGHSEN
    let _ = rd(RCC_AHB1ENR);
}

/// The core soft reset the crate does, but with timeouts (the crate's waits have none): it completes only with
/// the PHY's 60 MHz clock, so a dead PHY costs a red dot instead of a hung bike computer.
fn core_resets() -> bool {
    if !wait(|| rd(GRSTCTL) & (1 << 31) != 0, 10) {
        return false; // AHBIDL
    }
    // ULPI: PHYSEL 0, no FS/LS serial mode, internal VBUS indicator off, as the crate sets them
    modify(GUSBCFG, (1 << 6) | (1 << 17) | (1 << 20) | (1 << 21) | (1 << 22), 0);
    modify(GRSTCTL, 0, 1); // CSRST
    wait(|| rd(GRSTCTL) & 1 == 0, 10)
}

/// Powers USB up and starts the device; None if the PHY or the core does not respond. Called once per boot.
pub fn start() -> Option<Link> {
    power_up();
    if !core_resets() {
        return None;
    }
    let alloc: &'static UsbBusAllocator<Bus> = ALLOC.init(UsbBus::new(OtgHs, EP_MEMORY.init([0; 1024])));
    let cdc = CdcAcmClass::new(alloc, MPS as u16);
    let msc = Msc {
        iface: alloc.interface(),
        inp: alloc.bulk(MPS as u16),
        out: alloc.bulk(MPS as u16),
        reset: false,
        bus_reset: false,
        snap: Keeper::default(),
    };
    // pid.codes test VID / PID (0x1209 / 0x0001): fine for a device that never leaves the bench
    let dev = UsbDeviceBuilder::new(alloc, UsbVidPid(0x1209, 0x0001))
        .strings(&[StringDescriptors::default()
            .manufacturer("bryton-rustfw")
            .product("bryton-rustfw dev link (S510)")
            .serial_number("s510")])
        .ok()?
        .composite_with_iads()
        .max_packet_size_0(64)
        .ok()?
        .build();
    if FULL_SPEED {
        // DCFG.DSPD 01: full speed over the ULPI PHY (RM0399 OTG_DCFG); the crate set high speed, so off the bus,
        // the speed changed, back on
        modify(DCTL, 0, 1 << 1);
        modify(DCFG, 3, 1);
        wait(|| false, 20);
        modify(DCTL, 1 << 1, 0);
    }
    wr(NVIC_ISER2, OTG_HS_IRQ);
    Some(Link {
        dev,
        cdc,
        msc,
        out: [0; 512],
        len: 0,
        last_full: false,
        sof: (u32::MAX, 0),
        bus: (UsbState::Unconfigured, 0),
        was_configured: false,
        before: false,
        worked: false,
    })
}

impl Link {
    /// Services USB: runs a drive command (msc.rs, through the app) and any bytes received on the dev link through
    /// its protocol.
    pub fn poll(&mut self, s: &mut Session, app: &mut App, board: &mut impl Board, stats: Stats) -> Effect {
        let mut effect = Effect::None;
        if self.dev.poll(&mut [&mut self.cdc, &mut self.msc]) {
            let mut buf = [0u8; 512];
            if core::mem::take(&mut self.msc.reset) {
                app.drive_reset();
            }
            if core::mem::take(&mut self.msc.bus_reset) {
                app.drive_bus_reset();
            }
            if let Ok(n @ 1..) = self.msc.out.read(&mut buf) {
                let mut pipe = Bulk { dev: &mut self.dev, cdc: &mut self.cdc, msc: &mut self.msc };
                app.drive_packet(board, &buf[..n], &mut pipe);
                self.worked = true;
            }
            while effect == Effect::None {
                match self.cdc.read_packet(&mut buf) {
                    Ok(n) if n > 0 => effect = s.feed(&buf[..n], app, stats, self),
                    _ => break,
                }
            }
            self.flush();
        }
        effect
    }

    /// Keeps servicing USB for `ms`, so the last answer reaches the computer before a reset or a jump.
    pub fn linger(&mut self, ms: u32) {
        let t0 = cycles();
        while cycles().wrapping_sub(t0) < ms * (CPU_HZ / 1000) {
            self.dev.poll(&mut [&mut self.cdc, &mut self.msc]);
        }
    }

    /// A drive command ran since the last call (its card access may have slept: the caller's sleep slice is lost).
    pub fn worked(&mut self) -> bool {
        core::mem::take(&mut self.worked)
    }

    /// Watches the link once per poll (lib dev.rs [`dev::watch_usb`] decides): `now_ms` is the main loop's clock
    /// (cycles stop in WFI, and wrap in 9 s), `failed` the drive's last command ended in a phase error.
    /// - **Suspend is a normal state:** usb-device goes to `Suspend` on OTG_GINTSTS.USBSUSP (the bus idle 3 ms,
    ///   RM0399 OTG_HS, OTG_GINTSTS / OTG_DSTS.SUSPSTS) and back to the state before on WKUPINT or any traffic
    ///   (usb-device `UsbDevice::poll`); nothing to do meanwhile, no interrupt comes, WFI sleeps whole slices.
    /// - **A drop:** configured and the frame number (DSTS.FNSOF, every 125 us at high speed) still for
    ///   [`dev::SOF_LOST_MS`] without a suspend (2026-10-24: the Mac lost the device mid-transfer, green dot), or
    ///   configured once then not for [`dev::UNCONFIGURED_MS`], or suspended after a failed transfer (the port
    ///   disabled, docs/usb.md): a soft disconnect and connect, so the computer may enumerate it again.
    pub fn watch(&mut self, now_ms: u32, failed: bool) -> UsbWatch {
        let fnsof = rd(DSTS) >> 8 & 0x3fff;
        let bus = match self.dev.state() {
            UsbDeviceState::Configured => UsbState::Configured,
            UsbDeviceState::Suspend => UsbState::Suspended { configured: self.before },
            _ => UsbState::Unconfigured,
        };
        if bus == UsbState::Configured {
            (self.before, self.was_configured) = (true, true);
        } else if bus == UsbState::Unconfigured {
            self.before = false;
        }
        if bus != self.bus.0 {
            self.bus = (bus, now_ms);
        }
        if bus != UsbState::Configured || fnsof != self.sof.0 {
            self.sof = (fnsof, now_ms);
        }
        let since = if bus == UsbState::Configured { self.sof.1 } else { self.bus.1 };
        let w = dev::watch_usb(bus, self.was_configured, now_ms.wrapping_sub(since), failed);
        if w == UsbWatch::Drop {
            // the watch's failures only: the frame number frozen while configured, or a suspend after a phase error
            // (not the host going away or sleeping); before the reconnect changes the core
            if bus == UsbState::Configured || failed {
                self.msc.snap.note(|| snapshot(usbsnap::EVENT_DROP));
            }
            self.reconnect(now_ms);
        }
        w
    }

    /// The first failure's registers since the link came up, once (the app keeps them in backup registers).
    pub fn snapshot(&mut self) -> Option<UsbSnap> {
        self.msc.snap.take()
    }

    /// A soft disconnect and connect: the computer sees a detach, then an attach, and enumerates the device again.
    fn reconnect(&mut self, now_ms: u32) {
        modify(DCTL, 0, 1 << 1); // SDIS: the D+ pull-up off
        wait(|| false, 100);
        modify(DCTL, 1 << 1, 0);
        self.sof = (u32::MAX, now_ms);
        (self.was_configured, self.before) = (false, false);
    }

    /// Soft disconnect, so the computer sees the device leave before the next image enumerates again.
    pub fn disconnect(&mut self) {
        modify(DCTL, 0, 1 << 1); // SDIS
    }

    /// Writes the buffered bytes in packets of the bus's size (one zero-length packet for none), servicing USB while
    /// the endpoint is busy; gives up after 1 s a packet (nobody reading).
    fn write(&mut self, len: usize) {
        let data = self.out.get(..len).unwrap_or(&[]);
        for p in data.chunks(MPS).chain((len == 0).then_some(&[][..])) {
            let t0 = cycles();
            loop {
                match self.cdc.write_packet(p) {
                    Err(UsbError::WouldBlock) if cycles().wrapping_sub(t0) < CPU_HZ => {
                        self.dev.poll(&mut [&mut self.cdc, &mut self.msc]);
                    }
                    _ => break,
                }
            }
        }
        self.last_full = len > 0 && len.is_multiple_of(MPS);
        self.len = 0;
    }

    /// Sends what is buffered; a transfer ending on a full packet needs a zero-length one to end it.
    pub fn flush(&mut self) {
        if self.len > 0 || self.last_full {
            self.write(self.len);
        }
        self.last_full = false;
    }
}

impl Host for Link {
    fn send(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            let n = data.len().min(self.out.len() - self.len);
            self.out[self.len..self.len + n].copy_from_slice(&data[..n]);
            self.len += n;
            data = &data[n..];
            if self.len == self.out.len() {
                self.write(self.len);
            }
        }
    }

    fn stage(&mut self, offset: u32, data: &[u8]) {
        stage_write(offset, data);
    }

    fn flush(&mut self) {
        Link::flush(self);
    }

    // The computer's addresses reach rd / wr only here, checked again whatever the caller (dev.rs checks them too):
    // aligned, poke in the peripheral space only (RM0399's memory map: 0x40000000 to 0x5fffffff), so the poke itself
    // never writes RAM; peek there or in the DTCM or the SDRAM ([`dev::MEMORY`]), always mapped. A poke can still
    // make the hardware write RAM (a DMA, MDMA or FMC register, the RCC or the MPU's peers): the dev link is a
    // debugging tool, and that is its operator's to avoid, not something this check can promise.
    fn peek(&mut self, addr: u32) -> u32 {
        let ok = addr.is_multiple_of(4) && (dev::IO.contains(&addr) || dev::MEMORY.iter().any(|r| r.contains(&addr)));
        if ok { rd(addr) } else { 0 }
    }

    fn poke(&mut self, addr: u32, value: u32) {
        if addr.is_multiple_of(4) && dev::IO.contains(&addr) {
            wr(addr, value);
        }
    }
}
