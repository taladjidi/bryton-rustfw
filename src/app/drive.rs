//! The USB drive from the app's side: the eMMC's FAT volume lent to the computer (msc.rs) on request, one owner at
//! a time. Lent, the card answers nothing else ([`emmc::Emmc::lend`]) and the app's own card work (the recorder,
//! the scans, the basemap) waits; a ride cannot start. Taken back on the computer's eject, or from the unit when the
//! computer allows it.

use crate::*;

impl App {
    /// USB DRIVE (DEVICE menu, dev link `drive`): lends the card to the computer, unless USB is not up or a ride is
    /// running or not saved yet; why not is said in the banner and returned.
    pub fn drive_on(&mut self) -> Result<(), &'static str> {
        let why = if self.strip.usb != UsbDot::Ready {
            Some("CONNECT USB")
        } else if self.ride.state() != ride::State::Idle {
            Some("END THE RIDE FIRST")
        } else if !self.ride_settled() {
            Some("RIDE NOT SAVED YET")
        } else {
            None
        };
        if let Some(why) = why {
            self.say(why);
            return Err(why);
        }
        if !self.emmc.lent() {
            self.past.stop(); // a past ride being read
            self.emmc.lend(true);
            self.drive.insert();
        }
        self.say("USB DRIVE ON");
        Ok(())
    }

    /// From the unit: takes the card back, unless the computer has it mounted (it said so: PREVENT MEDIUM
    /// REMOVAL), which must eject it first.
    pub(crate) fn drive_off(&mut self) {
        if self.drive.prevented() {
            return self.say("EJECT ON COMPUTER");
        }
        self.drive.remove();
        self.take_card_back();
    }

    fn take_card_back(&mut self) {
        self.emmc.lend(false);
        if self.drive.wrote() {
            // ponytail: the computer changed the volume under every cache (the FAT reader's, the scan's index, the
            // course list, the basemap's tiles): a restart drops them all, in 2 s and with no ride open (refused
            // while one is). Ceiling: that restart; upgrade path: a forget() on each cache.
            self.restart = true;
            self.say("RESTARTING");
        } else {
            self.say("USB DRIVE OFF");
        }
    }

    /// A packet from the computer on the drive's Bulk-Out endpoint: a command, run to its end through `pipe`.
    pub fn drive_packet(&mut self, b: &mut impl Board, cbw: &[u8], pipe: &mut impl msc::Pipe) {
        let mut media = Touching {
            lent: emmc::Lent(&mut self.emmc, b),
            touch: &mut self.touch,
            ok: self.touch_ok,
            pending: &mut self.touch_pending,
            sampled: &mut self.touch_sampled,
        };
        let ev = self.drive.packet(cbw, pipe, &mut media);
        if ev == msc::Event::Ejected && self.emmc.lent() {
            self.take_card_back();
        }
    }

    /// The drive's Bulk-Only Mass Storage Reset (the class request).
    pub fn drive_reset(&mut self) {
        self.drive.reset();
    }

    /// A USB bus reset: the drive's state too (msc.rs [`msc::Drive::bus_reset`]).
    pub fn drive_bus_reset(&mut self) {
        self.drive.bus_reset();
    }

    /// The drive's last commands (the dev link's `drivelog`).
    pub fn drive_log(&self) -> &msc::Log {
        &self.drive.log
    }

    /// The card is lent to the computer.
    pub fn drive_lent(&self) -> bool {
        self.emmc.lent()
    }

    /// The app asks for a system reset (main.rs makes it): after the computer wrote to the volume.
    pub fn restart_wanted(&self) -> bool {
        self.restart
    }
}

/// The card lent, and the touch panel sampled after each chunk the drive reads or writes: a command can take a
/// second (a 1 MB read at full speed), during which the main loop reads no gesture (seen on the unit, 2026-10-24:
/// swipes missed during a copy). A gesture that ends waits in `pending` for the main loop.
struct Touching<'a, B: Board> {
    lent: emmc::Lent<'a, B>,
    touch: &'a mut touch::Recogniser,
    ok: bool,
    pending: &'a mut Option<Input>,
    sampled: &'a mut bool,
}

impl<B: Board> Touching<'_, B> {
    /// After `bytes` read or written.
    fn sample(&mut self, bytes: usize) {
        // ponytail: the time counted is what the bytes take at full speed, about 1 MB/s (no clock on Board); only a
        // hold's 600 ms reads it, and comes sooner at high speed (7 MB/s), where a read per 8 KB also costs about a
        // quarter of the throughput (review): sample less often there if high speed comes back
        let ms = (bytes / 1024).max(1) as u32;
        *self.sampled = true;
        if self.pending.is_none()
            // the point read after every chunk, finger or not: INT may only pulse (stock takes its falling edge, which
            // the main loop latches), and a swipe's start missed until the command ends is a short drag (seen on the
            // unit, 2026-10-24); one 3-byte I2C read per 8 KB
            && let (Some(g), _) = sample_touch(self.lent.1, self.touch, self.ok, true, ms)
        {
            *self.pending = Some(Input::Touch(g));
        }
    }
}

impl<B: Board> msc::Media for Touching<'_, B> {
    fn blocks(&mut self) -> Option<u32> {
        self.lent.blocks()
    }
    fn writable(&self) -> bool {
        self.lent.writable()
    }
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> bool {
        let r = self.lent.read(lba, buf);
        self.sample(buf.len());
        r
    }
    fn write(&mut self, lba: u32, buf: &[u8]) -> bool {
        let r = self.lent.write(lba, buf);
        self.sample(buf.len());
        r
    }
}
