//! The file service on the phone bond (roadmap 9, step A1, reads only): the phone lists and downloads the rides in
//! `\Activities\`, the unit answers. Pure `no_std` frame formats, encode and decode both ways, with no firmware
//! types in the API, so the phone app's Rust core can reuse this very file.
//!
//! **GATT** (all attributes need the encrypted bond, as the HID ones do). A vendor 128-bit base, random
//! (`aa3c0000-98d8-4772-99ed-dfb117c6746f`, generated for this project); the 16-bit part (bytes 12 and 13 of the
//! little endian form) names the attribute:
//! - service `aa3c0001-...`;
//! - Control `aa3c0002-...`: the phone writes a command (write with response, 20 bytes at most);
//! - Data `aa3c0003-...`: the unit notifies the answer (the phone enables its CCCD first).
//!
//! **Sizes:** the ATT MTU stays 23, so a write or a notification carries 20 bytes ([`FRAME_MAX`]); one number to
//! change for a larger MTU, plus the constants derived from it here.
//!
//! **Commands** (Control), the first byte is the command, `seq` a byte the phone chooses and every answer frame
//! echoes, so frames still in flight from an earlier command are told apart:
//! ```text
//! LIST    01 seq                          the rides, one ENTRY each, then END
//! READ    02 seq off[4] name[12]          file `name`.fit from byte `off` (u32 little endian), CHUNKs, then END
//! CANCEL  03                              stops the answer under way, silently
//! ```
//! `name` is the twelve digits of a stock ride name, `YYMMDDhhmmss` (`YYMMDDhhmmss.fit` in `\Activities\`), as
//! LIST gives them. Nothing else of the card is reachable: no path, no other folder.
//! A new command replaces the one under way. A READ resumes a dropped link: ask again from the bytes already kept.
//!
//! **Answers** (Data):
//! ```text
//! ENTRY   81 seq name[12] size[4]         one ride (size in bytes, u32 little endian)
//! CHUNK   82 seq data[1..=18]             the next bytes of the file, in order from `off`
//! END     83 seq status count[4]          the answer is complete; count: entries or bytes sent (u32 little endian)
//! ```
//! Status: 0 ok, 1 not found, 2 busy, 3 bad request, 4 card error. BUSY while a ride runs, is paused or is being
//! saved, or while the card is lent to a computer: the unit never reads under the recorder. A bad command gets an
//! END with BAD REQUEST (and the `seq` it carried, 0 if it carried none). END after the last CHUNK is how the phone
//! knows the file is whole: its bytes equal `count`, and `off + count` equals the size LIST gave.
//!
//! **Dropped and moved:** a frame the stack does not take for 5 s (Data not subscribed, the link quiet) drops the
//! answer silently: no frame for 5 s means dropped, re-issue READ from the bytes kept. A ride deleted from HISTORY
//! while it is read ends the answer with BUSY, so the phone checks the bytes it holds against the size LIST gave.
//! END's `count` is only what the stack took, so `off + count` is where to resume.
//! **Who:** the stored bond and an encrypted link are both required. The protection is the pairing itself: Just Works,
//! accepted only during the 60 s PAIR window of the PHONE menu, so whoever pairs then can read the rides (GPS tracks).
//!
//! Notifications are ordered and the link is reliable, so a CHUNK carries no offset; the phone counts.

/// The vendor base, little endian, bytes 12 and 13 (the 16-bit part) zero: what `sd_ble_uuid_vs_add` takes.
pub const BASE: [u8; 16] =
    [0x6f, 0x74, 0xc6, 0x17, 0xb1, 0xdf, 0xed, 0x99, 0x72, 0x47, 0xd8, 0x98, 0x00, 0x00, 0x3c, 0xaa];
/// The service's 16-bit part.
pub const SERVICE: u16 = 1;
/// Control's 16-bit part (the phone writes).
pub const CONTROL: u16 = 2;
/// Data's 16-bit part (the unit notifies).
pub const DATA: u16 = 3;

/// A write or a notification's longest value: the ATT MTU (23) less its 3-byte header. Change with the MTU.
pub const FRAME_MAX: usize = 20;
/// File bytes in a CHUNK: a frame less its kind and `seq`.
pub const CHUNK_MAX: usize = FRAME_MAX - 2;
/// A ride's name: twelve digits.
pub const NAME_LEN: usize = 12;
/// A ride's name, ASCII digits.
pub type Name = [u8; NAME_LEN];
/// `YYMMDDhhmmss.fit`: the file name of a ride.
pub const FILE_NAME_LEN: usize = NAME_LEN + 4;

/// How an answer ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Complete.
    Ok = 0,
    /// No such ride.
    NotFound = 1,
    /// A ride is on, or the card is lent: ask again later.
    Busy = 2,
    /// The command is not one, or its offset is past the end of the file.
    BadRequest = 3,
    /// The card did not answer.
    Io = 4,
}

