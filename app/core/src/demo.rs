//! A fake unit: answers the file service's commands with the firmware's own encoder. The tests use it to drive
//! [`crate::files::FileClient`]; the app's Demo mode (the Simulator has no Bluetooth) serves synthetic rides from it.
//! The rides are made by the firmware's FIT writer from invented samples: a made-up loop (a lap of about 5 km, a
//! wobbling circle) around a public landmark, the Eiffel Tower in Paris; no real ride, nothing of the owner's.

use bryton_rustfw::filesvc::{Command, FRAME_BIG, Frame, Name, Status};

/// The largest CHUNK payload: the fast link's frame (the largest MTU less 3) less the CHUNK header (kind, seq). The
/// demo unit serves it, as the unit does on a phone that exchanged the MTU.
pub const BIG_CHUNK: usize = FRAME_BIG - 2;
use bryton_rustfw::fit::{self, Sample, Sink, Writer};
use std::sync::Mutex;

/// A FIT file in memory for the writer.
struct Mem(Vec<u8>);

impl Sink for Mem {
    fn write_at(&mut self, offset: u32, data: &[u8]) -> Result<(), fit::Error> {
        let (start, end) = (offset as usize, offset as usize + data.len());
        if self.0.len() < end {
            self.0.resize(end, 0);
        }
        self.0.get_mut(start..end).ok_or(fit::Error)?.copy_from_slice(data);
        Ok(())
    }
    fn set_len(&mut self, len: u32) -> Result<(), fit::Error> {
        self.0.resize(len as usize, 0);
        Ok(())
    }
    fn flush(&mut self) -> Result<(), fit::Error> {
        Ok(())
    }
}

/// Where second `i` is on the demo loop, degrees x 1e7: a circle of about 0.8 km radius with a wobble, around the
/// Eiffel Tower (48.8584 N, 2.2945 E, public), one lap every 600 s.
fn loop_point(i: u32) -> (i32, i32) {
    let a = f64::from(i % 600) / 600.0 * std::f64::consts::TAU;
    let r = 0.007 + 0.0015 * (3.0 * a).sin();
    let (lat, lon) = (48.8584 + r * a.sin(), 2.2945 + r * 1.5 * a.cos());
    ((lat * 1e7) as i32, (lon * 1e7) as i32)
}

/// A synthetic ride of `seconds` starting at 2026-11-01 `hour`:00 UTC, by the firmware's FIT writer. Smooth waves
/// for the altitude, power, cadence and speed, a heart rate that lags the power, a coast every 450 s. The Demo unit
/// names its files in Paris time (UTC+1 in November), as the bike computer names them in local time.
pub fn synthetic_ride(hour: u8, seconds: u32) -> Vec<u8> {
    let t0 = fit::fit_time((2026, 11, 1, hour, 0, 0));
    let mut m = Mem(Vec::new());
    let Ok(mut w) = Writer::begin_with(&mut m, t0, false) else { return Vec::new() };
    let mut hr_f = 110.0f64;
    for i in 0..seconds {
        let x = f64::from(i);
        let wave = |period: f64, phase: f64| (std::f64::consts::TAU * x / period + phase).sin();
        // pseudo-random in [-1, 1], from the second and a channel
        let n =
            |k: u32| f64::from((i.wrapping_mul(2_654_435_761).wrapping_add(k * 40_503) >> 16) & 0xffff) / 32767.5 - 1.0;
        let coast = (300..320).contains(&(i % 450));
        let power = if coast {
            0.0
        } else {
            (185.0 + 55.0 * wave(240.0, 0.0) + 30.0 * wave(47.0, 1.0) + 12.0 * n(1)).clamp(0.0, 900.0)
        };
        // the heart rate follows the power with a lag
        hr_f += (105.0 + 0.38 * (power - 100.0) + 0.004 * x - hr_f) / 25.0;
        let cadence = if coast { 0.0 } else { 88.0 + 6.0 * wave(90.0, 0.0) + 0.03 * (power - 185.0) + 1.5 * n(2) };
        let alt_m = 65.0 + 15.0 * wave(600.0, 0.0) + 1.2 * wave(97.0, 0.0);
        let speed = 8.4 + 1.2 * wave(150.0, 0.0) + 0.9 * wave(600.0, 2.0) + 0.2 * n(3);
        let s = Sample {
            time: t0 + i,
            pos: Some(loop_point(i)),
            alt_mm: Some((alt_m * 1000.0) as i32),
            speed_mm_s: Some((speed * 1000.0) as u32),
            hr: Some(hr_f.round().clamp(60.0, 200.0) as u8),
            cadence: Some(cadence.clamp(0.0, 130.0) as u8),
            power: Some(power as u16),
            ..Sample::default()
        };
        if w.record(&mut m, &s).is_err() {
            break;
        }
    }
    if w.end(&mut m, t0 + seconds).is_err() || w.checkpoint(&mut m).is_err() {
        return Vec::new();
    }
    m.0
}

