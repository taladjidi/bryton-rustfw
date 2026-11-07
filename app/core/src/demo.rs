//! A fake unit: answers the file service's commands with the firmware's own encoder. The tests use it to drive
//! [`crate::files::FileClient`]; the app's Demo mode (the Simulator has no Bluetooth) serves synthetic rides from it.
//! The rides are made by the firmware's FIT writer from invented samples (a made-up track near latitude 0, longitude
//! 0): no real ride, no real place.

use bryton_rustfw::filesvc::{CHUNK_MAX, Command, FRAME_MAX, Frame, Name, Status};
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

/// A synthetic ride of `seconds` starting at 2026-11-01 `hour`:00 UTC, by the firmware's FIT writer.
pub fn synthetic_ride(hour: u8, seconds: u32) -> Vec<u8> {
    let t0 = fit::fit_time((2026, 11, 1, hour, 0, 0));
    let mut m = Mem(Vec::new());
    let Ok(mut w) = Writer::begin_with(&mut m, t0, false) else { return Vec::new() };
    for i in 0..seconds {
        let s = Sample {
            time: t0 + i,
            pos: Some((i32::try_from(i * 90).unwrap_or(0), i32::try_from(i * 60).unwrap_or(0))),
            alt_mm: Some(50_000 + (i32::try_from(i % 600).unwrap_or(0) - 300).abs() * 100),
            speed_mm_s: Some(7_000 + (i % 40) * 100),
            hr: Some(u8::try_from(120 + i % 50).unwrap_or(120)),
            cadence: Some(u8::try_from(80 + i % 15).unwrap_or(80)),
            power: Some(u16::try_from(150 + i % 120).unwrap_or(150)),
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
    let mut b = [0; FRAME_MAX];
    let n = f.encode(&mut b);
    b.get(..n).unwrap_or(&[]).to_vec()
}

impl FakeUnit {
    /// A unit with these rides.
    pub fn new(rides: Vec<(Name, Vec<u8>)>) -> FakeUnit {
        FakeUnit { rides, faults: Faults { chunk: CHUNK_MAX, ..Faults::default() } }
    }

    /// The Demo unit: three synthetic rides.
    pub fn demo() -> FakeUnit {
        FakeUnit::new(vec![
            (*b"261101090000", synthetic_ride(9, 1500)),
            (*b"261031173000", synthetic_ride(17, 600)),
            (*b"261029064500", synthetic_ride(6, 2400)),
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
                let mut out: Vec<Vec<u8>> =
                    rest.chunks(self.faults.chunk.clamp(1, CHUNK_MAX)).map(|c| enc(Frame::Chunk { seq, data: c })).collect();
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