impl Status {
    /// The status a byte stands for.
    pub fn from_u8(v: u8) -> Option<Status> {
        Some(match v {
            0 => Status::Ok,
            1 => Status::NotFound,
            2 => Status::Busy,
            3 => Status::BadRequest,
            4 => Status::Io,
            _ => return None,
        })
    }
}

/// A command the phone writes to Control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// List the rides.
    List {
        /// Echoed by every frame of the answer.
        seq: u8,
    },
    /// Read ride `name` from byte `offset`.
    Read {
        /// Echoed by every frame of the answer.
        seq: u8,
        /// First byte wanted.
        offset: u32,
        /// The ride.
        name: Name,
    },
    /// Stop the answer under way.
    Cancel,
}

const LIST: u8 = 1;
const READ: u8 = 2;
const CANCEL: u8 = 3;
const ENTRY: u8 = 0x81;
const CHUNK: u8 = 0x82;
const END: u8 = 0x83;

/// `v` at `at` in `out`; the end of what is written (unchanged when it does not fit: never, for the frames here).
fn put(out: &mut [u8; FRAME_MAX], at: usize, v: &[u8]) -> usize {
    match out.get_mut(at..at + v.len()) {
        Some(d) => {
            d.copy_from_slice(v);
            at + v.len()
        }
        None => at,
    }
}

/// Four bytes at `at` as a little endian number.
fn u32_at(p: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(p.get(at..at + 4)?.try_into().ok()?))
}

/// Twelve ASCII digits.
pub fn is_name(b: &[u8]) -> bool {
    b.len() == NAME_LEN && b.iter().all(u8::is_ascii_digit)
}

/// The ride name of a file called `YYMMDDhhmmss.fit` (any case), else None: stock's `tmp.fit` and anything else are
/// not rides here.
///
/// ```
/// use bryton_rustfw::filesvc::ride_name;
/// assert_eq!(ride_name(b"251026093000.fit"), Some(*b"251026093000"));
/// assert_eq!(ride_name(b"251026093000.FIT"), Some(*b"251026093000"));
/// assert_eq!(ride_name(b"tmp.fit"), None);
/// assert_eq!(ride_name(b"251026093000.gpx"), None);
/// ```
pub fn ride_name(file: &[u8]) -> Option<Name> {
    let (name, ext) = (file.get(..NAME_LEN)?, file.get(NAME_LEN..)?);
    (is_name(name) && ext.eq_ignore_ascii_case(b".fit")).then(|| {
        let mut n = [0; NAME_LEN];
        n.copy_from_slice(name);
        n
    })
}

/// The file name of ride `name`, `YYMMDDhhmmss.fit`, in `out`.
///
/// ```
/// use bryton_rustfw::filesvc::file_name;
/// let mut buf = [0; 16];
/// assert_eq!(file_name(b"251026093000", &mut buf), Some("251026093000.fit"));
/// assert_eq!(file_name(b"../../ble.bin", &mut buf), None);
/// ```
pub fn file_name<'a>(name: &[u8], out: &'a mut [u8; FILE_NAME_LEN]) -> Option<&'a str> {
    if !is_name(name) {
        return None;
    }
    let (a, b) = out.split_at_mut(NAME_LEN);
    a.copy_from_slice(name);
    b.copy_from_slice(b".fit");
    core::str::from_utf8(out).ok()
}

impl Command {
    /// A command from what was written, or the `seq` it carried (0: none) and why it is refused.
    ///
    /// ```
    /// use bryton_rustfw::filesvc::{Command, Status};
    /// assert_eq!(Command::parse(&[1, 7]), Ok(Command::List { seq: 7 }));
    /// assert_eq!(Command::parse(&[3]), Ok(Command::Cancel));
    /// assert_eq!(Command::parse(&[9, 7]), Err((7, Status::BadRequest)));
    /// assert_eq!(Command::parse(&[2, 7, 0, 0, 0, 0, b'x']), Err((7, Status::BadRequest)));
    /// ```
    pub fn parse(p: &[u8]) -> Result<Command, (u8, Status)> {
        let seq = p.get(1).copied().unwrap_or(0);
        let bad = Err((seq, Status::BadRequest));
        match p.first() {
            Some(&LIST) if p.len() == 2 => Ok(Command::List { seq }),
            Some(&CANCEL) if p.len() == 1 => Ok(Command::Cancel),
            Some(&READ) if p.len() == 6 + NAME_LEN => match (u32_at(p, 2), p.get(6..)) {
                (Some(offset), Some(n)) if is_name(n) => {
                    let mut name = [0; NAME_LEN];
                    name.copy_from_slice(n);
                    Ok(Command::Read { seq, offset, name })
                }
                _ => bad,
            },
            _ => bad,
        }
    }