/// What the fake unit does to its next answers (the tests set them, Demo leaves them off).
#[derive(Clone, Debug, Default)]
pub struct Faults {
    /// Answer every command with END BUSY.
    pub busy: bool,
    /// Stop a READ silently after this many bytes (a dropped link); cleared once used.
    pub drop_after: Option<usize>,
    /// Send this many bytes fewer than asked, then END OK (a short file).
    pub short_by: usize,
    /// END's count is wrong by this much.
    pub count_off: u32,
    /// CHUNK size (cut to the firmware's maximum).
    pub chunk: usize,
}

/// The fake unit.
pub struct FakeUnit {
    /// The rides, `(YYMMDDhhmmss, bytes)`.
    pub rides: Vec<(Name, Vec<u8>)>,
    /// What goes wrong next.
    pub faults: Faults,
}

fn enc(f: Frame) -> Vec<u8> {
    let mut b = [0; FRAME_BIG];
    let n = f.encode(&mut b);
    b.get(..n).unwrap_or(&[]).to_vec()
}

impl FakeUnit {
    /// A unit with these rides.
    pub fn new(rides: Vec<(Name, Vec<u8>)>) -> FakeUnit {
        FakeUnit { rides, faults: Faults { chunk: BIG_CHUNK, ..Faults::default() } }
    }

    /// The Demo unit: three synthetic rides.
    pub fn demo() -> FakeUnit {
        FakeUnit::new(vec![
            (*b"261101100000", synthetic_ride(9, 1500)),
            (*b"261101180000", synthetic_ride(17, 600)),
            (*b"261101070000", synthetic_ride(6, 2400)),
        ])
    }

    /// The Data frames the unit notifies for one write to Control, in order.
    pub fn handle(&mut self, cmd: &[u8]) -> Vec<Vec<u8>> {
        let end = |seq, status, count| vec![enc(Frame::End { seq, status, count })];
        let cmd = match Command::parse(cmd) {
            Ok(c) => c,
            Err((seq, st)) => return end(seq, st, 0),
        };
        match cmd {
            Command::Cancel => Vec::new(),
            Command::List { seq } if self.faults.busy => end(seq, Status::Busy, 0),
            Command::Read { seq, .. } if self.faults.busy => end(seq, Status::Busy, 0),
            Command::List { seq } => {
                let mut out: Vec<Vec<u8>> = self
                    .rides
                    .iter()
                    .map(|(name, d)| enc(Frame::Entry { seq, name: *name, size: u32::try_from(d.len()).unwrap_or(0) }))
                    .collect();
                out.push(enc(Frame::End { seq, status: Status::Ok, count: u32::try_from(out.len()).unwrap_or(0) }));
                out
            }
            Command::Read { seq, offset, name } => {
                let Some((_, data)) = self.rides.iter().find(|(n, _)| *n == name) else {
                    return end(seq, Status::NotFound, 0);
                };
                let Some(rest) = data.get(offset as usize..) else { return end(seq, Status::BadRequest, 0) };
                let mut rest = rest.get(..rest.len().saturating_sub(self.faults.short_by)).unwrap_or(&[]);
                let dropped = self.faults.drop_after.take();
                if let Some(n) = dropped {
                    rest = rest.get(..n.min(rest.len())).unwrap_or(&[]);
                }
                let mut out: Vec<Vec<u8>> = rest
                    .chunks(self.faults.chunk.clamp(1, BIG_CHUNK))
                    .map(|c| enc(Frame::Chunk { seq, data: c }))
                    .collect();
                if dropped.is_none() {
                    let count = u32::try_from(rest.len()).unwrap_or(0).wrapping_add(self.faults.count_off);
                    out.push(enc(Frame::End { seq, status: Status::Ok, count }));
                }
                out
            }
        }
    }
}

/// The unit for Demo mode, exposed to Swift.
#[derive(uniffi::Object)]
pub struct DemoUnit(Mutex<FakeUnit>);

#[uniffi::export]
impl DemoUnit {
    /// Three synthetic rides.
    #[uniffi::constructor]
    pub fn new() -> Self {
        DemoUnit(Mutex::new(FakeUnit::demo()))
    }

    /// The Data frames for one Control write.
    pub fn handle(&self, command: Vec<u8>) -> Vec<Vec<u8>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).handle(&command)
    }
}

impl Default for DemoUnit {
    fn default() -> Self {
        Self::new()
    }
}
