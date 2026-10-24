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
use bryton_rustfw::dev::{Effect, Host, Session, Stats};
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
/// Configured, but no start of frame for this long: the computer dropped the device (module doc, "Drops").
const SOF_LOST_MS: u32 = 200;
const RCC_AHB1RSTR: u32 = RCC + 0x080;
const RCC_AHB1ENR: u32 = RCC + 0x0d8;
const RCC_D2CCIP2R: u32 = RCC + 0x054;
const PWR_D3CR: u32 = 0x5802_4818;
const NVIC_ISER2: u32 = 0xe000_e108;
const NVIC_ICPR2: u32 = 0xe000_e288;
const OTG_HS_IRQ: u32 = 1 << (77 - 64); // IRQ 77

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
}

impl UsbClass<Bus> for Msc {
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
        let t0 = cycles();
        loop {
            match self.msc.inp.write(data) {
                Ok(_) => return true,
                Err(UsbError::WouldBlock) if cycles().wrapping_sub(t0) < CPU_HZ => {
                    self.dev.poll(&mut [&mut *self.cdc, &mut *self.msc]);
                }
                Err(_) => return false,
            }
        }
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
    /// DSTS.FNSOF when last seen changing, and when (cycles).
    sof: (u32, u32),
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
    let cdc = CdcAcmClass::new(alloc, 512);
    let msc = Msc { iface: alloc.interface(), inp: alloc.bulk(512), out: alloc.bulk(512), reset: false };
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
    wr(NVIC_ISER2, OTG_HS_IRQ);
    Some(Link { dev, cdc, msc, out: [0; 512], len: 0, last_full: false, sof: (u32::MAX, cycles()), worked: false })
}

impl Link {
    /// The computer has configured the device: the dev link is usable.
    pub fn configured(&self) -> bool {
        self.dev.state() == UsbDeviceState::Configured
    }

    /// Services USB: runs a drive command (msc.rs, through the app) and any bytes received on the dev link through
    /// its protocol.
    pub fn poll(&mut self, s: &mut Session, app: &mut App, board: &mut impl Board, stats: Stats) -> Effect {
        let mut effect = Effect::None;
        if self.dev.poll(&mut [&mut self.cdc, &mut self.msc]) {
            let mut buf = [0u8; 512];
            if core::mem::take(&mut self.msc.reset) {
                app.drive_reset();
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
        wr(NVIC_ICPR2, OTG_HS_IRQ); // the core's flags are handled: let the next activity wake WFI again
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

    /// Watches the frame number the computer's starts of frame advance (every 125 us at high speed): configured and
    /// still for [`SOF_LOST_MS`], the computer has dropped the device (seen on the unit, 2026-10-24: the Mac lost it
    /// mid-transfer while the unit still showed a green dot; stock dropped the same way on 2026-10-22). Then a soft
    /// disconnect and connect, so the computer enumerates it again without an unplug. True when it did.
    pub fn watch(&mut self) -> bool {
        let now = cycles();
        let fnsof = rd(DSTS) >> 8 & 0x3fff;
        if !self.configured() || fnsof != self.sof.0 {
            self.sof = (fnsof, now);
            return false;
        }
        if now.wrapping_sub(self.sof.1) < SOF_LOST_MS * (CPU_HZ / 1000) {
            return false;
        }
        modify(DCTL, 0, 1 << 1); // SDIS: the D+ pull-up off, the computer sees a detach
        wait(|| false, 100);
        modify(DCTL, 1 << 1, 0); // and an attach: it resets and enumerates the device again
        self.sof = (u32::MAX, cycles());
        true
    }

    /// Soft disconnect, so the computer sees the device leave before the next image enumerates again.
    pub fn disconnect(&mut self) {
        modify(DCTL, 0, 1 << 1); // SDIS
    }

    /// Writes the buffered packet, servicing USB while the endpoint is busy; gives up after 1 s (nobody reading).
    fn write(&mut self, len: usize) {
        let t0 = cycles();
        loop {
            match self.cdc.write_packet(&self.out[..len]) {
                Err(UsbError::WouldBlock) if cycles().wrapping_sub(t0) < CPU_HZ => {
                    self.dev.poll(&mut [&mut self.cdc, &mut self.msc]);
                }
                _ => break,
            }
        }
        self.last_full = len == self.out.len();
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

    fn peek(&mut self, addr: u32) -> u32 {
        rd(addr)
    }

    fn poke(&mut self, addr: u32, value: u32) {
        wr(addr, value);
    }
}