    /// The bytes to write for this command; their count.
    ///
    /// ```
    /// use bryton_rustfw::filesvc::Command;
    /// let c = Command::Read { seq: 3, offset: 0x0102_0304, name: *b"251026093000" };
    /// let mut f = [0; 20];
    /// let n = c.encode(&mut f);
    /// assert_eq!(Command::parse(&f[..n]), Ok(c));
    /// ```
    pub fn encode(&self, out: &mut [u8; FRAME_MAX]) -> usize {
        match self {
            Command::List { seq } => put(out, 0, &[LIST, *seq]),
            Command::Cancel => put(out, 0, &[CANCEL]),
            Command::Read { seq, offset, name } => {
                let n = put(out, 0, &[READ, *seq]);
                let n = put(out, n, &offset.to_le_bytes());
                put(out, n, name)
            }
        }
    }
}

/// A frame the unit notifies on Data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frame<'a> {
    /// One ride of a LIST.
    Entry {
        /// The command's.
        seq: u8,
        /// The ride.
        name: Name,
        /// Its size, bytes.
        size: u32,
    },
    /// Bytes of a READ.
    Chunk {
        /// The command's.
        seq: u8,
        /// Up to [`CHUNK_MAX`] bytes.
        data: &'a [u8],
    },
    /// The answer is over.
    End {
        /// The command's.
        seq: u8,
        /// How it went.
        status: Status,
        /// Entries or bytes sent in the answer.
        count: u32,
    },
}

impl<'a> Frame<'a> {
    /// The bytes to notify for this frame; their count (a CHUNK's data is cut to [`CHUNK_MAX`]).
    ///
    /// ```
    /// use bryton_rustfw::filesvc::{Frame, Status};
    /// let mut f = [0; 20];
    /// let n = Frame::End { seq: 2, status: Status::Busy, count: 0 }.encode(&mut f);
    /// assert_eq!(&f[..n], &[0x83, 2, 2, 0, 0, 0, 0]);
    /// ```
    pub fn encode(&self, out: &mut [u8; FRAME_MAX]) -> usize {
        match self {
            Frame::Entry { seq, name, size } => {
                let n = put(out, 0, &[ENTRY, *seq]);
                let n = put(out, n, name);
                put(out, n, &size.to_le_bytes())
            }
            Frame::Chunk { seq, data } => {
                let n = put(out, 0, &[CHUNK, *seq]);
                put(out, n, data.get(..data.len().min(CHUNK_MAX)).unwrap_or(&[]))
            }
            Frame::End { seq, status, count } => {
                let n = put(out, 0, &[END, *seq, *status as u8]);
                put(out, n, &count.to_le_bytes())
            }
        }
    }

    /// A frame from a notification's value, None if it is not one.
    ///
    /// ```
    /// use bryton_rustfw::filesvc::Frame;
    /// assert_eq!(Frame::parse(&[0x82, 1, 9, 8]), Some(Frame::Chunk { seq: 1, data: &[9, 8] }));
    /// assert_eq!(Frame::parse(&[0x82, 1]), None);
    /// assert_eq!(Frame::parse(&[0x55]), None);
    /// ```
    pub fn parse(p: &'a [u8]) -> Option<Frame<'a>> {
        let seq = *p.get(1)?;
        match *p.first()? {
            ENTRY if p.len() == 2 + NAME_LEN + 4 => {
                let mut name = [0; NAME_LEN];
                name.copy_from_slice(p.get(2..2 + NAME_LEN)?);
                is_name(&name).then_some(Frame::Entry { seq, name, size: u32_at(p, 2 + NAME_LEN)? })
            }
            CHUNK if (3..=FRAME_MAX).contains(&p.len()) => Some(Frame::Chunk { seq, data: p.get(2..)? }),
            END if p.len() == 7 => Some(Frame::End { seq, status: Status::from_u8(*p.get(2)?)?, count: u32_at(p, 3)? }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frame_round_trips() {
        let mut f = [0; FRAME_MAX];
        let data = [7u8; CHUNK_MAX];
        for fr in [
            Frame::Entry { seq: 1, name: *b"251026093000", size: 123_456 },
            Frame::Chunk { seq: 2, data: &data },
            Frame::Chunk { seq: 2, data: &data[..1] },
            Frame::End { seq: 3, status: Status::Io, count: u32::MAX },
        ] {
            let n = fr.encode(&mut f);
            assert_eq!(Frame::parse(&f[..n]), Some(fr));
        }
    }

    #[test]
    fn a_long_chunk_is_cut_and_a_bad_frame_refused() {
        let mut f = [0; FRAME_MAX];
        assert_eq!(Frame::Chunk { seq: 0, data: &[1; 40] }.encode(&mut f), FRAME_MAX);
        assert_eq!(Frame::parse(&[0x83, 0, 9, 0, 0, 0, 0]), None, "an unknown status");
        assert_eq!(Frame::parse(&[]), None);
    }

    #[test]
    fn the_base_keeps_its_16_bit_part_free() {
        assert_eq!(&BASE[12..14], &[0, 0]);
    }
}
