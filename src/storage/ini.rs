//! A tiny INI reader for stock's settings files (`\System\*.ini`, ghidra/STORAGE_AND_MAPS.md 2.1): `[Section]`
//! lines and `Key=Value` lines, LF or CRLF. Stock leaves a NUL byte at the end of some files, sometimes with text
//! after it: a NUL ends a line like LF. Lookups scan the text from the start (the files are a few KB); a line
//! longer than [`LINE`] bytes is skipped. Names compare without regard to ASCII case.

/// The longest line read.
pub const LINE: usize = 512;

fn trim(mut b: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = b
        && first.is_ascii_whitespace()
    {
        b = rest;
    }
    while let [rest @ .., last] = b
        && last.is_ascii_whitespace()
    {
        b = rest;
    }
    b
}

/// The value of `key` in section `section` of `text`, spaces around it removed; the first if there are several.
///
/// ```
/// use bryton_rustfw::ini::get;
/// let t = b"[A]\r\nx=1\r\n[B]\nx = 2\n\0[C]\ny=3";
/// assert_eq!(get(t, "b", "X"), Some(&b"2"[..]));
/// assert_eq!(get(t, "C", "y"), Some(&b"3"[..]));
/// assert_eq!(get(t, "A", "y"), None);
/// ```
pub fn get<'a>(text: &'a [u8], section: &str, key: &str) -> Option<&'a [u8]> {
    let mut inside = false;
    for line in text.split(|&b| b == b'\n' || b == 0).map(trim).filter(|l| l.len() <= LINE) {
        if let Some(name) = line.strip_prefix(b"[").and_then(|l| l.strip_suffix(b"]")) {
            inside = trim(name).eq_ignore_ascii_case(section.as_bytes());
        } else if inside
            && let Some(i) = line.iter().position(|&b| b == b'=')
            && trim(line.get(..i)?).eq_ignore_ascii_case(key.as_bytes())
        {
            return line.get(i + 1..).map(trim);
        }
    }
    None
}

/// A decimal number, its fraction kept to `places` digits (cut, not rounded); None if it is not one (a sign, no
/// digit, more than 9 digits before the point).
///
/// ```
/// use bryton_rustfw::ini::decimal;
/// assert_eq!(decimal(b"79.25000", 1), Some(792));
/// assert_eq!(decimal(b"240", 0), Some(240));
/// assert_eq!(decimal(b"2.5", 3), Some(2500));
/// assert_eq!([decimal(b"-1", 0), decimal(b"", 0), decimal(b"x", 0), decimal(b"1234567890", 0)], [None; 4]);
/// ```
pub fn decimal(v: &[u8], places: u32) -> Option<u32> {
    let (int, frac) = match v.iter().position(|&b| b == b'.') {
        Some(i) => (v.get(..i)?, v.get(i + 1..)?),
        None => (v, &[][..]),
    };
    if int.is_empty() || int.len() > 9 || !int.iter().chain(frac).all(u8::is_ascii_digit) {
        return None;
    }
    let digits = int.iter().copied().chain(frac.iter().copied().chain(core::iter::repeat(b'0')).take(places as usize));
    let mut n = 0u32;
    for d in digits {
        n = n.checked_mul(10)?.checked_add(u32::from(d - b'0'))?;
    }
    Some(n)
}

/// `key` in `section` as a number ([`decimal`]).
pub fn number(text: &[u8], section: &str, key: &str, places: u32) -> Option<u32> {
    get(text, section, key).and_then(|v| decimal(v, places))
}
